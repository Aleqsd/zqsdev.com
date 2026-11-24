//! Build-time inputs of the API Worker: version, commit, the /api/data body, the AI system message.
//!
//! Everything the original server loaded at startup is computed here, so a data error fails the
//! build (and `make deploy-api`) instead of a request.
use serde_json::{Map, Value};
use std::path::{Path, PathBuf};
use std::process::Command;

// SYSTEM_TEMPLATE: the same raw string literal as the original server/src/ai.rs:117-122.
include!("src/core/prompt.rs");

/// Section files, in the order of the original `TerminalDataPayload::load`.
const DATA_FILES: [&str; 7] = [
    "profile.json",
    "skills.json",
    "experience.json",
    "education.json",
    "projects.json",
    "testimonials.json",
    "faq.json",
];

/// Reviewed full-context limit of the knowledge JSON (original `AiClient::new`).
const KNOWLEDGE_LIMIT: usize = 64_000;

fn main() {
    let manifest = PathBuf::from(env_var("CARGO_MANIFEST_DIR"));
    let root = manifest.join("..");
    let out = PathBuf::from(env_var("OUT_DIR"));

    println!("cargo:rerun-if-changed=src/core/prompt.rs");
    println!("cargo:rerun-if-env-changed=ZQS_BUILD_COMMIT");

    // 1. Version from ../VERSION.
    let version_path = root.join("VERSION");
    println!("cargo:rerun-if-changed={}", version_path.display());
    let version = read(&version_path).trim().to_string();
    if !is_semver(&version) {
        panic!("VERSION must look like 1.2.3, found {version:?}");
    }
    println!("cargo:rustc-env=ZQS_VERSION={version}");

    // 2. Commit: ZQS_BUILD_COMMIT (all hex) or the full `git rev-parse HEAD`, else "unknown".
    println!("cargo:rustc-env=ZQS_COMMIT={}", commit());

    // 3. The 7 data files, parsed as arbitrary JSON (maps sort their keys: no preserve_order).
    let data_dir = root.join("static").join("data");
    let mut sections: Vec<(&str, Value)> = Vec::with_capacity(DATA_FILES.len());
    for name in DATA_FILES {
        let path = data_dir.join(name);
        println!("cargo:rerun-if-changed={}", path.display());
        let text = std::fs::read_to_string(&path)
            .unwrap_or_else(|_| panic!("Failed to read data file {path:?}"));
        let value: Value = serde_json::from_str(&text)
            .unwrap_or_else(|_| panic!("Failed to parse JSON from {path:?}"));
        sections.push((name, value));
    }
    let section = |file: &str| -> Value {
        sections
            .iter()
            .find(|(name, _)| *name == file)
            .map(|(_, value)| value.clone())
            .unwrap_or(Value::Null)
    };

    // 4. /api/data body: the original TerminalDataPayload fields plus the `faq` alias of `faqs`.
    let mut payload = Map::new();
    payload.insert("profile".into(), section("profile.json"));
    payload.insert("skills".into(), section("skills.json"));
    payload.insert("experiences".into(), section("experience.json"));
    payload.insert("education".into(), section("education.json"));
    payload.insert("projects".into(), section("projects.json"));
    payload.insert("testimonials".into(), section("testimonials.json"));
    payload.insert("faqs".into(), section("faq.json"));
    payload.insert("faq".into(), section("faq.json"));
    let data_json = to_json(&Value::Object(payload));
    write(&out.join("data.json"), &data_json);

    // 5. Knowledge: the original `knowledge_json()` keys, compact and sorted, with the 64 KB guard.
    let mut knowledge = Map::new();
    knowledge.insert("profile".into(), section("profile.json"));
    knowledge.insert("skills".into(), section("skills.json"));
    knowledge.insert("experience".into(), section("experience.json"));
    knowledge.insert("education".into(), section("education.json"));
    knowledge.insert("projects".into(), section("projects.json"));
    knowledge.insert("testimonials".into(), section("testimonials.json"));
    knowledge.insert("faq".into(), section("faq.json"));
    let knowledge = to_json(&Value::Object(knowledge));
    if knowledge.len() > KNOWLEDGE_LIMIT {
        panic!("Knowledge exceeds reviewed full-context limit (64 KB)");
    }

    // 6. KNOWLEDGE_UPDATED comes from profile.updated_at.
    let profile = section("profile.json");
    let updated = profile
        .get("updated_at")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    if !is_iso_date(&updated) {
        panic!("profile.json updated_at must look like 2026-10-17, found {updated:?}");
    }
    println!("cargo:rustc-env=ZQS_KNOWLEDGE_UPDATED={updated}");

    // 7. System message: the template with its single {knowledge}, as an escaped JSON string literal.
    if SYSTEM_TEMPLATE.matches("{knowledge}").count() != 1 {
        panic!("SYSTEM_TEMPLATE must contain exactly one {{knowledge}}");
    }
    if SYSTEM_TEMPLATE.contains('\r') {
        panic!("SYSTEM_TEMPLATE must not contain a carriage return");
    }
    let system = SYSTEM_TEMPLATE.replace("{knowledge}", &knowledge);
    write(
        &out.join("system_message.json"),
        &to_json(&Value::String(system)),
    );
}

fn env_var(name: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| panic!("{name} is not set"))
}

fn read(path: &Path) -> String {
    std::fs::read_to_string(path).unwrap_or_else(|e| panic!("Failed to read {path:?}: {e}"))
}

fn write(path: &Path, text: &str) {
    std::fs::write(path, text).unwrap_or_else(|e| panic!("Failed to write {path:?}: {e}"));
}

fn to_json(value: &Value) -> String {
    serde_json::to_string(value).unwrap_or_else(|e| panic!("Failed to serialize JSON: {e}"))
}

fn commit() -> String {
    if let Ok(commit) = std::env::var("ZQS_BUILD_COMMIT") {
        if !commit.is_empty() && commit.chars().all(|c| c.is_ascii_hexdigit()) {
            return commit;
        }
    }
    let output = Command::new("git").args(["rev-parse", "HEAD"]).output();
    if let Ok(output) = output {
        if output.status.success() {
            if let Ok(hash) = String::from_utf8(output.stdout) {
                let hash = hash.trim();
                if !hash.is_empty() && hash.chars().all(|c| c.is_ascii_hexdigit()) {
                    return hash.to_string();
                }
            }
        }
    }
    "unknown".to_string()
}

/// `^\d+\.\d+\.\d+$`
fn is_semver(text: &str) -> bool {
    let parts: Vec<&str> = text.split('.').collect();
    parts.len() == 3
        && parts
            .iter()
            .all(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()))
}

/// `^\d{4}-\d{2}-\d{2}$`
fn is_iso_date(text: &str) -> bool {
    let bytes = text.as_bytes();
    bytes.len() == 10
        && bytes.iter().enumerate().all(|(i, b)| match i {
            4 | 7 => *b == b'-',
            _ => b.is_ascii_digit(),
        })
}
