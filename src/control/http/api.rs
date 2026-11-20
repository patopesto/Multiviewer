use std::net::IpAddr;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};

use axum::body::Bytes;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{DefaultBodyLimit, Query, Request, State};
use axum::http::header::{CACHE_CONTROL, CONTENT_DISPOSITION, CONTENT_TYPE, HOST, ORIGIN};
use axum::http::{HeaderMap, StatusCode};
use axum::middleware::{self, Next};
use axum::response::{Html, IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::sync::broadcast;

use super::protocol::{self, Frame, InputEvent};
use super::server::Status;
use crate::config::Config;
use crate::control::Command;
use crate::engine::{ControlError, Engine, EngineSnapshot, EngineState, ProjectError, SnapshotSlot, publish_state};
use crate::sources::SourceKey;
use crate::APP_NAME;

const MAX_PROJECT_BYTES: usize = 2 * 1024 * 1024;

/// Everything the handlers need. The engine is only locked for writes.
#[derive(Clone)]
pub struct ApiHandles {
    pub engine: Arc<Mutex<Engine>>,
    pub snapshots: SnapshotSlot,
    pub frames: broadcast::Sender<Frame>,
    pub inputs: Sender<InputEvent>,
    pub clients: Arc<AtomicUsize>,
}

#[derive(Clone)]
struct ApiState {
    handles: ApiHandles,
    status: Arc<Mutex<Status>>,
    bind: String,
}

/// Bump when adding or removing a documented HTTP route; the docs route table
/// and the `documented_routes_parse` test are checked against this value.
/// Static assets (the favicon) are served without a docs entry.
#[cfg(test)]
const HTTP_ROUTE_COUNT: usize = 10;

pub fn router(handles: ApiHandles, status: Arc<Mutex<Status>>, bind: String) -> Router {
    let state = ApiState { handles, status, bind };
    return Router::new()
        // @http method="GET" path="/" group="Web UI" desc="Web UI. Add `?view=1` for read-only."
        .route("/", get(index))
        .route("/favicon.png", get(favicon))
        // @http method="GET" path="/api/v1/state" group="State" desc="Document state: canvas, sources, selection, view, revision."
        .route("/api/v1/state", get(state_handler))
        // @http method="GET" path="/api/v1/status" group="State" desc="Live source and output statistics, listener status, client count."
        .route("/api/v1/status", get(status_handler))
        // @http method="GET" path="/api/v1/project" group="Project" desc="Download the project."
        // @http method="PUT" path="/api/v1/project" group="Project" desc="Upload and apply a project; `?path=` saves it."
        .route("/api/v1/project", get(get_project).put(put_project))
        // @http method="POST" path="/api/v1/project/save" group="Project" desc="Save to the current project path."
        .route("/api/v1/project/save", post(save_project))
        // @http method="POST" path="/api/v1/project/open" group="Project" desc="Open a project path on this machine."
        .route("/api/v1/project/open", post(open_project))
        // @http method="POST" path="/api/v1/project/new" group="Project" desc="Start an empty project."
        .route("/api/v1/project/new", post(new_project))
        // @http method="POST" path="/api/v1/commands" group="Commands" desc="Run one control command."
        .route("/api/v1/commands", post(run_command))
        // @http method="GET" path="/ws" group="Web UI" desc="Browser socket."
        .route("/ws", get(ws))
        .layer(DefaultBodyLimit::max(MAX_PROJECT_BYTES))
        .layer(middleware::from_fn_with_state(state.clone(), check_origin))
        .with_state(state);
}

/// Reject browser requests that do not come from this listener. This is not
/// authentication: requests without an Origin (curl, native clients) still
/// work, and the web UI is same-origin.
async fn check_origin(State(state): State<ApiState>, request: Request, next: Next) -> Response {
    let path = request.uri().path();
    if path != "/ws" && !path.starts_with("/api/") {
        return next.run(request).await;
    }
    let headers = request.headers();
    let host = headers.get(HOST).and_then(|value| value.to_str().ok()).unwrap_or("");
    let forwarded = headers
        .get("x-forwarded-host")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.split(',').next())
        .map(str::trim)
        .filter(|value| !value.is_empty());
    let effective_host = forwarded.unwrap_or(host);
    if !host_allowed(host, &state.bind) && !forwarded.is_some_and(|forwarded| forwarded.eq_ignore_ascii_case(host)) {
        return StatusCode::FORBIDDEN.into_response();
    }
    if let Some(origin) = headers.get(ORIGIN) {
        let Ok(origin) = origin.to_str() else {
            return StatusCode::FORBIDDEN.into_response();
        };
        if !same_authority(origin, effective_host) {
            return StatusCode::FORBIDDEN.into_response();
        }
    }
    return next.run(request).await;
}

