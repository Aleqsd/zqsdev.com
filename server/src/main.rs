mod ai;
mod budget;
mod static_data;
use ai::{AiClient, AiRequest};

use axum::{
    body::Body,
    extract::{rejection::JsonRejection, ConnectInfo, DefaultBodyLimit, State},
    http::{header::CACHE_CONTROL, HeaderMap, HeaderValue, Request, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use budget::Budget;
use chrono::{SecondsFormat, Utc};
use dotenvy::Error as DotenvError;
use serde::{Deserialize, Serialize};

use static_data::TerminalDataPayload;
use std::{
    convert::Infallible,
    net::SocketAddr,
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};
use tokio::{
    fs::{self, OpenOptions},
    io::AsyncWriteExt,
    net::TcpListener,
    signal,
    sync::{Mutex, Semaphore},
};
use tower::{service_fn, ServiceExt};
use tower_http::services::ServeDir;
use tracing::{info, warn};
use uuid::Uuid;
const SERVER_VERSION: &str = env!("CARGO_PKG_VERSION");
const MAX_LOG_TEXT_CHARS: usize = 2000;
fn server_commit_hash() -> &'static str {
    option_env!("GIT_COMMIT_HASH").unwrap_or("unknown")
}
struct AppState {
    budget: Mutex<Option<Budget>>,
    client: AiClient,
    slots: Semaphore,
    deadline: Duration,
    log_slots: Semaphore,
    recent_logs: Mutex<std::collections::VecDeque<i64>>,
    terminal_data: Arc<TerminalDataPayload>,
    questions_log: PathBuf,
    answers_log: PathBuf,
}
#[derive(Debug, Deserialize)]
struct CommandLogRequest {
    command: String,
    #[serde(default)]
    mode: Option<String>,
}
#[derive(Debug, Serialize)]
struct AiResponse {
    answer: String,
    ai_enabled: bool,
    reason: Option<String>,
    model: Option<String>,
    sources: Vec<String>,
    knowledge_updated: &'static str,
}
fn fallback(status: StatusCode, reason: &str) -> Response {
    (status,Json(AiResponse {answer:"AI is temporarily unavailable. Classic commands are ready: about, experience, projects, testimonials, resume. / IA temporairement indisponible : les commandes classiques restent disponibles.".into(),ai_enabled:false,reason:Some(reason.into()),model:None,sources:vec![],knowledge_updated:"2026-09-08"})).into_response()
}
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    load_env_files();
    configure_tracing();
    let static_dir = PathBuf::from(std::env::var("STATIC_DIR").unwrap_or_else(|_| "static".into()));
    let data = Arc::new(TerminalDataPayload::load(&static_dir.join("data"))?);
    let client = AiClient::new(&data)?;
    let limits = [
        ("AI_MINUTE_BUDGET_USD", 0.5),
        ("AI_HOUR_BUDGET_USD", 2.0),
        ("AI_DAY_BUDGET_USD", 2.0),
        ("AI_MONTH_BUDGET_USD", 10.0),
    ]
    .map(|(name, default)| {
        std::env::var(name)
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(default)
    });
    let budget_path = resolve_log_path("AI_BUDGET_PATH", "logs/ai-budget.json");
    if let Some(parent) = budget_path.parent().filter(|p| !p.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent)?;
    }
    let parent = budget_path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    anyhow::ensure!(
        !std::fs::canonicalize(parent)?.starts_with(std::fs::canonicalize(&static_dir)?),
        "Budget must be outside public static files"
    );
    let budget = match Budget::open(budget_path, limits) {
        Ok(v) => Some(v),
        Err(e) => {
            warn!(error=%e,"AI disabled: budget ledger unavailable");
            None
        }
    };
    let state = Arc::new(AppState {
        budget: Mutex::new(budget),
        client,
        slots: Semaphore::new(2),
        deadline: Duration::from_secs(16),
        log_slots: Semaphore::new(2),
        recent_logs: Mutex::new(std::collections::VecDeque::new()),
        terminal_data: data,
        questions_log: resolve_log_path("QUESTIONS_LOG_PATH", "questions.log"),
        answers_log: resolve_log_path("ANSWERS_LOG_PATH", "answers.log"),
    });
    let static_root = Arc::new(static_dir);
    let static_service = service_fn(move |req: Request<Body>| {
        let path = req.uri().path().to_owned();
        let dir =
            ServeDir::new(static_root.as_ref().clone()).append_index_html_on_directories(true);
        async move {
            match dir.oneshot(req).await {
                Ok(response) => {
                    let mut response = response.into_response();
                    response.headers_mut().insert(
                        CACHE_CONTROL,
                        HeaderValue::from_static(cache_control_for_path(&path)),
                    );
                    Ok::<Response, Infallible>(response)
                }
                Err(_) => Ok(StatusCode::INTERNAL_SERVER_ERROR.into_response()),
            }
        }
    });
    let router = Router::new()
        .route("/api/ai", post(handle_ai))
        .route("/api/data", get(handle_data))
        .route("/api/version", get(handle_version))
        .route("/api/log/command", post(handle_command_log))
        .layer(DefaultBodyLimit::max(64 * 1024))
        .with_state(state)
        .fallback_service(static_service);
    let host = std::env::var("HOST").unwrap_or_else(|_| "127.0.0.1".into());
    let port = std::env::var("PORT").unwrap_or_else(|_| "3000".into());
    let listener = TcpListener::bind(format!("{host}:{port}")).await?;
    info!(version = SERVER_VERSION, "server ready");
    axum::serve(
        listener,
        router.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(shutdown_signal())
    .await?;
    Ok(())
}
async fn handle_ai(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    ConnectInfo(remote): ConnectInfo<SocketAddr>,
    payload: Result<Json<AiRequest>, JsonRejection>,
) -> Response {
    let Json(request) = match payload {
        Ok(v) => v,
        Err(_) => return fallback(StatusCode::BAD_REQUEST, "invalid_request"),
    };
    if request.validate().is_err() {
        return fallback(StatusCode::BAD_REQUEST, "invalid_request");
    }
    if !state.client.configured() {
        return fallback(StatusCode::SERVICE_UNAVAILABLE, "ai_not_configured");
    }
    let ip = client_ip(&headers, remote);
    let id = Uuid::new_v4().to_string();
    let body = state.client.body(&request);
    let reservation = state.client.reservation(&body);
    let now = Utc::now().timestamp();
    let _permit;
    {
        let mut guard = state.budget.lock().await;
        let Some(budget) = guard.as_mut() else {
            return fallback(StatusCode::SERVICE_UNAVAILABLE, "budget_unavailable");
        };
        if budget.admit_ip(&ip, now).is_err() {
            return fallback(StatusCode::TOO_MANY_REQUESTS, "request_limit");
        }
        _permit = match state.slots.try_acquire() {
            Ok(p) => p,
            Err(_) => return fallback(StatusCode::TOO_MANY_REQUESTS, "busy"),
        };
        if let Err(e) = budget.reserve(&id, reservation, now) {
            warn!(error=%e,"AI admission denied");
            return fallback(StatusCode::TOO_MANY_REQUESTS, "spending_limit");
        }
    }
    // Includes logging, upstream headers, body and parsing. No paid retrieval or retries.
    let result = tokio::time::timeout(state.deadline, async {
        record_ai_question(&state, &id, &request.question, &ip).await;
        state.client.ask(body).await
    })
    .await;
    match result {
        Ok(Ok(answer)) => {
            let mut guard = state.budget.lock().await;
            if let Some(budget) = guard.as_mut() {
                if let Err(e) = budget.settle(&id, answer.cost) {
                    warn!(error=%e,"Budget persistence failed; disabling AI");
                    *guard = None;
                }
            }
            drop(guard);
            info!(question_id=%id,model=%answer.model,cost_usd=answer.cost,usage=%answer.usage,"AI response completed");
            let response = AiResponse {
                answer: answer.text,
                ai_enabled: true,
                reason: None,
                model: Some(answer.model),
                sources: answer.sources,
                knowledge_updated: "2026-09-08",
            };
            let _ = tokio::time::timeout(
                Duration::from_secs(1),
                record_ai_answer(&state, &id, &response, &ip),
            )
            .await;
            Json(response).into_response()
        }
        Ok(Err(e)) => {
            warn!(question_id=%id,error=%e,"AI failed; reservation retained for uncertain usage");
            fallback(StatusCode::SERVICE_UNAVAILABLE, "provider_unavailable")
        }
        Err(_) => {
            warn!(question_id=%id,"AI timeout; reservation retained");
            fallback(StatusCode::GATEWAY_TIMEOUT, "timeout")
        }
    }
}
fn client_ip(headers: &HeaderMap, remote: SocketAddr) -> String {
    // Nginx overwrites X-Real-IP. Never trust the client-prependable first XFF entry.
    if remote.ip().is_loopback() {
        if let Some(ip) = headers
            .get("x-real-ip")
            .and_then(|h| h.to_str().ok())
            .and_then(|s| s.parse::<std::net::IpAddr>().ok())
        {
            return ip.to_string();
        }
    }
    remote.ip().to_string()
}
#[derive(Serialize)]
struct CommandLogEntry {
    timestamp: String,
    entry_type: &'static str,
    command: String,
    command_len: usize,
    mode: String,
    ip: String,
}

