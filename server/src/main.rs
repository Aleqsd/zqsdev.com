mod rate_limit;

use crate::rate_limit::RateLimiter;
use anyhow::{anyhow, Context};
use axum::extract::{ConnectInfo, State};
use axum::http::{Request, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::{body::Body, Json, Router};
use dotenvy::Error as DotenvError;
use once_cell::sync::Lazy;
use serde::{Deserialize, Serialize};
use std::convert::Infallible;
use std::env::VarError;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::net::TcpListener;
use tokio::signal;
use tokio::sync::Mutex;
use tower::service_fn;
use tower::ServiceExt;
use tower_http::services::ServeDir;
use tracing::{error, info, warn};

const GOOGLE_MODEL_NAME: &str = "gemini-2.5-flash-lite";
const GOOGLE_ENDPOINT: &str =
    "https://generativelanguage.googleapis.com/v1beta/models/gemini-2.5-flash-lite:generateContent";
const GROQ_MODEL_NAME: &str = "llama-3.1-8b-instant";
const GROQ_ENDPOINT: &str = "https://api.groq.com/openai/v1/chat/completions";
const OPENAI_MODEL_NAME: &str = "gpt-4o-mini";
const OPENAI_ENDPOINT: &str = "https://api.openai.com/v1/chat/completions";
const MAX_COMPLETION_TOKENS: usize = 384;
const USER_OVERHEAD_TOKENS: usize = 32;
const INPUT_COST_EUR_PER_1K: f64 = 0.000552; // Converted from $0.0006 ≈ €0.000552 (fx ~0.92)
const OUTPUT_COST_EUR_PER_1K: f64 = 0.002208; // Converted from $0.0024 ≈ €0.002208
const PER_MINUTE_BUDGET_EUR: f64 = 0.50;
const PER_HOUR_BUDGET_EUR: f64 = 2.00;
const PER_DAY_BUDGET_EUR: f64 = 2.00; // Align daily to €2 hard cap
const PER_MONTH_BUDGET_EUR: f64 = 10.00;

static KNOWLEDGE_FILES: Lazy<[&str; 7]> = Lazy::new(|| {
    [
        "profile.json",
        "skills.json",
        "experience.json",
        "education.json",
        "projects.json",
        "testimonials.json",
        "faq.json",
    ]
});

#[derive(Clone)]
struct AppState {
    limiter: Arc<Mutex<RateLimiter>>,
    knowledge: KnowledgeBase,
    client: AiClient,
}

#[derive(Debug, Clone)]
struct KnowledgeBase {
    system_prompt: String,
    system_tokens: usize,
}

#[derive(Clone)]
struct AiClient {
    http: reqwest::Client,
    google: Option<GoogleBackend>,
    groq: Option<ApiBackend>,
    openai: Option<ApiBackend>,
}

#[derive(Clone)]
struct GoogleBackend {
    endpoint: &'static str,
    model: &'static str,
    api_key: Arc<String>,
}

#[derive(Clone)]
struct ApiBackend {
    endpoint: &'static str,
    model: &'static str,
    api_key: Arc<String>,
}

struct AiAnswer {
    text: String,
    model: &'static str,
    cost_eur: f64,
}

#[derive(Debug, Deserialize)]
struct AiRequest {
    question: String,
}

#[derive(Debug, Serialize)]
struct AiResponse {
    answer: String,
    ai_enabled: bool,
    reason: Option<String>,
    model: Option<&'static str>,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    load_env_files();
    configure_tracing();

    let google_key = match std::env::var("GOOGLE_API_KEY") {
        Ok(value) => Some(value),
        Err(VarError::NotPresent) => {
            warn!(target: "ai", msg = "GOOGLE_API_KEY not set; defaulting to Groq/OpenAI backends");
            None
        }
        Err(VarError::NotUnicode(err)) => {
            return Err(anyhow!(
                "GOOGLE_API_KEY contains invalid unicode: {:?}",
                err
            ));
        }
    };

    let groq_key = match std::env::var("GROQ_API_KEY") {
        Ok(value) => Some(value),
        Err(VarError::NotPresent) => {
            warn!(target: "ai", msg = "GROQ_API_KEY not set; defaulting to Gemini/OpenAI backends");
            None
        }
        Err(VarError::NotUnicode(err)) => {
            return Err(anyhow!("GROQ_API_KEY contains invalid unicode: {:?}", err));
        }
    };

    let openai_key = std::env::var("OPENAI_API_KEY")
        .context("OPENAI_API_KEY is required to run the AI proxy server")?;

    let static_dir =
        PathBuf::from(std::env::var("STATIC_DIR").unwrap_or_else(|_| "static".to_string()));
    let data_dir = static_dir.join("data");
    let knowledge = KnowledgeBase::load(&data_dir)?;

    let client = AiClient::new(google_key, groq_key, Some(openai_key))?;
    if client.has_groq() {
        info!(
            target: "ai",
            model = GROQ_MODEL_NAME,
            msg = "Groq backend configured as primary model"
        );
    }
    if client.has_google() {
        info!(
            target: "ai",
            model = GOOGLE_MODEL_NAME,
            msg = if client.has_groq() {
                "Google backend configured as secondary fallback"
            } else {
                "Google backend configured as primary model"
            }
        );
    }
    if client.has_openai() {
        info!(
            target: "ai",
            model = OPENAI_MODEL_NAME,
            msg = "OpenAI fallback backend configured"
        );
    }
    let default_model = client.primary_model().unwrap_or(OPENAI_MODEL_NAME);
    let state = Arc::new(AppState {
        limiter: Arc::new(Mutex::new(RateLimiter::new(
            PER_MINUTE_BUDGET_EUR,
            PER_HOUR_BUDGET_EUR,
            PER_DAY_BUDGET_EUR,
            PER_MONTH_BUDGET_EUR,
        ))),
        knowledge,
        client,
    });

    let static_root = Arc::new(static_dir.clone());
    let static_service = service_fn(move |req: Request<Body>| {
        let dir =
            ServeDir::new(static_root.as_ref().clone()).append_index_html_on_directories(true);
        async move {
            match dir.oneshot(req).await {
                Ok(response) => Ok::<Response, Infallible>(response.into_response()),
                Err(err) => Ok((
                    StatusCode::INTERNAL_SERVER_ERROR,
                    format!("Static file error: {err}"),
                )
                    .into_response()),
            }
        }
    });

    let router = Router::new()
        .route("/api/ai", post(handle_ai))
        .with_state(state)
        .fallback_service(static_service);

    let host = std::env::var("HOST").unwrap_or_else(|_| "0.0.0.0".to_string());
    let port: u16 = std::env::var("PORT")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(3000);
    let addr: SocketAddr = format!("{host}:{port}")
        .parse()
        .context("Invalid HOST/PORT combination")?;

    let listener = TcpListener::bind(addr)
        .await
        .context("Failed to bind TCP listener")?;
    let bound = listener
        .local_addr()
        .context("Failed to read listener address")?;
    info!(listening = %bound, model = default_model, msg = "server ready");

    axum::serve(
        listener,
        router.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(shutdown_signal())
    .await?;

    Ok(())
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

async fn handle_ai(
    State(state): State<Arc<AppState>>,
    ConnectInfo(remote): ConnectInfo<SocketAddr>,
    Json(payload): Json<AiRequest>,
) -> impl IntoResponse {
    let question = payload.question.trim();
    let primary_model = state.client.primary_model();
    if question.is_empty() {
        let response = AiResponse {
            answer: "Please provide a question so the AI can help.".to_string(),
            ai_enabled: true,
            reason: Some("empty_question".to_string()),
            model: primary_model,
        };
        return (StatusCode::BAD_REQUEST, Json(response));
    }

    if question.len() > 800 {
        let response = AiResponse {
            answer: "Question is too long for the lightweight AI mode. Please shorten it."
                .to_string(),
            ai_enabled: true,
            reason: Some("question_too_long".to_string()),
            model: primary_model,
        };
        return (StatusCode::BAD_REQUEST, Json(response));
    }

    let openai_cost_estimate = state.estimate_openai_cost(question);
    let request_cost_estimate = state.estimate_cost(question);

    let ip = remote.ip().to_string();
    let mut limiter = state.limiter.lock().await;
    if let Err(limit) = limiter.check_and_record(&ip, request_cost_estimate) {
        let snapshot = limiter.usage_snapshot(&ip);
        drop(limiter);
        let (status, reason, detail) = limit.describe();
        warn!(
            target: "ai",
            ip = %ip,
            reason,
            minute_eur = snapshot.minute_spend,
            hour_eur = snapshot.hour_spend,
            day_eur = snapshot.day_spend,
            month_eur = snapshot.month_spend,
            ip_burst = snapshot.ip_burst,
            ip_minute = snapshot.ip_minute,
            ip_hour = snapshot.ip_hour,
            ip_day = snapshot.ip_day,
            cost_estimate_eur = request_cost_estimate,
            "AI request blocked by limiter"
        );
        let response = AiResponse {
            answer: format!(
                "AI usage limit reached ({detail}). Switching back to the classic mode for now."
            ),
            ai_enabled: false,
            reason: Some(reason.to_string()),
            model: primary_model,
        };
        return (status, Json(response));
    }
    let mut snapshot = limiter.usage_snapshot(&ip);
    drop(limiter);

    match state
        .client
        .ask(&state.knowledge, question, openai_cost_estimate)
        .await
    {
        Ok(ai_answer) => {
            let AiAnswer {
                text: answer_text,
                model,
                cost_eur,
            } = ai_answer;
            if cost_eur > 0.0 {
                let mut limiter = state.limiter.lock().await;
                if let Err(limit) = limiter.record_cost_if_within(cost_eur) {
                    let snapshot = limiter.usage_snapshot(&ip);
                    drop(limiter);
                    let (status, reason, detail) = limit.describe();
                    warn!(
                        target: "ai",
                        ip = %ip,
                        model,
                        minute_eur = snapshot.minute_spend,
                        hour_eur = snapshot.hour_spend,
                        day_eur = snapshot.day_spend,
                        month_eur = snapshot.month_spend,
                        ip_burst = snapshot.ip_burst,
                        ip_minute = snapshot.ip_minute,
                        ip_hour = snapshot.ip_hour,
                        ip_day = snapshot.ip_day,
                        cost_estimate_eur = cost_eur,
                        "AI response discarded due to budget after backend call"
                    );
                    let response = AiResponse {
                        answer: format!(
                            "AI usage limit reached ({detail}). Switching back to the classic mode for now."
                        ),
                        ai_enabled: false,
                        reason: Some(reason.to_string()),
                        model: Some(model),
                    };
                    return (status, Json(response));
                }
                snapshot = limiter.usage_snapshot(&ip);
                drop(limiter);
            }
            info!(
                target: "ai",
                ip = %ip,
                model,
                minute_eur = snapshot.minute_spend,
                hour_eur = snapshot.hour_spend,
                day_eur = snapshot.day_spend,
                month_eur = snapshot.month_spend,
                ip_burst = snapshot.ip_burst,
                ip_minute = snapshot.ip_minute,
                ip_hour = snapshot.ip_hour,
                ip_day = snapshot.ip_day,
                cost_estimate_eur = cost_eur,
                "AI request served"
            );
            info!(
                target: "ai",
                model,
                user_question = question,
                "AI request prompt logged"
            );
            info!(
                target: "ai",
                model,
                ai_answer = answer_text.as_str(),
                "AI request answer logged"
            );
            let response = AiResponse {
                answer: answer_text,
                ai_enabled: true,
                reason: None,
                model: Some(model),
            };
            (StatusCode::OK, Json(response))
        }
        Err(err) => {
            info!(
                target: "ai",
                ip = %ip,
                minute_eur = snapshot.minute_spend,
                hour_eur = snapshot.hour_spend,
                day_eur = snapshot.day_spend,
                month_eur = snapshot.month_spend,
                ip_burst = snapshot.ip_burst,
                ip_minute = snapshot.ip_minute,
                ip_hour = snapshot.ip_hour,
                ip_day = snapshot.ip_day,
                cost_estimate_eur = request_cost_estimate,
                "AI request failed"
            );
            error!(target: "ai", backend_error = %err, question = question);
            let response = AiResponse {
                answer: format!(
                    "The AI backend is temporarily unavailable ({err}). Please retry in a moment."
                ),
                ai_enabled: true,
                reason: Some("backend_error".to_string()),
                model: primary_model,
            };
            (StatusCode::SERVICE_UNAVAILABLE, Json(response))
        }
    }
}

impl AppState {
    fn estimate_cost(&self, question: &str) -> f64 {
        if self.client.has_free_backend() {
            0.0
        } else {
            self.estimate_openai_cost(question)
        }
    }

    fn estimate_openai_cost(&self, question: &str) -> f64 {
        let question_tokens = estimate_tokens(question);
        let input_tokens = self.knowledge.system_tokens + question_tokens + USER_OVERHEAD_TOKENS;
        let output_tokens = MAX_COMPLETION_TOKENS;
        tokens_to_cost(input_tokens, output_tokens)
    }
}

impl KnowledgeBase {
    fn load(dir: &Path) -> anyhow::Result<Self> {
        let mut merged = serde_json::Map::new();
        for file in KNOWLEDGE_FILES.iter() {
            let path = dir.join(file);
            let data = std::fs::read_to_string(&path)
                .with_context(|| format!("Failed to read knowledge file {path:?}"))?;
            let value: serde_json::Value = serde_json::from_str(&data)
                .with_context(|| format!("Failed to parse JSON from {path:?}"))?;
            let key = file.trim_end_matches(".json").to_string();
            merged.insert(key, value);
        }

        let combined = serde_json::Value::Object(merged);
        let pretty = serde_json::to_string_pretty(&combined)?;
        let system_prompt = format!(
            concat!(
                "You are an assistant for Alexandre DO-O ALMEIDA. ",
                "Only answer using the facts provided in the JSON knowledge base and keep responses detailed and structured. ",
                "If requested details are missing, share the closest matching facts and clearly label any inference as an \"educated guess\" without presenting it as certain. ",
                "Never invent information that contradicts the knowledge base.\n\n",
                "Knowledge base (JSON):\n{}"
            ),
            pretty
        );
        let system_tokens = estimate_tokens(&system_prompt);

        Ok(Self {
            system_prompt,
            system_tokens,
        })
    }
}

impl AiClient {
    fn new(
        google_key: Option<String>,
        groq_key: Option<String>,
        openai_key: Option<String>,
    ) -> anyhow::Result<Self> {
        if google_key.is_none() && groq_key.is_none() && openai_key.is_none() {
            return Err(anyhow!(
                "No AI provider configured. Provide GOOGLE_API_KEY, GROQ_API_KEY, or OPENAI_API_KEY."
            ));
        }

        let http = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(20))
            .build()?;

        let google = google_key.map(|key| GoogleBackend {
            endpoint: GOOGLE_ENDPOINT,
            model: GOOGLE_MODEL_NAME,
            api_key: Arc::new(key),
        });
        let groq = groq_key.map(|key| ApiBackend {
            endpoint: GROQ_ENDPOINT,
            model: GROQ_MODEL_NAME,
            api_key: Arc::new(key),
        });
        let openai = openai_key.map(|key| ApiBackend {
            endpoint: OPENAI_ENDPOINT,
            model: OPENAI_MODEL_NAME,
            api_key: Arc::new(key),
        });

        Ok(Self {
            http,
            google,
            groq,
            openai,
        })
    }

    fn has_google(&self) -> bool {
        self.google.is_some()
    }

    fn has_groq(&self) -> bool {
        self.groq.is_some()
    }

    fn has_openai(&self) -> bool {
        self.openai.is_some()
    }

    fn has_free_backend(&self) -> bool {
        self.groq.is_some() || self.google.is_some()
    }

    fn primary_model(&self) -> Option<&'static str> {
        if let Some(groq) = &self.groq {
            Some(groq.model)
        } else if let Some(google) = &self.google {
            Some(google.model)
        } else {
            self.openai.as_ref().map(|openai| openai.model)
        }
    }

    async fn ask(
        &self,
        knowledge: &KnowledgeBase,
        question: &str,
        openai_cost: f64,
    ) -> Result<AiAnswer, AiClientError> {
        let mut failures = Vec::new();

        if let Some(groq) = &self.groq {
            match self.ask_backend(groq, knowledge, question, 0.0).await {
                Ok(answer) => {
                    return Ok(AiAnswer {
                        text: answer,
                        model: groq.model,
                        cost_eur: 0.0,
                    });
                }
                Err(error) => {
                    let fallback = match (self.google.is_some(), self.openai.is_some()) {
                        (true, _) => "Gemini fallback",
                        (false, true) => "OpenAI fallback",
                        _ => "no fallback available",
                    };
                    warn!(
                        target: "ai",
                        model = groq.model,
                        error = %error,
                        fallback,
                        "Groq backend error"
                    );
                    failures.push(BackendFailure::new(BackendKind::Groq, error));
                }
            }
        }

        if let Some(google) = &self.google {
            match self.ask_google(google, knowledge, question).await {
                Ok(answer) => {
                    return Ok(AiAnswer {
                        text: answer,
                        model: google.model,
                        cost_eur: 0.0,
                    });
                }
                Err(error) => {
                    let fallback = if self.openai.is_some() {
                        "OpenAI fallback"
                    } else {
                        "no fallback available"
                    };
                    warn!(
                        target: "ai",
                        model = google.model,
                        error = %error,
                        fallback,
                        "Google backend error"
                    );
                    failures.push(BackendFailure::new(BackendKind::Google, error));
                }
            }
        }

        if let Some(openai) = &self.openai {
            match self
                .ask_backend(openai, knowledge, question, openai_cost)
                .await
            {
                Ok(answer) => {
                    return Ok(AiAnswer {
                        text: answer,
                        model: openai.model,
                        cost_eur: openai_cost,
                    });
                }
                Err(error) => {
                    error!(
                        target: "ai",
                        model = openai.model,
                        error = %error,
                        "OpenAI fallback failed after other backends"
                    );
                    failures.push(BackendFailure::new(BackendKind::OpenAi, error));
                    return Err(AiClientError::all_backends_failed(failures));
                }
            }
        }

        if failures.is_empty() {
            Err(AiClientError::NoBackendConfigured)
        } else {
            Err(AiClientError::all_backends_failed(failures))
        }
    }

    async fn ask_google(
        &self,
        backend: &GoogleBackend,
        knowledge: &KnowledgeBase,
        question: &str,
    ) -> Result<String, BackendError> {
        let payload = GoogleGenerateRequest::new(&knowledge.system_prompt, question);
        let response = self
            .http
            .post(backend.endpoint)
            .header("x-goog-api-key", backend.api_key.as_str())
            .json(&payload)
            .send()
            .await?;

        let status = response.status();
        if !status.is_success() {
            let detail = response.text().await.unwrap_or_default();
            return Err(BackendError::ApiFailure(status, detail));
        }

        let body: GoogleGenerateResponse = response.json().await?;
        let answer = body
            .candidates
            .unwrap_or_default()
            .into_iter()
            .find_map(GoogleCandidate::into_text)
            .filter(|value| !value.is_empty())
            .ok_or(BackendError::EmptyAnswer)?;

        info!(
            target: "ai",
            cost_eur = 0.0,
            chars = question.len(),
            model = backend.model,
            msg = "AI response generated by backend"
        );
        Ok(answer)
    }

    async fn ask_backend(
        &self,
        backend: &ApiBackend,
        knowledge: &KnowledgeBase,
        question: &str,
        cost_eur: f64,
    ) -> Result<String, BackendError> {
        let payload = ChatRequest::new(backend.model, knowledge, question);
        let response = self
            .http
            .post(backend.endpoint)
            .bearer_auth(backend.api_key.as_str())
            .json(&payload)
            .send()
            .await?;

        let status = response.status();
        if !status.is_success() {
            let detail = response.text().await.unwrap_or_default();
            return Err(BackendError::ApiFailure(status, detail));
        }

        let body: ChatResponse = response.json().await?;
        let answer = body
            .choices
            .into_iter()
            .find_map(|choice| choice.message.content.map(|c| c.trim().to_string()))
            .filter(|value| !value.is_empty())
            .ok_or(BackendError::EmptyAnswer)?;

        info!(
            target: "ai",
            cost_eur,
            chars = question.len(),
            model = backend.model,
            msg = "AI response generated by backend"
        );
        Ok(answer)
    }
}

#[derive(Debug)]
struct BackendFailure {
    backend: BackendKind,
    error: BackendError,
}

impl BackendFailure {
    fn new(backend: BackendKind, error: BackendError) -> Self {
        Self { backend, error }
    }
}

#[derive(Debug, Clone, Copy)]
enum BackendKind {
    Google,
    Groq,
    OpenAi,
}

impl BackendKind {
    fn as_str(&self) -> &'static str {
        match self {
            BackendKind::Google => "Google",
            BackendKind::Groq => "Groq",
            BackendKind::OpenAi => "OpenAI",
        }
    }
}

