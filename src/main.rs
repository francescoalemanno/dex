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
    dex_path, ensure_dex_dir, load_config, load_review_base_ref, reset_dex_runtime_artifacts,
    save_config, save_review_base_ref, Config,
};
use crate::phases::{
    bare_phase, finalize_phase, impl_phase, plan_phase, review_phase, PlanPhaseMode,
};
use crate::plan::validate_candidate_plan;
use crate::runner::Runner;
use crate::ui::{banner, err_msg, info, prompt_choice, prompt_multiline, warn, write_dim};

const REVISION: &str = env!("CARGO_PKG_VERSION");
const IMPORT_PLAN_CHOICE_PROMPT: &str =
    "A plan already exists in .dex/plan.md. Should dex replace it or quit?";
const HELP_EXAMPLES_SECTION: &str = concat!(
    "\nExamples:\n",
    "  Normal guided run:\n",
    "    dex \"add structured logging to the API and update tests\"\n",
    "  Resume a saved plan:\n",
    "    dex\n",
    "  Execute an imported plan:\n",
    "    dex --plan myplan.md\n",
    "  Open-ended bare loop:\n",
    "    dex --bare 10 \"explore the repo and improve test coverage\"\n",
    "  Finalize a feature branch:\n",
    "    dex --finalize main\n",
);

/// dex — AI-powered development workflow
///
/// Normal mode plans, implements, and reviews a request.
/// `--finalize` is a separate rerun step for an already-implemented branch.
#[derive(FromArgs)]
struct Args {
    /// print version and exit
    #[argh(switch)]
    version: bool,

    /// coding CLI to use; must be available in PATH
    #[argh(option, arg_name = "name", from_str_fn(parse_cli_name))]
    cli: Option<String>,

    /// kill agent after this many seconds idle
    #[argh(option, default = "1200")]
    timeout: u64,

    /// bare mode: send request straight to agent for N iterations
    #[argh(option, default = "0")]
    bare: usize,

    /// finalize an existing feature branch against a user-provided rebase target
    #[argh(switch)]
    finalize: bool,

    /// import a markdown plan; with an extra request, revise the imported draft before execution
    #[argh(option, arg_name = "file.md")]
    plan: Option<String>,

    /// request text, or the rebase target when used with --finalize
    #[argh(positional, greedy)]
    request: Vec<String>,
}

fn main() {
    let args = parse_args();

    if args.version {
        println!("dex {}", REVISION);
        return;
    }

    let defaults = load_config();

    let mut cli_name = args.cli.unwrap_or_else(|| defaults.cli.clone());
    let available_agents = crate::runner::dex_available_agents();
    let original_cli_name = cli_name.clone();
    cli_name = match normalize_active_cli(&cli_name, &available_agents) {
        Ok(name) => name,
        Err(e) => {
            eprintln!("{}", e);
            exit(1);
        }
    };
    if cli_name != original_cli_name {
        warn(&format!(
            "Active CLI {:?} is unavailable; switching to {:?}.",
            original_cli_name, cli_name
        ));
    }
    let timeout = Duration::from_secs(args.timeout);
    let bare = args.bare;
    let do_finalize = args.finalize;
    let imported_plan = args.plan;
    let positionals = args.request;

    if imported_plan.is_some() && bare > 0 {
        err_msg("--plan cannot be used together with --bare");
        exit(1);
    }
    if imported_plan.is_some() && do_finalize {
        err_msg("--plan cannot be used together with --finalize");
        exit(1);
    }

    let mut request = if do_finalize {
        String::new()
    } else {
        positionals.join(" ")
    };
    let default_plan_path = dex_path("plan.md");
    let resume_existing_plan = imported_plan.is_none()
        && request.trim().is_empty()
        && Path::new(&default_plan_path).exists();

    let mut stream = termcolor::StandardStream::stderr(termcolor::ColorChoice::Auto);
    write_dim(&mut stream, &format!("dex {}\n", REVISION));

    let runner = match Runner::new(&cli_name, timeout) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("{}", e);
            exit(1);
        }
    };

    // Persist stable preferences only after validating the CLI name.
    save_config(&Config {
        cli: cli_name.clone(),
    });

    // ── Bare mode ──
    if bare > 0 {
        if request.is_empty() {
            request = prompt_multiline("Enter your request:");
            if request.trim().is_empty() {
                exit(1);
            }
        }
        if let Err(e) = bare_phase(&runner, &request, bare) {
            err_msg(&e.to_string());
            exit(1);
        }
        banner("DONE");
        info("Bare mode complete.");
        return;
    }

    // ── Finalize-only mode ──
    if do_finalize {
        let finalize_target = match parse_finalize_target(&positionals) {
            Ok(target) => target,
            Err(e) => {
                err_msg(&e);
                exit(1);
            }
        };
        if let Err(e) = finalize_phase(&runner, &default_plan_path, &finalize_target) {
            err_msg(&e.to_string());
            exit(1);
        }
        banner("DONE");
        info("Finalize complete.");
        return;
    }

    // ── Standard guided mode ──
    if imported_plan.is_none() && request.trim().is_empty() && !resume_existing_plan {
        request = prompt_multiline("Enter your request:");
        if request.trim().is_empty() {
            exit(1);
        }
    }

    // Phase 1: Planning
    let (plan_path, refresh_review_base_ref) = if let Some(candidate_path) =
        imported_plan.as_deref()
    {
        let imported_plan_path = match import_plan(candidate_path) {
            Ok(Some(plan_path)) => plan_path,
            Ok(None) => exit(0),
            Err(e) => {
                err_msg(&e);
                exit(1);
            }
        };

        if request.trim().is_empty() {
            info(&format!(
                "Starting from imported plan at {}",
                imported_plan_path
            ));
            (imported_plan_path, true)
        } else {
            info("Imported plan will be revised against the provided request before execution.");
            match plan_phase(&runner, &request, PlanPhaseMode::ReviseImportedDraft) {
                Ok(Some(result)) => (result.plan_path, result.created_new_plan),
                Ok(None) => exit(0),
                Err(e) => {
                    err_msg(&e.to_string());
                    exit(1);
                }
            }
        }
    } else if resume_existing_plan {
        info(&format!("Resuming existing plan at {}", default_plan_path));
        (default_plan_path, false)
    } else {
        match plan_phase(&runner, &request, PlanPhaseMode::Standard) {
            Ok(Some(result)) => (result.plan_path, result.created_new_plan),
            Ok(None) => exit(0),
            Err(e) => {
                err_msg(&e.to_string());
                exit(1);
            }
        }
    };

    if let Err(e) = run_guided_workflow(&runner, &plan_path, refresh_review_base_ref) {
        err_msg(&e.to_string());
        exit(1);
    }

    banner("DONE");
    info("All phases complete.");
}

