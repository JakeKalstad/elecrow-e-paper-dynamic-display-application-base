mod dashboard;
mod render;
use axum::{
    body::Body,
    extract::{DefaultBodyLimit, Multipart, Path, Query, Request, State},
    http::{header, StatusCode},
    middleware::{self, Next},
    response::{Html, IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use std::{
    collections::VecDeque,
    path::PathBuf,
    sync::{Arc, Mutex},
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio::{
    process::Command,
    sync::{mpsc, Semaphore},
};

#[derive(Clone)]
struct App {
    jobs: Arc<Mutex<VecDeque<Job>>>,
    queue: mpsc::Sender<Work>,
    bin: PathBuf,
    port: String,
    dry: bool,
    previews: Arc<Semaphore>,
    host: Option<String>,
    dashboard: Arc<dashboard::Dashboard>,
}
#[derive(Clone, Deserialize, Serialize)]
struct Message {
    text: String,
    #[serde(default = "default_size")]
    size: u16,
}
fn default_size() -> u16 {
    40
}
#[derive(Clone, Serialize)]
struct Job {
    id: String,
    text: String,
    size: u16,
    status: String,
    created_at: u64,
    updated_at: u64,
    error: Option<String>,
}
struct Work {
    id: String,
    payload: Payload,
}
#[derive(Clone)]
enum Payload {
    Text(Message),
    Image(Vec<u8>),
}
type ApiError = (StatusCode, Json<serde_json::Value>);
fn err(code: StatusCode, message: impl ToString) -> ApiError {
    (code, Json(serde_json::json!({"error":message.to_string()})))
}
fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}
fn validate(m: &Message) -> Result<(), ApiError> {
    if m.text.trim().is_empty() {
        return Err(err(StatusCode::BAD_REQUEST, "Enter a message."));
    }
    if m.text.len() > 4096 {
        return Err(err(
            StatusCode::BAD_REQUEST,
            "Message must be at most 4096 UTF-8 bytes.",
        ));
    }
    if !(12..=96).contains(&m.size) {
        return Err(err(StatusCode::BAD_REQUEST, "Font size must be 12–96."));
    }
    if m.text.chars().any(|c| c.is_control() && c != '\n') {
        return Err(err(
            StatusCode::BAD_REQUEST,
            "Only newline control characters are supported.",
        ));
    }
    Ok(())
}
fn update(app: &App, id: &str, status: &str, error: Option<String>) {
    if let Some(j) = app.jobs.lock().unwrap().iter_mut().find(|j| j.id == id) {
        j.status = status.into();
        j.updated_at = now();
        j.error = error;
    }
}
async fn run(
    app: &App,
    payload: &Payload,
    preview: Option<&std::path::Path>,
) -> anyhow::Result<()> {
    let mut cmd = Command::new(&app.bin);
    cmd.kill_on_drop(true).arg("--port").arg(&app.port);
    if let Some(host) = &app.host {
        cmd.arg("--host").arg(host);
    }
    if let Some(path) = preview {
        cmd.arg("--dry-run").arg("--preview").arg(path);
    } else if app.dry {
        cmd.arg("--dry-run");
    }
    let dir = tempfile::tempdir()?;
    match payload {
        Payload::Text(m) => {
            cmd.arg("text")
                .arg("--size")
                .arg(m.size.to_string())
                .arg("--")
                .arg(&m.text);
        }
        Payload::Image(bytes) => {
            let path = dir.path().join("image.png");
            tokio::fs::write(&path, bytes).await?;
            cmd.arg("image").arg(path);
        }
    }
    let output = tokio::time::timeout(Duration::from_secs(180), cmd.output()).await??;
    anyhow::ensure!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr).trim()
    );
    Ok(())
}
fn queue_payload(app: &App, payload: Payload, label: String, size: u16) -> Result<Job, ApiError> {
    let id = uuid::Uuid::new_v4().to_string();
    let t = now();
    let job = Job {
        id: id.clone(),
        text: label,
        size,
        status: "queued".into(),
        created_at: t,
        updated_at: t,
        error: None,
    };
    let mut jobs = app.jobs.lock().unwrap();
    app.queue.try_send(Work { id, payload }).map_err(|_| {
        err(
            StatusCode::TOO_MANY_REQUESTS,
            "Display queue is full. Please wait.",
        )
    })?;
    if jobs.len() >= 100 {
        jobs.pop_front();
    }
    jobs.push_back(job.clone());
    Ok(job)
}

