mod core;
mod phases;
mod plan;
mod runner;
mod ui;

use argh::FromArgs;
use std::process::{exit, Command};
use std::time::Duration;

use crate::core::{load_config, save_config, Config};
use crate::phases::{bare_phase, finalize_phase, impl_phase, plan_phase, review_phase};
use crate::runner::{Runner, CLI_CONFIGS};
use crate::ui::{banner, err_msg, info, prompt_multiline, warn, write_dim};

const REVISION: &str = env!("CARGO_PKG_VERSION");

/// dex — AI-powered development workflow
///
/// Normal mode plans, implements, and reviews a request.
/// `--finalize` is a separate rerun step for an already-implemented branch.
#[derive(FromArgs)]
struct Args {
    /// print version and exit
    #[argh(switch)]
    version: bool,

    /// coding CLI to use
    #[argh(option, default = "String::new()")]
    cli: String,

    /// use an existing plan file for this run only; new plans always write .dex/plan.md
    #[argh(option, default = "String::new()")]
    plan: String,

    /// skip the review phase
    #[argh(switch)]
    no_review: bool,

    /// base git ref: for review this is the diff base; with --finalize this is the exact ref to rebase onto
    #[argh(option, default = "String::new()")]
    base_ref: String,

    /// kill agent after this many seconds idle
    #[argh(option, default = "1200")]
    timeout: u64,

    /// bare mode: send request straight to agent for N iterations
    #[argh(option, short = 'b', default = "0")]
    b: usize,

    /// finalize an existing feature branch against its base branch; run this manually after implementation is done, optionally overriding the rebase target with --base-ref
    #[argh(switch)]
    finalize: bool,

    /// the request text
    #[argh(positional, greedy)]
    request: Vec<String>,
}

fn main() {
    let args: Args = argh::from_env();

    if args.version {
        println!("dex {}", REVISION);
        return;
    }

    let defaults = load_config();

    let cli_name = if args.cli.is_empty() {
        defaults.cli.clone()
    } else {
        args.cli
    };
    let plan_file = args.plan;
    let skip_review = args.no_review;
    let base_ref = if args.base_ref.is_empty() {
        configured_base_ref(&defaults.base_ref)
    } else {
        args.base_ref
    };
    let timeout = Duration::from_secs(args.timeout);
    let bare = args.b;
    let do_finalize = args.finalize;
    let mut request = args.request.join(" ");

    let mut stream = termcolor::StandardStream::stderr(termcolor::ColorChoice::Auto);
    write_dim(&mut stream, &format!("dex {}\n", REVISION));

    // Persist stable preferences only. Runtime HEAD snapshots are transient.
    save_config(&Config {
        cli: cli_name.clone(),
        base_ref: persisted_base_ref(&base_ref),
    });

    let runner = match Runner::new(&cli_name, timeout) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("{}", e);
            exit(1);
        }
    };

    // ── Bare mode ──
    if bare > 0 {
        if request.is_empty() {
            request = prompt_multiline("Enter your request:");
            if request.trim().is_empty() {
                print_usage();
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
        let plan_path = if plan_file.is_empty() {
            crate::core::dex_path("plan.md")
        } else {
            plan_file.clone()
        };
        if let Err(e) = finalize_phase(&runner, &plan_path, &base_ref) {
            err_msg(&e.to_string());
            exit(1);
        }
        banner("DONE");
        info("Finalize complete.");
        return;
    }

    // ── Standard guided mode ──
    if request.is_empty() && plan_file.is_empty() {
        request = prompt_multiline("Enter your request:");
        if request.trim().is_empty() {
            print_usage();
            exit(1);
        }
    }

    // Phase 1: Planning
    let plan_path = if plan_file.is_empty() {
        match plan_phase(&runner, &request) {
            Ok(Some(p)) => p,
            Ok(None) => exit(0),
            Err(e) => {
                err_msg(&e.to_string());
                exit(1);
            }
        }
    } else {
        plan_file.clone()
    };

    // Phase 2: Implementation
    if let Err(e) = impl_phase(&runner, &plan_path) {
        err_msg(&e.to_string());
        exit(1);
    }

    // Phase 3: Review
    if !skip_review {
        let review_ctx = resolve_review_context(&base_ref);
        if !review_ctx.git_available {
            warn(
                "Git metadata is unavailable; review will run in best-effort mode without git diff context.",
            );
        }
        if let Err(e) = review_phase(
            &runner,
            &plan_path,
            review_ctx.base_ref.as_deref(),
            review_ctx.git_available,
        ) {
            err_msg(&e.to_string());
            exit(1);
        }
    }

    banner("DONE");
    info("All phases complete.");
}

fn print_usage() {
    eprintln!("Usage: dex [flags] <request...>");
    eprintln!(
        "\nSupported CLIs: {}",
        CLI_CONFIGS
            .iter()
            .map(|(k, _)| *k)
            .collect::<Vec<_>>()
            .join(", ")
    );
    eprintln!("\nFinalize usage:");
    eprintln!("  dex --finalize [--base-ref <ref>] [--plan <path>]");
    eprintln!("  Run this on a feature branch after implementation is committed.");
    eprintln!(
        "  Default target resolution: origin/HEAD, then origin/main, origin/master, main, master."
    );
    eprintln!("  Use --base-ref to choose the exact rebase target explicitly.");
}

fn configured_base_ref(default_base_ref: &str) -> String {
    if looks_like_full_git_oid(default_base_ref) || default_base_ref.trim().is_empty() {
        "HEAD".to_string()
    } else {
        default_base_ref.to_string()
    }
}

fn persisted_base_ref(base_ref: &str) -> String {
    if looks_like_full_git_oid(base_ref) || base_ref.trim().is_empty() {
        "HEAD".to_string()
    } else {
        base_ref.to_string()
    }
}

struct ReviewContext {
    base_ref: Option<String>,
    git_available: bool,
}

fn resolve_review_context(base_ref: &str) -> ReviewContext {
    if git_trimmed_output(&["rev-parse", "--is-inside-work-tree"]).is_err() {
        return ReviewContext {
            base_ref: None,
            git_available: false,
        };
    }

    let resolved = if base_ref == "HEAD" {
        git_trimmed_output(&["rev-parse", "HEAD"]).ok()
    } else {
        Some(base_ref.to_string())
    };

    ReviewContext {
        git_available: resolved.is_some(),
        base_ref: resolved,
    }
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

fn looks_like_full_git_oid(value: &str) -> bool {
    value.len() == 40 && value.bytes().all(|b| b.is_ascii_hexdigit())
}
