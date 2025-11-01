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
    groq: Option<ApiBackend>,
    openai: Option<ApiBackend>,
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
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    load_env_files();
    configure_tracing();

    let groq_key = match std::env::var("GROQ_API_KEY") {
        Ok(value) => Some(value),
        Err(VarError::NotPresent) => {
            warn!(target: "ai", msg = "GROQ_API_KEY not set; defaulting to OpenAI backend");
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

    let client = AiClient::new(groq_key, Some(openai_key))?;
    if client.has_groq() {
        info!(
            target: "ai",
            model = GROQ_MODEL_NAME,
            msg = "Groq backend configured as default model"
        );
    }
    if client.has_openai() {
        info!(
            target: "ai",
            model = OPENAI_MODEL_NAME,
            msg = "OpenAI fallback backend configured"
        );
    }
    let default_model = if client.has_groq() {
        GROQ_MODEL_NAME
    } else {
        OPENAI_MODEL_NAME
    };
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
    if question.is_empty() {
        let response = AiResponse {
            answer: "Please provide a question so the AI can help.".to_string(),
            ai_enabled: true,
            reason: Some("empty_question".to_string()),
        };
        return (StatusCode::BAD_REQUEST, Json(response));
    }

    if question.len() > 800 {
        let response = AiResponse {
            answer: "Question is too long for the lightweight AI mode. Please shorten it."
                .to_string(),
            ai_enabled: true,
            reason: Some("question_too_long".to_string()),
        };
        return (StatusCode::BAD_REQUEST, Json(response));
    }

    let estimate = state.estimate_cost(question);

    let ip = remote.ip().to_string();
    let mut limiter = state.limiter.lock().await;
    if let Err(limit) = limiter.check_and_record(&ip, estimate) {
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
            cost_estimate_eur = estimate,
            "AI request blocked by limiter"
        );
        let response = AiResponse {
            answer: format!(
                "AI usage limit reached ({detail}). Switching back to the classic mode for now."
            ),
            ai_enabled: false,
            reason: Some(reason.to_string()),
        };
        return (status, Json(response));
    }
    let snapshot = limiter.usage_snapshot(&ip);
    drop(limiter);

    match state
        .client
        .ask(&state.knowledge, question, estimate)
        .await
    {
        Ok(ai_answer) => {
            let AiAnswer { text: answer_text, model } = ai_answer;
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
                cost_estimate_eur = estimate,
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
                cost_estimate_eur = estimate,
                "AI request failed"
            );
            error!(target: "ai", backend_error = %err, question = question);
            let response = AiResponse {
                answer: format!(
                    "The AI backend is temporarily unavailable ({err}). Please retry in a moment."
                ),
                ai_enabled: true,
                reason: Some("backend_error".to_string()),
            };
            (StatusCode::SERVICE_UNAVAILABLE, Json(response))
        }
    }
}

impl AppState {
    fn estimate_cost(&self, question: &str) -> f64 {
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
    fn new(groq_key: Option<String>, openai_key: Option<String>) -> anyhow::Result<Self> {
        if groq_key.is_none() && openai_key.is_none() {
            return Err(anyhow!(
                "No AI provider configured. Provide GROQ_API_KEY or OPENAI_API_KEY."
            ));
        }

        let http = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(20))
            .build()?;

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

        Ok(Self { http, groq, openai })
    }

    fn has_groq(&self) -> bool {
        self.groq.is_some()
    }

    fn has_openai(&self) -> bool {
        self.openai.is_some()
    }

    async fn ask(
        &self,
        knowledge: &KnowledgeBase,
        question: &str,
        estimate_cost: f64,
    ) -> Result<AiAnswer, AiClientError> {
        if let Some(groq) = &self.groq {
            match self
                .ask_backend(groq, knowledge, question, estimate_cost)
                .await
            {
                Ok(answer) => {
                    return Ok(AiAnswer {
                        text: answer,
                        model: groq.model,
                    });
                }
                Err(err) => {
                    warn!(
                        target: "ai",
                        model = groq.model,
                        error = %err,
                        "Groq backend error; attempting OpenAI fallback"
                    );
                    if let Some(openai) = &self.openai {
                        match self
                            .ask_backend(openai, knowledge, question, estimate_cost)
                            .await
                        {
                            Ok(answer) => {
                                return Ok(AiAnswer {
                                    text: answer,
                                    model: openai.model,
                                });
                            }
                            Err(openai_err) => {
                                error!(
                                    target: "ai",
                                    model = openai.model,
                                    error = %openai_err,
                                    "OpenAI fallback failed after Groq error"
                                );
                                return Err(AiClientError::GroqAndFallbackFailed {
                                    groq: err,
                                    openai: openai_err,
                                });
                            }
                        }
                    } else {
                        return Err(AiClientError::GroqOnlyFailed { error: err });
                    }
                }
            }
        }

        if let Some(openai) = &self.openai {
            match self
                .ask_backend(openai, knowledge, question, estimate_cost)
                .await
            {
                Ok(answer) => Ok(AiAnswer {
                    text: answer,
                    model: openai.model,
                }),
                Err(err) => Err(AiClientError::OpenAiFailed { error: err }),
            }
        } else {
            Err(AiClientError::NoBackendConfigured)
        }
    }

    async fn ask_backend(
        &self,
        backend: &ApiBackend,
        knowledge: &KnowledgeBase,
        question: &str,
        estimate_cost: f64,
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
            cost_eur = estimate_cost,
            chars = question.len(),
            model = backend.model,
            msg = "AI response generated by backend"
        );
        Ok(answer)
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
    #[error("Groq backend failed without fallback: {error}")]
    GroqOnlyFailed {
        #[source]
        error: BackendError,
    },
    #[error("Groq backend failed ({groq}) and OpenAI fallback failed ({openai})")]
    GroqAndFallbackFailed {
        #[source]
        groq: BackendError,
        openai: BackendError,
    },
    #[error("OpenAI backend failed: {error}")]
    OpenAiFailed {
        #[source]
        error: BackendError,
    },
    #[error("Neither Groq nor OpenAI backend is configured")]
    NoBackendConfigured,
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
}
