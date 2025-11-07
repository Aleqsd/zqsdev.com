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

pub fn tag_resume_source(url: &str) -> String {
    const CV_HOST: &str = "cv.zqsdev.com";
    const PARAM_KEY: &str = "from";
    const PARAM_VALUE: &str = "interactive";

    if url.is_empty() {
        return String::new();
    }

    let lower = url.to_ascii_lowercase();
    let host_target = lower
        .split_once("://")
        .map(|(_, rest)| rest)
        .unwrap_or_else(|| lower.as_str())
        .split(&['/', '?', '#'][..])
        .next()
        .unwrap_or("");
    if host_target != CV_HOST {
        return url.to_string();
    }

    let (without_fragment, fragment) = match url.split_once('#') {
        Some((base, frag)) => (base, Some(frag)),
        None => (url, None),
    };

    let (prefix, query) = match without_fragment.split_once('?') {
        Some((p, q)) => (p, Some(q)),
        None => (without_fragment, None),
    };

    let mut result = String::with_capacity(url.len() + PARAM_KEY.len() + PARAM_VALUE.len() + 4);
    result.push_str(prefix);
    result.push('?');

    let mut wrote_any = false;
    if let Some(query) = query {
        for pair in query.split('&') {
            if pair.is_empty() {
                continue;
            }
            let (name, value) = pair
                .split_once('=')
                .map(|(n, v)| (n, Some(v)))
                .unwrap_or((pair, None));
            if name.eq_ignore_ascii_case(PARAM_KEY) {
                if value
                    .map(|v| v.eq_ignore_ascii_case(PARAM_VALUE))
                    .unwrap_or(false)
                {
                    return url.to_string();
                }
                continue;
            }
            if wrote_any {
                result.push('&');
            }
            result.push_str(pair);
            wrote_any = true;
        }
    }

    if wrote_any {
        result.push('&');
    }

    result.push_str(PARAM_KEY);
    result.push('=');
    result.push_str(PARAM_VALUE);

    if let Some(fragment) = fragment {
        result.push('#');
        result.push_str(fragment);
    }

    result
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

    #[test]
    fn tag_resume_source_appends_param_without_query() {
        let url = "https://cv.zqsdev.com/";
        let tagged = tag_resume_source(url);
        assert_eq!(tagged, "https://cv.zqsdev.com/?from=interactive");
    }

    #[test]
    fn tag_resume_source_appends_param_with_existing_query_and_fragment() {
        let url = "https://cv.zqsdev.com/view?lang=en#top";
        let tagged = tag_resume_source(url);
        assert_eq!(
            tagged,
            "https://cv.zqsdev.com/view?lang=en&from=interactive#top"
        );
    }

    #[test]
    fn tag_resume_source_ignores_non_cv_hosts() {
        let url = "https://example.com/resume";
        assert_eq!(tag_resume_source(url), url);
    }

    #[test]
    fn tag_resume_source_does_not_duplicate_existing_param() {
        let url = "https://cv.zqsdev.com/?from=interactive";
        assert_eq!(tag_resume_source(url), url);
        let url_mixed = "https://cv.zqsdev.com/?From=Interactive";
        assert_eq!(tag_resume_source(url_mixed), url_mixed);
    }

    #[test]
    fn tag_resume_source_replaces_different_from_value() {
        let url = "https://cv.zqsdev.com/?from=www";
        assert_eq!(
            tag_resume_source(url),
            "https://cv.zqsdev.com/?from=interactive"
        );

        let url_complex = "https://cv.zqsdev.com/?lang=en&from=www#top";
        assert_eq!(
            tag_resume_source(url_complex),
            "https://cv.zqsdev.com/?lang=en&from=interactive#top"
        );
    }
}