#[derive(Debug, thiserror::Error)]
enum BackendError {
    #[error("network error: {0}")]
    Network(#[from] reqwest::Error),
    #[error("api failure ({0}): {1}")]
    ApiFailure(StatusCode, String),
    #[error("AI response did not contain any answer")]
    EmptyAnswer,
}

#[derive(Debug, thiserror::Error)]
enum AiClientError {
    #[error("No AI backend is configured")]
    NoBackendConfigured,
    #[error("All AI backends failed: {0}")]
    AllBackendsFailed(String),
}

impl AiClientError {
    fn all_backends_failed(failures: Vec<BackendFailure>) -> Self {
        let summary = failures
            .into_iter()
            .map(|failure| {
                format!(
                    "{} backend failed: {}",
                    failure.backend.as_str(),
                    failure.error
                )
            })
            .collect::<Vec<_>>()
            .join("; ");
        AiClientError::AllBackendsFailed(summary)
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct GoogleGenerateRequest<'a> {
    contents: [GoogleContent<'a>; 1],
    system_instruction: GoogleContent<'a>,
    generation_config: GoogleGenerationConfig,
}

#[derive(Serialize)]
struct GoogleContent<'a> {
    #[serde(skip_serializing_if = "Option::is_none")]
    role: Option<&'a str>,
    parts: [GooglePart<'a>; 1],
}

#[derive(Serialize)]
struct GooglePart<'a> {
    text: &'a str,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct GoogleGenerationConfig {
    temperature: f32,
    max_output_tokens: u32,
}

impl<'a> GoogleGenerateRequest<'a> {
    fn new(system_prompt: &'a str, question: &'a str) -> Self {
        Self {
            contents: [GoogleContent::user(question)],
            system_instruction: GoogleContent::instruction(system_prompt),
            generation_config: GoogleGenerationConfig::new(0.3, MAX_COMPLETION_TOKENS as u32),
        }
    }
}

impl<'a> GoogleContent<'a> {
    fn instruction(text: &'a str) -> Self {
        Self {
            role: None,
            parts: [GooglePart { text }],
        }
    }

