mod core;
mod phases;
mod plan;
mod research;
mod runner;
mod ui;

use argh::FromArgs;
use std::fs;
use std::io::IsTerminal;
use std::path::Path;
use std::process::exit;
use std::time::Duration;

use crate::core::{
    dex_path, ensure_dex_dir, git_trimmed_output, load_config, load_feedbacks,
    load_review_base_ref, read_dex_file, reset_dex_runtime_artifacts, save_config,
    save_plan_request, save_review_base_ref, seed_prompts, Config,
};
use crate::phases::{bare_phase, finalize_phase, impl_phase, plan_phase, review_phase};
use crate::plan::validate_candidate_plan;
use crate::runner::{kill_all_children, Runner};
use crate::ui::{app_header, banner, err_msg, info, warn};

const REVISION: &str = env!("CARGO_PKG_VERSION");

// ── CLI argument definitions ──

/// dex — AI-powered development workflow
#[derive(FromArgs)]
struct Args {
    /// print version and exit
    #[argh(switch)]
    version: bool,

    /// overwrite local prompt templates with built-in defaults
    #[argh(switch)]
    update_prompts: bool,

    /// coding CLI to use; must be available in PATH
    #[argh(option, arg_name = "name")]
    cli: Option<String>,

    /// kill agent after this many seconds idle
    #[argh(option, default = "1200")]
    timeout: u64,

    #[argh(subcommand)]
    command: Option<SubCommand>,
}

#[derive(FromArgs)]
#[argh(subcommand)]
enum SubCommand {
    Plan(PlanCmd),
    Import(ImportCmd),
    Amend(AmendCmd),
    Apply(ApplyCmd),
    Review(ReviewCmd),
    Bare(BareCmd),
    Finalize(FinalizeCmd),
    Research(ResearchCmd),
}

/// create or replace the current plan from a request
#[derive(FromArgs)]
#[argh(
    subcommand,
    name = "plan",
    example = "Draft a new plan:\n  {command_name} \"redesign the caching layer\"",
    example = "Overwrite an existing plan:\n  {command_name} --force \"rewrite the auth module\""
)]
struct PlanCmd {
    /// overwrite an existing plan and start fresh
    #[argh(switch)]
    force: bool,

    /// request text or a file path containing the request
    #[argh(positional, greedy)]
    request: Vec<String>,
}

/// install a markdown plan file as the current plan
#[derive(FromArgs)]
#[argh(
    subcommand,
    name = "import",
    example = "Import a prepared plan:\n  {command_name} myplan.md"
)]
struct ImportCmd {
    /// overwrite an existing plan and start fresh
    #[argh(switch)]
    force: bool,

    /// path to the markdown plan file
    #[argh(positional)]
    file: String,
}

/// revise the current plan using feedback
#[derive(FromArgs)]
#[argh(
    subcommand,
    name = "amend",
    example = "Amend the current plan:\n  {command_name} \"split database and HTTP work into separate tasks\""
)]
struct AmendCmd {
    /// amendment feedback or a file path containing the feedback
    #[argh(positional, greedy)]
    feedback: Vec<String>,
}

/// implement the current plan
#[derive(FromArgs)]
#[argh(
    subcommand,
    name = "apply",
    example = "Apply the current plan:\n  {command_name}"
)]
struct ApplyCmd {}

/// review the current implementation
#[derive(FromArgs)]
#[argh(
    subcommand,
    name = "review",
    example = "Review the current implementation:\n  {command_name} --parallel 2"
)]
struct ReviewCmd {
    /// max reviewers to run in parallel (default: all)
    #[argh(option)]
    parallel: Option<usize>,
}

/// send a request straight to the agent for N iterations
#[derive(FromArgs)]
#[argh(
    subcommand,
    name = "bare",
    example = "Open-ended bare loop:\n  {command_name} 10 bare-request.txt"
)]
struct BareCmd {
    /// number of iterations
    #[argh(positional)]
    iterations: usize,

    /// request file; it is re-read on every iteration
    #[argh(positional)]
    request_file: String,
}

/// finalize a feature branch against a rebase target
#[derive(FromArgs)]
#[argh(
    subcommand,
    name = "finalize",
    example = "Finalize a feature branch:\n  {command_name} --onto main"
)]
struct FinalizeCmd {
    /// rebase target (e.g. main, origin/main)
    #[argh(option)]
    onto: String,
}

