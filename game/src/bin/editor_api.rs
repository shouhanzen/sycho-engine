use std::{
    env,
    net::SocketAddr,
    path::PathBuf,
    process::Command,
    sync::{Arc, Mutex},
};

use axum::{
    extract::State,
    http::Method,
    http::StatusCode,
    routing::{get, post},
    Json, Router,
};
use bytes::Bytes;
use http_body_util::{BodyExt, Full};
use hyper::{Request, Uri, header};
use hyper_util::{
    client::legacy::{connect::HttpConnector, Client},
    rt::TokioExecutor,
};
use serde::Serialize;
use tower_http::cors::{Any, CorsLayer};

use engine::editor::{
    EditorAction, EditorManifest, EditorSnapshot, EditorTimeline, FramesRequest, SeekRequest,
    StepRequest,
};

type HttpClient = Client<HttpConnector, Full<Bytes>>;

#[derive(Clone)]
struct AppState {
    remote_base: Arc<Mutex<Option<String>>>,
    client: HttpClient,
}

fn build_http_client() -> HttpClient {
    Client::builder(TokioExecutor::new()).build(HttpConnector::new())
}

fn default_manifest() -> EditorManifest {
    EditorManifest {
        title: "Tetree (Tetris)".to_string(),
        actions: vec![
            EditorAction {
                id: "moveLeft".to_string(),
                label: "Left".to_string(),
            },
            EditorAction {
                id: "moveRight".to_string(),
                label: "Right".to_string(),
            },
            EditorAction {
                id: "softDrop".to_string(),
                label: "Down".to_string(),
            },
            EditorAction {
                id: "rotateCw".to_string(),
                label: "Rotate CW".to_string(),
            },
            EditorAction {
                id: "rotateCcw".to_string(),
                label: "Rotate CCW".to_string(),
            },
            EditorAction {
                id: "rotate180".to_string(),
                label: "Rotate 180".to_string(),
            },
            EditorAction {
                id: "hardDrop".to_string(),
                label: "Hard Drop".to_string(),
            },
            EditorAction {
                id: "hold".to_string(),
                label: "Hold".to_string(),
            },
            EditorAction {
                id: "noop".to_string(),
                label: "Noop".to_string(),
            },
        ],
    }
}

