//! HTTP surface of the API Worker: request and response values exchanged with the glue, the route
//! table, and axum 0.7.9's JSON extractor rules (content type, body limit, error texts).

use serde::de::DeserializeOwned;

use super::ledger::protocol::LedgerOp;

/// `DefaultBodyLimit::max(64 * 1024)` of the original router; also the admin body cap.
pub const BODY_CAP: usize = 65_536;

pub const APPLICATION_JSON: &str = "application/json";
pub const TEXT_PLAIN: &str = "text/plain; charset=utf-8";
pub const NDJSON: &str = "application/x-ndjson";

/// `Cache-Control` of `/api/data` (original `handle_data`).
pub const DATA_CACHE_CONTROL: &str = "public, max-age=60, must-revalidate";
pub const NO_STORE: &str = "no-store";

/// `Allow` values of axum 0.7.9's `MethodRouter`.
pub const ALLOW_GET_HEAD: &str = "GET,HEAD";
pub const ALLOW_POST: &str = "POST";

/// axum rejection texts (`MissingJsonContentType`, `LengthLimitError`, `UnknownBodyError`,
/// `JsonSyntaxError`, `JsonDataError`); the last three are followed by `: {error}`.
pub const REJECT_CONTENT_TYPE: &str = "Expected request with `Content-Type: application/json`";
pub const REJECT_TOO_LARGE: &str = "Failed to buffer the request body: length limit exceeded";
pub const REJECT_BUFFER: &str = "Failed to buffer the request body";
pub const REJECT_SYNTAX: &str = "Failed to parse the request body as JSON";
pub const REJECT_DATA: &str = "Failed to deserialize the JSON body into the target type";

/// A request as the glue hands it to `core::api::handle`.
pub struct In<'a> {
    pub method: &'a str,
    /// Percent-encoded path, without the query string.
    pub path: &'a str,
    pub content_type: Option<&'a str>,
    pub content_length: Option<&'a str>,
    pub cf_connecting_ip: Option<&'a str>,
    pub body: Body,
}

