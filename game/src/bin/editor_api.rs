use std::{
    net::SocketAddr,
    sync::{Arc, Mutex},
};

use axum::{
    extract::State,
    http::StatusCode,
    routing::{get, post},
    Json, Router,
};
use tower_http::cors::{Any, CorsLayer};

use engine::editor::{EditorManifest, EditorSnapshot, EditorTimeline, FramesRequest, StepRequest};
use game::editor_api::{EditorApiError, EditorSession};

#[derive(Clone)]
struct AppState {
    session: Arc<Mutex<EditorSession>>,
}

fn router(state: AppState) -> Router {
    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods(Any)
        .allow_headers(Any);

    Router::new()
        .route("/api/health", get(health))
        .route("/api/manifest", get(manifest))
        .route("/api/agent/state", get(agent_state))
        .route("/api/agent/timeline", get(agent_timeline))
        .route("/api/agent/step", post(agent_step))
        .route("/api/agent/rewind", post(agent_rewind))
        .route("/api/agent/forward", post(agent_forward))
        .route("/api/agent/reset", post(agent_reset))
        .with_state(state)
        .layer(cors)
}

async fn health() -> &'static str {
    "ok"
}

async fn manifest(State(state): State<AppState>) -> Json<EditorManifest> {
    let session = state
        .session
        .lock()
        .expect("editor api session lock should be available");
    Json(session.manifest())
}

async fn agent_state(State(state): State<AppState>) -> Json<EditorSnapshot> {
    let snapshot = {
        let mut session = state
            .session
            .lock()
            .expect("editor api session lock should be available");
        session.state()
    };
    Json(snapshot)
}

async fn agent_timeline(State(state): State<AppState>) -> Json<EditorTimeline> {
    let timeline = {
        let session = state
            .session
            .lock()
            .expect("editor api session lock should be available");
        session.timeline()
    };
    Json(timeline)
}

async fn agent_step(
    State(state): State<AppState>,
    Json(payload): Json<StepRequest>,
) -> Result<Json<EditorSnapshot>, (StatusCode, String)> {
    let snapshot = {
        let mut session = state
            .session
            .lock()
            .expect("editor api session lock should be available");
        session.step(&payload.action_id)
    };

    match snapshot {
        Ok(snapshot) => Ok(Json(snapshot)),
        Err(EditorApiError::UnknownActionId(action_id)) => Err((
            StatusCode::BAD_REQUEST,
            format!("unknown actionId: {}", action_id),
        )),
    }
}

async fn agent_rewind(
    State(state): State<AppState>,
    Json(payload): Json<FramesRequest>,
) -> Json<EditorSnapshot> {
    let snapshot = {
        let mut session = state
            .session
            .lock()
            .expect("editor api session lock should be available");
        session.rewind(payload.frames)
    };
    Json(snapshot)
}

async fn agent_forward(
    State(state): State<AppState>,
    Json(payload): Json<FramesRequest>,
) -> Json<EditorSnapshot> {
    let snapshot = {
        let mut session = state
            .session
            .lock()
            .expect("editor api session lock should be available");
        session.forward(payload.frames)
    };
    Json(snapshot)
}

async fn agent_reset(State(state): State<AppState>) -> Json<EditorSnapshot> {
    let snapshot = {
        let mut session = state
            .session
            .lock()
            .expect("editor api session lock should be available");
        session.reset()
    };
    Json(snapshot)
}

#[tokio::main]
async fn main() {
    let state = AppState {
        session: Arc::new(Mutex::new(EditorSession::new(0))),
    };
    let app = router(state);

    let addr: SocketAddr = "127.0.0.1:4000".parse().expect("valid listen addr");
    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .expect("bind game editor api");

    axum::serve(listener, app)
        .await
        .expect("serve game editor api");
}

