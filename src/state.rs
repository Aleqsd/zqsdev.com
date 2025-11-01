use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ProfileLinks {
    pub github: Option<String>,
    pub linkedin: Option<String>,
    pub website: Option<String>,
    pub resume_url: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Profile {
    pub name: String,
    pub headline: String,
    pub summary_fr: Option<String>,
    pub summary_en: Option<String>,
    pub location: Option<String>,
    pub email: Option<String>,
    pub links: ProfileLinks,
    pub languages: Option<Vec<String>>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Experience {
    pub title: String,
    pub company: String,
    pub location: Option<String>,
    pub start: Option<String>,
    pub end: Option<String>,
    pub highlights: Vec<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Education {
    pub degree: String,
    pub school: String,
    pub years: Option<String>,
    pub location: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Project {
    pub name: String,
    pub desc: String,
    pub tech: Vec<String>,
    pub link: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Testimonial {
    pub quote: String,
    pub author: String,
    pub role: Option<String>,
    pub link: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct FaqEntry {
    pub question: String,
    pub answer: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TerminalData {
    pub profile: Profile,
    pub skills: BTreeMap<String, Vec<String>>,
    pub experiences: Vec<Experience>,
    pub education: Vec<Education>,
    pub projects: Vec<Project>,
    pub testimonials: Vec<Testimonial>,
    pub faqs: Vec<FaqEntry>,
}

impl TerminalData {
    pub fn new(
        profile: Profile,
        skills: BTreeMap<String, Vec<String>>,
        experiences: Vec<Experience>,
        education: Vec<Education>,
        projects: Vec<Project>,
        testimonials: Vec<Testimonial>,
        faqs: Vec<FaqEntry>,
    ) -> Self {
        Self {
            profile,
            skills,
            experiences,
            education,
            projects,
            testimonials,
            faqs,
        }
    }
}

#[derive(Debug, Clone)]
pub struct AppState {
    pub prompt_label: String,
    pub input_buffer: String,
    pub command_history: Vec<String>,
    pub history_index: Option<usize>,
    pub data: Option<TerminalData>,
    pub initialized: bool,
    pub ai_mode: bool,
    pub ai_model: Option<String>,
}

impl AppState {
    pub fn new() -> Self {
        Self {
            prompt_label: "zqs@dev:~$".to_string(),
            input_buffer: String::new(),
            command_history: Vec::new(),
            history_index: None,
            data: None,
            initialized: false,
            ai_mode: false,
            ai_model: None,
        }
    }

    pub fn set_data(&mut self, data: TerminalData) {
        self.data = Some(data);
        self.initialized = true;
    }

    pub fn remember_command(&mut self, command: &str) {
        if !command.trim().is_empty() {
            self.command_history.push(command.trim().to_string());
        }
        self.history_index = None;
    }

    pub fn set_ai_mode(&mut self, active: bool) {
        self.ai_mode = active;
    }

    pub fn set_ai_model(&mut self, model: Option<String>) {
        self.ai_model = model;
    }
}
