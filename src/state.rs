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
    pub title: String,
    #[serde(default)]
    pub date: Option<String>,
    pub description: String,
    #[serde(default)]
    pub tech: Vec<String>,
    #[serde(default)]
    pub link: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Publication {
    pub title: String,
    #[serde(default)]
    pub date: Option<String>,
    pub description: String,
    #[serde(default)]
    pub tech: Vec<String>,
    #[serde(default)]
    pub link: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Award {
    pub title: String,
    #[serde(default)]
    pub issuer: Option<String>,
    #[serde(default)]
    pub date: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct ProjectsCollection {
    #[serde(default)]
    pub projects: Vec<Project>,
    #[serde(default)]
    pub publications: Vec<Publication>,
    #[serde(default)]
    pub awards: Vec<Award>,
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
    pub projects: ProjectsCollection,
    pub testimonials: Vec<Testimonial>,
    pub faqs: Vec<FaqEntry>,
}

impl TerminalData {
    pub fn new(
        profile: Profile,
        skills: BTreeMap<String, Vec<String>>,
        experiences: Vec<Experience>,
        education: Vec<Education>,
        projects: ProjectsCollection,
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
    pub input_disabled: bool,
    pub konami_index: usize,
    pub konami_triggered: bool,
    pub pokemon_capture_chance: u8,
    pub achievement_shaw_unlocked: bool,
    pub achievement_pokemon_unlocked: bool,
    pub achievement_cookie_unlocked: bool,
    pub achievement_konami_unlocked: bool,
    pub achievement_shutdown_unlocked: bool,
    pub achievements_modal_open: bool,
    pub achievements_spoilers_enabled: bool,
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
            input_disabled: false,
            konami_index: 0,
            konami_triggered: false,
            pokemon_capture_chance: 1,
            achievement_shaw_unlocked: false,
            achievement_pokemon_unlocked: false,
            achievement_cookie_unlocked: false,
            achievement_konami_unlocked: false,
            achievement_shutdown_unlocked: false,
            achievements_modal_open: false,
            achievements_spoilers_enabled: false,
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

    pub fn set_input_disabled(&mut self, disabled: bool) {
        self.input_disabled = disabled;
    }

    pub fn input_disabled(&self) -> bool {
        self.input_disabled
    }

    pub fn pokemon_capture_chance(&self) -> u8 {
        self.pokemon_capture_chance
    }

    pub fn set_pokemon_capture_chance(&mut self, chance: u8) {
        self.pokemon_capture_chance = chance.clamp(1, 100);
    }

    pub fn unlock_shaw_celebration(&mut self) -> bool {
        Self::unlock_flag(&mut self.achievement_shaw_unlocked)
    }

    pub fn unlock_pokemon_master(&mut self) -> bool {
        Self::unlock_flag(&mut self.achievement_pokemon_unlocked)
    }

    pub fn unlock_cookie_rain(&mut self) -> bool {
        Self::unlock_flag(&mut self.achievement_cookie_unlocked)
    }

    pub fn unlock_konami_secret(&mut self) -> bool {
        Self::unlock_flag(&mut self.achievement_konami_unlocked)
    }

    pub fn unlock_shutdown_protocol(&mut self) -> bool {
        Self::unlock_flag(&mut self.achievement_shutdown_unlocked)
    }

    fn unlock_flag(flag: &mut bool) -> bool {
        if *flag {
            false
        } else {
            *flag = true;
            true
        }
    }
}