/// autonomous research loop — optimize a metric through experiments
#[derive(FromArgs)]
#[argh(
    subcommand,
    name = "research",
    example = "Start a new session:\n  {command_name} \"optimize test runtime\" --command \"./bench.sh\" --metric total_us --direction lower",
    example = "Interactive setup:\n  {command_name} \"optimize test runtime\"",
    example = "Resume a session:\n  {command_name} --resume"
)]
struct ResearchCmd {
    /// optimization goal
    #[argh(positional, greedy)]
    goal: Vec<String>,

    /// benchmark command to run each iteration
    #[argh(option)]
    command: Option<String>,

    /// primary metric name (default: duration_s = wall-clock time)
    #[argh(option)]
    metric: Option<String>,

    /// optimization direction: lower or higher (default: lower)
    #[argh(option)]
    direction: Option<String>,

    /// files the agent may modify (comma-separated)
    #[argh(option)]
    scope: Option<String>,

    /// constraints (e.g. "cargo test must pass")
    #[argh(option)]
    constraints: Option<String>,

    /// maximum iterations before stopping
    #[argh(option)]
    max_iterations: Option<usize>,

    /// checks command to validate correctness after each benchmark
    #[argh(option)]
    checks: Option<String>,

    /// resume a previous research session
    #[argh(switch)]
    resume: bool,

    /// show current session status
    #[argh(switch)]
    status: bool,

    /// clear session files and start fresh
    #[argh(switch)]
    clear: bool,
}

// ── Exit handling ──

type CmdResult = Result<(), CmdError>;

#[derive(Debug)]
enum CmdError {
    Failure(String),
    Cancelled,
}

impl From<String> for CmdError {
    fn from(s: String) -> Self {
        CmdError::Failure(s)
    }
}

// ── Main ──

struct ChildGuard;

impl Drop for ChildGuard {
    fn drop(&mut self) {
        kill_all_children();
    }
}

fn main() {
    let _guard = ChildGuard;

    ctrlc::set_handler(|| {
        kill_all_children();
        std::process::exit(130);
    })
    .ok();

    let parsed = parse_args();
    let args = parsed.args;

    if args.version {
        println!("dex {}", REVISION);
        return;
    }

    if let Some(ref name) = args.cli {
        if let Err(e) = crate::runner::validate_cli_name(name) {
            err_msg(&e);
            exit(1);
        }
    }

    let command = match args.command {
        Some(cmd) => cmd,
        None => {
            print_help(&parsed.command_name);
            return;
        }
    };

    ensure_dex_dir();
    if args.update_prompts {
        seed_prompts(true);
        info("Prompt templates updated to built-in defaults.");
    } else {
        seed_prompts(false);
    }

    let cli_name = resolve_cli(args.cli);

    app_header();

    let timeout = Duration::from_secs(args.timeout);
    let runner = match Runner::new(&cli_name, timeout) {
        Ok(r) => r,
        Err(e) => {
            err_msg(&e);
            exit(1);
        }
    };

    save_config(&Config {
        cli: cli_name.clone(),
    });

    let result = match command {
        SubCommand::Plan(cmd) => cmd_plan(&runner, cmd),
        SubCommand::Import(cmd) => cmd_import(cmd),
        SubCommand::Amend(cmd) => cmd_amend(&runner, cmd),
        SubCommand::Apply(cmd) => cmd_apply(&runner, cmd),
        SubCommand::Review(cmd) => cmd_review(&runner, cmd),
        SubCommand::Bare(cmd) => cmd_bare(&runner, cmd),
        SubCommand::Finalize(cmd) => cmd_finalize(&runner, cmd),
        SubCommand::Research(cmd) => cmd_research(&runner, cmd),
    };

    match result {
        Ok(()) => {}
        Err(CmdError::Cancelled) => exit(2),
        Err(CmdError::Failure(msg)) => {
            err_msg(&msg);
            exit(1);
        }
    }
}

// ── Subcommand handlers ──

fn cmd_plan(runner: &Runner, cmd: PlanCmd) -> CmdResult {
    ensure_interactive_stdin("plan")?;

    let plan_path = dex_path("plan.md");
    let plan_exists = Path::new(&plan_path).exists();
    if plan_exists && !cmd.force {
        return Err(CmdError::Failure(
            "A plan already exists. Use `dex plan --force` to overwrite it.".into(),
        ));
    }
    if plan_exists && cmd.force {
        info("Clearing existing artifacts (plan, progress, feedbacks, reviews).");
        reset_dex_runtime_artifacts();
    }

    let request = read_text_or_file(cmd.request, "request")?;
    match plan_phase(runner, &request, Vec::new())? {
        Some(_) => {
            snapshot_review_base_ref();
            banner("DONE");
            info("Plan saved.");
            Ok(())
        }
        None => Err(CmdError::Cancelled),
    }
}