#[derive(Serialize)]
struct AiQuestionLogEntry {
    timestamp: String,
    entry_type: &'static str,
    question_id: String,
    question: String,
    question_len: usize,
    ip: String,
}

#[derive(Serialize)]
struct AiAnswerLogEntry {
    timestamp: String,
    entry_type: &'static str,
    question_id: String,
    answer_id: String,
    answer: String,
    answer_len: usize,
    model: Option<String>,
    ai_enabled: bool,
    reason: Option<String>,
    ip: String,
}

async fn shutdown_signal() {
    let ctrl_c = async {
        signal::ctrl_c()
            .await
            .expect("failed to install Ctrl+C handler");
    };

    #[cfg(unix)]
    let terminate = async {
        use tokio::signal::unix::{signal, SignalKind};
        let mut sigterm =
            signal(SignalKind::terminate()).expect("failed to install signal handler");
        sigterm.recv().await;
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {}
        _ = terminate => {}
    }

    info!("msg" = "shutdown signal received");
}

fn configure_tracing() {
    let default_filter = "info";
    tracing_subscriber::fmt()
        .with_target(false)
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| default_filter.into()),
        )
        .init();
}

fn cache_control_for_path(path: &str) -> &'static str {
    let path = if path.is_empty() { "/" } else { path };
    if path == "/" || path.ends_with('/') || path.ends_with(".html") {
        "no-store"
    } else if path.ends_with(".css") || path.ends_with(".json") {
        "no-store"
    } else if path.ends_with(".webp")
        || path.ends_with(".ico")
        || path.ends_with(".svg")
        || path.ends_with(".png")
    {
        "public, max-age=31536000, immutable"
    } else {
        "public, max-age=3600, must-revalidate"
    }
}