fn router(state: AppState) -> Router {
    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods(Any)
        .allow_headers(Any);

    Router::new()
        .route("/api/health", get(health))
        .route("/api/game/status", get(game_status_with_state))
        .route("/api/game/launch", post(game_launch))
        .route("/api/manifest", get(manifest))
        .route("/api/agent/state", get(agent_state))
        .route("/api/agent/timeline", get(agent_timeline))
        .route("/api/agent/step", post(agent_step))
        .route("/api/agent/rewind", post(agent_rewind))
        .route("/api/agent/forward", post(agent_forward))
        .route("/api/agent/seek", post(agent_seek))
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

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct GameStatusResponse {
    running: bool,
    detail: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct GameLaunchResponse {
    ok: bool,
    detail: String,
}

#[cfg(test)]
static TEST_GO_SH_ROOT: std::sync::OnceLock<Mutex<Option<PathBuf>>> = std::sync::OnceLock::new();

#[cfg(test)]
static TEST_GO_SH_ENV: std::sync::OnceLock<Mutex<std::collections::BTreeMap<String, String>>> =
    std::sync::OnceLock::new();

fn repo_root() -> PathBuf {
    #[cfg(test)]
    {
        if let Some(root) = TEST_GO_SH_ROOT
            .get_or_init(|| Mutex::new(None))
            .lock()
            .expect("test go.sh root lock should be available")
            .clone()
        {
            return root;
        }
    }

    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("game crate should live under the repo root")
        .to_path_buf()
}

fn output_detail(output: &std::process::Output) -> String {
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();

    match (stdout.is_empty(), stderr.is_empty()) {
        (true, true) => String::new(),
        (false, true) => stdout,
        (true, false) => stderr,
        (false, false) => format!("{stdout}\n{stderr}"),
    }
}

fn resolve_bash_bin() -> String {
    if let Ok(bash) = env::var("BASH") {
        let trimmed = bash.trim();
        if !trimmed.is_empty() {
            return trimmed.to_string();
        }
    }

    if cfg!(windows) {
        if let Ok(shell) = env::var("SHELL") {
            let trimmed = shell.trim();
            if !trimmed.is_empty() && trimmed.to_ascii_lowercase().contains("bash") {
                return trimmed.to_string();
            }
        }
    }

    "bash".to_string()
}

fn run_go_sh_with_env(
    args: &str,
    extra_env: &[(String, String)],
) -> Result<std::process::Output, String> {
    let root = repo_root();
    let mut cmd = Command::new(resolve_bash_bin());
    cmd.arg("-lc")
        // Execute go.sh via bash so the script doesn't need executable bits (notably on Windows).
        .arg(format!("bash ./go.sh {args}"))
        .current_dir(root);

    for (k, v) in extra_env {
        cmd.env(k, v);
    }

    #[cfg(test)]
    {
        let env_overrides = TEST_GO_SH_ENV
            .get_or_init(|| Mutex::new(std::collections::BTreeMap::new()))
            .lock()
            .expect("test go.sh env override lock should be available");
        for (k, v) in env_overrides.iter() {
            cmd.env(k, v);
        }
    }

    let output = cmd
        .output()
        .map_err(|e| format!("failed to run go.sh via bash: {e}"))?;
    Ok(output)
}

fn run_go_sh(args: &str) -> Result<std::process::Output, String> {
    run_go_sh_with_env(args, &[])
}

fn game_remote_base(state: &AppState) -> Result<String, (StatusCode, String)> {
    state
        .remote_base
        .lock()
        .expect("remote base lock should be available")
        .clone()
        .ok_or_else(|| {
            (
                StatusCode::CONFLICT,
                "game is not connected (launch the game first)".to_string(),
            )
        })
}

async fn proxy_to_game(
    state: &AppState,
    method: Method,
    path: &str,
    body: Option<Vec<u8>>,
) -> Result<(StatusCode, Bytes), (StatusCode, String)> {
    let base = game_remote_base(state)?;
    let uri: Uri = format!("{base}{path}").parse().map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("invalid game url: {e}"),
        )
    })?;

    let req = Request::builder()
        .method(method)
        .uri(uri)
        .header(header::CONTENT_TYPE, "application/json")
        .body(Full::new(Bytes::from(body.unwrap_or_default())))
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("request build failed: {e}")))?;

    let res = state
        .client
        .request(req)
        .await
        .map_err(|e| (StatusCode::BAD_GATEWAY, format!("failed reaching game: {e}")))?;

    let status = res.status();
    let bytes = res
        .into_body()
        .collect()
        .await
        .map_err(|e| (StatusCode::BAD_GATEWAY, format!("failed reading game response: {e}")))?
        .to_bytes();

    Ok((status, bytes))
}

fn pick_free_local_port() -> Result<u16, String> {
    let listener = std::net::TcpListener::bind("127.0.0.1:0")
        .map_err(|e| format!("failed to allocate local port: {e}"))?;
    let port = listener
        .local_addr()
        .map_err(|e| format!("failed reading allocated port: {e}"))?
        .port();
    Ok(port)
}

async fn game_status_impl(state: Option<AppState>) -> Json<GameStatusResponse> {
    match run_go_sh("--status --game") {
        Ok(output) => {
            let running = output.status.success();
            if let Some(state) = state {
                if !running {
                    let mut guard = state
                        .remote_base
                        .lock()
                        .expect("remote base lock should be available");
                    *guard = None;
                }
            }
            Json(GameStatusResponse {
                running,
                detail: output_detail(&output),
            })
        }
        Err(err) => Json(GameStatusResponse {
            running: false,
            detail: err,
        }),
    }
}

async fn game_status_with_state(State(state): State<AppState>) -> Json<GameStatusResponse> {
    game_status_impl(Some(state)).await
}

