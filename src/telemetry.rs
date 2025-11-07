use crate::utils;
use serde::Serialize;
use wasm_bindgen::JsCast;
use wasm_bindgen::JsValue;
use wasm_bindgen_futures::{spawn_local, JsFuture};
use web_sys::{Request, RequestInit, RequestMode, Response};

const COMMAND_LOG_ENDPOINT: &str = "/api/log/command";

#[derive(Clone, Copy)]
pub enum CommandLogMode {
    Classic,
    Ai,
}

impl CommandLogMode {
    fn as_str(self) -> &'static str {
        match self {
            CommandLogMode::Classic => "classic",
            CommandLogMode::Ai => "ai",
        }
    }
}

#[derive(Serialize)]
struct CommandLogPayload {
    command: String,
    mode: String,
}

pub fn log_command_submission(command: &str, mode: CommandLogMode) {
    let trimmed = command.trim();
    if trimmed.is_empty() {
        return;
    }
    let Some(window) = utils::window() else {
        return;
    };
    let payload = CommandLogPayload {
        command: trimmed.to_string(),
        mode: mode.as_str().to_string(),
    };
    let body = match serde_json::to_string(&payload) {
        Ok(value) => value,
        Err(err) => {
            utils::log(&format!("Failed to encode command log payload: {err}"));
            return;
        }
    };
    spawn_local(async move {
        if let Err(err) = dispatch_command_log(window, body).await {
            utils::log(&format!("Command log dispatch failed: {err}"));
        }
    });
}

async fn dispatch_command_log(window: web_sys::Window, body: String) -> Result<(), String> {
    let opts = RequestInit::new();
    opts.set_method("POST");
    opts.set_mode(RequestMode::SameOrigin);
    opts.set_body(&JsValue::from_str(&body));

    let request = Request::new_with_str_and_init(COMMAND_LOG_ENDPOINT, &opts)
        .map_err(|err| format_js_error("Failed to create command log request", err))?;
    request
        .headers()
        .set("Content-Type", "application/json")
        .map_err(|err| format_js_error("Failed to set command log headers", err))?;

    let response_value = JsFuture::from(window.fetch_with_request(&request))
        .await
        .map_err(|err| format_js_error("Failed to send command log request", err))?;
    let response: Response = response_value
        .dyn_into()
        .map_err(|_| "Failed to parse command log response".to_string())?;
    if !response.ok() {
        let status = response.status();
        return Err(format!("Command log endpoint returned status {status}"));
    }
    Ok(())
}

fn format_js_error(context: &str, err: JsValue) -> String {
    if let Some(value) = err.as_string() {
        format!("{context}: {value}")
    } else {
        format!("{context}: {:?}", err)
    }
}
