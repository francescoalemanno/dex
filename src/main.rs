mod core;
mod phases;
mod plan;
mod runner;
mod ui;

use argh::FromArgs;
use std::fs;
use std::path::Path;
use std::process::{exit, Command};
use std::time::Duration;

use crate::core::{
    dex_path, ensure_dex_dir, load_config, load_feedbacks, load_review_base_ref, read_dex_file,
    reset_dex_runtime_artifacts, save_config, save_review_base_ref, seed_prompts, Config,
};
use crate::phases::{bare_phase, finalize_phase, impl_phase, plan_phase, review_phase};
use crate::plan::validate_candidate_plan;
use crate::runner::Runner;
use crate::ui::{banner, err_msg, info, prompt_choice, prompt_multiline, warn, write_dim};

const REVISION: &str = env!("CARGO_PKG_VERSION");

const HELP_EXAMPLES_SECTION: &str = concat!(
    "\nExamples:\n",
    "  New guided run:\n",
    "    dex run \"add structured logging to the API and update tests\"\n",
    "  Resume a saved plan:\n",
    "    dex resume\n",
    "  Revise an existing plan:\n",
    "    dex revise \"use a different database library\"\n",
    "  Open-ended bare loop:\n",
    "    dex bare 10 \"explore the repo and improve test coverage\"\n",
    "  Finalize a feature branch:\n",
    "    dex finalize --onto main\n",
    "  Execute an imported plan:\n",
    "    dex import myplan.md\n",
    "  Import and revise before execution:\n",
    "    dex import myplan.md --revise \"adjust the testing approach\"\n",
);

// ── CLI argument definitions ──

/// dex — AI-powered development workflow
#[derive(FromArgs)]
struct Args {
    /// print version and exit
    #[argh(switch)]
    version: bool,

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
    Run(RunCmd),
    Resume(ResumeCmd),
    Revise(ReviseCmd),
    Bare(BareCmd),
    Finalize(FinalizeCmd),
    Import(ImportCmd),
}

/// plan, implement, and review a request
#[derive(FromArgs)]
#[argh(subcommand, name = "run")]
struct RunCmd {
    /// overwrite an existing plan and start fresh
    #[argh(switch)]
    force: bool,

    /// request text
    #[argh(positional, greedy)]
    request: Vec<String>,
}

/// resume an existing plan from where it left off
#[derive(FromArgs)]
#[argh(subcommand, name = "resume")]
struct ResumeCmd {}

/// revise an existing plan with new feedback, then continue
#[derive(FromArgs)]
#[argh(subcommand, name = "revise")]
struct ReviseCmd {
    /// revision feedback
    #[argh(positional, greedy)]
    feedback: Vec<String>,
}

/// send a request straight to the agent for N iterations
#[derive(FromArgs)]
#[argh(subcommand, name = "bare")]
struct BareCmd {
    /// number of iterations
    #[argh(positional)]
    iterations: usize,

    /// request text
    #[argh(positional, greedy)]
    request: Vec<String>,
}

/// finalize a feature branch against a rebase target
#[derive(FromArgs)]
#[argh(subcommand, name = "finalize")]
struct FinalizeCmd {
    /// rebase target (e.g. main, origin/main)
    #[argh(option)]
    onto: String,
}

/// import a markdown plan and execute it
#[derive(FromArgs)]
#[argh(subcommand, name = "import")]
struct ImportCmd {
    /// path to the markdown plan file
    #[argh(positional)]
    file: String,

    /// optional revision feedback; revise the imported plan before execution
    #[argh(option)]
    revise: Option<String>,
}

// ── Exit handling ──

type CmdResult = Result<(), CmdError>;

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

