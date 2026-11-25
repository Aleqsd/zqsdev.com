//! Build-time constants (see `build.rs`): the `/api/data` body, the escaped system message, the
//! version, the commit and the knowledge date.

/// `/api/data` body: the 7 data files with sorted keys and the `faq` alias (16,275 bytes at 1.0.39).
pub const DATA_JSON: &str = include_str!(concat!(env!("OUT_DIR"), "/data.json"));

/// The first system message (template + knowledge) as an escaped JSON string literal, quotes
/// included, pasted into every provider body.
pub const SYSTEM_MESSAGE_JSON: &str =
    include_str!(concat!(env!("OUT_DIR"), "/system_message.json"));

/// `../VERSION`.
pub const VERSION: &str = env!("ZQS_VERSION");

/// `ZQS_BUILD_COMMIT`, else the full `git rev-parse HEAD`, else `unknown`.
pub const COMMIT: &str = env!("ZQS_COMMIT");

/// `profile.json` `updated_at`.
pub const KNOWLEDGE_UPDATED: &str = env!("ZQS_KNOWLEDGE_UPDATED");

pub const KNOWLEDGE_MODE: &str = "full_context";

/// The only valid `sources` IDs: the top-level keys of the knowledge JSON.
pub const SOURCE_IDS: [&str; 7] = [
    "profile",
    "skills",
    "experience",
    "education",
    "projects",
    "testimonials",
    "faq",
];

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::prompt::SYSTEM_TEMPLATE;
    use serde::Serialize;
    use serde_json::Value;
    use sha2::{Digest, Sha256};
    use std::path::{Path, PathBuf};

    fn data_dir() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../static/data")
    }

    fn load(name: &str) -> Value {
        let text = std::fs::read_to_string(data_dir().join(name)).unwrap();
        serde_json::from_str(&text).unwrap()
    }

    /// A copy of the original `TerminalDataPayload` (field order of `static_data.rs`).
    #[derive(Serialize)]
    struct TerminalDataPayload {
        profile: Value,
        skills: Value,
        experiences: Value,
        education: Value,
        projects: Value,
        testimonials: Value,
        faqs: Value,
    }

    fn payload() -> TerminalDataPayload {
        TerminalDataPayload {
            profile: load("profile.json"),
            skills: load("skills.json"),
            experiences: load("experience.json"),
            education: load("education.json"),
            projects: load("projects.json"),
            testimonials: load("testimonials.json"),
            faqs: load("faq.json"),
        }
    }

    /// The original `knowledge_json()`, serialised with `serde_json::to_string`.
    fn knowledge() -> String {
        let p = payload();
        let mut merged = serde_json::Map::new();
        merged.insert("profile".to_string(), p.profile);
        merged.insert("skills".to_string(), p.skills);
        merged.insert("experience".to_string(), p.experiences);
        merged.insert("education".to_string(), p.education);
        merged.insert("projects".to_string(), p.projects);
        merged.insert("testimonials".to_string(), p.testimonials);
        merged.insert("faq".to_string(), p.faqs);
        serde_json::to_string(&Value::Object(merged)).unwrap()
    }

    fn system_message() -> String {
        serde_json::from_str(SYSTEM_MESSAGE_JSON).unwrap()
    }

    fn sha256_hex(bytes: &[u8]) -> String {
        Sha256::digest(bytes)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect()
    }

    /// The original `terminal_payload_with_alias`, byte for byte.
    #[test]
    fn data_json_matches_the_original_payload() {
        let mut value = serde_json::to_value(payload()).unwrap();
        if let Some(map) = value.as_object_mut() {
            if let Some(faqs) = map.get("faqs").cloned() {
                map.entry("faq".to_string()).or_insert(faqs);
            }
        }
        assert_eq!(DATA_JSON, serde_json::to_string(&value).unwrap());
        // Keys sorted at every level: re-serialising the parsed value changes nothing.
        let parsed: Value = serde_json::from_str(DATA_JSON).unwrap();
        assert_eq!(parsed.to_string(), DATA_JSON);
        assert_eq!(parsed["faq"], parsed["faqs"]);
        let keys: Vec<&String> = parsed.as_object().unwrap().keys().collect();
        assert_eq!(
            keys,
            [
                "education",
                "experiences",
                "faq",
                "faqs",
                "profile",
                "projects",
                "skills",
                "testimonials"
            ]
        );
    }

    /// Guards against `preserve_order` arriving through feature unification.
    #[test]
    fn serde_json_maps_sort_their_keys() {
        assert_eq!(
            serde_json::json!({"b":1,"a":2}).to_string(),
            r#"{"a":2,"b":1}"#
        );
    }

    /// Prompt golden: the template is the original raw string (SHA-256 confirmed against the
    /// original server's output by the one-time cross-check), with exactly one knowledge copy.
    #[test]
    fn prompt_golden() {
        assert_eq!(SYSTEM_TEMPLATE.len(), 1_659);
        assert_eq!(
            sha256_hex(SYSTEM_TEMPLATE.as_bytes()),
            "1cf6117c5839f466122c5b602f4532516cc05573bd7fe0ba60fd51ca52706db4"
        );
        assert!(!SYSTEM_TEMPLATE.contains('\r'));
        assert_eq!(SYSTEM_TEMPLATE.matches("{knowledge}").count(), 1);
        assert_eq!(SYSTEM_TEMPLATE.lines().count(), 6);
        let knowledge = knowledge();
        assert!(knowledge.len() <= 64_000);
        let system = system_message();
        assert_eq!(system.matches(&knowledge).count(), 1);
        assert_eq!(system, SYSTEM_TEMPLATE.replace("{knowledge}", &knowledge));
        assert_eq!(SYSTEM_MESSAGE_JSON, serde_json::to_string(&system).unwrap());
    }

    #[test]
    fn version_matches_the_version_file() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../VERSION");
        assert_eq!(VERSION, std::fs::read_to_string(path).unwrap().trim());
        assert!(COMMIT == "unknown" || COMMIT.chars().all(|c| c.is_ascii_hexdigit()));
        assert_eq!(KNOWLEDGE_MODE, "full_context");
    }

    /// Ported from the original server test.
    #[test]
    fn knowledge_updated_matches_profile() {
        let profile = load("profile.json");
        assert_eq!(profile["updated_at"].as_str(), Some(KNOWLEDGE_UPDATED));
    }

    /// Ported from the original server test, on the embedded constants.
    #[test]
    fn preserves_rich_knowledge_and_current_role() {
        let text = system_message();
        for fact in [
            "50,000",
            "Langfuse",
            "Micro Mages",
            "BeeToBee",
            "Machine Learning and Cancer Prediction",
            "500K",
            "60+",
            "Mar 2026",
            "Apr 2026",
        ] {
            assert!(text.contains(fact), "missing {fact}");
        }
        let data: Value = serde_json::from_str(DATA_JSON).unwrap();
        assert!(data["experiences"][0]["company"]
            .as_str()
            .unwrap()
            .contains("Studi"));
        assert_eq!(data["testimonials"].as_array().unwrap().len(), 2);
        assert_eq!(
            data["profile"]["links"]["resume_url"],
            "https://cv.zqsdev.com/"
        );
        let knowledge: Value = serde_json::from_str(&knowledge()).unwrap();
        let keys: Vec<&str> = knowledge
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        let mut ids = SOURCE_IDS.to_vec();
        ids.sort_unstable();
        assert_eq!(keys, ids);
    }

    /// Lengths and SHA-256 for the one-time cross-check against the original server (7.2 step 1).
    /// Run with `cargo test --lib golden_values_for_cross_check -- --ignored --nocapture`.
    #[test]
    #[ignore = "prints the golden table; not an assertion"]
    fn golden_values_for_cross_check() {
        use crate::core::provider::build_body;
        use crate::core::request::Message;
        let history = [
            Message {
                role: "user".into(),
                content: " Sony? \"quoted\" \\ é 😀".into(),
            },
            Message {
                role: "assistant".into(),
                content: "Games.\n\u{1}".into(),
            },
        ];
        let rows = [
            ("template", SYSTEM_TEMPLATE.to_string()),
            ("system", system_message()),
            ("knowledge", knowledge()),
            ("data", DATA_JSON.to_string()),
            (
                "body_gpt56",
                build_body("gpt-5.6-luna", "2026-11-24", &[], "Micro Mages?"),
            ),
            (
                "body_gpt6",
                build_body("gpt-6-luna", "2026-11-24", &[], "Micro Mages?"),
            ),
            (
                "body_gpt6_history",
                build_body("gpt-6-luna", "2026-11-24", &history, "  And which tools?\t"),
            ),
        ];
        let dir = std::env::var("ZQS_GOLDEN_DIR").ok();
        for (name, text) in rows {
            if let Some(dir) = &dir {
                std::fs::write(Path::new(dir).join(format!("{name}.bin")), &text).unwrap();
            }
            println!(
                "GOLDEN {name} {} {}",
                text.len(),
                sha256_hex(text.as_bytes())
            );
        }
    }
}
