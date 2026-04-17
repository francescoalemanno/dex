use handlebars::Handlebars;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::fs;
use std::io::Write;
use std::path::PathBuf;
use std::process::Command;

const DEX_DIR: &str = ".dex";
const DEX_PROMPTS_DIR: &str = ".dex/prompts";
const REVIEW_BASE_REF_FILE: &str = "review-base-ref.txt";
const BUILTIN_REVIEWERS: &str = include_str!("../prompts/reviewers.json");

const BUILTIN_TEMPLATES: &[(&str, &str)] = &[
    ("bare.txt", include_str!("../prompts/bare.txt")),
    ("finalize.txt", include_str!("../prompts/finalize.txt")),
    ("fix.txt", include_str!("../prompts/fix.txt")),
    ("impl.txt", include_str!("../prompts/impl.txt")),
    ("plan.txt", include_str!("../prompts/plan.txt")),
    ("review.txt", include_str!("../prompts/review.txt")),
    ("research.txt", include_str!("../prompts/research.txt")),
];

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

    for (name, builtin_content) in BUILTIN_TEMPLATES {
        let content = load_user_prompt(name).unwrap_or_else(|| builtin_content.to_string());
        hbs.register_template_string(name, content)
            .unwrap_or_else(|e| panic!("template {}: {}", name, e));
    }
    hbs
}

fn load_user_prompt(name: &str) -> Option<String> {
    let path = PathBuf::from(DEX_PROMPTS_DIR).join(name);
    match fs::read_to_string(&path) {
        Ok(content) if !content.trim().is_empty() => Some(content),
        _ => None,
    }
}

pub fn seed_prompts(force: bool) {
    fs::create_dir_all(DEX_PROMPTS_DIR).ok();
    for (name, builtin_content) in BUILTIN_TEMPLATES {
        let path = PathBuf::from(DEX_PROMPTS_DIR).join(name);
        if force || !path.exists() {
            fs::write(&path, builtin_content).ok();
        }
    }
    let reviewers_path = PathBuf::from(DEX_DIR).join("reviewers.json");
    if force || !reviewers_path.exists() {
        fs::write(&reviewers_path, BUILTIN_REVIEWERS).ok();
    }
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

pub fn append_progress(section: &str, body: &str) {
    let body = body.trim();
    if body.is_empty() {
        return;
    }

    ensure_dex_dir();
    let path = dex_path("progress.txt");
    let mut file = match fs::OpenOptions::new().create(true).append(true).open(&path) {
        Ok(file) => file,
        Err(_) => return,
    };

    let needs_header = file.metadata().map(|m| m.len() == 0).unwrap_or(true);
    if needs_header {
        let _ = writeln!(file, "# Dex Progress Log");
        let _ = writeln!(file);
    } else {
        let _ = writeln!(file);
    }

    let _ = writeln!(file, "## {}", section);
    let _ = writeln!(file);
    let _ = writeln!(file, "{}", body);
}

pub fn reset_dex_runtime_artifacts() {
    remove_dex_file("plan.md");
    remove_dex_file("request.txt");
    remove_dex_file("feedbacks.json");
    remove_dex_file("questions.md");
    remove_dex_file("progress.txt");
    remove_dex_file(REVIEW_BASE_REF_FILE);

    let entries = match fs::read_dir(DEX_DIR) {
        Ok(entries) => entries,
        Err(_) => return,
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_file() {
            if let Some(name) = path.file_name().and_then(|name| name.to_str()) {
                if name.starts_with("review-") && name.ends_with(".md") {
                    fs::remove_file(path).ok();
                }
            }
        }
    }
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

pub fn git_trimmed_output(args: &[&str]) -> Result<String, String> {
    let out = Command::new("git")
        .args(args)
        .output()
        .map_err(|e| format!("git {}: {}", args.join(" "), e))?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
        let detail = if stderr.is_empty() {
            format!("exit {}", out.status)
        } else {
            stderr
        };
        return Err(format!("git {}: {}", args.join(" "), detail));
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

#[cfg(test)]
mod tests {
    use super::{dex_path, render_prompt, Config};

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
                "BaseRef": "",
            }),
        );

        assert!(prompt.contains("The implementation plan is at `custom-plan.md`."));
        assert!(prompt.contains(&format!(
            "Write your review to `{}`",
            dex_path("review-quality.md")
        )));
    }

    #[test]
    fn config_ignores_legacy_base_ref_field() {
        let cfg: Config = serde_json::from_str(r#"{"cli":"claude","base_ref":"main"}"#).unwrap();

        assert_eq!(cfg.cli, "claude");
    }
}
