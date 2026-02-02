use std::{
    env,
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

fn resolve_editor_api_addr<F>(mut get_env: F) -> SocketAddr
where
    F: FnMut(&str) -> Option<String>,
{
    if let Some(addr) = get_env("ROLLOUT_EDITOR_API_ADDR").and_then(|v| v.parse().ok()) {
        return addr;
    }

    if let Some(port) = get_env("ROLLOUT_EDITOR_API_PORT")
        .and_then(|v| v.parse::<u16>().ok())
    {
        return SocketAddr::from(([127, 0, 0, 1], port));
    }

    "127.0.0.1:4000"
        .parse()
        .expect("default editor api listen addr should parse")
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

    let addr = resolve_editor_api_addr(|k| env::var(k).ok());
    println!("editor api listening on http://{addr}");
    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .expect("bind editor api");

    axum::serve(listener, app)
        .await
        .expect("serve editor api");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_editor_api_addr_defaults_to_4000() {
        let addr = resolve_editor_api_addr(|_| None);
        assert_eq!(addr, "127.0.0.1:4000".parse().unwrap());
    }

    #[test]
    fn resolve_editor_api_addr_prefers_explicit_addr() {
        let addr = resolve_editor_api_addr(|k| match k {
            "ROLLOUT_EDITOR_API_ADDR" => Some("127.0.0.1:4555".to_string()),
            _ => None,
        });
        assert_eq!(addr, "127.0.0.1:4555".parse().unwrap());
    }

    #[test]
    fn resolve_editor_api_addr_accepts_port_env() {
        let addr = resolve_editor_api_addr(|k| match k {
            "ROLLOUT_EDITOR_API_PORT" => Some("4556".to_string()),
            _ => None,
        });
        assert_eq!(addr, SocketAddr::from(([127, 0, 0, 1], 4556)));
    }

    #[test]
    fn resolve_editor_api_addr_ignores_invalid_addr_but_uses_valid_port() {
        let addr = resolve_editor_api_addr(|k| match k {
            "ROLLOUT_EDITOR_API_ADDR" => Some("not-an-addr".to_string()),
            "ROLLOUT_EDITOR_API_PORT" => Some("4557".to_string()),
            _ => None,
        });
        assert_eq!(addr, SocketAddr::from(([127, 0, 0, 1], 4557)));
    }
}