fn load_env_files() {
    fn load(file: &str) {
        match dotenvy::from_filename(file) {
            Ok(_) => {}
            Err(DotenvError::Io(err)) if err.kind() == std::io::ErrorKind::NotFound => {}
            Err(err) => eprintln!("Warning: unable to load {file}: {err}"),
        }
    }

    load(".env.local");
    load(".env");
}

fn resolve_log_path(env_key: &str, default: &str) -> PathBuf {
    std::env::var(env_key)
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from(default))
}

fn current_timestamp() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true)
}

async fn append_log_entry<T>(path: &Path, entry: &T) -> anyhow::Result<()>
where
    T: Serialize + ?Sized,
{
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        fs::create_dir_all(parent).await?;
    }
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .await?;
    let line = serde_json::to_vec(entry)?;
    file.write_all(&line).await?;
    file.write_all(b"\n").await?;
    Ok(())
}

async fn record_ai_question(app_state: &AppState, question_id: &str, question: &str, ip: &str) {
    let entry = AiQuestionLogEntry {
        timestamp: current_timestamp(),
        entry_type: "ai_question",
        question_id: question_id.to_string(),
        question: sanitize_log_text(question),
        question_len: question.chars().count(),
        ip: ip.to_string(),
    };
    if let Err(err) = append_log_entry(&app_state.questions_log, &entry).await {
        warn!(target: "log", error = %err, "Failed to persist AI question log entry");
    }
}

async fn record_ai_answer(
    app_state: &AppState,
    question_id: &str,
    response: &AiResponse,
    ip: &str,
) {
    let entry = AiAnswerLogEntry {
        timestamp: current_timestamp(),
        entry_type: "ai_answer",
        question_id: question_id.to_string(),
        answer_id: Uuid::new_v4().to_string(),
        answer: sanitize_log_text(&response.answer),
        answer_len: response.answer.chars().count(),
        model: response.model.clone(),
        ai_enabled: response.ai_enabled,
        reason: response.reason.clone(),
        ip: ip.to_string(),
    };
    if let Err(err) = append_log_entry(&app_state.answers_log, &entry).await {
        warn!(target: "log", error = %err, "Failed to persist AI answer log entry");
    }
}

