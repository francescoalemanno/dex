use handlebars::Handlebars;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;
use std::process::Command;

use crate::ui::err_msg;

const DEX_DIR: &str = ".dex";
const DEX_PROMPTS_DIR: &str = ".dex/prompts";
const DEX_CONFIG_DIR: &str = "dex";
const BUILTIN_REVIEWERS: &str = include_str!("../prompts/reviewers.json");
const IMPL_COMMITS_FILE: &str = "impl_commits.jsonl";

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
        let path = PathBuf::from(DEX_PROMPTS_DIR).join(name);
        let content = match fs::read_to_string(&path) {
            Ok(content) if !content.trim().is_empty() => content,
            _ => builtin_content.to_string(),
        };
        hbs.register_template_string(name, content)
            .unwrap_or_else(|e| panic!("template {}: {}", name, e));
    }
    hbs
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

pub fn dex_available_agents(clis: &BTreeMap<String, CliConfig>) -> Vec<String> {
    clis.iter()
        .filter_map(|(name, cli)| which::which(&cli.command).is_ok().then_some(name.clone()))
        .collect()
}

pub fn validate_cli_name(clis: &BTreeMap<String, CliConfig>, name: &str) -> Result<(), String> {
    let configured: Vec<String> = clis.keys().cloned().collect();
    let configured_list = if configured.is_empty() {
        "none".to_string()
    } else {
        configured.join(", ")
    };
    let cfg = clis.get(name).ok_or_else(|| {
        format!(
            "unknown CLI {:?}; configured agents: {}",
            name, configured_list
        )
    })?;
    if which::which(&cfg.command).is_err() {
        return Err(format!(
            "CLI {:?} is not available in PATH (command {:?} not found)",
            name, cfg.command
        ));
    }
    Ok(())
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    #[serde(default = "default_cli_name")]
    pub cli: String,
    #[serde(default = "default_cli_timeout")]
    pub timeout: u64,
    #[serde(default = "default_cli_configs")]
    pub clis: BTreeMap<String, CliConfig>,
}

impl Default for Config {
    fn default() -> Self {
        let clis = default_cli_configs();
        let mut cli = default_cli_name();
        if let Some(name) = dex_available_agents(&clis).first() {
            cli = name.clone();
        } else if let Err(err) = validate_cli_name(&clis, &cli) {
            err_msg("No Agentic CLI found, consider installing OpenCode.");
            err_msg(&err);
        }

        Config {
            cli,
            timeout: default_cli_timeout(),
            clis,
        }
    }
}
fn dex_config_path() -> PathBuf {
    let root = if let Some(path) = std::env::var_os("XDG_CONFIG_HOME") {
        PathBuf::from(path)
    } else if cfg!(windows) {
        if let Some(path) = std::env::var_os("APPDATA") {
            PathBuf::from(path)
        } else if let Some(path) = std::env::var_os("USERPROFILE") {
            PathBuf::from(path).join("AppData").join("Roaming")
        } else {
            PathBuf::from(".config")
        }
    } else if let Some(path) = std::env::var_os("HOME") {
        PathBuf::from(path).join(".config")
    } else {
        PathBuf::from(".config")
    };
    root.join(DEX_CONFIG_DIR).join("config.json")
}

pub fn load_config() -> Config {
    ensure_config();
    let path = dex_config_path();
    match fs::read_to_string(&path) {
        Ok(data) => {
            let parsed: Config = serde_json::from_str(&data).unwrap_or_default();
            let mut merged = Config::default();
            if !parsed.cli.trim().is_empty() {
                merged.cli = parsed.cli;
            }
            merged.timeout = parsed.timeout;
            merged.clis.extend(parsed.clis);
            merged
        }
        Err(_) => Config::default(),
    }
}

