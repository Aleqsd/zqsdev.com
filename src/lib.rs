mod ai;
mod commands;
mod input;
mod keyword_icons;
mod markdown;
mod renderer;
mod state;
mod terminal;
mod utils;

use crate::renderer::Renderer;
use crate::state::{AppState, Profile, TerminalData};
use crate::terminal::Terminal;
use std::cell::RefCell;
use std::rc::Rc;
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::spawn_local;

#[wasm_bindgen(start)]
pub fn start() -> Result<(), JsValue> {
    console_error_panic_hook::set_once();

    let state = Rc::new(RefCell::new(AppState::new()));
    let renderer = Rc::new(Renderer::new()?);
    let terminal = Rc::new(Terminal::new(Rc::clone(&state), Rc::clone(&renderer)));

    terminal.initialize()?;
    terminal.push_system_message("Booting…");

    input::install_listeners(Rc::clone(&terminal))?;

    spawn_local(load_terminal_data(Rc::clone(&terminal), Rc::clone(&state)));

    Ok(())
}

async fn load_terminal_data(terminal: Rc<Terminal>, state: Rc<RefCell<AppState>>) {
    match fetch_all_data().await {
        Ok(data) => {
            {
                let mut state_mut = state.borrow_mut();
                state_mut.set_data(data);
            }
            if let Err(err) = terminal.on_data_ready() {
                utils::log(&format!("Failed to render welcome message: {:?}", err));
            }
        }
        Err(err) => {
            utils::log(&format!("Failed to load résumé data: {:?}", err));
            terminal.push_system_message(
                "⚠️ Unable to load résumé data. Please refresh and try again.",
            );
        }
    }
}

async fn fetch_all_data() -> Result<TerminalData, JsValue> {
    use state::{Education, Experience, FaqEntry, Project, Testimonial};
    use std::collections::BTreeMap;

    let base = "./data";

    let profile: Profile = utils::fetch_json(&format!("{base}/profile.json")).await?;
    let skills: BTreeMap<String, Vec<String>> =
        utils::fetch_json(&format!("{base}/skills.json")).await?;
    let experiences: Vec<Experience> =
        utils::fetch_json(&format!("{base}/experience.json")).await?;
    let education: Vec<Education> = utils::fetch_json(&format!("{base}/education.json")).await?;
    let projects: Vec<Project> = utils::fetch_json(&format!("{base}/projects.json")).await?;
    let testimonials: Vec<Testimonial> =
        utils::fetch_json(&format!("{base}/testimonials.json")).await?;
    let faqs: Vec<FaqEntry> = utils::fetch_json(&format!("{base}/faq.json")).await?;

    Ok(TerminalData::new(
        profile,
        skills,
        experiences,
        education,
        projects,
        testimonials,
        faqs,
    ))
}
