use handlebars::Handlebars;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::fs;
use std::path::PathBuf;

const DEX_DIR: &str = ".dex";
const REVIEW_BASE_REF_FILE: &str = "review-base-ref.txt";

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
    hbs.register_helper(
        "dex_path",
        Box::new(
            |h: &handlebars::Helper,
             _: &Handlebars,
             _: &handlebars::Context,
             _: &mut handlebars::RenderContext,
             out: &mut dyn handlebars::Output|
             -> handlebars::HelperResult {
                let name = h.param(0).and_then(|p| p.value().as_str()).unwrap_or("");
                out.write(&dex_path(name))?;
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

fn is_review_artifact(name: &str) -> bool {
    name.starts_with("review-") && name.ends_with(".md")
}

fn remove_review_artifacts() {
    let entries = match fs::read_dir(DEX_DIR) {
        Ok(entries) => entries,
        Err(_) => return,
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        if is_review_artifact(name) {
            fs::remove_file(path).ok();
        }
    }
}

pub fn load_review_base_ref() -> Option<String> {
    read_dex_file(REVIEW_BASE_REF_FILE)
}

pub fn save_review_base_ref(base_ref: Option<&str>) {
    match base_ref.map(str::trim).filter(|value| !value.is_empty()) {
        Some(base_ref) => {
            ensure_dex_dir();
            fs::write(dex_path(REVIEW_BASE_REF_FILE), format!("{}\n", base_ref)).ok();
        }
        None => remove_dex_file(REVIEW_BASE_REF_FILE),
    }
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

pub fn reset_dex_runtime_artifacts() {
    remove_dex_file("request.txt");
    remove_dex_file("feedbacks.json");
    remove_dex_file("questions.md");
    remove_dex_file(REVIEW_BASE_REF_FILE);
    remove_review_artifacts();
}

pub fn clear_plan_state() {
    remove_dex_file("plan.md");
    reset_dex_runtime_artifacts();
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub cli: String,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            cli: "opencode".to_string(),
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

#[cfg(test)]
mod tests {
    use super::{dex_path, is_review_artifact, render_prompt, Config};

    #[test]
    fn plan_prompt_renders_internal_state_paths_via_helper() {
        let prompt = render_prompt("plan.txt", &serde_json::json!({"Request": "test request"}));

        assert!(prompt.contains(&format!(
            "1. If {} exists, read it in full",
            dex_path("plan.md")
        )));
        assert!(prompt.contains(&format!(
            "write your questions to {} using this exact format",
            dex_path("questions.md")
        )));
    }

    #[test]
    fn review_prompt_uses_dex_path_for_review_output_only() {
        let prompt = render_prompt(
            "review.txt",
            &serde_json::json!({
                "PlanPath": "custom-plan.md",
                "GitAvailable": false,
                "RoleName": "quality",
                "RoleScope": "bugs",
                "RolePrompt": "Focus on bugs.",
                "ReviewName": "review-quality.md",
            }),
        );

        assert!(prompt.contains("The implementation plan is at `custom-plan.md`."));
        assert!(prompt.contains(&format!(
            "Write your review to `{}` using this exact format:",
            dex_path("review-quality.md")
        )));
    }

    #[test]
    fn config_ignores_legacy_base_ref_field() {
        let cfg: Config = serde_json::from_str(r#"{"cli":"claude","base_ref":"main"}"#).unwrap();

        assert_eq!(cfg.cli, "claude");
    }

    #[test]
    fn review_artifact_matcher_is_precise() {
        assert!(is_review_artifact("review-quality.md"));
        assert!(is_review_artifact("review-critical-coverage.md"));
        assert!(!is_review_artifact("review-quality.txt"));
        assert!(!is_review_artifact("plan.md"));
    }
}