fn same_authority(origin: &str, host: &str) -> bool {
    let Some((scheme, authority)) = origin.split_once("://") else {
        return false;
    };
    if authority.is_empty() {
        return false;
    }
    if authority.eq_ignore_ascii_case(host) {
        return true;
    }
    let default_port = match scheme.to_ascii_lowercase().as_str() {
        "http" => ":80",
        "https" => ":443",
        _ => return false,
    };
    return authority.strip_suffix(default_port).is_some_and(|stripped| stripped.eq_ignore_ascii_case(host))
        || host.strip_suffix(default_port).is_some_and(|stripped| stripped.eq_ignore_ascii_case(authority));
}

/// A browser request must target this machine: loopback, an IP literal, or the
/// configured bind. That blocks DNS rebinding through arbitrary hostnames.
fn host_allowed(host_header: &str, bind: &str) -> bool {
    let Some(host) = authority_host(host_header) else {
        return false;
    };
    if host.eq_ignore_ascii_case("localhost") {
        return true;
    }
    if host.parse::<IpAddr>().is_ok() {
        return true;
    }
    let bind_host = authority_host(bind).unwrap_or("");
    return !bind_host.is_empty() && host.eq_ignore_ascii_case(bind_host);
}

fn authority_host(authority: &str) -> Option<&str> {
    let authority = authority.trim();
    if authority.is_empty() {
        return None;
    }
    if authority.parse::<IpAddr>().is_ok() {
        return Some(authority);
    }
    if let Some(rest) = authority.strip_prefix('[') {
        return rest.find(']').map(|end| &rest[..end]);
    }
    return match authority.rsplit_once(':') {
        Some((host, port)) if port.chars().all(|character| character.is_ascii_digit()) => Some(host),
        _ => Some(authority),
    };
}

async fn index() -> Html<&'static str> {
    return Html(protocol::INDEX_HTML);
}

async fn favicon() -> impl IntoResponse {
    let icon: &'static [u8] = include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/assets/appicon/linux/128x128.png"));
    return ([(CONTENT_TYPE, "image/png"), (CACHE_CONTROL, "public, max-age=604800")], icon);
}

async fn state_handler(State(state): State<ApiState>) -> Result<Json<EngineState>, ApiError> {
    let snapshot = snapshot(&state)?;
    return Ok(Json(snapshot.state.clone()));
}

#[derive(Serialize)]
struct StatusResponse {
    app: &'static str,
    version: &'static str,
    revision: u64,
    project_path: Option<String>,
    dirty: bool,
    canvas: CanvasStatus,
    selected_source: Option<String>,
    expanded_source: Option<String>,
    ui_visible: bool,
    view: ViewStatus,
    sources: Vec<SourceStatus>,
    outputs: Vec<OutputStatus>,
    osc: String,
    http: String,
    clients: usize,
}

#[derive(Serialize)]
struct CanvasStatus {
    width: u32,
    height: u32,
}

#[derive(Serialize)]
struct ViewStatus {
    zoom: f32,
    pan: [f32; 2],
}

#[derive(Serialize)]
struct SourceStatus {
    id: String,
    name: String,
    protocol: String,
    x: f32,
    y: f32,
    width: u32,
    height: u32,
    z: i32,
    connected: bool,
    frame_width: u32,
    frame_height: u32,
    fps: f64,
    frames_received: u64,
    frames_dropped: u64,
    restart_pending: bool,
}

#[derive(Serialize)]
struct OutputStatus {
    id: String,
    name: String,
    protocol: String,
    enabled: bool,
    frame_width: u32,
    frame_height: u32,
    fps: f64,
    frames_sent: u64,
    frames_dropped: u64,
    error: Option<String>,
}