async fn worker(app: App, mut rx: mpsc::Receiver<Work>) {
    while let Some(w) = rx.recv().await {
        update(&app, &w.id, "sending", None);
        match run(&app, &w.payload, None).await {
            Ok(()) => update(
                &app,
                &w.id,
                if app.dry { "simulated" } else { "displayed" },
                None,
            ),
            Err(e) => update(&app, &w.id, "failed", Some(e.to_string())),
        }
    }
}
async fn enqueue(
    State(app): State<App>,
    Json(m): Json<Message>,
) -> Result<impl IntoResponse, ApiError> {
    validate(&m)?;
    let job = queue_payload(&app, Payload::Text(m.clone()), m.text, m.size)?;
    Ok((StatusCode::ACCEPTED, Json(job)))
}

async fn jobs(State(app): State<App>) -> Json<Vec<Job>> {
    Json(app.jobs.lock().unwrap().iter().rev().cloned().collect())
}
async fn job(State(app): State<App>, Path(id): Path<String>) -> Result<Json<Job>, ApiError> {
    app.jobs
        .lock()
        .unwrap()
        .iter()
        .find(|j| j.id == id)
        .cloned()
        .map(Json)
        .ok_or_else(|| {
            err(
                StatusCode::NOT_FOUND,
                "Job not found (history is kept in memory).",
            )
        })
}
async fn status(State(app): State<App>) -> Json<serde_json::Value> {
    Json(
        serde_json::json!({"transport":if app.host.is_some(){"wifi"}else{"usb"},"device_host":app.host,"mode":if app.dry {"simulation"} else {"hardware"},"serial_port":app.port,"port_present":std::path::Path::new(&app.port).exists(),"width":792,"height":272,"queue_available":app.queue.capacity()}),
    )
}
async fn preview(State(app): State<App>, Json(m): Json<Message>) -> Result<Response, ApiError> {
    validate(&m)?;
    let _permit = app
        .previews
        .clone()
        .try_acquire_owned()
        .map_err(|_| err(StatusCode::TOO_MANY_REQUESTS, "Preview is busy; try again."))?;
    let dir = tempfile::tempdir().map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, e))?;
    let path = dir.path().join("preview.png");
    run(&app, &Payload::Text(m), Some(&path))
        .await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, e))?;
    let bytes = tokio::fs::read(path)
        .await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, e))?;
    Ok((
        [
            (header::CONTENT_TYPE, "image/png"),
            (header::CACHE_CONTROL, "no-store"),
        ],
        bytes,
    )
        .into_response())
}
#[derive(Deserialize)]
struct PreviewQuery {
    #[serde(default)]
    preview: bool,
}
fn png_response(bytes: Vec<u8>) -> Response {
    (
        [
            (header::CONTENT_TYPE, "image/png"),
            (header::CACHE_CONTROL, "no-store"),
        ],
        bytes,
    )
        .into_response()
}
async fn submit_image(
    app: &App,
    bytes: Vec<u8>,
    label: String,
    preview: bool,
) -> Result<Response, ApiError> {
    if preview {
        Ok(png_response(bytes))
    } else {
        let job = queue_payload(app, Payload::Image(bytes), label, 0)?;
        Ok((StatusCode::ACCEPTED, Json(job)).into_response())
    }
}
async fn image_upload(
    State(app): State<App>,
    Query(q): Query<PreviewQuery>,
    mut multipart: Multipart,
) -> Result<Response, ApiError> {
    let _permit = app
        .previews
        .clone()
        .try_acquire_owned()
        .map_err(|_| err(StatusCode::TOO_MANY_REQUESTS, "Renderer busy"))?;
    let field = multipart
        .next_field()
        .await
        .map_err(|e| err(StatusCode::BAD_REQUEST, e))?
        .ok_or_else(|| err(StatusCode::BAD_REQUEST, "Upload one image as file"))?;
    let bytes = field
        .bytes()
        .await
        .map_err(|e| err(StatusCode::BAD_REQUEST, e))?;
    let png = tokio::task::spawn_blocking(move || render::image(&bytes))
        .await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, e))?
        .map_err(|e| err(StatusCode::BAD_REQUEST, e))?;
    submit_image(&app, png, "Image upload".into(), q.preview).await
}
async fn chart(
    State(app): State<App>,
    Query(q): Query<PreviewQuery>,
    Json(c): Json<render::Chart>,
) -> Result<Response, ApiError> {
    let png = render::chart(&c).map_err(|e| err(StatusCode::BAD_REQUEST, e))?;
    submit_image(&app, png, c.title, q.preview).await
}
async fn dashboard_frame(
    State(app): State<App>,
    Query(q): Query<PreviewQuery>,
) -> Result<Response, ApiError> {
    let png = app
        .dashboard
        .frame()
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, e))?;
    submit_image(&app, png, "Weather & tickers dashboard".into(), q.preview).await
}
async fn dashboard_state(State(app): State<App>) -> Json<dashboard::Snapshot> {
    Json(app.dashboard.snapshot())
}
async fn config_get(State(app): State<App>) -> Json<dashboard::Config> {
    Json(app.dashboard.config())
}
async fn config_set(
    State(app): State<App>,
    Json(c): Json<dashboard::Config>,
) -> Result<Json<dashboard::Config>, ApiError> {
    app.dashboard
        .save(c)
        .await
        .map_err(|e| err(StatusCode::BAD_REQUEST, e))?;
    Ok(Json(app.dashboard.config()))
}
async fn quote(
    State(app): State<App>,
    Json(q): Json<dashboard::Quote>,
) -> Result<Json<serde_json::Value>, ApiError> {
    app.dashboard
        .ingest(q)
        .map_err(|e| err(StatusCode::BAD_REQUEST, e))?;
    Ok(Json(serde_json::json!({"accepted":true})))
}
// A localhost-only API. Reject requests from other browser origins; curl needs no Origin.
async fn local_origin(req: Request, next: Next) -> Response {
    let host = req
        .headers()
        .get(header::HOST)
        .and_then(|h| h.to_str().ok())
        .unwrap_or("");
    if !(host.starts_with("127.0.0.1:") || host.starts_with("localhost:")) {
        return StatusCode::FORBIDDEN.into_response();
    }
    if let Some(origin) = req.headers().get(header::ORIGIN) {
        let expected = format!("http://{host}");
        if origin.to_str().ok() != Some(expected.as_str()) {
            return StatusCode::FORBIDDEN.into_response();
        }
    }
    next.run(req).await
}
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .to_owned();
    let bin = std::env::var_os("EPAPER_BIN")
        .map(PathBuf::from)
        .unwrap_or_else(|| root.join("cli/target/release/epaper"));
    anyhow::ensure!(bin.is_file(),"Sender missing at {}. Build it first: cargo build --release --manifest-path cli/Cargo.toml",bin.display());
    let port = std::env::var("EPAPER_PORT")
        .unwrap_or_else(|_| "/dev/serial/by-id/usb-1a86_USB_Serial-if00-port0".into());
    let dry = std::env::var("EPAPER_DRY_RUN").as_deref() == Ok("1");
    let http_port: u16 = std::env::var("EPAPER_HTTP_PORT")
        .unwrap_or_else(|_| "8787".into())
        .parse()?;
    let (queue, rx) = mpsc::channel(8);
    let state = App {
        jobs: Default::default(),
        queue,
        bin,
        port,
        dry,
        previews: Arc::new(Semaphore::new(2)),
        host: std::env::var("EPAPER_DEVICE_HOST")
            .ok()
            .filter(|s| !s.is_empty()),
        dashboard: Arc::new(dashboard::Dashboard::new()?),
    };
    tokio::spawn(worker(state.clone(), rx));
    tokio::spawn(dashboard::poller(state.clone()));
    let app = Router::new()
        .route(
            "/",
            get(|| async { Html(include_str!("../static/index.html")) }),
        )
        .route(
            "/app.js",
            get(|| async {
                (
                    [(header::CONTENT_TYPE, "text/javascript")],
                    include_str!("../static/app.js"),
                )
            }),
        )
        .route(
            "/style.css",
            get(|| async {
                (
                    [(header::CONTENT_TYPE, "text/css")],
                    include_str!("../static/style.css"),
                )
            }),
        )
        .route("/api/images", post(image_upload))
        .route("/api/charts", post(chart))
        .route(
            "/api/dashboard/frame",
            get(dashboard_frame).post(dashboard_frame),
        )
        .route("/api/dashboard", get(dashboard_state))
        .route("/api/config", get(config_get).put(config_set))
        .route("/api/quotes", post(quote))
        .route("/api/status", get(status))
        .route("/api/messages", get(jobs).post(enqueue))
        .route("/api/messages/{id}", get(job))
        .route("/api/preview", post(preview))
        .fallback(|| async { (StatusCode::NOT_FOUND, Body::from("Not found")) })
        .layer(DefaultBodyLimit::max(5 * 1024 * 1024))
        .layer(middleware::from_fn(local_origin))
        .with_state(state);
    let listener =
        tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, http_port)).await?;
    println!(
        "E-paper desk: http://127.0.0.1:{http_port}{}",
        if dry {
            " (SIMULATION — nothing sent to display)"
        } else {
            ""
        }
    );
    axum::serve(listener, app)
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await?;
    Ok(())
}