fn import_plan(candidate_path: &str) -> Result<Option<String>, String> {
    validate_candidate_plan(candidate_path)?;
    let imported_plan = fs::read_to_string(candidate_path)
        .map_err(|e| format!("read imported plan {:?}: {}", candidate_path, e))?;
    let plan_path = dex_path("plan.md");

    if Path::new(&plan_path).exists() {
        let choice = prompt_choice(IMPORT_PLAN_CHOICE_PROMPT, &["replace", "quit"]);
        if choice == "quit" {
            info("Exiting without changing the current plan.");
            return Ok(None);
        }
    }

    ensure_dex_dir();
    reset_dex_runtime_artifacts();
    fs::write(&plan_path, imported_plan)
        .map_err(|e| format!("write imported plan to {}: {}", plan_path, e))?;
    Ok(Some(plan_path))
}

fn run_guided_workflow(
    runner: &Runner,
    plan_path: &str,
    refresh_review_base_ref: bool,
) -> Result<(), String> {
    // Snapshot the review base BEFORE implementation so review diffs cover impl commits.
    if refresh_review_base_ref {
        ensure_review_base_ref_snapshot();
    }

    impl_phase(runner, plan_path)?;

    let review_ctx = load_review_context();
    if !review_ctx.git_available {
        warn(
            "Review base metadata is unavailable; review will run in best-effort mode without git diff context.",
        );
    }

    review_phase(
        runner,
        plan_path,
        review_ctx.base_ref.as_deref(),
        review_ctx.git_available,
    )
}

fn normalize_active_cli(preferred: &str, available: &[&str]) -> Result<String, String> {
    if available.contains(&preferred) {
        return Ok(preferred.to_string());
    }

    available
        .first()
        .map(|candidate| (*candidate).to_string())
        .ok_or_else(|| {
            format!(
                "Active CLI {:?} is unavailable and no supported agents were found in PATH.",
                preferred
            )
        })
}

fn parse_cli_name(value: &str) -> Result<String, String> {
    crate::runner::validate_cli_name(value)?;
    Ok(value.to_string())
}

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
        available_agents_help_section_with, normalize_active_cli, parse_finalize_target,
        render_help_output_with, HELP_EXAMPLES_SECTION,
    };

    #[test]
    fn finalize_requires_exactly_one_target() {
        assert_eq!(
            parse_finalize_target(&["origin/main".to_string()]).unwrap(),
            "origin/main"
        );
        assert!(parse_finalize_target(&[]).is_err());
        assert!(parse_finalize_target(&["main".to_string(), "extra".to_string()]).is_err());
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

    #[test]
    fn keeps_active_cli_when_it_is_available() {
        assert_eq!(
            normalize_active_cli("codex", &["codex", "gemini"]).unwrap(),
            "codex"
        );
    }

    #[test]
    fn switches_active_cli_to_first_available() {
        assert_eq!(
            normalize_active_cli("claude", &["codex", "gemini"]).unwrap(),
            "codex"
        );
    }

    #[test]
    fn errors_when_no_agents_are_available() {
        assert_eq!(
            normalize_active_cli("claude", &[]).unwrap_err(),
            "Active CLI \"claude\" is unavailable and no supported agents were found in PATH."
        );
    }
}

struct ReviewContext {
    base_ref: Option<String>,
    git_available: bool,
}

fn parse_finalize_target(args: &[String]) -> Result<String, String> {
    match args {
        [target] if !target.trim().is_empty() => Ok(target.clone()),
        [] => {
            Err("finalize requires a rebase target: dex --finalize <target-for-rebase>".to_string())
        }
        _ => Err(
            "finalize accepts exactly one rebase target: dex --finalize <target-for-rebase>"
                .to_string(),
        ),
    }
}

fn ensure_review_base_ref_snapshot() {
    if load_review_base_ref().is_some() {
        return;
    }

    let review_base_ref = resolve_current_head_for_review();
    save_review_base_ref(review_base_ref.as_deref());
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