fn cmd_import(cmd: ImportCmd) -> CmdResult {
    let plan_path = dex_path("plan.md");
    let plan_exists = Path::new(&plan_path).exists();
    if plan_exists && !cmd.force {
        return Err(CmdError::Failure(
            "A plan already exists. Use `dex import --force` to overwrite it.".into(),
        ));
    }
    if plan_exists && cmd.force {
        info("Clearing existing artifacts (plan, progress, feedbacks, reviews).");
    }

    validate_candidate_plan(&cmd.file)?;
    let imported_plan = fs::read_to_string(&cmd.file)
        .map_err(|e| format!("read imported plan {:?}: {}", cmd.file, e))?;

    ensure_dex_dir();
    reset_dex_runtime_artifacts();
    fs::write(&plan_path, &imported_plan)
        .map_err(|e| format!("write imported plan to {}: {}", plan_path, e))?;
    save_plan_request(&format!("Imported plan from {}", cmd.file));
    snapshot_review_base_ref();

    banner("DONE");
    info(&format!("Plan imported from {}.", cmd.file));
    Ok(())
}

fn cmd_amend(runner: &Runner, cmd: AmendCmd) -> CmdResult {
    ensure_interactive_stdin("amend")?;

    let plan_path = dex_path("plan.md");
    if !Path::new(&plan_path).exists() {
        return Err(CmdError::Failure(
            "No plan exists. Use `dex plan` or `dex import` first.".into(),
        ));
    }

    let feedback = read_text_or_file(cmd.feedback, "feedback")?;
    let request = read_dex_file("request.txt").unwrap_or_else(|| "Amend the current plan.".into());
    let mut feedbacks = load_feedbacks();
    feedbacks.push(feedback);

    match plan_phase(runner, &request, feedbacks)? {
        Some(_) => {
            snapshot_review_base_ref();
            banner("DONE");
            info("Plan updated.");
            Ok(())
        }
        None => Err(CmdError::Cancelled),
    }
}

fn cmd_apply(runner: &Runner, _cmd: ApplyCmd) -> CmdResult {
    let plan_path = dex_path("plan.md");
    if !Path::new(&plan_path).exists() {
        return Err(CmdError::Failure(
            "No plan exists. Use `dex plan` or `dex import` first.".into(),
        ));
    }

    impl_phase(runner, &plan_path)?;
    banner("DONE");
    info("Implementation complete.");
    Ok(())
}

fn cmd_review(runner: &Runner, cmd: ReviewCmd) -> CmdResult {
    let plan_path = dex_path("plan.md");
    if !Path::new(&plan_path).exists() {
        return Err(CmdError::Failure(
            "No plan exists. Use `dex plan` or `dex import` first.".into(),
        ));
    }

    let review_ctx = load_review_context();
    if !review_ctx.git_available {
        warn("Review base ref is unavailable; review will run without git diff context.");
    }

    review_phase(
        runner,
        &plan_path,
        review_ctx.base_ref.as_deref(),
        review_ctx.git_available,
        cmd.parallel,
    )?;

    banner("DONE");
    info("Review complete.");
    Ok(())
}

fn cmd_bare(runner: &Runner, cmd: BareCmd) -> CmdResult {
    bare_phase(runner, &cmd.request_file, cmd.iterations)?;

    banner("DONE");
    info("Bare mode complete.");
    Ok(())
}

fn cmd_research(runner: &Runner, cmd: ResearchCmd) -> CmdResult {
    if cmd.clear {
        research::research_clear()?;
        return Ok(());
    }
    if cmd.status {
        research::research_status()?;
        return Ok(());
    }
    if cmd.resume {
        research::research_resume(runner, cmd.max_iterations)?;
        return Ok(());
    }

    let goal = cmd.goal.join(" ");
    let goal = goal.trim().to_string();
    if goal.is_empty() {
        return Err(CmdError::Failure(
            "missing goal; pass optimization goal as argument (e.g. dex research \"optimize test runtime\")".into(),
        ));
    }

    let config = if cmd.command.is_some() {
        research::ResearchConfig::new(
            goal,
            cmd.command.unwrap(),
            cmd.metric.unwrap_or_else(|| "duration_s".to_string()),
            String::new(),
            cmd.direction.unwrap_or_else(|| "lower".to_string()),
            cmd.scope.unwrap_or_else(|| "(all project files)".to_string()),
            cmd.constraints.unwrap_or_default(),
            cmd.checks,
        )
    } else if std::io::stdin().is_terminal() {
        research::interactive_setup(&goal)?
    } else {
        return Err(CmdError::Failure(
            "--command is required in non-interactive mode".into(),
        ));
    };

    research::research_new(runner, config, cmd.max_iterations)?;
    Ok(())
}