async fn game_launch(State(state): State<AppState>) -> Json<GameLaunchResponse> {
    let port = state
        .remote_base
        .lock()
        .expect("remote base lock should be available")
        .as_ref()
        .and_then(|base| base.rsplit(':').next().and_then(|p| p.parse::<u16>().ok()))
        .unwrap_or_else(|| pick_free_local_port().unwrap_or(0));

    if port == 0 {
        return Json(GameLaunchResponse {
            ok: false,
            detail: "failed allocating a local port for the headful editor api".to_string(),
        });
    }

    let env = [(
        "ROLLOUT_HEADFUL_EDITOR_PORT".to_string(),
        port.to_string(),
    )];

    match run_go_sh_with_env("--restart --game --detach", &env) {
        Ok(output) => {
            let ok = output.status.success();
            if ok {
                let mut guard = state
                    .remote_base
                    .lock()
                    .expect("remote base lock should be available");
                *guard = Some(format!("http://127.0.0.1:{port}"));
            }
            Json(GameLaunchResponse {
                ok,
                detail: output_detail(&output),
            })
        }
        Err(err) => Json(GameLaunchResponse {
            ok: false,
            detail: err,
        }),
    }
}

async fn manifest() -> Json<EditorManifest> {
    Json(default_manifest())
}

async fn agent_state(State(state): State<AppState>) -> Result<Json<EditorSnapshot>, (StatusCode, String)> {
    let (status, bytes) = proxy_to_game(&state, Method::GET, "/api/agent/state", None).await?;
    if !status.is_success() {
        return Err((status, String::from_utf8_lossy(&bytes).to_string()));
    }
    let snapshot: EditorSnapshot =
        serde_json::from_slice(&bytes).map_err(|e| (StatusCode::BAD_GATEWAY, e.to_string()))?;
    Ok(Json(snapshot))
}

async fn agent_timeline(
    State(state): State<AppState>,
) -> Result<Json<EditorTimeline>, (StatusCode, String)> {
    let (status, bytes) = proxy_to_game(&state, Method::GET, "/api/agent/timeline", None).await?;
    if !status.is_success() {
        return Err((status, String::from_utf8_lossy(&bytes).to_string()));
    }
    let timeline: EditorTimeline =
        serde_json::from_slice(&bytes).map_err(|e| (StatusCode::BAD_GATEWAY, e.to_string()))?;
    Ok(Json(timeline))
}

async fn agent_step(
    State(state): State<AppState>,
    Json(payload): Json<StepRequest>,
) -> Result<Json<EditorSnapshot>, (StatusCode, String)> {
    let body = serde_json::to_vec(&payload)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let (status, bytes) = proxy_to_game(&state, Method::POST, "/api/agent/step", Some(body)).await?;
    if !status.is_success() {
        return Err((status, String::from_utf8_lossy(&bytes).to_string()));
    }
    let snapshot: EditorSnapshot =
        serde_json::from_slice(&bytes).map_err(|e| (StatusCode::BAD_GATEWAY, e.to_string()))?;
    Ok(Json(snapshot))
}

async fn agent_rewind(
    State(state): State<AppState>,
    Json(payload): Json<FramesRequest>,
) -> Result<Json<EditorSnapshot>, (StatusCode, String)> {
    let body = serde_json::to_vec(&payload)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let (status, bytes) = proxy_to_game(&state, Method::POST, "/api/agent/rewind", Some(body)).await?;
    if !status.is_success() {
        return Err((status, String::from_utf8_lossy(&bytes).to_string()));
    }
    let snapshot: EditorSnapshot =
        serde_json::from_slice(&bytes).map_err(|e| (StatusCode::BAD_GATEWAY, e.to_string()))?;
    Ok(Json(snapshot))
}

async fn agent_forward(
    State(state): State<AppState>,
    Json(payload): Json<FramesRequest>,
) -> Result<Json<EditorSnapshot>, (StatusCode, String)> {
    let body = serde_json::to_vec(&payload)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let (status, bytes) =
        proxy_to_game(&state, Method::POST, "/api/agent/forward", Some(body)).await?;
    if !status.is_success() {
        return Err((status, String::from_utf8_lossy(&bytes).to_string()));
    }
    let snapshot: EditorSnapshot =
        serde_json::from_slice(&bytes).map_err(|e| (StatusCode::BAD_GATEWAY, e.to_string()))?;
    Ok(Json(snapshot))
}

async fn agent_seek(
    State(state): State<AppState>,
    Json(payload): Json<SeekRequest>,
) -> Result<Json<EditorSnapshot>, (StatusCode, String)> {
    let body = serde_json::to_vec(&payload)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let (status, bytes) = proxy_to_game(&state, Method::POST, "/api/agent/seek", Some(body)).await?;
    if !status.is_success() {
        return Err((status, String::from_utf8_lossy(&bytes).to_string()));
    }
    let snapshot: EditorSnapshot =
        serde_json::from_slice(&bytes).map_err(|e| (StatusCode::BAD_GATEWAY, e.to_string()))?;
    Ok(Json(snapshot))
}