async fn status_handler(State(state): State<ApiState>) -> Result<Json<StatusResponse>, ApiError> {
    let snapshot = snapshot(&state)?;
    let canvas = &snapshot.state.cfg.canvas;
    let sources = canvas
        .sources
        .iter()
        .map(|source| {
            let runtime = source
                .source_ref
                .as_ref()
                .and_then(|source_ref| snapshot.runtime.source(&SourceKey::new(source.protocol.clone(), source_ref.clone())));
            return SourceStatus {
                id: source.uuid.clone(),
                name: source.name.clone(),
                protocol: source.protocol.label(),
                x: source.x,
                y: source.y,
                width: source.width,
                height: source.height,
                z: source.z,
                connected: runtime.is_some(),
                frame_width: runtime.map(|view| view.stats.width).unwrap_or(0),
                frame_height: runtime.map(|view| view.stats.height).unwrap_or(0),
                fps: runtime.map(|view| view.stats.computed_fps).unwrap_or(0.0),
                frames_received: runtime.map(|view| view.stats.frames_received).unwrap_or(0),
                frames_dropped: runtime.map(|view| view.stats.frames_dropped).unwrap_or(0),
                restart_pending: runtime.map(|view| view.restart_pending).unwrap_or(false),
            };
        })
        .collect();
    let outputs = canvas
        .outputs
        .iter()
        .map(|output| {
            let runtime = snapshot.runtime.outputs.iter().find(|view| view.id == output.uuid);
            return OutputStatus {
                id: output.uuid.clone(),
                name: output.name.clone(),
                protocol: output.protocol.label(),
                enabled: output.enabled,
                frame_width: runtime.map(|view| view.stats.width).unwrap_or(0),
                frame_height: runtime.map(|view| view.stats.height).unwrap_or(0),
                fps: runtime.map(|view| view.stats.computed_fps).unwrap_or(0.0),
                frames_sent: runtime.map(|view| view.stats.frames_sent).unwrap_or(0),
                frames_dropped: runtime.map(|view| view.stats.frames_dropped).unwrap_or(0),
                error: runtime.and_then(|view| view.stats.error.clone()),
            };
        })
        .collect();
    let http = state.status.lock().unwrap().clone();
    let clients = state.handles.clients.load(Ordering::Relaxed);
    return Ok(Json(StatusResponse {
        app: APP_NAME,
        version: env!("CARGO_PKG_VERSION"),
        revision: snapshot.state.revision,
        project_path: snapshot.state.project_path.as_ref().map(|path| path.display().to_string()),
        dirty: snapshot.state.dirty,
        canvas: CanvasStatus {
            width: canvas.width,
            height: canvas.height,
        },
        selected_source: snapshot.state.selected_source_id.clone(),
        expanded_source: snapshot.state.expanded_source_id.clone(),
        ui_visible: snapshot.state.ui_visible,
        view: ViewStatus {
            zoom: snapshot.state.view.zoom,
            pan: snapshot.state.view.pan,
        },
        sources,
        outputs,
        osc: snapshot.runtime.control.osc.clone(),
        http: http.label(clients),
        clients,
    }));
}

#[derive(Deserialize)]
struct ProjectQuery {
    path: Option<PathBuf>,
    control: Option<bool>,
}

async fn get_project(State(state): State<ApiState>) -> Result<Response, ApiError> {
    let snapshot = snapshot(&state)?;
    let body = serde_json::to_vec_pretty(&snapshot.state.cfg).map_err(|error| ApiError::internal(error.to_string()))?;
    let name = snapshot
        .state
        .project_path
        .as_ref()
        .and_then(|path| path.file_name())
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_else(|| "project.multiviewer".to_string());
    let name = sanitize_filename(&name);
    let mut headers = HeaderMap::new();
    headers.insert(CONTENT_TYPE, "application/json".parse().unwrap());
    headers.insert(CONTENT_DISPOSITION, format!("attachment; filename=\"{name}\"").parse().unwrap());
    return Ok((headers, body).into_response());
}

async fn put_project(State(state): State<ApiState>, Query(query): Query<ProjectQuery>, body: Bytes) -> Result<Json<Value>, ApiError> {
    let cfg: Config = serde_json::from_slice(&body).map_err(ApiError::bad_request)?;
    let path = query.path.clone();
    let preserve_control = !query.control.unwrap_or(false);
    let (revision, warnings) = with_engine(&state, move |engine| {
        engine.replace_project(cfg, None, preserve_control).map_err(ApiError::project)?;
        if let Some(path) = path {
            engine.save_project_as(path).map_err(ApiError::project)?;
        }
        return Ok((engine.state.revision, engine.load_warnings.clone()));
    })
    .await?;
    return Ok(Json(json!({
        "revision": revision,
        "warnings": warnings,
    })));
}

