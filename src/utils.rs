use serde::de::DeserializeOwned;
use serde_wasm_bindgen::from_value;
use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;
use wasm_bindgen_futures::JsFuture;
use web_sys::{console, Document, Request, RequestInit, RequestMode, Response};

pub fn document() -> Result<Document, JsValue> {
    window()
        .and_then(|win| win.document())
        .ok_or_else(|| JsValue::from_str("Document unavailable"))
}

pub fn log(message: &str) {
    console::log_1(&JsValue::from_str(message));
}

pub async fn fetch_json<T>(path: &str) -> Result<T, JsValue>
where
    T: DeserializeOwned,
{
    let window = window().ok_or_else(|| JsValue::from_str("Window unavailable"))?;

    let opts = RequestInit::new();
    opts.set_method("GET");
    opts.set_mode(RequestMode::SameOrigin);

    let request = Request::new_with_str_and_init(path, &opts)?;
    let response_value = JsFuture::from(window.fetch_with_request(&request)).await?;
    let response: Response = response_value.dyn_into()?;

    if !response.ok() {
        let status = response.status();
        return Err(JsValue::from_str(&format!(
            "Failed to fetch {path} (status {status})"
        )));
    }

    let json = JsFuture::from(response.json()?).await?;
    from_value(json).map_err(|e| JsValue::from_str(&format!("JSON error for {path}: {e}")))
}

pub fn open_link(url: &str) {
    if let Some(win) = window() {
        let _ = win.open_with_url_and_target(url, "_blank");
    }
}

pub fn escape_html(input: &str) -> String {
    let mut escaped = String::with_capacity(input.len());
    for ch in input.chars() {
        match ch {
            '&' => escaped.push_str("&amp;"),
            '<' => escaped.push_str("&lt;"),
            '>' => escaped.push_str("&gt;"),
            '"' => escaped.push_str("&quot;"),
            '\'' => escaped.push_str("&#39;"),
            _ => escaped.push(ch),
        }
    }
    escaped
}

pub fn window() -> Option<web_sys::Window> {
    web_sys::window()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escape_html_encodes_special_characters() {
        let original = "<tag attr=\"value & more\">";
        let escaped = escape_html(original);
        assert_eq!(escaped, "&lt;tag attr=&quot;value &amp; more&quot;&gt;");
        assert!(
            !escaped.contains('<') && !escaped.contains('>'),
            "Escaped string should not contain raw angle brackets: {escaped}"
        );
    }
}