fn cmd_finalize(runner: &Runner, cmd: FinalizeCmd) -> CmdResult {
    let plan_path = dex_path("plan.md");
    finalize_phase(runner, &plan_path, &cmd.onto)?;

    banner("DONE");
    info("Finalize complete.");
    Ok(())
}

fn ensure_interactive_stdin(command: &str) -> CmdResult {
    if std::io::stdin().is_terminal() {
        return Ok(());
    }

    Err(CmdError::Failure(format!(
        "`dex {}` requires an interactive stdin; pass the {} as an argument or a file path instead.",
        command,
        if command == "amend" {
            "feedback"
        } else {
            "request"
        }
    )))
}

fn read_text_or_file(words: Vec<String>, kind: &str) -> Result<String, CmdError> {
    let raw = words.join(" ");
    let raw = raw.trim();
    if raw.is_empty() {
        return Err(CmdError::Failure(format!(
            "missing {}; pass {} text or a file path containing it.",
            kind, kind
        )));
    }

    let text = if Path::new(raw).is_file() {
        fs::read_to_string(raw).map_err(|e| format!("read {} file {:?}: {}", kind, raw, e))?
    } else {
        raw.to_string()
    };

    let text = text.trim().to_string();
    if text.is_empty() {
        return Err(CmdError::Failure(format!(
            "{} is empty after trimming.",
            kind
        )));
    }

    Ok(text)
}

fn resolve_cli(explicit: Option<String>) -> String {
    if let Some(name) = explicit {
        return name;
    }
    if Path::new(&dex_path("config.json")).exists() {
        return load_config().cli;
    }
    if let Some(first) = crate::runner::dex_available_agents().first() {
        return first.to_string();
    }
    Config::default().cli
}

fn snapshot_review_base_ref() {
    let review_base_ref = resolve_current_head_for_review();
    save_review_base_ref(review_base_ref.as_deref());
}

struct ReviewContext {
    base_ref: Option<String>,
    git_available: bool,
}

fn load_review_context() -> ReviewContext {
    if git_trimmed_output(&["rev-parse", "--is-inside-work-tree"]).is_err() {
        return ReviewContext {
            base_ref: None,
            git_available: false,
        };
    }

    let base_ref = load_review_base_ref().and_then(|base_ref| {
        git_trimmed_output(&["rev-parse", "--verify", &base_ref])
            .ok()
            .map(|_| base_ref)
    });

    ReviewContext {
        git_available: base_ref.is_some(),
        base_ref,
    }
}

fn resolve_current_head_for_review() -> Option<String> {
    if git_trimmed_output(&["rev-parse", "--is-inside-work-tree"]).is_err() {
        return None;
    }
    git_trimmed_output(&["rev-parse", "HEAD"]).ok()
}

// ── Arg parsing & help ──

struct ParsedArgs {
    args: Args,
    command_name: String,
}

fn parse_args() -> ParsedArgs {
    let strings: Vec<String> = std::env::args_os()
        .map(|s| s.into_string())
        .collect::<Result<Vec<_>, _>>()
        .unwrap_or_else(|arg| {
            eprintln!("Invalid utf8: {}", arg.to_string_lossy());
            exit(1)
        });

    if strings.is_empty() {
        eprintln!("No program name, argv is empty");
        exit(1);
    }

    let command_name = base_command_name(&strings[0]).to_string();
    let args: Vec<&str> = strings.iter().map(String::as_str).collect();

    let parsed = Args::from_args(&[&command_name], &args[1..]).unwrap_or_else(|early_exit| {
        exit(match early_exit.status {
            Ok(()) => {
                print!(
                    "{}",
                    render_help_output_with(
                        &early_exit.output,
                        &crate::runner::dex_available_agents(),
                    )
                );
                0
            }
            Err(()) => {
                eprintln!(
                    "{}\nRun {} --help for more information.",
                    early_exit.output, command_name
                );
                1
            }
        })
    });

    ParsedArgs {
        args: parsed,
        command_name,
    }
}

