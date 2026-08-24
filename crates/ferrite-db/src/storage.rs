//! Storage — Segment persistence: immutable Segment files, atomic-rename
//! Commit, footer, Tombstone bitmaps (AGENTS.md §4, ADR 0003). Sole owner of
//! on-disk layout; no other component reads or writes Segment files.
//!
//! On-disk format (Ferrite-controlled sidecar, `.fseg`; ADR 0002/R2 keeps it
//! outside Lance-owned payloads):
//!
//! ```text
//! bytes 0..4    magic b"FRSG"
//!      4..8     format_version (u32 LE, currently 1)
//!      8..12    dimension      (u32 LE)
//!      12..16   flags          (u32 LE, must be 0)
//!      16..24   row_count      (u64 LE)
//!      24..32   vector_data_len(u64 LE, bytes)
//!      32..40   bitmap_offset  (u64 LE = HEADER_LEN + vector_data_len)
//!      40..48   bitmap_len     (u64 LE, ceil(row_count/8))
//!      48..52   payload_crc32  (u32 LE)
//!      52..56   reserved       (u32 LE, must be 0)
//!      56..60   header_crc32   (u32 LE, covers bytes 0..56)
//!      60..64   padding        (must be 0)
//!      then row_count*dimension f32s (LE), then bitmap bytes
//! ```
//!
//! Durability follows ADR 0003: a Segment becomes visible only through an
//! atomic rename after `sync_all`; there is no WAL. An interrupted write
//! leaves at most an unreferenced `.tmp` file behind.
//!
//! The reader validates magic, version, structural lengths, and both CRCs
//! eagerly at open — corrupt input is rejected as [`Error::CorruptSegment`]
//! before any payload access. Owned by ROADMAP FDB-012.

use std::fmt;
use std::fs;
use std::io::{self, Seek, Write};
use std::path::{Path, PathBuf};

use crate::errors::{Error, Result};

/// On-disk format magic: `FRSG` (Ferrite SeGment).
pub const MAGIC: [u8; 4] = *b"FRSG";

/// Current on-disk format version.
pub const FORMAT_VERSION: u32 = 1;

/// Size of the fixed Segment file header in bytes.
pub const HEADER_LEN: u64 = 64;

// Header field offsets.
const OFF_VERSION: usize = 4;
const OFF_DIMENSION: usize = 8;
const OFF_ROW_COUNT: usize = 16;
const OFF_VEC_LEN: usize = 24;
const OFF_BITMAP_OFFSET: usize = 32;
const OFF_BITMAP_LEN: usize = 40;
const OFF_PAYLOAD_CRC: usize = 48;
const OFF_HEADER_CRC: usize = 56;

/// Validated description of one committed Segment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SegmentHeader {
    /// On-disk format version of this Segment file.
    pub format_version: u32,
    /// Vector dimensionality fixed for every stored row.
    pub dimension: u32,
    /// Total rows stored (live plus tombstoned).
    pub row_count: u64,
}

/// Bit set marking deleted rows; physically reclaimed by Compaction later
/// (ROADMAP FDB-040). The Tombstone type lives in Storage by G1 decision.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TombstoneBitmap {
    bits: Vec<u8>,
    len: u64,
}

impl TombstoneBitmap {
    /// Creates an empty bitmap able to address `len` rows.
    pub fn new(len: u64) -> Self {
        Self {
            bits: vec![0u8; byte_len(len)],
            len,
        }
    }

    /// Marks `row` as deleted. Fails if out of range (caller-fixable).
    pub fn set(&mut self, row: u64) -> Result<()> {
        self.check(row)?;
        self.bits[row as usize / 8] |= 1 << (row % 8);
        Ok(())
    }

    /// Clears the marker on `row`. Fails if out of range (caller-fixable).
    pub fn clear(&mut self, row: u64) -> Result<()> {
        self.check(row)?;
        self.bits[row as usize / 8] &= !(1 << (row % 8));
        Ok(())
    }

    /// Reports whether `row` carries a Tombstone. Out-of-range rows report
    /// `false`.
    pub fn get(&self, row: u64) -> bool {
        if row >= self.len {
            return false;
        }
        self.bits[row as usize / 8] & (1 << (row % 8)) != 0
    }

    /// Number of addressed rows.
    pub fn len(&self) -> u64 {
        self.len
    }

    /// Whether the bitmap addresses no rows.
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Iterates over all rows carrying a Tombstone, ascending.
    pub fn iter(&self) -> impl Iterator<Item = u64> + '_ {
        (0..self.len).filter(|&row| self.get(row))
    }

    fn check(&self, row: u64) -> Result<()> {
        if row >= self.len {
            return Err(Error::SchemaViolation {
                reason: format!("row {row} out of range (bitmap holds {} rows)", self.len),
            });
        }
        Ok(())
    }
}

