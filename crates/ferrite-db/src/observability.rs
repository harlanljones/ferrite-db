//! Observability — optional `tracing` spans and histograms (ROADMAP FDB-060).
//!
//! Every instrumented point is reached only through the gated macros
//! [`ferrite_span!`] and [`ferrite_histogram!`]. With the default (feature-less)
//! build these macros expand to nothing and the `tracing` dependency is not
//! compiled in, so the shipped library carries zero observability cost
//! (AGENTS.md §4, FDB-060 exit criterion). Enabling the `tracing` cargo feature
//! turns instrumentation on.
//!
//! This module owns only the seam; it does not edit other concern modules.
//! Call sites live in the feature-gated golden-workload capture test and any
//! future feature-gated wrappers.

#![cfg_attr(not(feature = "tracing"), allow(unused_macros))]

/// Enters a span named `name` for the duration of the enclosing block when the
/// `tracing` feature is enabled; a no-op (expands to nothing) otherwise.
///
/// The span name is a Rust identifier (`ferrite_span!(search)`); it is
/// stringified into a static callsite so the underlying `tracing` span is
/// zero-cost when the feature is off.
#[cfg(feature = "tracing")]
#[macro_export]
macro_rules! ferrite_span {
    ($name:ident) => {
        let _ferrite_span = ::tracing::span!(::tracing::Level::INFO, stringify!($name)).entered();
    };
}

#[cfg(not(feature = "tracing"))]
#[macro_export]
macro_rules! ferrite_span {
    ($($tokens:tt)*) => {};
}

/// Records a histogram sample `value` under `name` when the `tracing` feature
/// is enabled; a no-op (expands to nothing) otherwise.
#[cfg(feature = "tracing")]
#[macro_export]
macro_rules! ferrite_histogram {
    ($name:expr, $value:expr) => {
        $crate::observability::record_histogram($name, $value as f64);
    };
}

#[cfg(not(feature = "tracing"))]
#[macro_export]
macro_rules! ferrite_histogram {
    ($($tokens:tt)*) => {};
}

#[cfg(feature = "tracing")]
pub fn record_histogram(name: &'static str, value: f64) {
    ::tracing::info!(
        histogram.name = name,
        histogram.value = value,
        "ferrite histogram sample"
    );
}

#[cfg(all(test, feature = "tracing"))]
mod golden_capture {
    use std::collections::BTreeMap;
    use std::io::{self, Write};
    use std::sync::{Arc, Mutex};

    use tracing_subscriber::fmt;
    use tracing_subscriber::fmt::format::FmtSpan;
    use tracing_subscriber::fmt::writer::MakeWriter;

    use crate::search::{SearchOptions, search};
    use crate::table::{ColumnType, MetadataColumn, MetadataSchema, Metric, TableManager};
    use crate::write_path::{InsertRecord, MetadataValue, WritePath};

    /// A `Write` sink that appends everything a `fmt` subscriber emits into a
    /// shared buffer, so the golden workload can assert what instrumentation
    /// was actually produced.
    #[derive(Clone)]
    struct CapturingWriter(Arc<Mutex<Vec<u8>>>);

    impl Write for CapturingWriter {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    impl<'writer> MakeWriter<'writer> for CapturingWriter {
        type Writer = CapturingWriter;
        fn make_writer(&'writer self) -> Self::Writer {
            self.clone()
        }
    }

    #[test]
    fn golden_workload_emits_spans_and_histograms() {
        let buf = Arc::new(Mutex::new(Vec::new()));
        let subscriber = fmt::Subscriber::builder()
            .with_writer(CapturingWriter(buf.clone()))
            .with_span_events(FmtSpan::CLOSE)
            .finish();
        let _guard = tracing::subscriber::set_default(subscriber);

        let schema = MetadataSchema::new(vec![
            MetadataColumn::new("active".to_string(), ColumnType::Bool),
            MetadataColumn::new("rank".to_string(), ColumnType::I64),
        ])
        .expect("schema");
        let table = TableManager::new()
            .create("golden".to_string(), 4, Metric::L2, schema)
            .expect("create table");
        let mut path = WritePath::new(table);

        crate::ferrite_span!(insert);
        for id in 0..64u64 {
            let vector = vec![id as f32, (id + 1) as f32, (id * 2) as f32, (id * 3) as f32];
            let metadata = BTreeMap::from([
                ("active".to_string(), MetadataValue::Bool(id % 2 == 0)),
                ("rank".to_string(), MetadataValue::I64(id as i64)),
            ]);
            path.insert(vec![InsertRecord::new(id, vector, metadata)])
                .expect("insert");
        }

        // Frozen query sample — a stand-in for the FDB-021 query set. Fixed for
        // reproducibility until that harness lands; the capture binds to the
        // same public API, so no change is needed when the real corpus arrives.
        let query = [0.0f32, 1.0, 2.0, 3.0];
        let start = std::time::Instant::now();
        crate::ferrite_span!(search);
        let results = search(
            path.delta(),
            &query,
            None,
            SearchOptions::new().with_top_k(10).expect("options"),
        )
        .expect("search");
        let elapsed_ms = start.elapsed().as_secs_f64() * 1000.0;
        crate::ferrite_histogram!("search_latency_ms", elapsed_ms);
        assert_eq!(results.len(), 10);

        let output = String::from_utf8(buf.lock().unwrap().clone()).expect("utf8 output");
        assert!(
            output.contains("insert"),
            "expected `insert` span in output:\n{output}"
        );
        assert!(
            output.contains("search"),
            "expected `search` span in output:\n{output}"
        );
        assert!(
            output.contains("search_latency_ms"),
            "expected `search_latency_ms` histogram sample in output:\n{output}"
        );
    }
}
