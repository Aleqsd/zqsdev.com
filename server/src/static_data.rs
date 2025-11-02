use anyhow::{Context, Result};
use serde::Serialize;
use serde_json::{self, Value};
use std::path::Path;

#[derive(Debug, Clone, Serialize)]
pub struct TerminalDataPayload {
    pub profile: Value,
    pub skills: Value,
    pub experiences: Value,
    pub education: Value,
    pub projects: Value,
    pub testimonials: Value,
    pub faqs: Value,
}

impl TerminalDataPayload {
    pub fn load(data_dir: &Path) -> Result<Self> {
        Ok(Self {
            profile: load_json(data_dir, "profile.json")?,
            skills: load_json(data_dir, "skills.json")?,
            experiences: load_json(data_dir, "experience.json")?,
            education: load_json(data_dir, "education.json")?,
            projects: load_json(data_dir, "projects.json")?,
            testimonials: load_json(data_dir, "testimonials.json")?,
            faqs: load_json(data_dir, "faq.json")?,
        })
    }

    pub fn knowledge_json(&self) -> Value {
        let mut merged = serde_json::Map::new();
        merged.insert("profile".to_string(), self.profile.clone());
        merged.insert("skills".to_string(), self.skills.clone());
        merged.insert("experience".to_string(), self.experiences.clone());
        merged.insert("education".to_string(), self.education.clone());
        merged.insert("projects".to_string(), self.projects.clone());
        merged.insert("testimonials".to_string(), self.testimonials.clone());
        merged.insert("faq".to_string(), self.faqs.clone());
        Value::Object(merged)
    }
}

fn load_json(data_dir: &Path, filename: &str) -> Result<Value> {
    let path = data_dir.join(filename);
    let content = std::fs::read_to_string(&path)
        .with_context(|| format!("Failed to read data file {path:?}"))?;
    let value = serde_json::from_str(&content)
        .with_context(|| format!("Failed to parse JSON from {path:?}"))?;
    Ok(value)
}
