//! Per-isolate configuration: vars and secrets are read once, on the isolate's first request
//! (they cannot change while the isolate lives: a deploy or a secret change starts new ones).

use std::cell::OnceCell;
use std::rc::Rc;

use serde_json::json;
use worker::js_sys::Reflect;
use worker::wasm_bindgen::JsValue;
use worker::Env;

use super::log;
use crate::core::config::Config;
use crate::core::embedded::{COMMIT, VERSION};
use crate::core::logline::Level;

thread_local! {
    static CONFIG: OnceCell<Rc<Config>> = const { OnceCell::new() };
}

/// A string var or secret; `None` when it is absent or not a string.
pub fn string_binding(env: &Env, name: &str) -> Option<String> {
    Reflect::get(env.as_ref(), &JsValue::from_str(name))
        .ok()?
        .as_string()
}

/// The isolate's `Config`. The first call builds it and logs `🚀 API isolate started`, plus
/// `⚙️ Invalid AI configuration; AI disabled` when a price or the base URL is invalid.
pub fn config(env: &Env) -> Rc<Config> {
    CONFIG.with(|cell| {
        Rc::clone(cell.get_or_init(|| {
            let cfg = Config::from_lookup(&|name| string_binding(env, name));
            log::emit(
                cfg.log_level,
                Level::Info,
                "🚀 API isolate started",
                &json!({
                    "version": VERSION,
                    "commit": COMMIT,
                    "model": cfg.model,
                    "ai_configured": cfg.ai_configured,
                }),
            );
            if let Some(error) = cfg.ai_error {
                log::emit(
                    cfg.log_level,
                    Level::Warn,
                    "⚙️ Invalid AI configuration; AI disabled",
                    &json!({ "error": error }),
                );
            }
            Rc::new(cfg)
        }))
    })
}