fn print_help(command_name: &str) {
    let help = top_level_help_output(command_name);
    print!(
        "{}",
        render_help_output_with(&help, &crate::runner::dex_available_agents(),)
    );
}

fn top_level_help_output(command_name: &str) -> String {
    match Args::from_args(&[command_name], &["--help"]) {
        Err(early_exit) => {
            debug_assert!(early_exit.status.is_ok());
            early_exit.output
        }
        Ok(_) => unreachable!("--help should trigger an early exit"),
    }
}

fn base_command_name(path: &str) -> &str {
    Path::new(path)
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or(path)
}

fn render_help_output_with(base_help: &str, available: &[&str]) -> String {
    let mut output = String::with_capacity(base_help.len() + 64);
    output.push_str(base_help);

    if !output.ends_with('\n') {
        output.push('\n');
    }
    output.push_str(&available_agents_help_section_with(available));
    output
}

fn available_agents_help_section_with(available: &[&str]) -> String {
    let mut section = String::from("\nAvailable agents:\n");
    if available.is_empty() {
        section.push_str("  none found in PATH\n");
        return section;
    }

    for agent in available {
        section.push_str("  ");
        section.push_str(agent);
        section.push('\n');
    }
    section
}

#[cfg(test)]
mod tests {
    use super::{
        available_agents_help_section_with, read_text_or_file, render_help_output_with,
        top_level_help_output, CmdError,
    };
    use std::fs;
    use std::path::PathBuf;

    fn write_temp_file(contents: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "dex-main-test-{}-{}.txt",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        fs::write(&path, contents).unwrap();
        path
    }

    #[test]
    fn help_section_lists_available_agents() {
        assert_eq!(
            available_agents_help_section_with(&["codex", "gemini"]),
            "\nAvailable agents:\n  codex\n  gemini\n"
        );
    }

    #[test]
    fn help_output_appends_available_agents_section() {
        assert_eq!(
            render_help_output_with("Usage: dex", &[]),
            "Usage: dex\n\nAvailable agents:\n  none found in PATH\n"
        );
    }

    #[test]
    fn help_output_appends_available_agents_after_options() {
        assert_eq!(
            render_help_output_with("Usage: dex\n\nOptions:\n  --help\n", &[]),
            "Usage: dex\n\nOptions:\n  --help\n\nAvailable agents:\n  none found in PATH\n"
        );
    }

    #[test]
    fn top_level_help_output_matches_real_help() {
        let help = top_level_help_output("dex");

        assert!(
            help.starts_with(
                "Usage: dex [--version] [--update-prompts] [--cli <name>] [--timeout <timeout>] [<command>] [<args>]"
            ),
            "unexpected help output: {help}"
        );
        assert!(help.contains("\nCommands:\n  plan"));
        assert!(!help.contains("Run `dex --help`"));
        assert!(!help.contains("\nExamples:\n"));
    }

    #[test]
    fn read_text_or_file_uses_trimmed_inline_text() {
        assert_eq!(
            read_text_or_file(vec!["  hello world  ".into()], "request").unwrap(),
            "hello world"
        );
    }

    #[test]
    fn read_text_or_file_loads_and_trims_file_contents() {
        let path = write_temp_file("  file request  \n");
        let value = read_text_or_file(vec![path.to_string_lossy().into_owned()], "request");
        let _ = fs::remove_file(&path);

        assert_eq!(value.unwrap(), "file request");
    }

    #[test]
    fn read_text_or_file_rejects_missing_input() {
        match read_text_or_file(Vec::new(), "feedback") {
            Err(CmdError::Failure(msg)) => assert_eq!(
                msg,
                "missing feedback; pass feedback text or a file path containing it."
            ),
            _ => panic!("expected failure"),
        }
    }

    #[test]
    fn read_text_or_file_rejects_empty_trimmed_file_contents() {
        let path = write_temp_file(" \n\t ");
        let value = read_text_or_file(vec![path.to_string_lossy().into_owned()], "request");
        let _ = fs::remove_file(&path);

        match value {
            Err(CmdError::Failure(msg)) => assert_eq!(msg, "request is empty after trimming."),
            _ => panic!("expected failure"),
        }
    }
}
