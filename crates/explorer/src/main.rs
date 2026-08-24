//! Ferrite DB Explorer — localhost-only demo server (HJ-252, rescoped).
//!
//! Binds 127.0.0.1 exclusively; opt-in binary; host-application composition
//! of the public ferrite-db APIs (see lib.rs ADR note).

use std::net::SocketAddr;
use std::sync::Arc;

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{Html, IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;

use explorer::{BenchMetrics, Explorer, QueryOutcome, SessionSpec, SessionStatus};
use ferrite_db::index_substrate::IndexFamily;

type Shared = Arc<tokio::sync::RwLock<Option<Arc<Explorer>>>>;

/// Runs ferrite-touching work on a plain OS thread. The index-substrate seam
/// owns its own tokio runtime and calls `block_on`; that must never happen on
/// any thread of THIS server's runtime (tokio forbids nested `block_on`).
/// A bare thread carries no runtime context, so the seam stays usable
/// verbatim while `spawn_blocking` keeps the HTTP runtime responsive.
fn off_runtime<T, F>(work: F) -> Result<T, ApiError>
where
    T: Send + 'static,
    F: FnOnce() -> Result<T, String> + Send + 'static,
{
    let result = std::thread::Builder::new()
        .name("explorer-ferrite".to_string())
        .spawn(work)
        .map_err(|e| ApiError(format!("spawning worker: {e}")))?
        .join()
        .map_err(|_| ApiError("ferrite worker panicked".to_string()))?;
    result.map_err(ApiError)
}

#[derive(Deserialize)]
struct IngestRequest {
    #[serde(default)]
    num_vectors: Option<u32>,
    #[serde(default)]
    dimension: Option<u32>,
    #[serde(default)]
    num_categories: Option<u32>,
    #[serde(default)]
    seed: Option<u64>,
    #[serde(default)]
    family: Option<String>,
}

#[derive(Deserialize)]
struct QueryRequest {
    #[serde(default)]
    query_index: Option<usize>,
    #[serde(default = "default_top_k")]
    top_k: u32,
    #[serde(default)]
    probes: Option<u32>,
    #[serde(default)]
    ef_search: Option<u32>,
}

fn default_top_k() -> u32 {
    10
}

#[derive(Deserialize)]
struct BenchRequest {
    #[serde(default = "default_passes")]
    passes: usize,
    #[serde(default = "default_top_k")]
    top_k: u32,
    #[serde(default)]
    probes: Option<u32>,
    #[serde(default)]
    ef_search: Option<u32>,
}

fn default_passes() -> usize {
    3
}

fn family_from_name(name: &str) -> Result<IndexFamily, String> {
    match name {
        "hnsw" | "IvfHnswFlat" | "" => Ok(IndexFamily::IvfHnswFlat),
        "pq" | "IvfPq" => Ok(IndexFamily::IvfPq),
        other => Err(format!("unknown index family {other:?} (hnsw | pq)")),
    }
}

async fn ingest(
    State(state): State<Shared>,
    Json(request): Json<IngestRequest>,
) -> Result<Json<SessionStatus>, ApiError> {
    let spec = SessionSpec {
        num_vectors: request.num_vectors.unwrap_or(2_000).min(200_000),
        dimension: request.dimension.unwrap_or(64).clamp(8, 1024),
        num_categories: request.num_categories.unwrap_or(50).max(1),
        seed: request.seed.unwrap_or(42),
        family: family_from_name(request.family.as_deref().unwrap_or("hnsw"))?,
    };
    // Ferrite + seam work runs on a context-free thread (see `off_runtime`);
    // `spawn_blocking` keeps the HTTP runtime responsive meanwhile.
    let explorer = tokio::task::spawn_blocking(move || {
        off_runtime(move || Explorer::create(spec).map(Arc::new))
    })
    .await
    .map_err(|e| ApiError(format!("join: {e}")))??;
    let status = explorer.status();
    *state.write().await = Some(explorer);
    Ok(Json(status))
}

async fn status(State(state): State<Shared>) -> Result<Json<SessionStatus>, ApiError> {
    let guard = state.read().await;
    let explorer = guard
        .as_ref()
        .ok_or_else(|| ApiError("no dataset loaded yet — POST /api/ingest first".into()))?;
    Ok(Json(explorer.status()))
}

async fn query(
    State(state): State<Shared>,
    Json(request): Json<QueryRequest>,
) -> Result<Json<QueryOutcome>, ApiError> {
    let guard = state.read().await;
    let explorer = guard
        .as_ref()
        .ok_or_else(|| ApiError("no dataset loaded yet".into()))?;
    let outcome = tokio::task::spawn_blocking({
        let explorer = Arc::clone(explorer);
        move || {
            off_runtime(move || {
                explorer.query(
                    request.query_index.unwrap_or(0),
                    request.top_k.clamp(1, 1000),
                    request.probes,
                    request.ef_search,
                )
            })
        }
    })
    .await
    .map_err(|e| ApiError(format!("join: {e}")))??;
    Ok(Json(outcome))
}

async fn bench(
    State(state): State<Shared>,
    Json(request): Json<BenchRequest>,
) -> Result<Json<BenchMetrics>, ApiError> {
    let guard = state.read().await;
    let explorer = guard
        .as_ref()
        .ok_or_else(|| ApiError("no dataset loaded yet".into()))?;
    let metrics = tokio::task::spawn_blocking({
        let explorer = Arc::clone(explorer);
        move || {
            off_runtime(move || {
                explorer.bench(
                    request.passes.clamp(1, 20),
                    request.top_k.clamp(1, 1000),
                    request.probes,
                    request.ef_search,
                )
            })
        }
    })
    .await
    .map_err(|e| ApiError(format!("join: {e}")))??;
    Ok(Json(metrics))
}

async fn reindex(State(state): State<Shared>) -> Result<Json<serde_json::Value>, ApiError> {
    let guard = state.read().await;
    let explorer = guard
        .as_ref()
        .ok_or_else(|| ApiError("no dataset loaded yet".into()))?;
    let result = tokio::task::spawn_blocking({
        let explorer = Arc::clone(explorer);
        move || off_runtime(move || explorer.rebuild_index().map(|()| true))
    })
    .await
    .map_err(|e| ApiError(format!("join: {e}")))?;
    let result = match result {
        Ok(reindexed) => Ok(serde_json::json!({ "reindexed": reindexed })),
        Err(api) => Err(api),
    };
    match result {
        Ok(value) => Ok(Json(value)),
        Err(err) => Err(err),
    }
}

async fn index_page() -> Html<&'static str> {
    Html(include_str!("index.html"))
}

struct ApiError(String);

impl From<String> for ApiError {
    fn from(message: String) -> Self {
        Self(message)
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (StatusCode::BAD_GATEWAY, self.0).into_response()
    }
}

#[tokio::main]
async fn main() {
    let port: u16 = std::env::var("EXPLORER_PORT")
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or(8787);
    let state: Shared = Arc::new(tokio::sync::RwLock::new(None));
    let app = Router::new()
        .route("/", get(index_page))
        .route("/api/status", get(status))
        .route("/api/ingest", post(ingest))
        .route("/api/query", post(query))
        .route("/api/bench", post(bench))
        .route("/api/reindex", post(reindex))
        .with_state(state);
    let addr = SocketAddr::from(([127, 0, 0, 1], port));
    println!("explorer listening on http://{addr} — localhost only");
    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .unwrap_or_else(|error| {
            eprintln!("error: cannot bind {addr}: {error}");
            std::process::exit(1);
        });
    if let Err(error) = axum::serve(listener, app).await {
        eprintln!("error: {error}");
        std::process::exit(1);
    }
}