async fn save_project(State(state): State<ApiState>) -> Result<Json<Value>, ApiError> {
    let has_path = snapshot(&state)?.state.project_path.is_some();
    if !has_path {
        return Err(ApiError::conflict("no project path; use PUT /api/v1/project?path=..."));
    }
    let revision = with_engine(&state, |engine| {
        engine.save_project().map_err(ApiError::project)?;
        return Ok(engine.state.revision);
    })
    .await?;
    return Ok(Json(json!({"revision": revision})));
}

#[derive(Deserialize)]
struct OpenRequest {
    path: PathBuf,
    #[serde(default)]
    control: bool,
}

async fn open_project(State(state): State<ApiState>, Json(request): Json<OpenRequest>) -> Result<Json<Value>, ApiError> {
    let revision = with_engine(&state, move |engine| {
        let cfg = Config::load_from(&request.path).map_err(ApiError::bad_request)?;
        engine.replace_project(cfg, Some(request.path), !request.control).map_err(ApiError::project)?;
        return Ok(engine.state.revision);
    })
    .await?;
    return Ok(Json(json!({"revision": revision})));
}

#[derive(Deserialize, Default)]
struct NewQuery {
    control: Option<bool>,
}

async fn new_project(State(state): State<ApiState>, Query(query): Query<NewQuery>) -> Result<Json<Value>, ApiError> {
    let preserve_control = !query.control.unwrap_or(false);
    let revision = with_engine(&state, move |engine| {
        engine.new_in_place(preserve_control).map_err(ApiError::project)?;
        return Ok(engine.state.revision);
    })
    .await?;
    return Ok(Json(json!({"revision": revision})));
}

async fn run_command(State(state): State<ApiState>, Json(command): Json<Command>) -> Result<Json<Value>, ApiError> {
    let revision = with_engine(&state, move |engine| {
        engine.apply_command(command).map_err(ApiError::control)?;
        return Ok(engine.state.revision);
    })
    .await?;
    return Ok(Json(json!({"revision": revision})));
}

#[derive(Deserialize)]
struct WsQuery {
    view: Option<String>,
}

async fn ws(ws: WebSocketUpgrade, State(state): State<ApiState>, Query(query): Query<WsQuery>) -> impl IntoResponse {
    let read_only = query.view.as_deref() == Some("1");
    return ws.on_upgrade(move |socket| serve_ws(socket, state, read_only));
}