fn byte_len(rows: u64) -> usize {
    rows.div_ceil(8) as usize
}

/// Incremental IEEE CRC-32 (poly 0xEDB88320 reflected).
struct Crc32(u32);

impl Crc32 {
    fn new() -> Self {
        Crc32(0xFFFF_FFFF)
    }

    fn update(&mut self, data: &[u8]) {
        let mut state = self.0;
        for &b in data {
            state ^= b as u32;
            for _ in 0..8 {
                let mask = (state & 1).wrapping_neg();
                state = (state >> 1) ^ (0xEDB8_8320 & mask);
            }
        }
        self.0 = state;
    }

    fn finalize(self) -> u32 {
        !self.0
    }
}

fn crc32(data: &[u8]) -> u32 {
    let mut c = Crc32::new();
    c.update(data);
    c.finalize()
}

/// Builds Segments incrementally and publishes them atomically.
///
/// Rows are streamed to `<id>.fseg.tmp`; [`SegmentWriter::commit`] writes the
/// final header, forces the bytes to storage, and renames the tmp file onto
/// `<id>.fseg` (ADR 0003). Dropping an uncommitted writer leaves the tmp file
/// behind — deliberately, so crash states stay inspectable; such files are
/// never referenced.
pub struct SegmentWriter {
    dir: PathBuf,
    id: u64,
    dimension: u32,
    rows: u64,
    file: fs::File,
    payload_crc: Crc32,
    vector_bytes: u64,
    tombstones: TombstoneBitmap,
}

impl fmt::Debug for SegmentWriter {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SegmentWriter")
            .field("id", &self.id)
            .field("dimension", &self.dimension)
            .field("rows", &self.rows)
            .finish_non_exhaustive()
    }
}

impl SegmentWriter {
    /// Starts a new Segment for `dimension`-wide vectors in `dir`.
    ///
    /// Creates `<dir>/<id>.fseg.tmp`, failing if the tmp already exists so a
    /// live writer is never silently hijacked.
    pub fn create(dir: &Path, id: u64, dimension: u32) -> io::Result<Self> {
        if dimension == 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "dimension must be nonzero",
            ));
        }
        let path = tmp_path(dir, id);
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)?;
        // Placeholder; replaced with the real header at commit time.
        file.write_all(&[0u8; HEADER_LEN as usize])?;
        Ok(Self {
            dir: dir.to_path_buf(),
            id,
            dimension,
            rows: 0,
            file,
            payload_crc: Crc32::new(),
            vector_bytes: 0,
            tombstones: TombstoneBitmap::new(0),
        })
    }

    /// Appends whole vectors given as a flat row-major slice.
    ///
    /// `vectors.len()` must be a multiple of the Segment's dimension;
    /// otherwise the batch is rejected as [`Error::DimensionMismatch`].
    pub fn append_vectors(&mut self, vectors: &[f32]) -> Result<()> {
        if !(vectors.len() as u64).is_multiple_of(self.dimension as u64) {
            return Err(Error::DimensionMismatch {
                expected: self.dimension,
                actual: vectors.len() as u32,
            });
        }
        let bytes = floats_to_le_bytes(vectors);
        self.file.write_all(&bytes)?;
        self.payload_crc.update(&bytes);
        self.vector_bytes += bytes.len() as u64;
        self.rows += (vectors.len() / self.dimension as usize) as u64;
        // Tombstones appended implicitly for new rows default to cleared.
        if self.tombstones.len() < self.rows {
            self.tombstones = TombstoneBitmap::new(self.rows);
        }
        Ok(())
    }

    /// Records a Tombstone for `row`, hiding it from search results until
    /// Compaction reclaims it physically.
    pub fn tombstone(&mut self, row: u64) -> Result<()> {
        if row >= self.rows {
            return Err(Error::SchemaViolation {
                reason: format!("tombstone row {row} beyond {} written rows", self.rows),
            });
        }
        self.tombstones.set(row)
    }

    /// Publishes the Segment atomically and returns its final path.
    ///
    /// Final header → `sync_all` → rename. If anything fails before the
    /// rename, no Segment file exists at the final path.
    pub fn commit(mut self) -> Result<PathBuf> {
        let final_path = final_path(&self.dir, self.id);
        let bitmap_bytes = std::mem::take(&mut self.tombstones.bits);

        let bitmap_offset = HEADER_LEN + self.vector_bytes;
        let mut header = [0u8; HEADER_LEN as usize];
        header[0..4].copy_from_slice(&MAGIC);
        put_u32(&mut header, OFF_VERSION, FORMAT_VERSION);
        put_u32(&mut header, OFF_DIMENSION, self.dimension);
        put_u64(&mut header, OFF_ROW_COUNT, self.rows);
        put_u64(&mut header, OFF_VEC_LEN, self.vector_bytes);
        put_u64(&mut header, OFF_BITMAP_OFFSET, bitmap_offset);
        put_u64(&mut header, OFF_BITMAP_LEN, bitmap_bytes.len() as u64);
        put_u32(
            &mut header,
            OFF_PAYLOAD_CRC,
            std::mem::replace(&mut self.payload_crc, Crc32::new()).finalize(),
        );
        let header_crc = crc32(&header[0..OFF_HEADER_CRC]);
        put_u32(&mut header, OFF_HEADER_CRC, header_crc);

        self.file.write_all(&bitmap_bytes)?;
        self.file.seek(io::SeekFrom::Start(0))?;
        self.file.write_all(&header)?;
        self.file.sync_all()?;
        fs::rename(tmp_path(&self.dir, self.id), &final_path)?;
        Ok(final_path)
    }
}

