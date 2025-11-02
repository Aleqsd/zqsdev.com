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

    terminal.restore_achievements_from_storage();
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
            if let Err(err) = keyword_icons::preload_all_icons() {
                utils::log(&format!("Failed to preload keyword icons: {:?}", err));
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
    use state::{Education, Experience, FaqEntry, ProjectsCollection, Testimonial};
    use std::collections::BTreeMap;

    let base = "./data";

    let profile_path = format!("{base}/profile.json");
    let skills_path = format!("{base}/skills.json");
    let experiences_path = format!("{base}/experience.json");
    let education_path = format!("{base}/education.json");
    let projects_path = format!("{base}/projects.json");
    let testimonials_path = format!("{base}/testimonials.json");
    let faqs_path = format!("{base}/faq.json");

    let profile_fut = utils::fetch_json::<Profile>(&profile_path);
    let skills_fut = utils::fetch_json::<BTreeMap<String, Vec<String>>>(&skills_path);
    let experiences_fut = utils::fetch_json::<Vec<Experience>>(&experiences_path);
    let education_fut = utils::fetch_json::<Vec<Education>>(&education_path);
    let projects_fut = utils::fetch_json::<ProjectsCollection>(&projects_path);
    let testimonials_fut = utils::fetch_json::<Vec<Testimonial>>(&testimonials_path);
    let faqs_fut = utils::fetch_json::<Vec<FaqEntry>>(&faqs_path);

    let (profile, skills, experiences, education, projects, testimonials, faqs) = futures::try_join!(
        profile_fut,
        skills_fut,
        experiences_fut,
        education_fut,
        projects_fut,
        testimonials_fut,
        faqs_fut,
    )?;

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