async fn agent_reset(State(state): State<AppState>) -> Result<Json<EditorSnapshot>, (StatusCode, String)> {
    let (status, bytes) = proxy_to_game(&state, Method::POST, "/api/agent/reset", None).await?;
    if !status.is_success() {
        return Err((status, String::from_utf8_lossy(&bytes).to_string()));
    }
    let snapshot: EditorSnapshot =
        serde_json::from_slice(&bytes).map_err(|e| (StatusCode::BAD_GATEWAY, e.to_string()))?;
    Ok(Json(snapshot))
}

#[tokio::main]
async fn main() {
    let state = AppState {
        remote_base: Arc::new(Mutex::new(None)),
        client: build_http_client(),
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

    use axum::body::Body;
    use axum::http::{header, Method, Request};
    use http_body_util::BodyExt;
    use serde::de::DeserializeOwned;
    use serde::Deserialize;
    use tower::ServiceExt;

    use std::collections::BTreeMap;
    use std::fs;
    use std::time::{Duration, SystemTime, UNIX_EPOCH};

    use game::editor_api::{EditorApiError, EditorSession};

    #[derive(Clone)]
    struct StubGameState {
        session: Arc<Mutex<EditorSession>>,
    }

    fn stub_game_router(state: StubGameState) -> Router {
        Router::new()
            .route("/api/agent/state", get(stub_state))
            .route("/api/agent/timeline", get(stub_timeline))
            .route("/api/agent/step", post(stub_step))
            .route("/api/agent/rewind", post(stub_rewind))
            .route("/api/agent/forward", post(stub_forward))
            .route("/api/agent/seek", post(stub_seek))
            .route("/api/agent/reset", post(stub_reset))
            .with_state(state)
    }

    async fn stub_state(State(state): State<StubGameState>) -> Json<EditorSnapshot> {
        let snapshot = state
            .session
            .lock()
            .expect("stub session lock")
            .state();
        Json(snapshot)
    }

    async fn stub_timeline(State(state): State<StubGameState>) -> Json<EditorTimeline> {
        let timeline = state
            .session
            .lock()
            .expect("stub session lock")
            .timeline();
        Json(timeline)
    }

    async fn stub_step(
        State(state): State<StubGameState>,
        Json(payload): Json<StepRequest>,
    ) -> Result<Json<EditorSnapshot>, (StatusCode, String)> {
        let snapshot = state
            .session
            .lock()
            .expect("stub session lock")
            .step(&payload.action_id);
        match snapshot {
            Ok(snapshot) => Ok(Json(snapshot)),
            Err(EditorApiError::UnknownActionId(action_id)) => Err((
                StatusCode::BAD_REQUEST,
                format!("unknown actionId: {}", action_id),
            )),
        }
    }

    async fn stub_rewind(
        State(state): State<StubGameState>,
        Json(payload): Json<FramesRequest>,
    ) -> Json<EditorSnapshot> {
        let snapshot = state
            .session
            .lock()
            .expect("stub session lock")
            .rewind(payload.frames);
        Json(snapshot)
    }

    async fn stub_forward(
        State(state): State<StubGameState>,
        Json(payload): Json<FramesRequest>,
    ) -> Json<EditorSnapshot> {
        let snapshot = state
            .session
            .lock()
            .expect("stub session lock")
            .forward(payload.frames);
        Json(snapshot)
    }

    async fn stub_seek(
        State(state): State<StubGameState>,
        Json(payload): Json<SeekRequest>,
    ) -> Json<EditorSnapshot> {
        let snapshot = state
            .session
            .lock()
            .expect("stub session lock")
            .seek(payload.frame);
        Json(snapshot)
    }

    async fn stub_reset(State(state): State<StubGameState>) -> Json<EditorSnapshot> {
        let snapshot = state.session.lock().expect("stub session lock").reset();
        Json(snapshot)
    }

    async fn start_stub_game() -> (SocketAddr, tokio::sync::oneshot::Sender<()>) {
        let state = StubGameState {
            session: Arc::new(Mutex::new(EditorSession::new(0))),
        };
        let app = stub_game_router(state);

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind stub game server");
        let addr = listener.local_addr().expect("stub addr");
        let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel::<()>();

        tokio::spawn(async move {
            let _ = axum::serve(listener, app)
                .with_graceful_shutdown(async move {
                    let _ = shutdown_rx.await;
                })
                .await;
        });

        (addr, shutdown_tx)
    }

    fn test_state(remote_base: Option<String>) -> AppState {
        AppState {
            remote_base: Arc::new(Mutex::new(remote_base)),
            client: build_http_client(),
        }
    }

    static E2E_LOCK: std::sync::OnceLock<Mutex<()>> = std::sync::OnceLock::new();

    fn e2e_lock() -> std::sync::MutexGuard<'static, ()> {
        E2E_LOCK
            .get_or_init(|| Mutex::new(()))
            .lock()
            .expect("e2e lock should be available")
    }

    struct GoShTestOverrides {
        prev_root: Option<PathBuf>,
        prev_env: BTreeMap<String, String>,
    }

    impl GoShTestOverrides {
        fn new(root: Option<PathBuf>, env: BTreeMap<String, String>) -> Self {
            let prev_root = {
                let mut guard = TEST_GO_SH_ROOT
                    .get_or_init(|| Mutex::new(None))
                    .lock()
                    .expect("test go.sh root lock should be available");
                let prev = guard.clone();
                *guard = root;
                prev
            };

            let prev_env = {
                let mut guard = TEST_GO_SH_ENV
                    .get_or_init(|| Mutex::new(BTreeMap::new()))
                    .lock()
                    .expect("test go.sh env override lock should be available");
                let prev = guard.clone();
                *guard = env;
                prev
            };

            Self { prev_root, prev_env }
        }
    }

    impl Drop for GoShTestOverrides {
        fn drop(&mut self) {
            let mut guard = TEST_GO_SH_ROOT
                .get_or_init(|| Mutex::new(None))
                .lock()
                .expect("test go.sh root lock should be available");
            *guard = self.prev_root.clone();

            let mut guard = TEST_GO_SH_ENV
                .get_or_init(|| Mutex::new(BTreeMap::new()))
                .lock()
                .expect("test go.sh env override lock should be available");
            *guard = self.prev_env.clone();
        }
    }

    fn unique_temp_dir(prefix: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        std::env::temp_dir().join(format!("{prefix}_{nanos}"))
    }

    async fn http_call(app: &Router, req: Request<Body>) -> (StatusCode, Vec<u8>) {
        let res = app.clone().oneshot(req).await.expect("router call should succeed");
        let status = res.status();
        let body = res
            .into_body()
            .collect()
            .await
            .expect("response body should be readable")
            .to_bytes()
            .to_vec();
        (status, body)
    }

    async fn http_get_json<T: DeserializeOwned>(app: &Router, uri: &str) -> T {
        let req = Request::builder()
            .method(Method::GET)
            .uri(uri)
            .body(Body::empty())
            .expect("build request");
        let (status, body) = http_call(app, req).await;
        assert_eq!(
            status,
            StatusCode::OK,
            "GET {uri} failed: {}",
            String::from_utf8_lossy(&body)
        );
        serde_json::from_slice(&body).expect("response should be valid json")
    }

    async fn http_post(app: &Router, uri: &str, json: serde_json::Value) -> (StatusCode, Vec<u8>) {
        let body = serde_json::to_vec(&json).expect("serialize json");
        let req = Request::builder()
            .method(Method::POST)
            .uri(uri)
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(body))
            .expect("build request");
        http_call(app, req).await
    }

    async fn http_post_json_ok<T: DeserializeOwned>(
        app: &Router,
        uri: &str,
        json: serde_json::Value,
    ) -> T {
        let (status, body) = http_post(app, uri, json).await;
        assert_eq!(
            status,
            StatusCode::OK,
            "POST {uri} failed: {}",
            String::from_utf8_lossy(&body)
        );
        serde_json::from_slice(&body).expect("response should be valid json")
    }

    #[derive(Debug, Clone, Deserialize)]
    #[serde(rename_all = "camelCase")]
    #[allow(dead_code)]
    struct GameStatus {
        running: bool,
        detail: String,
    }

    #[derive(Debug, Clone, Deserialize)]
    #[serde(rename_all = "camelCase")]
    #[allow(dead_code)]
    struct GameLaunch {
        ok: bool,
        detail: String,
    }

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

    #[tokio::test]
    async fn e2e_editor_timeline_seek_rewind_forward_roundtrips() {
        let (addr, shutdown) = start_stub_game().await;
        let app = router(test_state(Some(format!("http://{addr}"))));

        let tl0: EditorTimeline = http_get_json(&app, "/api/agent/timeline").await;
        assert_eq!(tl0.frame, 0);
        assert_eq!(tl0.history_len, 1);
        assert!(!tl0.can_rewind);
        assert!(!tl0.can_forward);

        let s1: EditorSnapshot =
            http_post_json_ok(&app, "/api/agent/step", serde_json::json!({"actionId":"noop"}))
                .await;
        assert_eq!(s1.frame, 1);

        let s2: EditorSnapshot =
            http_post_json_ok(&app, "/api/agent/step", serde_json::json!({"actionId":"noop"}))
                .await;
        assert_eq!(s2.frame, 2);

        let tl2: EditorTimeline = http_get_json(&app, "/api/agent/timeline").await;
        assert_eq!(tl2.frame, 2);
        assert_eq!(tl2.history_len, 3);
        assert!(tl2.can_rewind);
        assert!(!tl2.can_forward);

        let rewound: EditorSnapshot =
            http_post_json_ok(&app, "/api/agent/rewind", serde_json::json!({"frames":1})).await;
        assert_eq!(rewound.frame, 1);
        let tl_rewound: EditorTimeline = http_get_json(&app, "/api/agent/timeline").await;
        assert_eq!(tl_rewound.frame, 1);
        assert!(tl_rewound.can_rewind);
        assert!(tl_rewound.can_forward);

        let forwarded: EditorSnapshot =
            http_post_json_ok(&app, "/api/agent/forward", serde_json::json!({"frames":1})).await;
        assert_eq!(forwarded.frame, 2);
        let tl_forwarded: EditorTimeline = http_get_json(&app, "/api/agent/timeline").await;
        assert_eq!(tl_forwarded.frame, 2);
        assert!(tl_forwarded.can_rewind);
        assert!(!tl_forwarded.can_forward);

        // Seeking beyond the end should clamp to the last recorded frame.
        let clamped: EditorSnapshot =
            http_post_json_ok(&app, "/api/agent/seek", serde_json::json!({"frame":999})).await;
        assert_eq!(clamped.frame, 2);

        // Seek back to the start.
        let start: EditorSnapshot =
            http_post_json_ok(&app, "/api/agent/seek", serde_json::json!({"frame":0})).await;
        assert_eq!(start.frame, 0);
        let tl_start: EditorTimeline = http_get_json(&app, "/api/agent/timeline").await;
        assert_eq!(tl_start.frame, 0);
        assert!(!tl_start.can_rewind);
        assert!(tl_start.can_forward);

        let _ = shutdown.send(());
    }

    #[tokio::test]
    async fn e2e_editor_step_rejects_unknown_action_id() {
        let (addr, shutdown) = start_stub_game().await;
        let app = router(test_state(Some(format!("http://{addr}"))));

        let (status, body) = http_post(
            &app,
            "/api/agent/step",
            serde_json::json!({"actionId":"doesNotExist"}),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        let msg = String::from_utf8_lossy(&body);
        assert!(
            msg.contains("unknown actionId"),
            "expected unknown action error, got: {msg}"
        );

        let _ = shutdown.send(());
    }

    #[tokio::test]
    async fn e2e_editor_can_launch_game_and_report_status_via_go_sh_stub() {
        let _lock = e2e_lock();

        let stub_root = unique_temp_dir("rollout_editor_go_sh_stub");
        fs::create_dir_all(&stub_root).expect("create stub root");
        let stub_go_sh = stub_root.join("go.sh");
        fs::write(
            &stub_go_sh,
            r#"#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PID_FILE="$ROOT_DIR/.go.pid"

ACTION=""
TARGET="game"

while [[ $# -gt 0 ]]; do
  case "$1" in
    --start|--stop|--status|--restart)
      ACTION="$1"
      shift
      ;;
    --game|--editor|--detach|--foreground|--headful|--headless|--release)
      shift
      ;;
    *)
      shift
      ;;
  esac
done

case "$ACTION" in
  --status)
    if [[ -f "$PID_FILE" ]]; then
      echo "running (pid $(cat "$PID_FILE" 2>/dev/null || echo "?"))"
      exit 0
    fi
    echo "not running"
    exit 1
    ;;
  --start)
    echo $$ >"$PID_FILE"
    echo "started headful game (pid $$)"
    exit 0
    ;;
  --restart)
    rm -f "$PID_FILE"
    echo $$ >"$PID_FILE"
    echo "restarted headful game (pid $$)"
    exit 0
    ;;
  --stop)
    rm -f "$PID_FILE"
    echo "stopped"
    exit 0
    ;;
  *)
    echo "unsupported stub action: $ACTION" >&2
    exit 2
    ;;
