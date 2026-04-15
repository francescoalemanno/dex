use handlebars::Handlebars;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::fs;
use std::path::PathBuf;

const DEX_DIR: &str = ".dex";

fn template_engine() -> Handlebars<'static> {
    let mut hbs = Handlebars::new();
    hbs.set_strict_mode(false);
    hbs.register_escape_fn(handlebars::no_escape);
    hbs.register_helper(
        "inc",
        Box::new(
            |h: &handlebars::Helper,
             _: &Handlebars,
             _: &handlebars::Context,
             _: &mut handlebars::RenderContext,
             out: &mut dyn handlebars::Output|
             -> handlebars::HelperResult {
                let v = h.param(0).and_then(|p| p.value().as_u64()).unwrap_or(0);
                out.write(&(v + 1).to_string())?;
                Ok(())
            },
        ),
    );

    let templates = [
        ("bare.txt", include_str!("../prompts/bare.txt")),
        ("finalize.txt", include_str!("../prompts/finalize.txt")),
        ("fix.txt", include_str!("../prompts/fix.txt")),
        ("impl.txt", include_str!("../prompts/impl.txt")),
        ("plan.txt", include_str!("../prompts/plan.txt")),
        ("review.txt", include_str!("../prompts/review.txt")),
    ];
    for (name, content) in templates {
        hbs.register_template_string(name, content)
            .unwrap_or_else(|e| panic!("template {}: {}", name, e));
    }
    hbs
}

pub fn render_prompt(name: &str, data: &Value) -> String {
    let hbs = template_engine();
    hbs.render(name, data)
        .unwrap_or_else(|e| panic!("template {}: {}", name, e))
}

pub fn ensure_dex_dir() {
    fs::create_dir_all(DEX_DIR).ok();
    let gitignore = PathBuf::from(DEX_DIR).join(".gitignore");
    if !gitignore.exists() {
        fs::write(&gitignore, "*\n").ok();
    }
}

pub fn dex_path(name: &str) -> String {
    PathBuf::from(DEX_DIR)
        .join(name)
        .to_string_lossy()
        .to_string()
}

pub fn read_dex_file(name: &str) -> Option<String> {
    let path = dex_path(name);
    match fs::read_to_string(&path) {
        Ok(content) => {
            let trimmed = content.trim().to_string();
            if trimmed.is_empty() {
                None
            } else {
                Some(trimmed)
            }
        }
        Err(_) => None,
    }
}

pub fn remove_dex_file(name: &str) {
    fs::remove_file(dex_path(name)).ok();
}

pub fn save_plan_request(request: &str) {
    ensure_dex_dir();
    fs::write(dex_path("request.txt"), request).ok();
}

pub fn save_feedbacks(feedbacks: &[String]) {
    ensure_dex_dir();
    let data = serde_json::to_string_pretty(feedbacks).unwrap_or_default();
    fs::write(dex_path("feedbacks.json"), data).ok();
}

pub fn load_feedbacks() -> Vec<String> {
    let path = dex_path("feedbacks.json");
    match fs::read_to_string(&path) {
        Ok(data) => serde_json::from_str(&data).unwrap_or_default(),
        Err(_) => Vec::new(),
    }
}

pub fn clear_plan_state() {
    remove_dex_file("plan.md");
    remove_dex_file("request.txt");
    remove_dex_file("feedbacks.json");
    remove_dex_file("questions.md");
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub cli: String,
    #[serde(default)]
    pub base_ref: String,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            cli: "opencode".to_string(),
            base_ref: "HEAD".to_string(),
        }
    }
}

pub fn load_config() -> Config {
    let path = dex_path("config.json");
    match fs::read_to_string(&path) {
        Ok(data) => serde_json::from_str(&data).unwrap_or_default(),
        Err(_) => Config::default(),
    }
}

pub fn save_config(cfg: &Config) {
    ensure_dex_dir();
    let data = serde_json::to_string_pretty(cfg).unwrap_or_default();
    fs::write(dex_path("config.json"), format!("{}\n", data)).ok();
}
