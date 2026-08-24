use std::{error::Error, fs, path::Path, sync::Arc};

use futures::TryStreamExt;
use lancedb::{
    arrow::{
        arrow_array::{
            types::Float32Type, FixedSizeListArray, Int32Array, RecordBatch,
        },
        arrow_schema::{DataType, Field, Schema},
    },
    connect,
    index::{
        vector::{IvfHnswFlatIndexBuilder, IvfPqIndexBuilder},
        Index,
    },
    query::{ExecutableQuery, QueryBase},
};

const DIMENSION: i32 = 512;
const ROWS: i32 = 256;

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let root = tempfile::tempdir()?;
    let lance_path = root.path().join("lance");
    let sidecar_path = root.path().join("segment-0001.fseg");
    write_external_segment(&sidecar_path)?;

    let db = connect(path_string(&lance_path)?).execute().await?;
    let schema = schema();
    let batch = data(&schema)?;

    let hnsw_table = db
        .create_table(
            "hnsw",
            vec![batch.clone()],
        )
        .execute()
        .await?;
    hnsw_table
        .create_index(
            &["vector"],
            Index::IvfHnswFlat(
                IvfHnswFlatIndexBuilder::default()
                    .num_partitions(4)
                    .ef_construction(64),
            ),
        )
        .execute()
        .await?;
    let hnsw_rows = search(&hnsw_table, 8, 32).await?;

    let pq_table = db
        .create_table(
            "ivf_pq",
            vec![batch],
        )
        .execute()
        .await?;
    pq_table
        .create_index(
            &["vector"],
            Index::IvfPq(
                IvfPqIndexBuilder::default()
                    .num_partitions(4)
                    .num_sub_vectors(8),
            ),
        )
        .execute()
        .await?;
    let pq_rows = search(&pq_table, 4, 0).await?;

    if !sidecar_path.is_file() {
        return Err("external Segment sidecar was not preserved".into());
    }

    println!("capability.hnsw.build=true");
    println!("capability.hnsw.query=true");
    println!("capability.hnsw.query_knobs=ef,nprobes");
    println!("capability.ivf_pq.build=true");
    println!("capability.ivf_pq.query=true");
    println!("capability.ivf_pq.query_knobs=nprobes");
    println!("capability.external_segment_coexistence=true");
    println!("observed.hnsw_rows={hnsw_rows}");
    println!("observed.ivf_pq_rows={pq_rows}");
    Ok(())
}

fn path_string(path: &Path) -> Result<&str, Box<dyn Error>> {
    path.to_str().ok_or_else(|| "temporary path is not UTF-8".into())
}

fn schema() -> Arc<Schema> {
    Arc::new(Schema::new(vec![
        Field::new("id", DataType::Int32, false),
        Field::new(
            "vector",
            DataType::FixedSizeList(
                Arc::new(Field::new("item", DataType::Float32, true)),
                DIMENSION,
            ),
            false,
        ),
    ]))
}

fn data(schema: &Arc<Schema>) -> Result<RecordBatch, Box<dyn Error>> {
    let ids = Int32Array::from_iter_values(0..ROWS);
    let vectors = FixedSizeListArray::from_iter_primitive::<Float32Type, _, _>(
        (0..ROWS).map(|row| {
            Some((0..DIMENSION).map(|column| Some((row + column) as f32)).collect::<Vec<_>>())
        }),
        DIMENSION,
    );
    Ok(RecordBatch::try_new(
        schema.clone(),
        vec![Arc::new(ids), Arc::new(vectors)],
    )?)
}

async fn search(
    table: &lancedb::Table,
    nprobes: usize,
    ef: usize,
) -> Result<usize, Box<dyn Error>> {
    let query = vec![0.0_f32; DIMENSION as usize];
    let mut request = table.query().nearest_to(query)?.limit(5).nprobes(nprobes);
    if ef > 0 {
        request = request.ef(ef);
    }
    let batches = request.execute().await?.try_collect::<Vec<_>>().await?;
    Ok(batches.iter().map(RecordBatch::num_rows).sum())
}

fn write_external_segment(path: &Path) -> Result<(), Box<dyn Error>> {
    fs::write(path, b"FRSG\x01\x00\x00\x00spike-sidecar")?;
    Ok(())
}