/// Request body as read by the glue (only for routes that need one, see `wants_body`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Body {
    Empty,
    Bytes(Vec<u8>),
    /// More than `BODY_CAP` bytes (declared or streamed).
    TooLarge,
    /// The stream failed; the text follows axum's `Failed to buffer the request body: `.
    Unreadable(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OutBody {
    Empty,
    Static(&'static str),
    Bytes(Vec<u8>),
}

impl OutBody {
    pub fn as_bytes(&self) -> &[u8] {
        match self {
            OutBody::Empty => &[],
            OutBody::Static(text) => text.as_bytes(),
            OutBody::Bytes(bytes) => bytes,
        }
    }
}

/// A response for the glue to send.
#[derive(Clone, Debug, PartialEq)]
pub struct Out {
    pub status: u16,
    pub content_type: Option<&'static str>,
    pub headers: Vec<(&'static str, String)>,
    pub body: OutBody,
    /// A settlement to repeat once in `wait_until` (after 1 s) because the inline one failed.
    pub retry: Option<LedgerOp>,
}

impl Out {
    pub fn empty(status: u16) -> Out {
        Out {
            status,
            content_type: None,
            headers: Vec::new(),
            body: OutBody::Empty,
            retry: None,
        }
    }

    pub fn json(status: u16, body: String) -> Out {
        Out {
            status,
            content_type: Some(APPLICATION_JSON),
            headers: Vec::new(),
            body: OutBody::Bytes(body.into_bytes()),
            retry: None,
        }
    }

    pub fn json_static(status: u16, body: &'static str) -> Out {
        Out {
            status,
            content_type: Some(APPLICATION_JSON),
            headers: Vec::new(),
            body: OutBody::Static(body),
            retry: None,
        }
    }

    /// An axum rejection: `text/plain; charset=utf-8` body.
    pub fn text(status: u16, body: String) -> Out {
        Out {
            status,
            content_type: Some(TEXT_PLAIN),
            headers: Vec::new(),
            body: OutBody::Bytes(body.into_bytes()),
            retry: None,
        }
    }

    /// Unknown path (and every rejected admin request): 404, empty body, `no-store`.
    pub fn not_found() -> Out {
        Out::empty(404).header("Cache-Control", NO_STORE.to_string())
    }

    pub fn method_not_allowed(allow: &'static str) -> Out {
        Out::empty(405).header("Allow", allow.to_string())
    }

    pub fn header(mut self, name: &'static str, value: String) -> Out {
        self.headers.push((name, value));
        self
    }

    /// HEAD: same status and headers, no body.
    pub fn without_body(mut self) -> Out {
        self.body = OutBody::Empty;
        self
    }

    pub fn header_value(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }
}

/// Admin operations, all `POST /api/admin/<op>` with the token in the JSON body.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AdminRoute {
    Stats,
    Export,
    Import,
    ImportClose,
    Purge,
    Rebuild,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Route {
    Data,
    Version,
    Health,
    Ai,
    CommandLog,
    Admin(AdminRoute),
    /// Any other request under `/api/admin/`: 404 like unknown paths, plus a throttled warning.
    AdminRejected,
    /// Known path, wrong method: 405 with this `Allow` value.
    NotAllowed(&'static str),
    NotFound,
}

/// Exact path match (no trailing-slash normalisation; the query string is not part of `path`).
pub fn route(method: &str, path: &str) -> Route {
    let get = method == "GET" || method == "HEAD";
    let post = method == "POST";
    match path {
        "/api/data" if get => Route::Data,
        "/api/version" if get => Route::Version,
        "/api/health" if get => Route::Health,
        "/api/data" | "/api/version" | "/api/health" => Route::NotAllowed(ALLOW_GET_HEAD),
        "/api/ai" if post => Route::Ai,
        "/api/log/command" if post => Route::CommandLog,
        "/api/ai" | "/api/log/command" => Route::NotAllowed(ALLOW_POST),
        _ => match path.strip_prefix("/api/admin/") {
            Some(op) => match (post, op) {
                (true, "stats") => Route::Admin(AdminRoute::Stats),
                (true, "export") => Route::Admin(AdminRoute::Export),
                (true, "import-ledger") => Route::Admin(AdminRoute::Import),
                (true, "import-ledger/close") => Route::Admin(AdminRoute::ImportClose),
                (true, "purge") => Route::Admin(AdminRoute::Purge),
                (true, "rebuild") => Route::Admin(AdminRoute::Rebuild),
                _ => Route::AdminRejected,
            },
            None => Route::NotFound,
        },
    }
}

/// Whether the glue should read the request body: only for routes that parse one, and (axum's
/// order) only once the content type is acceptable.
pub fn wants_body(method: &str, path: &str, content_type: Option<&str>) -> bool {
    matches!(
        route(method, path),
        Route::Ai | Route::CommandLog | Route::Admin(_)
    ) && json_content_type(content_type)
}

/// axum 0.7.9 `json_content_type` (`json.rs:117-140`) on mime 0.3.17: `application/json` or
/// `application/<x>+json`, case-insensitive, malformed parameters rejected. A value that is not
/// visible ASCII fails `HeaderValue::to_str` there and is rejected here as well.
pub fn json_content_type(content_type: Option<&str>) -> bool {
    let Some(value) = content_type else {
        return false;
    };
    if !value.bytes().all(|b| b == b'\t' || (32..127).contains(&b)) {
        return false;
    }
    let Ok(mime) = value.parse::<mime::Mime>() else {
        return false;
    };
    mime.type_() == "application"
        && (mime.subtype() == "json" || mime.suffix().is_some_and(|s| s == "json"))
}

/// The body after the `Content-Length` check: a declared length above the cap is rejected
/// before reading, as `Limited` would after reading.
pub fn effective_body(content_length: Option<&str>, body: Body) -> Body {
    let declared = content_length.and_then(|v| v.trim().parse::<u64>().ok());
    if declared.is_some_and(|n| n > BODY_CAP as u64) {
        return Body::TooLarge;
    }
    match body {
        Body::Bytes(bytes) if bytes.len() > BODY_CAP => Body::TooLarge,
        other => other,
    }
}

/// JSON body errors, classified like axum 0.7.9 `Json::from_bytes`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum JsonError {
    /// serde category Data: 422.
    Data(String),
    /// Syntax, Eof or Io: 400.
    Syntax(String),
}

/// axum 0.7.9 `Json::from_bytes` (`json.rs:157-183`): serde_path_to_error over
/// `Deserializer::from_slice`, without `end()`, so bytes after the JSON value are accepted.
pub fn from_bytes<T: DeserializeOwned>(bytes: &[u8]) -> Result<T, JsonError> {
    let deserializer = &mut serde_json::Deserializer::from_slice(bytes);
    serde_path_to_error::deserialize(deserializer).map_err(|err| match err.inner().classify() {
        serde_json::error::Category::Data => JsonError::Data(err.to_string()),
        _ => JsonError::Syntax(err.to_string()),
    })
}

/// Why axum's `Json<T>` extractor rejected a request, in its order of checks.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Rejection {
    ContentType,
    TooLarge,
    Unreadable(String),
    Json(JsonError),
}

impl Rejection {
    /// axum's rejection response: status and `text/plain; charset=utf-8` body.
    pub fn into_out(self) -> Out {
        match self {
            Rejection::ContentType => Out::text(415, REJECT_CONTENT_TYPE.to_string()),
            Rejection::TooLarge => Out::text(413, REJECT_TOO_LARGE.to_string()),
            Rejection::Unreadable(error) => Out::text(400, format!("{REJECT_BUFFER}: {error}")),
            Rejection::Json(JsonError::Syntax(error)) => {
                Out::text(400, format!("{REJECT_SYNTAX}: {error}"))
            }
            Rejection::Json(JsonError::Data(error)) => {
                Out::text(422, format!("{REJECT_DATA}: {error}"))
            }
        }
    }
}

/// axum's `Json<T>` extractor: content type, then body (limit), then JSON.
pub fn extract_json<T: DeserializeOwned>(
    content_type: Option<&str>,
    content_length: Option<&str>,
    body: Body,
) -> Result<T, Rejection> {
    if !json_content_type(content_type) {
        return Err(Rejection::ContentType);
    }
    let bytes = match effective_body(content_length, body) {
        Body::Empty => Vec::new(),
        Body::Bytes(bytes) => bytes,
        Body::TooLarge => return Err(Rejection::TooLarge),
        Body::Unreadable(error) => return Err(Rejection::Unreadable(error)),
    };
    from_bytes(&bytes).map_err(Rejection::Json)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::request::{AiRequest, CommandLogRequest};

    #[test]
    fn content_types_follow_axum() {
        for ok in [
            "application/json",
            "Application/JSON",
            "application/json; charset=utf-8",
            "application/json;",
            "application/ld+json",
            "application/vnd.api+json; charset=UTF-8",
        ] {
            assert!(json_content_type(Some(ok)), "{ok}");
        }
        for bad in [
            "application/json; charset",
            "application/json; charset=\"utf-8",
            "application/json; =x",
            "text/json",
            "application/x-json",
            "application/jsonx",
            "json",
            "",
            "application/json; charset=é",
        ] {
            assert!(!json_content_type(Some(bad)), "{bad}");
        }
        assert!(!json_content_type(None));
    }

    #[test]
    fn trailing_bytes_are_accepted() {
        let c: CommandLogRequest = from_bytes(br#"{"command":"projects"} garbage"#).unwrap();
        assert_eq!(c.command, "projects");
        let c: CommandLogRequest = from_bytes(br#"{"command":"projects"}{"x":1}"#).unwrap();
        assert_eq!(c.command, "projects");
        let a: AiRequest = from_bytes(br#"{"question":"hi"}xyz"#).unwrap();
        assert_eq!(a.question, "hi");
        let a: AiRequest = from_bytes(br#"{"question":"hi","history":[]} 1"#).unwrap();
        assert!(a.history.is_empty());
        // serde_json::from_slice, which calls end(), would refuse the same input.
        assert!(
            serde_json::from_slice::<CommandLogRequest>(br#"{"command":"projects"} garbage"#)
                .is_err()
        );
    }

    /// The texts axum 0.7.9 returns, including serde_path_to_error's `path: ` prefix.
    #[test]
    fn rejection_texts_match_axum() {
        let out = |ct: Option<&str>, body: &[u8]| {
            extract_json::<CommandLogRequest>(ct, None, Body::Bytes(body.to_vec()))
                .err()
                .map(Rejection::into_out)
        };
        let text = |o: Option<Out>| {
            let o = o.unwrap();
            assert_eq!(o.content_type, Some(TEXT_PLAIN));
            (
                o.status,
                String::from_utf8(o.body.as_bytes().to_vec()).unwrap(),
            )
        };
        assert_eq!(
            text(out(None, b"{}")),
            (
                415,
                "Expected request with `Content-Type: application/json`".into()
            )
        );
        assert_eq!(
            text(out(Some("application/json"), br#"{"mode":"ai"}"#)),
            (
                422,
                "Failed to deserialize the JSON body into the target type: missing field `command` at line 1 column 13".into()
            )
        );
        assert_eq!(
            text(out(Some("application/json"), br#"{"command":5}"#)),
            (
                422,
                "Failed to deserialize the JSON body into the target type: command: invalid type: integer `5`, expected a string at line 1 column 12".into()
            )
        );
        assert_eq!(
            text(out(Some("application/json"), b"[]")),
            (
                422,
                "Failed to deserialize the JSON body into the target type: invalid length 0, expected struct CommandLogRequest with 2 elements at line 1 column 2".into()
            )
        );
        assert_eq!(
            text(out(Some("application/json"), br#"{"command":"x","mode":7}"#)),
            (
                422,
                "Failed to deserialize the JSON body into the target type: mode: invalid type: integer `7`, expected a string at line 1 column 23".into()
            )
        );
        assert_eq!(
            text(out(Some("application/json"), br#"{"command":"#)),
            (
                400,
                "Failed to parse the request body as JSON: command: EOF while parsing a value at line 1 column 11".into()
            )
        );
        assert_eq!(
            text(out(Some("application/json"), b"")),
            (
                400,
                "Failed to parse the request body as JSON: EOF while parsing a value at line 1 column 0".into()
            )
        );
        assert_eq!(
            text(out(Some("application/json"), b"{x}")),
            (
                400,
                "Failed to parse the request body as JSON: key must be a string at line 1 column 2"
                    .into()
            )
        );
        // Raw bytes reach the parser, invalid UTF-8 included.
        assert_eq!(
            text(out(Some("application/json"), b"{\"command\":\"\xff\"}")),
            (
                400,
                "Failed to parse the request body as JSON: command: invalid unicode code point at line 1 column 14"
                    .into()
            )
        );
        assert_eq!(
            text(out(
                Some("application/json"),
                br#"{"command":"x","command":"y"}"#
            )),
            (
                422,
                "Failed to deserialize the JSON body into the target type: duplicate field `command` at line 1 column 24"
                    .into()
            )
        );
    }

    #[test]
    fn body_cap() {
        let ct = Some("application/json");
        let at_cap = format!(r#"{{"command":"{}"}}"#, "x".repeat(BODY_CAP - 14));
        assert_eq!(at_cap.len(), BODY_CAP);
        assert!(
            extract_json::<CommandLogRequest>(ct, None, Body::Bytes(at_cap.into_bytes())).is_ok()
        );
        let over = format!(r#"{{"command":"{}"}}"#, "x".repeat(BODY_CAP - 13));
        let rejected = extract_json::<CommandLogRequest>(ct, None, Body::Bytes(over.into_bytes()))
            .unwrap_err();
        assert_eq!(rejected, Rejection::TooLarge);
        let o = rejected.into_out();
        assert_eq!(o.status, 413);
        assert_eq!(
            o.body.as_bytes(),
            b"Failed to buffer the request body: length limit exceeded"
        );
        assert_eq!(
            extract_json::<CommandLogRequest>(ct, Some("65537"), Body::Empty).unwrap_err(),
            Rejection::TooLarge
        );
        assert_eq!(
            extract_json::<CommandLogRequest>(ct, None, Body::TooLarge).unwrap_err(),
            Rejection::TooLarge
        );
        let unreadable =
            extract_json::<CommandLogRequest>(ct, None, Body::Unreadable("reset".into()))
                .unwrap_err()
                .into_out();
        assert_eq!(
            (unreadable.status, unreadable.body.as_bytes()),
            (400, b"Failed to buffer the request body: reset".as_slice())
        );
    }

    #[test]
    fn route_table() {
        assert_eq!(route("GET", "/api/data"), Route::Data);
        assert_eq!(route("HEAD", "/api/data"), Route::Data);
        assert_eq!(route("GET", "/api/version"), Route::Version);
        assert_eq!(route("HEAD", "/api/health"), Route::Health);
        assert_eq!(route("POST", "/api/data"), Route::NotAllowed("GET,HEAD"));
        assert_eq!(
            route("OPTIONS", "/api/version"),
            Route::NotAllowed("GET,HEAD")
        );
        assert_eq!(route("POST", "/api/ai"), Route::Ai);
        assert_eq!(route("POST", "/api/log/command"), Route::CommandLog);
        for m in ["GET", "HEAD", "OPTIONS", "PUT", "DELETE", "post"] {
            assert_eq!(route(m, "/api/ai"), Route::NotAllowed("POST"), "{m}");
            assert_eq!(
                route(m, "/api/log/command"),
                Route::NotAllowed("POST"),
                "{m}"
            );
        }
        for p in [
            "/api/data/",
            "/api/unknown",
            "/api",
            "/",
            "/api/Data",
            "/api/%64ata",
        ] {
            assert_eq!(route("GET", p), Route::NotFound, "{p}");
            assert_eq!(route("POST", p), Route::NotFound, "{p}");
        }
        assert_eq!(
            route("POST", "/api/admin/stats"),
            Route::Admin(AdminRoute::Stats)
        );
        assert_eq!(
            route("POST", "/api/admin/export"),
            Route::Admin(AdminRoute::Export)
        );
        assert_eq!(
            route("POST", "/api/admin/import-ledger"),
            Route::Admin(AdminRoute::Import)
        );
        assert_eq!(
            route("POST", "/api/admin/import-ledger/close"),
            Route::Admin(AdminRoute::ImportClose)
        );
        assert_eq!(
            route("POST", "/api/admin/purge"),
            Route::Admin(AdminRoute::Purge)
        );
        assert_eq!(
            route("POST", "/api/admin/rebuild"),
            Route::Admin(AdminRoute::Rebuild)
        );
        assert_eq!(route("GET", "/api/admin/stats"), Route::AdminRejected);
        assert_eq!(route("POST", "/api/admin/unknown"), Route::AdminRejected);
        assert_eq!(route("POST", "/api/admin/stats/"), Route::AdminRejected);
        assert_eq!(route("POST", "/api/admin"), Route::NotFound);
        assert!(wants_body("POST", "/api/ai", Some("application/json")));
        assert!(!wants_body("POST", "/api/ai", Some("text/plain")));
        assert!(!wants_body("GET", "/api/data", Some("application/json")));
        assert!(wants_body(
            "POST",
            "/api/admin/stats",
            Some("application/json")
        ));
        assert!(!wants_body(
            "GET",
            "/api/admin/stats",
            Some("application/json")
        ));
    }

    #[test]
    fn responses() {
        let nf = Out::not_found();
        assert_eq!((nf.status, nf.body.clone()), (404, OutBody::Empty));
        assert_eq!(nf.header_value("cache-control"), Some("no-store"));
        let na = Out::method_not_allowed(ALLOW_POST);
        assert_eq!((na.status, na.header_value("Allow")), (405, Some("POST")));
        assert_eq!(na.content_type, None);
        assert_eq!(na.body, OutBody::Empty);
    }
}