async fn handle_data(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let value = terminal_payload_with_alias(state.terminal_data.as_ref());
    let mut response = Json(value).into_response();
    let header = HeaderValue::from_static("public, max-age=60, must-revalidate");
    response.headers_mut().insert(CACHE_CONTROL, header);
    response
}

async fn handle_version(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    Json(
        serde_json::json!({"version":SERVER_VERSION,"commit":server_commit_hash(),"model":state.client.model,"knowledge_updated":"2026-09-08","knowledge_mode":"full_context","ai_configured":state.client.configured()}),
    )
}

async fn handle_command_log(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    ConnectInfo(remote): ConnectInfo<SocketAddr>,
    Json(payload): Json<CommandLogRequest>,
) -> impl IntoResponse {
    let trimmed = payload.command.trim();
    if trimmed.is_empty() || trimmed.chars().count() > MAX_LOG_TEXT_CHARS {
        return StatusCode::BAD_REQUEST;
    }
    let Ok(_permit) = state.log_slots.try_acquire() else {
        return StatusCode::TOO_MANY_REQUESTS;
    };
    {
        let mut times = state.recent_logs.lock().await;
        let now = Utc::now().timestamp();
        while times.front().is_some_and(|t| now - *t >= 60) {
            times.pop_front();
        }
        if times.len() >= 120 {
            return StatusCode::TOO_MANY_REQUESTS;
        }
        times.push_back(now);
    }
    let mode_value = payload
        .mode
        .unwrap_or_else(|| "classic".to_string())
        .trim()
        .to_string();
    let mode = if mode_value.is_empty() {
        "classic".to_string()
    } else {
        mode_value
    };
    if mode != "classic" && mode != "ai" {
        return StatusCode::BAD_REQUEST;
    }
    let entry = CommandLogEntry {
        timestamp: current_timestamp(),
        entry_type: "command",
        command: sanitize_log_text(trimmed),
        command_len: trimmed.chars().count(),
        mode,
        ip: client_ip(&headers, remote),
    };
    match append_log_entry(&state.questions_log, &entry).await {
        Ok(_) => StatusCode::NO_CONTENT,
        Err(err) => {
            warn!(target: "log", error = %err, "Failed to persist command log entry");
            StatusCode::INTERNAL_SERVER_ERROR
        }
    }
}

fn sanitize_log_text(input: &str) -> String {
    let normalized = normalize_log_text(input);
    let redacted = redact_known_secret_patterns(&normalized);
    truncate_for_log(&redacted, MAX_LOG_TEXT_CHARS)
}

fn normalize_log_text(input: &str) -> String {
    let mut normalized = String::with_capacity(input.len());
    let mut last_was_space = false;

    for ch in input.chars() {
        let mapped = if ch.is_control() { ' ' } else { ch };
        if mapped.is_whitespace() {
            if !last_was_space {
                normalized.push(' ');
                last_was_space = true;
            }
        } else {
            normalized.push(mapped);
            last_was_space = false;
        }
    }

    normalized.trim().to_string()
}

fn redact_known_secret_patterns(input: &str) -> String {
    let mut redacted = input.to_string();
    redacted = redact_prefixed_secret(&redacted, "sk-proj-", "[redacted-openai-key]", 12);
    redacted = redact_prefixed_secret(&redacted, "sk-", "[redacted-openai-key]", 20);
    redacted = redact_prefixed_secret(&redacted, "gsk_", "[redacted-groq-key]", 12);
    redacted = redact_prefixed_secret(&redacted, "pcsk_", "[redacted-pinecone-key]", 12);
    redacted = redact_prefixed_secret(&redacted, "AIza", "[redacted-google-key]", 12);
    redact_bearer_token(&redacted)
}

