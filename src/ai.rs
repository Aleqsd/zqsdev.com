use crate::utils;
use serde::{Deserialize, Serialize};
use wasm_bindgen::JsValue;
use web_sys::{RequestInit, RequestMode};
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Message {
    pub role: String,
    pub content: String,
}
#[derive(Debug, Deserialize)]
pub struct AiServerResponse {
    pub answer: String,
    pub ai_enabled: bool,
    pub reason: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub sources: Vec<String>,
}
#[derive(Serialize)]
struct AiClientRequest<'a> {
    question: &'a str,
    history: &'a [Message],
}
pub async fn ask_ai(question: &str, history: &[Message]) -> Result<AiServerResponse, String> {
    let opts = RequestInit::new();
    opts.set_method("POST");
    opts.set_mode(RequestMode::SameOrigin);
    let body = serde_json::to_string(&AiClientRequest { question, history })
        .map_err(|_| "Invalid question")?;
    opts.set_body(&JsValue::from_str(&body));
    let headers = web_sys::Headers::new().map_err(|_| "Invalid request")?;
    headers
        .set("Content-Type", "application/json")
        .map_err(|_| "Invalid request")?;
    opts.set_headers(&headers);
    let (status, text) = utils::fetch_text("/api/ai", opts, 20000)
        .await
        .map_err(|_| "Connection interrupted or timed out")?;
    let payload: AiServerResponse =
        serde_json::from_str(&text).map_err(|_| "AI endpoint unavailable")?;
    if !(200..300).contains(&status) && payload.ai_enabled {
        return Err("AI endpoint unavailable".into());
    }
    Ok(payload)
}