async fn serve_ws(mut socket: WebSocket, state: ApiState, read_only: bool) {
    let mut frames = state.handles.frames.subscribe();
    state.handles.clients.fetch_add(1, Ordering::Relaxed);
    let _ = state.handles.inputs.send(InputEvent::Keyframe);

    loop {
        tokio::select! {
            frame = frames.recv() => {
                match frame {
                    Ok(frame) => {
                        if socket.send(Message::Binary(frame)).await.is_err() {
                            break;
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(_)) => {
                        // A keyframe is the only way to restore missed texture
                        // deltas; close so the browser reconnects for one.
                        tracing::debug!("web client fell behind; closing its socket");
                        break;
                    }
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }
            message = socket.recv() => {
                match message {
                    Some(Ok(Message::Text(text))) => {
                        if read_only {
                            continue;
                        }
                        match serde_json::from_str::<InputEvent>(&text) {
                            Ok(event) => {
                                let _ = state.handles.inputs.send(event);
                            }
                            Err(error) => tracing::debug!("ignoring web input: {error}"),
                        }
                    }
                    Some(Ok(Message::Close(_))) | None => break,
                    Some(Ok(_)) => {}
                    Some(Err(_)) => break,
                }
            }
        }
    }

    state.handles.clients.fetch_sub(1, Ordering::Relaxed);
    let _ = state.handles.inputs.send(InputEvent::ClientGone);
}

fn snapshot(state: &ApiState) -> Result<Arc<EngineSnapshot>, ApiError> {
    return state.handles.snapshots.lock().unwrap().clone().ok_or_else(|| ApiError::unavailable("engine is not ready"));
}

async fn with_engine<T, F>(state: &ApiState, work: F) -> Result<T, ApiError>
where
    T: Send + 'static,
    F: FnOnce(&mut Engine) -> Result<T, ApiError> + Send + 'static,
{
    let engine = state.handles.engine.clone();
    let snapshots = state.handles.snapshots.clone();
    let task = tokio::task::spawn_blocking(move || {
        let mut engine = engine.lock().unwrap();
        let result = work(&mut engine);
        // A failing write can still have changed the engine (for example, an
        // upload whose save failed), so publish either way.
        publish_state(&engine, &snapshots);
        return result;
    });
    return match task.await {
        Ok(result) => result,
        Err(error) => Err(ApiError::internal(error.to_string())),
    };
}

pub struct ApiError {
    status: StatusCode,
    message: String,
}

impl ApiError {
    fn new(status: StatusCode, message: impl Into<String>) -> Self {
        return Self {
            status,
            message: message.into(),
        };
    }

    fn bad_request(error: impl std::fmt::Display) -> Self {
        return Self::new(StatusCode::BAD_REQUEST, error.to_string());
    }

    fn conflict(message: impl Into<String>) -> Self {
        return Self::new(StatusCode::CONFLICT, message);
    }

    fn internal(error: impl std::fmt::Display) -> Self {
        return Self::new(StatusCode::INTERNAL_SERVER_ERROR, error.to_string());
    }

    fn unavailable(message: impl Into<String>) -> Self {
        return Self::new(StatusCode::SERVICE_UNAVAILABLE, message);
    }

    fn project(error: ProjectError) -> Self {
        return Self::new(StatusCode::BAD_REQUEST, error.to_string());
    }

    fn control(error: ControlError) -> Self {
        return match error {
            ControlError::UnknownSource => Self::new(StatusCode::NOT_FOUND, "unknown source"),
            ControlError::UnknownOutput => Self::new(StatusCode::NOT_FOUND, "unknown output"),
            ControlError::InvalidArgument => Self::new(StatusCode::BAD_REQUEST, "invalid argument"),
            ControlError::Project(error) => Self::project(error),
        };
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        return (self.status, Json(json!({"error": self.message}))).into_response();
    }
}

fn sanitize_filename(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '.' | '-' | '_') {
                character
            } else {
                '_'
            }
        })
        .collect();
    if cleaned.is_empty() {
        return "project.multiviewer".to_string();
    }
    return cleaned;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Canvas;
    use super::super::config::HttpControlConfig;
    use super::super::server::{ListenerState, Server};
    use crate::engine::SnapshotSlot;
    use std::io::{Read, Write};
    use std::net::TcpStream;
    use std::sync::atomic::AtomicUsize;

    fn start() -> (Server, Arc<Mutex<Engine>>) {
        let engine = Arc::new(Mutex::new(Engine::test_instance(Canvas::default())));
        let snapshot = Arc::new(engine.lock().unwrap().snapshot());
        let snapshots: SnapshotSlot = Arc::new(Mutex::new(Some(snapshot)));
        let (frames, _) = broadcast::channel(1);
        let (inputs, _) = std::sync::mpsc::channel();
        let handles = ApiHandles {
            engine: engine.clone(),
            snapshots,
            frames,
            inputs,
            clients: Arc::new(AtomicUsize::new(0)),
        };
        let config = HttpControlConfig {
            enabled: true,
            bind: "127.0.0.1".to_string(),
            port: 0,
        };
        let server = Server::start(&config, handles).unwrap();
        return (server, engine);
    }

    fn request_bytes(addr: std::net::SocketAddr, path: &str) -> Vec<u8> {
        let mut stream = TcpStream::connect(addr).unwrap();
        write!(stream, "GET {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n").unwrap();
        let mut response = Vec::new();
        stream.read_to_end(&mut response).unwrap();
        return response;
    }

    fn request_with_headers(
        addr: std::net::SocketAddr,
        method: &str,
        path: &str,
        host: &str,
        origin: Option<&str>,
        forwarded: Option<&str>,
    ) -> String {
        let mut stream = TcpStream::connect(addr).unwrap();
        let origin = origin.map(|origin| format!("Origin: {origin}\r\n")).unwrap_or_default();
        let forwarded = forwarded.map(|forwarded| format!("X-Forwarded-Host: {forwarded}\r\n")).unwrap_or_default();
        write!(
            stream,
            "{method} {path} HTTP/1.1\r\nHost: {host}\r\n{origin}{forwarded}Content-Length: 0\r\nConnection: close\r\n\r\n"
        )
        .unwrap();
        let mut response = String::new();
        stream.read_to_string(&mut response).unwrap();
        return response;
    }

    fn request(addr: std::net::SocketAddr, method: &str, path: &str, body: &str) -> String {
        let mut stream = TcpStream::connect(addr).unwrap();
        write!(
            stream,
            "{method} {path} HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        )
        .unwrap();
        let mut response = String::new();
        stream.read_to_string(&mut response).unwrap();
        return response;
    }

    fn addr(server: &Server) -> std::net::SocketAddr {
        match server.status().state {
            ListenerState::Listening(addr) => addr,
            _ => panic!("server is not listening"),
        }
    }

    #[test]
    fn documented_routes_parse() {
        let source = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/src/control/http/api.rs")).expect("read the HTTP source");
        let routes: Vec<&str> = source.lines().filter_map(|line| line.trim().strip_prefix("// @http ")).collect();
        assert_eq!(routes.len(), HTTP_ROUTE_COUNT);
        let mut keys = std::collections::HashSet::new();
        for route in routes {
            let method = directive_value(route, "method").expect("method");
            let path = directive_value(route, "path").expect("path");
            assert!(!directive_value(route, "desc").unwrap_or("").is_empty());
            assert!(keys.insert(format!("{method} {path}")), "duplicate route: {method} {path}");
        }
    }

    fn directive_value<'a>(line: &'a str, key: &str) -> Option<&'a str> {
        let needle = format!("{key}=\"");
        let start = line.find(&needle)? + needle.len();
        let rest = &line[start..];
        let end = rest.find('"')?;
        return Some(&rest[..end]);
    }

    #[test]
    fn only_same_origin_browser_requests_reach_the_api() {
        let (server, _) = start();
        let addr = addr(&server);
        let host = addr.to_string();
        let same_origin = format!("http://{host}");
        assert_eq!(
            request_with_headers(addr, "GET", "/api/v1/state", &host, Some(&same_origin), None).split(' ').nth(1),
            Some("200")
        );
        assert_eq!(
            request_with_headers(addr, "GET", "/api/v1/state", &host, Some("http://evil.example"), None).split(' ').nth(1),
            Some("403")
        );
        assert_eq!(
            request_with_headers(addr, "GET", "/api/v1/state", "evil.example", None, None).split(' ').nth(1),
            Some("403")
        );
        assert_eq!(request_with_headers(addr, "GET", "/api/v1/state", &host, None, None).split(' ').nth(1), Some("200"));
        // A reverse proxy may keep the upstream Host and forward the public
        // host; the Origin is compared to the forwarded host.
        let public = "multiviewer.lan:8090";
        assert_eq!(
            request_with_headers(addr, "GET", "/api/v1/state", &host, Some("http://multiviewer.lan:8090"), Some(public),)
                .split(' ')
                .nth(1),
            Some("200")
        );
        // A proxy that forwards the default TLS port still matches the
        // browser's Origin, which omits it.
        assert_eq!(
            request_with_headers(addr, "GET", "/api/v1/state", &host, Some("https://multiviewer.lan"), Some("multiviewer.lan:443"),)
                .split(' ')
                .nth(1),
            Some("200")
        );
    }

    #[test]
    fn serves_the_app_icon_as_the_tab_icon() {
        let (server, _) = start();
        let response = request_bytes(addr(&server), "/favicon.png");
        assert!(response.starts_with(b"HTTP/1.1 200"), "{response:?}");
        let body = response.windows(4).position(|window| window == b"\r\n\r\n").map(|index| index + 4).expect("response headers");
        let headers = String::from_utf8_lossy(&response[..body]).to_ascii_lowercase();
        assert!(headers.contains("content-type: image/png"), "{headers}");
        assert_eq!(&response[body..body + 8], b"\x89PNG\r\n\x1a\n");
    }

    #[test]
    fn serves_state_and_applies_commands() {
        let (server, engine) = start();
        let addr = addr(&server);

        let response = request(addr, "GET", "/api/v1/state", "");
        assert!(response.starts_with("HTTP/1.1 200"), "{response}");
        assert!(response.contains("\"canvas\""), "{response}");

        let response = request(addr, "POST", "/api/v1/commands", r#"{"type":"add_source"}"#);
        assert!(response.starts_with("HTTP/1.1 200"), "{response}");
        assert!(response.contains("\"revision\":1"), "{response}");
        assert_eq!(engine.lock().unwrap().state.cfg.canvas.sources.len(), 1);

        let response = request(addr, "GET", "/api/v1/project", "");
        assert!(response.starts_with("HTTP/1.1 200"), "{response}");
        assert!(response.contains("attachment"), "{response}");

        let response = request(addr, "GET", "/", "");
        assert!(response.starts_with("HTTP/1.1 200"), "{response}");
        assert!(response.contains("Multiviewer"), "{response}");
    }
}