fn redact_prefixed_secret(
    input: &str,
    prefix: &str,
    replacement: &str,
    min_secret_tail_len: usize,
) -> String {
    let mut redacted = String::with_capacity(input.len());
    let mut remaining = input;

    while let Some(index) = remaining.find(prefix) {
        let (before, candidate) = remaining.split_at(index);
        redacted.push_str(before);

        let mut end = prefix.len();
        for ch in candidate[prefix.len()..].chars() {
            if is_secret_char(ch) {
                end += ch.len_utf8();
            } else {
                break;
            }
        }

        if end.saturating_sub(prefix.len()) >= min_secret_tail_len {
            redacted.push_str(replacement);
        } else {
            redacted.push_str(&candidate[..end]);
        }

        remaining = &candidate[end..];
    }

    redacted.push_str(remaining);
    redacted
}

fn redact_bearer_token(input: &str) -> String {
    let marker = "Bearer ";
    let mut redacted = String::with_capacity(input.len());
    let mut remaining = input;

    while let Some(index) = remaining.find(marker) {
        let (before, after_marker) = remaining.split_at(index);
        redacted.push_str(before);
        redacted.push_str(marker);

        let token = &after_marker[marker.len()..];
        let mut end = 0;
        for ch in token.chars() {
            if is_secret_char(ch) {
                end += ch.len_utf8();
            } else {
                break;
            }
        }

        if end >= 12 {
            redacted.push_str("[redacted-bearer-token]");
        } else {
            redacted.push_str(&token[..end]);
        }

        remaining = &token[end..];
    }

    redacted.push_str(remaining);
    redacted
}

fn truncate_for_log(input: &str, max_chars: usize) -> String {
    let char_count = input.chars().count();
    if char_count <= max_chars {
        return input.to_string();
    }

    let truncated = input.chars().take(max_chars).collect::<String>();
    format!("{truncated} [truncated {} chars]", char_count - max_chars)
}

fn is_secret_char(ch: char) -> bool {
    ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.')
}