pub fn save_config(cfg: &Config) {
    if let Some(parent) = dex_config_path().parent() {
        fs::create_dir_all(parent).ok();
    }
    let data = serde_json::to_string_pretty(cfg).unwrap_or_default();
    if let Err(e) = fs::write(dex_config_path(), format!("{}\n", data)) {
        eprintln!("Error: failed to save config: {}", e);
    }
}

pub fn ensure_config() {
    let path = dex_config_path();
    if path.exists() {
        return;
    }
    save_config(&Config::default());
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
    if let Err(e) = fs::write(dex_path("request.txt"), request) {
        eprintln!("Error: failed to save plan request: {}", e);
    }
}

pub fn save_feedbacks(feedbacks: &[String]) {
    ensure_dex_dir();
    let data = serde_json::to_string_pretty(feedbacks).unwrap_or_default();
    if let Err(e) = fs::write(dex_path("feedbacks.json"), data) {
        eprintln!("Error: failed to save feedbacks: {}", e);
    }
}

pub fn load_feedbacks() -> Vec<String> {
    let path = dex_path("feedbacks.json");
    match fs::read_to_string(&path) {
        Ok(data) => serde_json::from_str(&data).unwrap_or_else(|e| {
            eprintln!("Warning: failed to parse feedbacks.json: {}", e);
            Vec::new()
        }),
        Err(_) => Vec::new(),
    }
}