    fn user(text: &'a str) -> Self {
        Self {
            role: Some("user"),
            parts: [GooglePart { text }],
        }
    }
}

impl GoogleGenerationConfig {
    fn new(temperature: f32, max_output_tokens: u32) -> Self {
        Self {
            temperature,
            max_output_tokens,
        }
    }
}

#[derive(Serialize)]
struct ChatRequest<'a> {
    model: &'a str,
    temperature: f32,
    max_tokens: usize,
    messages: [ChatMessage<'a>; 2],
}

#[derive(Serialize)]
struct ChatMessage<'a> {
    role: &'static str,
    content: &'a str,
}

impl<'a> ChatRequest<'a> {
    fn new(model: &'a str, knowledge: &'a KnowledgeBase, question: &'a str) -> Self {
        Self {
            model,
            temperature: 0.3,
            max_tokens: MAX_COMPLETION_TOKENS,
            messages: [
                ChatMessage {
                    role: "system",
                    content: &knowledge.system_prompt,
                },
                ChatMessage {
                    role: "user",
                    content: question,
                },
            ],
        }
    }
}

#[derive(Deserialize)]
struct ChatResponse {
    choices: Vec<ChatChoice>,
}

#[derive(Deserialize)]
struct ChatChoice {
    message: ChatChoiceMessage,
}

#[derive(Deserialize)]
struct ChatChoiceMessage {
    content: Option<String>,
}

#[derive(Deserialize)]
struct GoogleGenerateResponse {
    candidates: Option<Vec<GoogleCandidate>>,
}

#[derive(Deserialize)]
struct GoogleCandidate {
    content: Option<GoogleCandidateContent>,
}

#[derive(Deserialize)]
struct GoogleCandidateContent {
    parts: Option<Vec<GoogleCandidatePart>>,
}

#[derive(Deserialize)]
struct GoogleCandidatePart {
    text: Option<String>,
}

impl GoogleCandidate {
    fn into_text(self) -> Option<String> {
        self.content.and_then(|content| {
            content
                .parts
                .unwrap_or_default()
                .into_iter()
                .find_map(|part| {
                    part.text
                        .map(|text| text.trim().to_string())
                        .filter(|value| !value.is_empty())
                })
        })
    }
}

fn estimate_tokens(text: &str) -> usize {
    let chars = text.chars().count() as f64;
    (chars / 4.0).ceil() as usize
}

fn tokens_to_cost(input_tokens: usize, output_tokens: usize) -> f64 {
    let input_cost = INPUT_COST_EUR_PER_1K * (input_tokens as f64 / 1000.0);
    let output_cost = OUTPUT_COST_EUR_PER_1K * (output_tokens as f64 / 1000.0);
    (input_cost + output_cost).max(0.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn load_embedded_knowledge() -> serde_json::Value {
        let data_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../static/data");
        let knowledge =
            KnowledgeBase::load(&data_dir).expect("should load knowledge base from static data");
        let (_, json_text) = knowledge
            .system_prompt
            .split_once("Knowledge base (JSON):\n")
            .expect("system prompt should embed knowledge JSON");
        serde_json::from_str(json_text).expect("embedded knowledge should be valid JSON")
    }

    #[test]
    fn profile_links_target_primary_domains() {
        let data = load_embedded_knowledge();
        let links = data
            .get("profile")
            .and_then(|profile| profile.get("links"))
            .expect("profile.links should be present");

        let resume = links
            .get("resume_url")
            .and_then(|value| value.as_str())
            .expect("profile.links.resume_url should be populated");
        assert!(
            resume.starts_with("https://cv.zqsdev.com"),
            "Résumé link should point to the cv subdomain: {resume}"
        );

        let website = links
            .get("website")
            .and_then(|value| value.as_str())
            .expect("profile.links.website should be populated");
        assert!(
            website.starts_with("https://www.zqsdev.com")
                || website.starts_with("https://zqsdev.com"),
            "Website link should target the primary domain: {website}"
        );
    }

    #[test]
    fn token_estimate_is_positive() {
        let sample = "Hello world";
        assert!(estimate_tokens(sample) > 0);
    }

    #[test]
    fn cost_calculation_scales_with_tokens() {
        let low = tokens_to_cost(500, 100);
        let high = tokens_to_cost(5000, 1000);
        assert!(high > low);
    }

    #[test]
    fn primary_model_falls_back_through_backends() {
        let client = AiClient::new(
            Some("google-key".to_string()),
            Some("groq-key".to_string()),
            Some("openai-key".to_string()),
        )
        .expect("client should construct");
        assert_eq!(client.primary_model(), Some(GROQ_MODEL_NAME));

        let client = AiClient::new(
            Some("google-key".to_string()),
            None,
            Some("openai-key".to_string()),
        )
        .expect("client should construct without Groq");
        assert_eq!(client.primary_model(), Some(GOOGLE_MODEL_NAME));

        let client =
            AiClient::new(None, None, Some("openai-key".to_string())).expect("OpenAI only");
        assert_eq!(client.primary_model(), Some(OPENAI_MODEL_NAME));
    }

    #[test]
    fn ai_response_serializes_model_field() {
        let response = AiResponse {
            answer: "Answer".to_string(),
            ai_enabled: true,
            reason: None,
            model: Some(GROQ_MODEL_NAME),
        };
        let value = serde_json::to_value(&response).expect("serialize response");
        assert_eq!(
            value.get("model").and_then(|entry| entry.as_str()),
            Some(GROQ_MODEL_NAME),
            "Serialized AI response should expose the backend model"
        );
    }

    #[test]
    fn chat_request_uses_backend_model() {
        let knowledge = KnowledgeBase {
            system_prompt: "prompt".to_string(),
            system_tokens: 4,
        };
        let question = "What is the latest project?";
        let request = ChatRequest::new(GROQ_MODEL_NAME, &knowledge, question);
        assert_eq!(request.model, GROQ_MODEL_NAME);
        assert_eq!(request.messages[0].content, "prompt");
        assert_eq!(request.messages[1].content, question);
    }

    #[test]
    fn google_request_includes_prompt_and_question() {
        let prompt = "system instructions";
        let question = "Tell me about Alexandre.";
        let request = GoogleGenerateRequest::new(prompt, question);
        assert_eq!(request.system_instruction.parts[0].text, prompt);
        assert_eq!(request.contents[0].parts[0].text, question);
        assert_eq!(request.contents[0].role, Some("user"));
        assert_eq!(
            request.generation_config.max_output_tokens,
            MAX_COMPLETION_TOKENS as u32
        );
    }

    #[test]
    fn google_candidate_extracts_trimmed_text() {
        let candidate = GoogleCandidate {
            content: Some(GoogleCandidateContent {
                parts: Some(vec![GoogleCandidatePart {
                    text: Some("  Answer with whitespace  ".to_string()),
                }]),
            }),
        };
        assert_eq!(
            GoogleCandidate::into_text(candidate),
            Some("Answer with whitespace".to_string())
        );
    }

    #[test]
    fn estimate_cost_zero_when_free_backend_available() {
        let client = AiClient::new(
            Some("google_key".to_string()),
            None,
            Some("openai_key".to_string()),
        )
        .expect("client should construct");
        let knowledge = KnowledgeBase {
            system_prompt: "prompt".to_string(),
            system_tokens: 8,
        };
        let app_state = AppState {
            limiter: std::sync::Arc::new(tokio::sync::Mutex::new(RateLimiter::new(
                PER_MINUTE_BUDGET_EUR,
                PER_HOUR_BUDGET_EUR,
                PER_DAY_BUDGET_EUR,
                PER_MONTH_BUDGET_EUR,
            ))),
            knowledge,
            client,
        };
        assert_eq!(app_state.estimate_cost("Hello AI?"), 0.0);
    }

    #[test]
    fn faq_knowledge_reflects_latest_details() {
        let data = load_embedded_knowledge();
        let faqs = data
            .get("faq")
            .and_then(|value| value.as_array())
            .expect("faq data should be an array");

        let remote = faqs
            .iter()
            .find(|entry| {
                entry.get("question").and_then(|value| value.as_str())
                    == Some("🌍 Are you open to remote roles?")
            })
            .and_then(|entry| entry.get("answer"))
            .and_then(|value| value.as_str())
            .expect("remote roles FAQ should be present");
        assert!(
            remote.contains("remote-first"),
            "Remote FAQ answer should mention remote-first culture: {remote}"
        );
        assert!(
            remote.contains("Montpellier"),
            "Remote FAQ answer should include current location: {remote}"
        );
        assert!(
            remote.contains("2026"),
            "Remote FAQ answer should reference relocation timeline: {remote}"
        );

        let industries = faqs
            .iter()
            .find(|entry| {
                entry.get("question").and_then(|value| value.as_str())
                    == Some("🏢 What industries do you focus on?")
            })
            .and_then(|entry| entry.get("answer"))
            .and_then(|value| value.as_str())
            .expect("industry focus FAQ should be present");
        assert!(
            industries.contains("Gaming"),
            "Industry answer should include gaming focus: {industries}"
        );
        assert!(
            industries.contains("biotech"),
            "Industry answer should include biotech focus: {industries}"
        );
        assert!(
            industries.contains("automation"),
            "Industry answer should mention automation projects: {industries}"
        );

        let leadership = faqs
            .iter()
            .find(|entry| {
                entry.get("question").and_then(|value| value.as_str())
                    == Some("👥 Can you lead cross-functional teams?")
            })
            .and_then(|entry| entry.get("answer"))
            .and_then(|value| value.as_str())
            .expect("leadership FAQ should be present");
        assert!(
            leadership.contains("PlayStation") && leadership.contains("Atos"),
            "Leadership answer should reference enterprise teams: {leadership}"
        );
        assert!(
            leadership.contains("Jam.gg"),
            "Leadership answer should mention Jam.gg founding role: {leadership}"
        );
        assert!(
            leadership.contains("artists")
                && leadership.contains("QA")
                && leadership.contains("marketing"),
            "Leadership answer should cover cross-discipline personal projects: {leadership}"
        );

        let ai_usage = faqs
            .iter()
            .find(|entry| {
                entry.get("question").and_then(|value| value.as_str())
                    == Some("🤖 How do you use AI in your workflow?")
            })
            .and_then(|entry| entry.get("answer"))
            .and_then(|value| value.as_str())
            .expect("AI workflow FAQ should be present");
        assert!(
            ai_usage.contains("Codex CLI") && ai_usage.contains("Gemini CLI"),
            "AI usage answer should cite key copilots: {ai_usage}"
        );
        assert!(
            ai_usage.contains("multiple projects") && ai_usage.contains("parallel"),
            "AI usage answer should highlight concurrent workflows: {ai_usage}"
        );

        let availability = faqs
            .iter()
            .find(|entry| {
                entry.get("question").and_then(|value| value.as_str())
                    == Some("⏱️ How soon can you start?")
            })
            .and_then(|entry| entry.get("answer"))
            .and_then(|value| value.as_str())
            .expect("availability FAQ should be present");
        assert!(
            availability.contains("start this month"),
            "Availability answer should confirm immediate start: {availability}"
        );
    }
}