fn terminal_payload_with_alias(payload: &TerminalDataPayload) -> serde_json::Value {
    let mut value = serde_json::to_value(payload).expect("terminal data payload should serialize");
    if let Some(map) = value.as_object_mut() {
        if let Some(faqs) = map.get("faqs").cloned() {
            map.entry("faq".to_string()).or_insert(faqs);
        }
    }
    value
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    async fn fixture(
        status: StatusCode,
        delay_ms: u64,
        limits: [f64; 4],
    ) -> (
        Arc<AppState>,
        Arc<AtomicUsize>,
        tokio::task::JoinHandle<()>,
        PathBuf,
    ) {
        let calls = Arc::new(AtomicUsize::new(0));
        let counter = calls.clone();
        let router=Router::new().route("/",post(move || {
            let counter=counter.clone(); async move {
                counter.fetch_add(1,Ordering::SeqCst);
                tokio::time::sleep(Duration::from_millis(delay_ms)).await;
                (status,Json(serde_json::json!({"model":"gpt-5.6-luna","choices":[{"finish_reason":"stop","message":{"content":"{\"answer\":\"Micro Mages used Python.\",\"sources\":[\"projects\"]}"}}],"usage":{"prompt_tokens":100,"completion_tokens":20}})))
            }
        }));
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let task = tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap();
        });
        let dir = std::env::temp_dir().join(format!("zqs-api-test-{}", Uuid::new_v4()));
        std::fs::create_dir(&dir).unwrap();
        let data = TerminalDataPayload::load(
            &Path::new(env!("CARGO_MANIFEST_DIR")).join("../static/data"),
        )
        .unwrap();
        let state = Arc::new(AppState {
            budget: Mutex::new(Some(Budget::open(dir.join("budget.json"), limits).unwrap())),
            client: AiClient::fixture(format!("http://{addr}/")),
            slots: Semaphore::new(2),
            deadline: Duration::from_millis(80),
            log_slots: Semaphore::new(2),
            recent_logs: Mutex::new(std::collections::VecDeque::new()),
            terminal_data: Arc::new(data),
            questions_log: dir.join("questions.log"),
            answers_log: dir.join("answers.log"),
        });
        (state, calls, task, dir)
    }
    async fn request(state: Arc<AppState>) -> (StatusCode, serde_json::Value) {
        let response = handle_ai(
            State(state),
            HeaderMap::new(),
            ConnectInfo("127.0.0.1:5000".parse().unwrap()),
            Ok(Json(AiRequest {
                question: "Micro Mages?".into(),
                history: vec![],
            })),
        )
        .await;
        let status = response.status();
        let body = axum::body::to_bytes(response.into_body(), 65536)
            .await
            .unwrap();
        (status, serde_json::from_slice(&body).unwrap())
    }
    #[tokio::test]
    async fn paid_call_once_and_usage_reconciled() {
        let (state, calls, task, dir) = fixture(StatusCode::OK, 0, [1.; 4]).await;
        let (status, body) = request(state).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["sources"][0], "projects");
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        let ledger: serde_json::Value =
            serde_json::from_slice(&std::fs::read(dir.join("budget.json")).unwrap()).unwrap();
        assert_eq!(ledger.as_array().unwrap().len(), 1);
        assert!((ledger[0]["usd"].as_f64().unwrap() - 0.000044).abs() < 1e-9);
        task.abort();
        std::fs::remove_dir_all(dir).unwrap();
    }
    #[tokio::test]
    async fn spending_and_concurrency_block_before_provider() {
        let (state, calls, task, dir) = fixture(StatusCode::OK, 0, [0.; 4]).await;
        assert_eq!(request(state).await.0, StatusCode::TOO_MANY_REQUESTS);
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        task.abort();
        std::fs::remove_dir_all(dir).unwrap();
        let (state, calls, task, dir) = fixture(StatusCode::OK, 0, [1.; 4]).await;
        let permits = state.slots.acquire_many(2).await.unwrap();
        assert_eq!(request(state.clone()).await.1["reason"], "busy");
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        drop(permits);
        task.abort();
        std::fs::remove_dir_all(dir).unwrap();
    }
    #[tokio::test]
    async fn upstream_failure_and_timeout_restore_classic_mode() {
        for (status, delay, expected) in [
            (StatusCode::TOO_MANY_REQUESTS, 0, "provider_unavailable"),
            (StatusCode::OK, 300, "timeout"),
        ] {
            let (state, calls, task, dir) = fixture(status, delay, [1.; 4]).await;
            let (_, body) = request(state.clone()).await;
            assert_eq!(body["ai_enabled"], false);
            assert_eq!(body["reason"], expected);
            assert_eq!(calls.load(Ordering::SeqCst), 1);
            assert_eq!(state.slots.available_permits(), 2);
            let data = terminal_payload_with_alias(&state.terminal_data);
            assert!(data["experiences"][0]["company"]
                .as_str()
                .unwrap()
                .contains("Studi"));
            task.abort();
            std::fs::remove_dir_all(dir).unwrap();
        }
    }
    #[test]
    fn forwarded_header_cannot_spoof_limiter() {
        let mut h = HeaderMap::new();
        h.insert("x-forwarded-for", HeaderValue::from_static("fake, 1.2.3.4"));
        h.insert("x-real-ip", HeaderValue::from_static("1.2.3.4"));
        assert_eq!(client_ip(&h, "127.0.0.1:1".parse().unwrap()), "1.2.3.4");
        assert_eq!(client_ip(&h, "5.6.7.8:1".parse().unwrap()), "5.6.7.8");
    }
    #[test]
    fn preserves_rich_knowledge_and_current_role() {
        let data = TerminalDataPayload::load(
            &Path::new(env!("CARGO_MANIFEST_DIR")).join("../static/data"),
        )
        .unwrap();
        let text = data.knowledge_json().to_string();
        for fact in [
            "50,000",
            "Langfuse",
            "Micro Mages",
            "BeeToBee",
            "Machine Learning and Cancer Prediction",
            "500K",
            "60+",
            "Mar 2026",
            "Apr 2026",
        ] {
            assert!(text.contains(fact), "missing {fact}");
        }
        assert!(data.experiences[0]["company"]
            .as_str()
            .unwrap()
            .contains("Studi"));
        assert_eq!(data.testimonials.as_array().unwrap().len(), 2);
        assert_eq!(
            data.profile["links"]["resume_url"],
            "https://cv.zqsdev.com/"
        );
    }
    #[test]
    fn log_redaction_and_cache_behavior() {
        let output = sanitize_log_text(
            "hey\n sk-abcdefghijklmnopqrstuvwxyz Bearer abcdefghijklmnopqrstuvwxyz",
        );
        assert!(!output.contains("abcdefghijklmnopqrstuvwxyz"));
        assert!(!output.contains('\n'));
        assert_eq!(cache_control_for_path("/cv/fr.html"), "no-store");
    }
}