fn floats_to_le_bytes(vectors: &[f32]) -> Vec<u8> {
    let mut out = Vec::with_capacity(vectors.len() * 4);
    for v in vectors {
        out.extend_from_slice(&float_to_le_bytes(*v));
    }
    out
}

fn float_to_le_bytes(v: f32) -> [u8; 4] {
    v.to_bits().to_le_bytes()
}

fn put_u32(buf: &mut [u8], off: usize, v: u32) {
    buf[off..off + 4].copy_from_slice(&v.to_le_bytes());
}

fn put_u64(buf: &mut [u8], off: usize, v: u64) {
    buf[off..off + 8].copy_from_slice(&v.to_le_bytes());
}

fn get_u32(buf: &[u8], off: usize) -> u32 {
    u32::from_le_bytes([buf[off], buf[off + 1], buf[off + 2], buf[off + 3]])
}

fn get_u64(buf: &[u8], off: usize) -> u64 {
    u64::from_le_bytes([
        buf[off],
        buf[off + 1],
        buf[off + 2],
        buf[off + 3],
        buf[off + 4],
        buf[off + 5],
        buf[off + 6],
        buf[off + 7],
    ])
}

fn tmp_path(dir: &Path, id: u64) -> PathBuf {
    dir.join(format!("{id}.fseg.tmp"))
}

fn final_path(dir: &Path, id: u64) -> PathBuf {
    dir.join(format!("{id}.fseg"))
}

/// Read-only view over one committed, fully validated Segment file.
///
/// Open validates structure and both CRCs eagerly; every accessor afterwards
/// operates on trusted data.
#[derive(Debug)]
pub struct SegmentReader {
    header: SegmentHeader,
    vectors: Vec<f32>,
    tombstones: TombstoneBitmap,
}

impl SegmentReader {
    /// Opens and fully validates a committed Segment file.
    pub fn open(path: &Path) -> Result<Self> {
        let raw = fs::read(path).map_err(Error::Io)?;
        let header = validate_header(path, &raw)?;
        let expected_total = bitmap_offset_of(&header) + get_u64(&raw, OFF_BITMAP_LEN);
        if raw.len() as u64 != expected_total {
            return Err(Error::CorruptSegment {
                detail: format!(
                    "{}: file length {} does not match header ({expected_total})",
                    path.display(),
                    raw.len()
                ),
            });
        }

        let vec_len = get_u64(&raw, OFF_VEC_LEN) as usize;
        let payload_crc = get_u32(&raw, OFF_PAYLOAD_CRC);
        if crc32(&raw[HEADER_LEN as usize..HEADER_LEN as usize + vec_len]) != payload_crc {
            return Err(Error::CorruptSegment {
                detail: format!("{}: payload checksum mismatch", path.display()),
            });
        }

        let mut vectors = Vec::with_capacity(vec_len / 4);
        for four in raw[HEADER_LEN as usize..HEADER_LEN as usize + vec_len].chunks_exact(4) {
            vectors.push(f32::from_bits(u32::from_le_bytes([
                four[0], four[1], four[2], four[3],
            ])));
        }

        let bits = raw[bitmap_offset_of(&header) as usize..raw.len()].to_vec();
        let tombstones = TombstoneBitmap {
            bits,
            len: header.row_count,
        };

        Ok(Self {
            header,
            vectors,
            tombstones,
        })
    }

    /// The validated header.
    pub fn header(&self) -> &SegmentHeader {
        &self.header
    }