esac
"#,
        )
        .expect("write stub go.sh");

        let _overrides = GoShTestOverrides::new(Some(stub_root.clone()), BTreeMap::new());

        let app = router(test_state(None));

        let status0: GameStatus = http_get_json(&app, "/api/game/status").await;
        assert!(!status0.running, "expected stopped, got: {:?}", status0);

        let launched: GameLaunch =
            http_post_json_ok(&app, "/api/game/launch", serde_json::json!({})).await;
        assert!(launched.ok, "expected ok launch, got: {:?}", launched);

        // The stub should now report running.
        let status1: GameStatus = http_get_json(&app, "/api/game/status").await;
        assert!(status1.running, "expected running, got: {:?}", status1);

        // And the editor API should be able to stop it (via go.sh).
        let output = run_go_sh("--stop --game").expect("stop via stub go.sh");
        assert!(
            output.status.success(),
            "stub stop should succeed: {}",
            output_detail(&output)
        );

        let status2: GameStatus = http_get_json(&app, "/api/game/status").await;
        assert!(!status2.running, "expected stopped, got: {:?}", status2);

        let _ = fs::remove_dir_all(stub_root);
    }

    fn env_flag(name: &str) -> bool {
        std::env::var(name)
            .ok()
            .map(|v| matches!(v.to_ascii_lowercase().as_str(), "1" | "true" | "yes" | "on"))
            .unwrap_or(false)
    }

    #[tokio::test]
    #[ignore]
    async fn e2e_editor_can_launch_real_game_via_go_sh() {
        if !env_flag("ROLLOUT_E2E_EDITOR_REAL_GAME_LAUNCH") {
            eprintln!("skipping: set ROLLOUT_E2E_EDITOR_REAL_GAME_LAUNCH=1 to enable");
            return;
        }

        let _lock = e2e_lock();

        let tmp = unique_temp_dir("rollout_editor_real_game_launch");
        fs::create_dir_all(&tmp).expect("create temp dir");

        let pid = tmp.join("game.pid");
        let log = tmp.join("game.log");

        // Pass isolated pid/log locations to go.sh so this test doesn't interfere with a
        // developer's interactive ./go.sh session.
        let mut env = BTreeMap::new();
        env.insert(
            "ROLLOUT_GO_GAME_PID_FILE".to_string(),
            pid.to_string_lossy().replace('\\', "/"),
        );
        env.insert(
            "ROLLOUT_GO_GAME_LOG_FILE".to_string(),
            log.to_string_lossy().replace('\\', "/"),
        );

        let _overrides = GoShTestOverrides::new(None, env);

        // Ensure a clean slate for this isolated pidfile set.
        let _ = run_go_sh("--stop --game");

        let app = router(test_state(None));

        let status0: GameStatus = http_get_json(&app, "/api/game/status").await;
        assert!(!status0.running, "expected stopped, got: {:?}", status0);

        let launched: GameLaunch =
            http_post_json_ok(&app, "/api/game/launch", serde_json::json!({})).await;
        assert!(
            launched.ok,
            "expected ok launch (see logs for details): {:?}",
            launched
        );

        // Wait briefly for go.sh to finish wiring up the pidfile and for the process to be alive.
        let mut running = false;
        for _ in 0..20 {
            let status: GameStatus = http_get_json(&app, "/api/game/status").await;
            if status.running {
                running = true;
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        assert!(running, "game never reported running after launch");

        let stopped = run_go_sh("--stop --game").expect("stop game via go.sh");
        assert!(
            stopped.status.success(),
            "expected go.sh stop to succeed: {}",
            output_detail(&stopped)
        );

        let _ = fs::remove_dir_all(tmp);
    }
}

