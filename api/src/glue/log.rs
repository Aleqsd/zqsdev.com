//! Console sink: one JSON object per line (`core::logline::line`), filtered by `LOG_LEVEL`.
//! Workers Logs indexes the fields; `console.warn` / `console.error` set the record level.

use serde_json::Value;
use worker::wasm_bindgen::{JsCast, JsValue};
use worker::{console_debug, console_error, console_log, console_warn, js_sys};

use crate::core::logline::{line, Level};

/// Writes one line when `level` passes the configured filter.
pub fn emit(configured: Level, level: Level, msg: &str, fields: &Value) {
    if !level.enabled(configured) {
        return;
    }
    let text = line(level, msg, fields);
    match level {
        Level::Error => console_error!("{text}"),
        Level::Warn => console_warn!("{text}"),
        Level::Info => console_log!("{text}"),
        Level::Debug => console_debug!("{text}"),
    }
}

/// The message of a JavaScript exception (or of any thrown value), for logs and errors.
pub fn js_error(value: &JsValue) -> String {
    if let Some(text) = value.as_string() {
        return text;
    }
    if let Some(error) = value.dyn_ref::<js_sys::Error>() {
        return String::from(error.message());
    }
    "unknown JavaScript error".to_string()
}