    /// Rows not carrying a Tombstone.
    pub fn live_row_count(&self) -> u64 {
        self.header.row_count - self.tombstones.iter().count() as u64
    }

    /// Copies the vector at `row`.
    pub fn vector(&self, row: u64) -> Result<Vec<f32>> {
        let dim = self.header.dimension as u64;
        if row >= self.header.row_count {
            return Err(Error::SchemaViolation {
                reason: format!(
                    "row {row} out of range (segment holds {})",
                    self.header.row_count
                ),
            });
        }
        let start = (row * dim) as usize;
        Ok(self.vectors[start..start + dim as usize].to_vec())
    }

    /// All stored vectors, row-major (including tombstoned rows).
    pub fn all_vectors(&self) -> &[f32] {
        &self.vectors
    }

    /// Whether `row` carries a Tombstone. Out-of-range rows report `false`.
    pub fn is_tombstoned(&self, row: u64) -> bool {
        self.tombstones.get(row)
    }

    /// Ascending iterator over tombstoned rows.
    pub fn tombstoned_rows(&self) -> impl Iterator<Item = u64> + '_ {
        self.tombstones.iter()
    }
}

fn bitmap_offset_of(header: &SegmentHeader) -> u64 {
    HEADER_LEN + header.row_count * header.dimension as u64 * 4
}

fn validate_header(path: &Path, raw: &[u8]) -> Result<SegmentHeader> {
    let short = |what: &str| Error::CorruptSegment {
        detail: format!("{}: {what}", path.display()),
    };
    if raw.len() < HEADER_LEN as usize {
        return Err(short("file shorter than header"));
    }
    if raw[0..4] != MAGIC {
        return Err(short("bad magic"));
    }
    let version = get_u32(raw, OFF_VERSION);
    if version != FORMAT_VERSION {
        return Err(short(&format!("unsupported format version {version}")));
    }
    if get_u32(raw, OFF_HEADER_CRC) != crc32(&raw[0..OFF_HEADER_CRC]) {
        return Err(short("header checksum mismatch"));
    }
    let header = SegmentHeader {
        format_version: version,
        dimension: get_u32(raw, OFF_DIMENSION),
        row_count: get_u64(raw, OFF_ROW_COUNT),
    };
    if header.dimension == 0 {
        return Err(short("zero dimension"));
    }
    let expect_vec_len = header.row_count * header.dimension as u64 * 4;
    if get_u64(raw, OFF_VEC_LEN) != expect_vec_len {
        return Err(short(
            "vector_data_len inconsistent with row_count/dimension",
        ));
    }
    if get_u64(raw, OFF_BITMAP_OFFSET) != HEADER_LEN + expect_vec_len {
        return Err(short("bitmap_offset inconsistent"));
    }
    if get_u64(raw, OFF_BITMAP_LEN) != byte_len(header.row_count) as u64 {
        return Err(short("bitmap_len inconsistent with row_count"));
    }
    if raw[12..16] != [0; 4] || raw[52..56] != [0; 4] || raw[60..64] != [0; 4] {
        return Err(short("reserved header fields must be zero"));
    }
    Ok(header)
}

#[cfg(test)]
mod tests {
    use super::*;

    const DIM: u32 = 4;

    fn vec_row(base: f32, dim: u32) -> Vec<f32> {
        (0..dim).map(|i| base + i as f32 * 0.5).collect()
    }

    fn write_sample(dir: &Path, id: u64) -> PathBuf {
        let mut w = SegmentWriter::create(dir, id, DIM).expect("create");
        w.append_vectors(&[
            0.0, 1.0, 2.0, 3.0, //
            10.0, 11.0, 12.0, 13.0, //
            20.0, 21.0, 22.0, 23.0,
        ])
        .expect("append");
        w.tombstone(1).expect("tombstone");
        w.commit().expect("commit")
    }

    #[test]
    fn crc32_matches_known_vector() {
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
    }

    #[test]
    fn round_trip_is_lossless() {
        let dir = tempfile_dir();
        let path = write_sample(&dir, 1);
        let r = SegmentReader::open(&path).expect("open");
        assert_eq!(r.header().dimension, DIM);
        assert_eq!(r.header().row_count, 3);
        assert_eq!(r.all_vectors().len(), 12);
        assert_eq!(r.vector(2).expect("vector"), vec![20.0, 21.0, 22.0, 23.0]);
        assert!(r.is_tombstoned(1));
        assert!(!r.is_tombstoned(0));
        assert_eq!(r.live_row_count(), 2);
        assert_eq!(r.tombstoned_rows().collect::<Vec<_>>(), vec![1]);
        fs::remove_file(&path).ok();
    }