pub fn reset_dex_runtime_artifacts() {
    remove_dex_file("plan.md");
    remove_dex_file("request.txt");
    remove_dex_file("feedbacks.json");
    remove_dex_file("questions.md");
    remove_dex_file(IMPL_COMMITS_FILE);

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

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum OutputFormat {
    Plain,
    JsonNd,
    PiJsonNd,
}

fn default_output_format() -> OutputFormat {
    OutputFormat::Plain
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CliConfig {
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub stdin: bool,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    #[serde(default = "default_output_format")]
    pub output_format: OutputFormat,
}

fn cli_config(
    command: &str,
    args: &[&str],
    stdin: bool,
    env: &[(&str, &str)],
    output_format: OutputFormat,
) -> CliConfig {
    let mut env_map = BTreeMap::new();
    for (key, value) in env {
        env_map.insert((*key).to_string(), (*value).to_string());
    }
    CliConfig {
        command: command.to_string(),
        args: args.iter().map(|arg| (*arg).to_string()).collect(),
        stdin,
        env: env_map,
        output_format,
    }
}

fn default_cli_name() -> String {
    "opencode".to_string()
}

fn default_cli_timeout() -> u64 {
    600
}

fn default_cli_configs() -> BTreeMap<String, CliConfig> {
    let mut clis = BTreeMap::new();
    clis.insert(
        "amp".to_string(),
        cli_config(
            "amp",
            &["--dangerously-allow-all", "-x", "--stream-json"],
            true,
            &[],
            OutputFormat::JsonNd,
        ),
    );
    clis.insert(
        "claude".to_string(),
        cli_config(
            "claude",
            &[
                "--dangerously-skip-permissions",
                "--allow-dangerously-skip-permissions",
                "--output-format=stream-json",
                "--verbose",
                "-p",
            ],
            false,
            &[],
            OutputFormat::JsonNd,
        ),
    );
    clis.insert(
        "codex".to_string(),
        cli_config(
            "codex",
            &["exec", "--yolo", "--ephemeral", "--json"],
            true,
            &[],
            OutputFormat::JsonNd,
        ),
    );
    clis.insert(
        "droid".to_string(),
        cli_config(
            "droid",
            &["exec", "--skip-permissions-unsafe"],
            false,
            &[],
            OutputFormat::Plain,
        ),
    );
    clis.insert(
        "gemini".to_string(),
        cli_config("gemini", &["-y", "-p"], false, &[], OutputFormat::Plain),
    );
    clis.insert(
        "opencode".to_string(),
        cli_config(
            "opencode",
            &["run", "--thinking", "--format", "json"],
            true,
            &[(
                "OPENCODE_CONFIG_CONTENT",
                r#"{"$schema":"https://opencode.ai/config.json","permission":"allow","lsp":false}"#,
            )],
            OutputFormat::JsonNd,
        ),
    );
    clis.insert(
        "pi".to_string(),
        cli_config(
            "pi",
            &["--no-session", "-p", "--mode", "json"],
            false,
            &[],
            OutputFormat::PiJsonNd,
        ),
    );
    clis.insert(
        "raijin".to_string(),
        cli_config(
            "raijin",
            &["-ephemeral", "-no-echo", "-no-thinking"],
            false,
            &[],
            OutputFormat::Plain,
        ),
    );
    clis
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

/// Verify the current directory is inside a git work tree.
pub fn require_git_repo() -> Result<(), String> {
    git_trimmed_output(&["rev-parse", "--is-inside-work-tree"]).map_err(|_| {
        "dex requires a git repository. Please run from inside a git repo.".to_string()
    })?;
    Ok(())
}

/// Return the current HEAD commit hash (short-circuit if not in a repo).
pub fn git_head() -> Result<String, String> {
    git_trimmed_output(&["rev-parse", "HEAD"])
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImplCommit {
    pub before: String,
    pub after: String,
    pub message: String,
}

/// Collect commits in `(before_ref, head_ref]` as `ImplCommit` entries.
/// Returns them in chronological order (oldest first).
pub fn git_commits_between(before_ref: &str, head_ref: &str) -> Vec<ImplCommit> {
    if before_ref == head_ref {
        return Vec::new();
    }
    let range = format!("{}..{}", before_ref, head_ref);
    let Ok(log) = git_trimmed_output(&["log", "--reverse", "--format=%H %P%n%B%x00", &range])
    else {
        return Vec::new();
    };
    log.split('\0')
        .filter_map(|entry| {
            let entry = entry.trim();
            if entry.is_empty() {
                return None;
            }
            let (first_line, body) = entry.split_once('\n').unwrap_or((entry, ""));
            let mut parts = first_line.split_whitespace();
            let after = parts.next().unwrap_or("").to_string();
            let before = parts.next().unwrap_or("").to_string();
            let message = body.trim().to_string();
            if after.is_empty() {
                return None;
            }
            Some(ImplCommit {
                before,
                after,
                message,
            })
        })
        .collect()
}

/// Append impl commits to the JSONL file (one JSON object per line).
pub fn append_impl_commits(commits: &[ImplCommit]) {
    if commits.is_empty() {
        return;
    }
    ensure_dex_dir();
    let path = dex_path(IMPL_COMMITS_FILE);
    let mut file = match fs::OpenOptions::new().create(true).append(true).open(&path) {
        Ok(f) => f,
        Err(e) => {
            eprintln!("Error: failed to open {}: {}", IMPL_COMMITS_FILE, e);
            return;
        }
    };
    use std::io::Write;
    for commit in commits {
        if let Ok(json) = serde_json::to_string(commit) {
            if let Err(e) = writeln!(file, "{}", json) {
                eprintln!("Error: failed to write to {}: {}", IMPL_COMMITS_FILE, e);
            }
        }
    }
}

/// Load the most recent `n` impl commits from the JSONL file.
/// Returns them in chronological order (oldest first).
pub fn load_recent_impl_commits(n: usize) -> Vec<ImplCommit> {
    let path = dex_path(IMPL_COMMITS_FILE);
    let content = match fs::read_to_string(&path) {
        Ok(c) => c,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Vec::new(),
        Err(e) => {
            eprintln!("Warning: failed to read {}: {}", IMPL_COMMITS_FILE, e);
            return Vec::new();
        }
    };
    let all: Vec<ImplCommit> = content
        .lines()
        .filter_map(|line| serde_json::from_str::<ImplCommit>(line).ok())
        .collect();
    let start = all.len().saturating_sub(n);
    all[start..].to_vec()
}

/// Return the `before` SHA of the very first impl commit, if the JSONL exists.
pub fn impl_commits_base_ref() -> Option<String> {
    let path = dex_path(IMPL_COMMITS_FILE);
    let content = fs::read_to_string(&path).ok()?;
    let first_line = content.lines().next()?;
    let commit: ImplCommit = serde_json::from_str(first_line).ok()?;
    if commit.before.is_empty() {
        None
    } else {
        Some(commit.before)
    }
}

/// Build a summary string from recent impl commits for prompt injection.
/// Latest 5 get full body, remaining older ones get first line only.
pub fn impl_commit_history_summary() -> Option<String> {
    let commits = load_recent_impl_commits(25);
    if commits.is_empty() {
        return None;
    }
    let total = commits.len();
    let full_start = total.saturating_sub(5);
    let mut lines = Vec::new();
    for (i, c) in commits.iter().enumerate() {
        let short_sha = &c.after[..c.after.len().min(8)];
        if i < full_start {
            let first_line = c.message.lines().next().unwrap_or("(empty)");
            lines.push(format!("- {} {}", short_sha, first_line));
        } else {
            lines.push(format!("- {} {}", short_sha, c.message));
        }
    }
    Some(lines.join("\n"))
}

#[cfg(test)]
mod tests {
    use super::{dex_path, git_commits_between, render_prompt, Config};
    use std::process::Command;

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
        assert_eq!(cfg.timeout, 600);
        assert!(cfg.clis.contains_key("amp"));
        assert!(cfg.clis.contains_key("gemini"));
    }

    #[test]
    fn config_reads_timeout_from_json() {
        let cfg: Config = serde_json::from_str(r#"{"timeout":42}"#).unwrap();

        assert_eq!(cfg.timeout, 42);
    }

    fn git(dir: &std::path::Path, args: &[&str]) -> String {
        let out = Command::new("git")
            .args(args)
            .current_dir(dir)
            .output()
            .expect("git command failed");
        assert!(out.status.success(), "git {:?} failed", args);
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }

    #[test]
    fn git_commits_between_captures_multiple_commits() {
        let tmp = std::env::temp_dir().join(format!(
            "dex-test-multi-commit-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        std::fs::create_dir_all(&tmp).unwrap();

        git(&tmp, &["init"]);
        git(&tmp, &["config", "user.email", "test@test.com"]);
        git(&tmp, &["config", "user.name", "Test"]);

        std::fs::write(tmp.join("a.txt"), "a").unwrap();
        git(&tmp, &["add", "."]);
        git(&tmp, &["commit", "-m", "initial"]);
        let before = git(&tmp, &["rev-parse", "HEAD"]);

        std::fs::write(tmp.join("b.txt"), "b").unwrap();
        git(&tmp, &["add", "."]);
        git(&tmp, &["commit", "-m", "first\n\nfirst body"]);

        std::fs::write(tmp.join("c.txt"), "c").unwrap();
        git(&tmp, &["add", "."]);
        git(&tmp, &["commit", "-m", "second\n\nsecond body"]);
        let after = git(&tmp, &["rev-parse", "HEAD"]);

        // Run from inside the temp repo
        let _prev = std::env::current_dir().unwrap();
        std::env::set_current_dir(&tmp).unwrap();
        let commits = git_commits_between(&before, &after);
        std::env::set_current_dir(&_prev).unwrap();

        let _ = std::fs::remove_dir_all(&tmp);

        assert_eq!(commits.len(), 2, "expected 2 commits, got {:?}", commits);

        assert!(commits[0].message.starts_with("first"));
        assert!(commits[0].message.contains("first body"));
        assert_eq!(commits[0].before, before);

        assert!(commits[1].message.starts_with("second"));
        assert!(commits[1].message.contains("second body"));
        assert_eq!(commits[1].before, commits[0].after);
        assert_eq!(commits[1].after, after);
    }
}
