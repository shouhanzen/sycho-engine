use std::{
    io,
    net::{IpAddr, Ipv4Addr, SocketAddr, TcpListener},
    thread,
    time::Duration,
};

use axum::{
    Json, Router,
    extract::State,
    http::StatusCode,
    routing::{get, post},
};
use serde::Serialize;
use tokio::sync::{mpsc, oneshot};
use tower_http::cors::{Any, CorsLayer};

use engine::editor::{
    EditorManifest, EditorSnapshot, EditorTimeline, FramesRequest, SeekRequest, StepRequest,
};

use crate::editor_actions;

#[derive(Debug)]
pub enum RemoteCmd {
    GetState {
        respond: oneshot::Sender<EditorSnapshot>,
    },
    GetTimeline {
        respond: oneshot::Sender<EditorTimeline>,
    },
    Step {
        action_id: String,
        respond: oneshot::Sender<Result<EditorSnapshot, String>>,
    },
    Rewind {
        frames: usize,
        respond: oneshot::Sender<EditorSnapshot>,
    },
    Forward {
        frames: usize,
        respond: oneshot::Sender<EditorSnapshot>,
    },
    Seek {
        frame: usize,
        respond: oneshot::Sender<EditorSnapshot>,
    },
    Reset {
        respond: oneshot::Sender<EditorSnapshot>,
    },
}

#[derive(Clone)]
struct RemoteState {
    tx: mpsc::UnboundedSender<RemoteCmd>,
}

async fn health() -> &'static str {
    "ok"
}

fn default_manifest() -> EditorManifest {
    editor_actions::default_manifest()
}

async fn manifest() -> Json<EditorManifest> {
    Json(default_manifest())
}

async fn send_cmd<T>(
    tx: &mpsc::UnboundedSender<RemoteCmd>,
    cmd: RemoteCmd,
    rx: oneshot::Receiver<T>,
) -> Result<T, (StatusCode, String)> {
    tx.send(cmd).map_err(|_| {
        (
            StatusCode::SERVICE_UNAVAILABLE,
            "game command channel closed".to_string(),
        )
    })?;

    match tokio::time::timeout(Duration::from_secs(2), rx).await {
        Ok(Ok(v)) => Ok(v),
        Ok(Err(_)) => Err((
            StatusCode::SERVICE_UNAVAILABLE,
            "game did not respond".to_string(),
        )),
        Err(_) => Err((StatusCode::GATEWAY_TIMEOUT, "game timed out".to_string())),
    }
}

async fn agent_state(
    State(state): State<RemoteState>,
) -> Result<Json<EditorSnapshot>, (StatusCode, String)> {
    let (tx, rx) = oneshot::channel();
    let snapshot = send_cmd(&state.tx, RemoteCmd::GetState { respond: tx }, rx).await?;
    Ok(Json(snapshot))
}

async fn agent_timeline(
    State(state): State<RemoteState>,
) -> Result<Json<EditorTimeline>, (StatusCode, String)> {
    let (tx, rx) = oneshot::channel();
    let timeline = send_cmd(&state.tx, RemoteCmd::GetTimeline { respond: tx }, rx).await?;
    Ok(Json(timeline))
}

async fn agent_step(
    State(state): State<RemoteState>,
    Json(payload): Json<StepRequest>,
) -> Result<Json<EditorSnapshot>, (StatusCode, String)> {
    let (tx, rx) = oneshot::channel();
    let res = send_cmd(
        &state.tx,
        RemoteCmd::Step {
            action_id: payload.action_id,
            respond: tx,
        },
        rx,
    )
    .await?;

    match res {
        Ok(snapshot) => Ok(Json(snapshot)),
        Err(msg) => Err((StatusCode::BAD_REQUEST, msg)),
    }
}

async fn agent_rewind(
    State(state): State<RemoteState>,
    Json(payload): Json<FramesRequest>,
) -> Result<Json<EditorSnapshot>, (StatusCode, String)> {
    let (tx, rx) = oneshot::channel();
    let snapshot = send_cmd(
        &state.tx,
        RemoteCmd::Rewind {
            frames: payload.frames,
            respond: tx,
        },
        rx,
    )
    .await?;
    Ok(Json(snapshot))
}

async fn agent_forward(
    State(state): State<RemoteState>,
    Json(payload): Json<FramesRequest>,
) -> Result<Json<EditorSnapshot>, (StatusCode, String)> {
    let (tx, rx) = oneshot::channel();
    let snapshot = send_cmd(
        &state.tx,
        RemoteCmd::Forward {
            frames: payload.frames,
            respond: tx,
        },
        rx,
    )
    .await?;
    Ok(Json(snapshot))
}

async fn agent_seek(
    State(state): State<RemoteState>,
    Json(payload): Json<SeekRequest>,
) -> Result<Json<EditorSnapshot>, (StatusCode, String)> {
    let (tx, rx) = oneshot::channel();
    let snapshot = send_cmd(
        &state.tx,
        RemoteCmd::Seek {
            frame: payload.frame,
            respond: tx,
        },
        rx,
    )
    .await?;
    Ok(Json(snapshot))
}

async fn agent_reset(
    State(state): State<RemoteState>,
) -> Result<Json<EditorSnapshot>, (StatusCode, String)> {
    let (tx, rx) = oneshot::channel();
    let snapshot = send_cmd(&state.tx, RemoteCmd::Reset { respond: tx }, rx).await?;
    Ok(Json(snapshot))
}

fn router(tx: mpsc::UnboundedSender<RemoteCmd>) -> Router {
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
        .route("/api/agent/seek", post(agent_seek))
        .route("/api/agent/reset", post(agent_reset))
        .with_state(RemoteState { tx })
        .layer(cors)
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteServerInfo {
    pub addr: SocketAddr,
}

pub struct RemoteServer {
    pub rx: mpsc::UnboundedReceiver<RemoteCmd>,
    shutdown: Option<oneshot::Sender<()>>,
    pub info: RemoteServerInfo,
}

impl RemoteServer {
    pub fn start(port: u16) -> io::Result<Self> {
        let (tx, rx) = mpsc::unbounded_channel::<RemoteCmd>();
        let (shutdown_tx, shutdown_rx) = oneshot::channel::<()>();

        let addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), port);

        // Bind synchronously so we can fail fast if the port is unavailable.
        let std_listener = TcpListener::bind(addr)?;
        std_listener.set_nonblocking(true)?;

        let info = RemoteServerInfo { addr };

        thread::spawn(move || {
            let rt = tokio::runtime::Runtime::new().expect("remote editor api tokio runtime");
            rt.block_on(async move {
                let listener = tokio::net::TcpListener::from_std(std_listener)
                    .expect("remote editor api listener should convert");
                let app = router(tx);

                let serve = axum::serve(listener, app).with_graceful_shutdown(async move {
                    let _ = shutdown_rx.await;
                });

                if let Err(err) = serve.await {
                    eprintln!("remote editor api server error: {err}");
                }
            });
        });

        Ok(Self {
            rx,
            shutdown: Some(shutdown_tx),
            info,
        })
    }

    pub fn shutdown(&mut self) {
        if let Some(tx) = self.shutdown.take() {
            let _ = tx.send(());
        }
    }
}
