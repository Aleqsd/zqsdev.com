use crate::utils;
use serde::{Deserialize, Serialize};
use serde_json::to_string;
use wasm_bindgen::{JsCast, JsValue};
use wasm_bindgen_futures::JsFuture;
use web_sys::{Request, RequestInit, RequestMode, Response};

const AI_API_ENDPOINT: &str = "/api/ai";

#[derive(Debug, Deserialize)]
pub struct AiServerResponse {
    pub answer: String,
    pub ai_enabled: bool,
    pub reason: Option<String>,
}

#[derive(Serialize)]
struct AiClientRequest<'a> {
    question: &'a str,
}

pub async fn ask_ai(question: &str) -> Result<AiServerResponse, String> {
    if question.trim().is_empty() {
        return Err("Please type a question before hitting enter.".to_string());
    }

    let window = utils::window().ok_or_else(|| "Window unavailable.".to_string())?;

    let body = build_request_body(question)?;
    let opts = RequestInit::new();
    opts.set_method("POST");
    opts.set_mode(RequestMode::SameOrigin);
    let body_js = JsValue::from_str(&body);
    opts.set_body(&body_js);

    let request = Request::new_with_str_and_init(AI_API_ENDPOINT, &opts)
        .map_err(|err| format_js_error("Failed to create AI request", err))?;
    request
        .headers()
        .set("Content-Type", "application/json")
        .map_err(|err| format_js_error("Failed to set request header", err))?;

    let response_value = JsFuture::from(window.fetch_with_request(&request))
        .await
        .map_err(|err| format_js_error("Failed to contact AI endpoint", err))?;
    let response: Response = response_value
        .dyn_into()
        .map_err(|_| "Failed to interpret AI endpoint response.".to_string())?;

    let status = response.status();
    let json_future = response
        .json()
        .map_err(|err| format_js_error("Failed to read AI response body", err))?;
    match JsFuture::from(json_future).await {
        Ok(value) => {
            let parsed: AiServerResponse =
                serde_wasm_bindgen::from_value(value).map_err(|err| {
                    format!("AI response deserialisation error (status {status}): {err}")
                })?;
            Ok(parsed)
        }
        Err(err) => {
            let text_future = response.text().map_err(|text_err| {
                format_js_error("Failed to read AI response fallback body", text_err)
            })?;
            let fallback = JsFuture::from(text_future)
                .await
                .ok()
                .and_then(|value| value.as_string())
                .unwrap_or_else(|| "No additional details.".to_string());
            Err(format!(
                "AI response decoding error (status {status}): {} — {fallback}",
                format_js_error("JSON parsing failed", err)
            ))
        }
    }
}

fn build_request_body(question: &str) -> Result<String, String> {
    to_string(&AiClientRequest { question })
        .map_err(|err| format!("Failed to encode AI request: {err}"))
}

fn format_js_error(context: &str, err: JsValue) -> String {
    if let Some(value) = err.as_string() {
        format!("{context}: {value}")
    } else {
        format!("{context}: {:?}", err)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_request_body_includes_question() {
        let payload = build_request_body("Who is Alex?").expect("payload");
        assert!(
            payload.contains("Who is Alex?"),
            "Request payload should embed the original question: {payload}"
        );
        assert!(
            payload.starts_with('{') && payload.ends_with('}'),
            "Payload should be JSON: {payload}"
        );
    }
}