fn main() {
    let args = parse_args();

    if args.version {
        println!("dex {}", REVISION);
        return;
    }

    let command = match args.command {
        Some(cmd) => cmd,
        None => {
            print_help();
            return;
        }
    };

    ensure_dex_dir();
    seed_prompts();

    let cli_name = resolve_cli(args.cli);

    let mut stream = termcolor::StandardStream::stderr(termcolor::ColorChoice::Auto);
    write_dim(&mut stream, &format!("dex {}\n", REVISION));

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
        SubCommand::Run(cmd) => cmd_run(&runner, cmd),
        SubCommand::Resume(_) => cmd_resume(&runner),
        SubCommand::Revise(cmd) => cmd_revise(&runner, cmd),
        SubCommand::Bare(cmd) => cmd_bare(&runner, cmd),
        SubCommand::Finalize(cmd) => cmd_finalize(&runner, cmd),
        SubCommand::Import(cmd) => cmd_import(&runner, cmd),
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

fn cmd_run(runner: &Runner, cmd: RunCmd) -> CmdResult {
    let plan_path = dex_path("plan.md");

    if Path::new(&plan_path).exists() && !cmd.force {
        return Err(CmdError::Failure(
            "A plan already exists. Use `dex resume`, `dex revise`, or `dex run --force`.".into(),
        ));
    }

    if cmd.force && Path::new(&plan_path).exists() {
        info("Clearing existing artifacts (plan, progress, feedbacks, reviews).");
        reset_dex_runtime_artifacts();
    }

    preflight_git()?;

    let mut request = cmd.request.join(" ");
    if request.trim().is_empty() {
        request = prompt_multiline("Enter your request:");
        if request.trim().is_empty() {
            return Err(CmdError::Cancelled);
        }
    }

    ensure_review_base_ref_snapshot();

    match plan_phase(runner, &request, Vec::new())? {
        Some(_) => {}
        None => return Err(CmdError::Cancelled),
    }

    impl_phase(runner, &plan_path)?;
    run_review(runner, &plan_path)?;

    banner("DONE");
    info("All phases complete.");
    Ok(())
}

fn cmd_resume(runner: &Runner) -> CmdResult {
    let plan_path = dex_path("plan.md");

    if !Path::new(&plan_path).exists() {
        return Err(CmdError::Failure(
            "No plan exists. Use `dex run` to start a new workflow.".into(),
        ));
    }

    preflight_git()?;

    info(&format!("Resuming existing plan at {}", plan_path));

    impl_phase(runner, &plan_path)?;
    run_review(runner, &plan_path)?;

    banner("DONE");
    info("All phases complete.");
    Ok(())
}

fn cmd_revise(runner: &Runner, cmd: ReviseCmd) -> CmdResult {
    let plan_path = dex_path("plan.md");

    if !Path::new(&plan_path).exists() {
        return Err(CmdError::Failure(
            "No plan exists to revise. Use `dex run` to start a new workflow.".into(),
        ));
    }

    preflight_git()?;

    let mut feedback = cmd.feedback.join(" ");
    if feedback.trim().is_empty() {
        feedback = prompt_multiline("Enter your revision feedback:");
        if feedback.trim().is_empty() {
            return Err(CmdError::Cancelled);
        }
    }

    let request = read_dex_file("request.txt").unwrap_or_else(|| feedback.clone());
    let mut feedbacks = load_feedbacks();
    feedbacks.push(feedback);

    match plan_phase(runner, &request, feedbacks)? {
        Some(_) => {}
        None => return Err(CmdError::Cancelled),
    }

    impl_phase(runner, &plan_path)?;
    run_review(runner, &plan_path)?;

    banner("DONE");
    info("All phases complete.");
    Ok(())
}

fn cmd_bare(runner: &Runner, cmd: BareCmd) -> CmdResult {
    let mut request = cmd.request.join(" ");
    if request.trim().is_empty() {
        request = prompt_multiline("Enter your request:");
        if request.trim().is_empty() {
            return Err(CmdError::Cancelled);
        }
    }

    bare_phase(runner, &request, cmd.iterations)?;

    banner("DONE");
    info("Bare mode complete.");
    Ok(())
}

fn cmd_finalize(runner: &Runner, cmd: FinalizeCmd) -> CmdResult {
    let plan_path = dex_path("plan.md");

    finalize_phase(runner, &plan_path, &cmd.onto)?;

    banner("DONE");
    info("Finalize complete.");
    Ok(())
}

fn cmd_import(runner: &Runner, cmd: ImportCmd) -> CmdResult {
    let plan_path = dex_path("plan.md");

    validate_candidate_plan(&cmd.file)?;
    let imported_plan = fs::read_to_string(&cmd.file)
        .map_err(|e| format!("read imported plan {:?}: {}", cmd.file, e))?;

    if Path::new(&plan_path).exists() {
        let choice = prompt_choice(
            "A plan already exists. Replace it or quit?",
            &["replace", "quit"],
        );
        if choice == "quit" {
            return Err(CmdError::Cancelled);
        }
    }

    ensure_dex_dir();
    reset_dex_runtime_artifacts();
    fs::write(&plan_path, &imported_plan)
        .map_err(|e| format!("write imported plan to {}: {}", plan_path, e))?;

    preflight_git()?;
    ensure_review_base_ref_snapshot();

    if let Some(revision) = cmd.revise {
        info("Imported plan will be revised before execution.");
        match plan_phase(runner, &revision, Vec::new())? {
            Some(_) => {}
            None => return Err(CmdError::Cancelled),
        }
    } else {
        info(&format!("Starting from imported plan at {}", cmd.file));
    }

    impl_phase(runner, &plan_path)?;
    run_review(runner, &plan_path)?;

    banner("DONE");
    info("All phases complete.");
    Ok(())
}

// ── Shared helpers ──

fn preflight_git() -> CmdResult {
    if git_trimmed_output(&["rev-parse", "--is-inside-work-tree"]).is_ok() {
        return Ok(());
    }
    warn("Not inside a git repository. Review will run without diff context.");
    let choice = prompt_choice("Continue anyway?", &["yes", "no"]);
    if choice == "no" {
        return Err(CmdError::Cancelled);
    }
    Ok(())
}

fn run_review(runner: &Runner, plan_path: &str) -> Result<(), String> {
    let review_ctx = load_review_context();
    if !review_ctx.git_available {
        warn("Review base ref is unavailable; review will run without git diff context.");
    }

    review_phase(
        runner,
        plan_path,
        review_ctx.base_ref.as_deref(),
        review_ctx.git_available,
    )
}

fn resolve_cli(explicit: Option<String>) -> String {
    explicit.unwrap_or_else(|| load_config().cli)
}

fn ensure_review_base_ref_snapshot() {
    if load_review_base_ref().is_some() {
        return;
    }

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

fn git_trimmed_output(args: &[&str]) -> Result<String, String> {
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

// ── Arg parsing & help ──

fn parse_args() -> Args {
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

    let command_name = base_command_name(&strings[0]);
    let args: Vec<&str> = strings.iter().map(String::as_str).collect();

    Args::from_args(&[command_name], &args[1..]).unwrap_or_else(|early_exit| {
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
    })
}

fn print_help() {
    let help = format!("dex {}\n\nRun `dex --help` for usage.", REVISION);
    print!(
        "{}",
        render_help_output_with(&help, &crate::runner::dex_available_agents(),)
    );
}

fn base_command_name(path: &str) -> &str {
    Path::new(path)
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or(path)
}

fn render_help_output_with(base_help: &str, available: &[&str]) -> String {
    let mut output = String::with_capacity(base_help.len() + HELP_EXAMPLES_SECTION.len());
    output.push_str(base_help);
    if !output.ends_with('\n') {
        output.push('\n');
    }
    output.push_str(HELP_EXAMPLES_SECTION);

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
        available_agents_help_section_with, render_help_output_with, HELP_EXAMPLES_SECTION,
    };

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
            format!(
                "Usage: dex\n{}\nAvailable agents:\n  none found in PATH\n",
                HELP_EXAMPLES_SECTION
            )
        );
    }

    #[test]
    fn help_output_appends_examples_after_options() {
        assert_eq!(
            render_help_output_with("Usage: dex\n\nOptions:\n  --help\n", &[]),
            format!(
                "Usage: dex\n\nOptions:\n  --help\n{}\nAvailable agents:\n  none found in PATH\n",
                HELP_EXAMPLES_SECTION
            )
        );
    }
}