    #[test]
    fn drop_without_commit_publishes_nothing() {
        let dir = tempfile_dir();
        {
            let mut w = SegmentWriter::create(&dir, 7, DIM).expect("create");
            w.append_vectors(&vec_row(1.0, DIM)).expect("append");
            drop(w); // simulated crash before rename
        }
        assert!(!final_path(&dir, 7).exists(), "no final file may exist");
        assert!(tmp_path(&dir, 7).exists(), "tmp stays for inspection");
        // The abandoned tmp never opens as a Segment (placeholder header).
        match SegmentReader::open(&tmp_path(&dir, 7)) {
            Err(Error::CorruptSegment { .. }) => {}
            other => panic!("expected CorruptSegment, got {other:?}"),
        }
    }

    #[test]
    fn truncated_file_rejected_before_use() {
        let dir = tempfile_dir();
        let path = write_sample(&dir, 2);
        let full = fs::read(&path).expect("read");
        for cut in [0usize, 30, HEADER_LEN as usize, full.len() - 1] {
            fs::write(&path, &full[..cut]).expect("truncate");
            match SegmentReader::open(&path) {
                Err(Error::CorruptSegment { .. }) => {}
                other => panic!("cut {cut}: expected CorruptSegment, got {other:?}"),
            }
        }
    }

    #[test]
    fn bit_flips_are_detected() {
        let dir = tempfile_dir();
        let path = write_sample(&dir, 3);
        let good = fs::read(&path).expect("read");

        // Flip one bit in the header (version field) and one in the payload.
        for off in [OFF_VERSION, HEADER_LEN as usize + 8] {
            let mut bad = good.clone();
            bad[off] ^= 0b0000_0001;
            fs::write(&path, &bad).expect("write");
            match SegmentReader::open(&path) {
                Err(Error::CorruptSegment { .. }) => {}
                other => panic!("flip at {off}: expected CorruptSegment, got {other:?}"),
            }
        }
        fs::write(&path, &good).expect("restore");
        assert!(SegmentReader::open(&path).is_ok());
    }

    #[test]
    fn bad_magic_and_version_rejected() {
        let dir = tempfile_dir();
        let path = write_sample(&dir, 4);
        let good = fs::read(&path).expect("read");

        let mut bad = good.clone();
        bad[0] = b'X';
        fs::write(&path, &bad).expect("write");
        assert!(matches!(
            SegmentReader::open(&path),
            Err(Error::CorruptSegment { .. })
        ));

        let mut bad = good;
        bad[OFF_VERSION..OFF_VERSION + 4].copy_from_slice(&99u32.to_le_bytes());
        fs::write(&path, &bad).expect("write");
        assert!(matches!(
            SegmentReader::open(&path),
            Err(Error::CorruptSegment { .. })
        ));
    }

    #[test]
    fn append_validates_whole_vectors() {
        let dir = tempfile_dir();
        let mut w = SegmentWriter::create(&dir, 5, DIM).expect("create");
        let e = w.append_vectors(&[1.0, 2.0, 3.0]).expect_err("must reject");
        assert!(matches!(
            e,
            Error::DimensionMismatch {
                expected: 4,
                actual: 3
            }
        ));
        assert_eq!(e.class(), crate::errors::ErrorClass::CallerFixable);
        drop(w);
    }

    #[test]
    fn bitmap_set_clear_iterate_including_boundaries() {
        let mut bm = TombstoneBitmap::new(70); // non-byte-aligned edge at 69
        assert_eq!(bm.len(), 70);
        assert!(!bm.get(0));
        bm.set(0).expect("set first");
        bm.set(69).expect("set last");
        bm.set(8).expect("set byte boundary");
        assert!(bm.get(0) && bm.get(8) && bm.get(69));
        bm.clear(8).expect("clear middle");
        assert!(!bm.get(8));
        assert_eq!(
            bm.iter().collect::<Vec<_>>(),
            vec![0, 69],
            "ascending iteration"
        );
        assert!(bm.set(70).is_err(), "out of range rejected, not panicked");
        assert!(!bm.get(u64::MAX), "out-of-range get is false, not a panic");

        // Re-set is idempotent; clear of unset bit stays clear.
        bm.set(0).expect("re-set");
        bm.clear(5).expect("clear unset");
        assert_eq!(bm.iter().collect::<Vec<_>>(), vec![0, 69]);
    }

    fn tempfile_dir() -> PathBuf {
        let d = std::env::temp_dir().join(format!("fdb-test-{}", std::process::id()));
        fs::create_dir_all(&d).expect("mkdir");
        d
    }
}
