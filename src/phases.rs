use similar::TextDiff;
use std::fs;
use std::process::Command;

use crate::core::{
    clear_plan_state, dex_path, ensure_dex_dir, load_feedbacks, read_dex_file, remove_dex_file,
    render_prompt, save_feedbacks, save_plan_request,
};
use crate::plan::{all_tasks_done, next_open_task};
use crate::runner::Runner;
use crate::ui::{
    banner, err_msg, info, prompt_choice, prompt_multiline, show_block, show_markdown, warn,
};

// ── Phase 1: Planning ──

pub fn plan_phase(r: &Runner, user_input: &str) -> Result<Option<String>, String> {
    banner("PLANNING");
    ensure_dex_dir();

    let mut feedbacks: Vec<String> = Vec::new();
    let mut request = user_input.to_string();
    let plan_path = dex_path("plan.md");

    if let Some(existing) = read_dex_file("plan.md") {
        show_markdown("Existing plan", &existing);
        let choice = prompt_choice(
            "Is your request a revision of this plan, or a new plan?",
            &["revise", "new"],
        );
        match choice.as_str() {
            "new" => clear_plan_state(),
            "revise" => {
                if let Some(orig) = read_dex_file("request.txt") {
                    request = orig;
                }
                feedbacks = load_feedbacks();
                feedbacks.push(user_input.to_string());
            }
            _ => {}
        }
    }

    save_plan_request(&request);
    save_feedbacks(&feedbacks);

    let mut iteration = 1;
    loop {
        info(&format!("Planning iteration {}", iteration));
        iteration += 1;

        remove_dex_file("questions.md");

        let fb_values: Vec<serde_json::Value> = feedbacks
            .iter()
            .map(|s| serde_json::Value::String(s.clone()))
            .collect();
        let p = render_prompt(
            "plan.txt",
            &serde_json::json!({
                "Request": request,
                "Feedbacks": fb_values,
            }),
        );

        if let Err(e) = r.run(&p) {
            err_msg(&format!("CLI error: {}", e));
            return Err(format!("planning failed after automatic retries: {}", e));
        }

        if let Some(questions) = read_dex_file("questions.md") {
            show_block("Questions from CLI", &questions);
            let answer = prompt_multiline("Your answers:");
            feedbacks.push(format!("Questions:\n{}\n\nAnswers:\n{}", questions, answer));
            save_feedbacks(&feedbacks);
            continue;
        }

        let plan = match read_dex_file("plan.md") {
            Some(p) => p,
            None => {
                warn("CLI did not produce a plan or questions. Retrying...");
                feedbacks.push(
                    "You did not produce a plan in .dex/plan.md or questions in .dex/questions.md. Please do so."
                        .to_string(),
                );
                save_feedbacks(&feedbacks);
                continue;
            }
        };

        show_markdown("Plan", &plan);

        let choice = prompt_choice(
            "Accept, edit, revise, or reject?",
            &["accept", "edit", "revise", "reject"],
        );
        match choice.as_str() {
            "accept" => {
                info("Plan accepted!");
                return Ok(Some(plan_path));
            }
            "reject" => {
                warn("Plan rejected.");
                return Ok(None);
            }
            "edit" => {
                match edit_plan_in_editor(&plan) {
                    Ok(Some(diff)) => {
                        let feedback = format!(
                            "user provided feedback in the form of a unified diff: \n\n{}",
                            diff
                        );
                        feedbacks.push(feedback);
                        save_feedbacks(&feedbacks);
                    }
                    Ok(None) => {} // no changes
                    Err(e) => {
                        err_msg(&format!("Editor error: {}", e));
                    }
                }
            }
            "revise" => {
                let feedback = prompt_multiline("Your revision feedback:");
                feedbacks.push(feedback);
                save_feedbacks(&feedbacks);
            }
            _ => {}
        }
    }
}

fn editor_cmd() -> String {
    std::env::var("VISUAL")
        .or_else(|_| std::env::var("EDITOR"))
        .unwrap_or_else(|_| "vi".to_string())
}

fn edit_plan_in_editor(plan: &str) -> Result<Option<String>, String> {
    let tmp_path = std::env::temp_dir().join(format!(
        "dex-plan-{}-{}.md",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    fs::write(&tmp_path, plan).map_err(|e| format!("write temp file: {}", e))?;

    let editor = editor_cmd();
    let mut parts = editor.split_whitespace();
    let cmd = parts.next().unwrap_or("vi");
    let editor_args: Vec<&str> = parts.collect();
    let status = Command::new(cmd)
        .args(&editor_args)
        .arg(&tmp_path)
        .stdin(std::process::Stdio::inherit())
        .stdout(std::process::Stdio::inherit())
        .stderr(std::process::Stdio::inherit())
        .status()
        .map_err(|e| format!("editor {:?}: {}", editor, e))?;

    if !status.success() {
        return Err(format!("editor {:?} exited with {}", editor, status));
    }

    let edited = fs::read_to_string(&tmp_path).map_err(|e| format!("read temp file: {}", e))?;
    let _ = fs::remove_file(&tmp_path);

    if edited == plan {
        warn("No changes detected.");
        return Ok(None);
    }

    let text_diff = TextDiff::from_lines(plan, &edited);
    let unified = format!(
        "{}",
        text_diff
            .unified_diff()
            .context_radius(5)
            .header("original", "edited")
    );

    Ok(Some(unified))
}

// ── Phase 2: Implementation ──

pub fn impl_phase(r: &Runner, plan_path: &str) -> Result<(), String> {
    banner("IMPLEMENTATION");

    let mut iteration = 1;
    loop {
        let task = next_open_task(plan_path)?;
        let task = match task {
            Some(t) => t,
            None => {
                info("All tasks complete!");
                return Ok(());
            }
        };

        let header = if task.header.is_empty() {
            "(unnamed task)".to_string()
        } else {
            task.header.clone()
        };
        info(&format!(
            "Iteration {} — working on: {} ({}/{} steps open)",
            iteration,
            header,
            task.open,
            task.open + task.done
        ));

        let p = render_prompt(
            "impl.txt",
            &serde_json::json!({
                "PlanPath": plan_path,
                "TaskHeader": task.header,
                "TaskBody": task.body(),
            }),
        );

        if let Err(e) = r.run(&p) {
            err_msg(&format!("CLI error: {}", e));
            return Err(format!(
                "implementation failed after automatic retries: {}",
                e
            ));
        }

        if all_tasks_done(plan_path)? {
            info("All tasks complete!");
            return Ok(());
        }

        iteration += 1;
    }
}

// ── Phase 3: Review ──

struct ReviewRole {
    name: &'static str,
    scope: &'static str,
    prompt: &'static str,
}

static BROAD_REVIEWERS: &[ReviewRole] = &[
    ReviewRole {
        name: "quality",
        scope: "bugs, security, correctness, simplicity",
        prompt: "Focus on:\n- logic errors\n- edge cases\n- error handling\n- resource management\n- concurrency issues\n- input validation and security issues\n- unnecessary abstraction or over-engineering when a simpler solution would work",
    },
    ReviewRole {
        name: "implementation",
        scope: "goal coverage, wiring, completeness, logic flow",
        prompt: "Focus on:\n- requirement coverage — does the code actually achieve the plan's goal?\n- correctness of the chosen approach\n- wiring and integration between components\n- completeness — are any requirements missing?\n- logic flow and edge cases",
    },
    ReviewRole {
        name: "simplification",
        scope: "unnecessary complexity, over-engineering",
        prompt: "Focus on:\n- excessive abstraction layers\n- premature generalization\n- unnecessary indirection\n- unused extension points\n- unnecessary fallbacks\n- premature optimization",
    },
    ReviewRole {
        name: "testing",
        scope: "coverage, test quality, edge cases",
        prompt: "Focus on:\n- missing tests for changed code\n- untested error paths\n- weak assertions\n- fake tests that do not verify behavior\n- missing edge-case coverage\n- test independence",
    },
    ReviewRole {
        name: "documentation",
        scope: "README, internal docs, plan alignment",
        prompt: "Focus on:\n- missing README updates for new features, flags, configuration, APIs, or changed behavior\n- missing internal documentation updates for new patterns, commands, or architecture\n- plan file drift that should be corrected while addressing documentation gaps",
    },
];

static FOCUSED_REVIEWERS: &[ReviewRole] = &[
    ReviewRole {
        name: "quality",
        scope: "critical and major correctness, security, reliability",
        prompt: "Review code only for critical and major bugs, security issues, and correctness problems.\nIgnore style issues and minor suggestions.\nFocus on:\n- logic errors that cause incorrect behavior\n- security vulnerabilities\n- data loss or corruption risks\n- concurrency bugs",
    },
    ReviewRole {
        name: "implementation",
        scope: "critical and major goal coverage, integration, completeness",
        prompt: "Review whether any critical or major requirement-coverage or integration issues remain.\nIgnore style issues and minor suggestions.\nFocus on:\n- requirements that are not implemented at all\n- integration bugs between components\n- critical logic flow errors",
    },
];

const MAX_FOCUSED_ROUNDS: usize = 3;

pub fn review_phase(
    r: &Runner,
    plan_path: &str,
    base_ref: Option<&str>,
    git_available: bool,
) -> Result<(), String> {
    // Broad pass: all reviewers, once
    let issues = run_review_fanout(
        r,
        plan_path,
        base_ref,
        git_available,
        BROAD_REVIEWERS,
        "broad",
        1,
        1,
    );
    if let Some(issues) = issues {
        run_fixer(r, plan_path, base_ref, git_available, &issues)?;
    }

    // Focused pass: critical/major reviewers, loop till clean
    for round in 1..=MAX_FOCUSED_ROUNDS {
        let issues = run_review_fanout(
            r,
            plan_path,
            base_ref,
            git_available,
            FOCUSED_REVIEWERS,
            "focused",
            round,
            MAX_FOCUSED_ROUNDS,
        );
        match issues {
            None => {
                info("All focused reviewers report ZERO ISSUES. Review phase complete!");
                return Ok(());
            }
            Some(issues) => {
                run_fixer(r, plan_path, base_ref, git_available, &issues)?;
            }
        }
    }

    warn(&format!(
        "Focused review cap of {} rounds reached, accepting current state.",
        MAX_FOCUSED_ROUNDS
    ));
    Ok(())
}

fn run_review_fanout(
    r: &Runner,
    plan_path: &str,
    base_ref: Option<&str>,
    git_available: bool,
    reviewers: &[ReviewRole],
    label: &str,
    round: usize,
    max_rounds: usize,
) -> Option<Vec<String>> {
    banner(&format!(
        "{}-review | round {}/{}",
        label, round, max_rounds
    ));

    for rv in reviewers {
        remove_dex_file(&format!("review-{}.md", rv.name));
    }

    // Run reviewers in parallel using threads
    let handles: Vec<_> = reviewers
        .iter()
        .map(|rv| {
            let lr = r.labeled(rv.name);
            let plan_path = plan_path.to_string();
            let base_ref = base_ref.map(str::to_string);
            let role_name = rv.name.to_string();
            let role_scope = rv.scope.to_string();
            let role_prompt = rv.prompt.to_string();
            let review_path = dex_path(&format!("review-{}.md", rv.name));

            info(&format!(
                "[parallel:{}] running {} review",
                role_name, role_scope
            ));

            let p = render_prompt(
                "review.txt",
                &serde_json::json!({
                    "PlanPath": plan_path,
                    "BaseRef": base_ref.unwrap_or_default(),
                    "GitAvailable": git_available,
                    "RoleName": role_name,
                    "RoleScope": role_scope,
                    "RolePrompt": role_prompt,
                    "ReviewPath": review_path,
                }),
            );

            let name_clone = role_name.clone();
            let scope_clone = role_scope.clone();
            std::thread::spawn(move || {
                let result = lr.run(&p);
                match &result {
                    Ok(()) => info(&format!(
                        "[parallel:{}] done {} review (exit=0)",
                        name_clone, scope_clone
                    )),
                    Err(_) => err_msg(&format!(
                        "[parallel:{}] done {} review (exit=1)",
                        name_clone, scope_clone
                    )),
                }
                result
            })
        })
        .collect();

    // Wait for all threads
    for handle in handles {
        let _ = handle.join();
    }

    let mut all_clean = true;
    let mut issues = Vec::new();
    for rv in reviewers {
        let review = read_dex_file(&format!("review-{}.md", rv.name));
        match review {
            None => {
                warn(&format!("Reviewer {:?} produced no output", rv.name));
                all_clean = false;
            }
            Some(review) => {
                show_markdown(&format!("Review: {}", rv.name), &review);
                if !is_clean_review(&review) {
                    all_clean = false;
                    issues.push(format!("── {} ──\n{}", rv.name, review));
                }
            }
        }
    }

    if all_clean {
        None
    } else {
        Some(issues)
    }
}

fn run_fixer(
    r: &Runner,
    plan_path: &str,
    base_ref: Option<&str>,
    git_available: bool,
    issues: &[String],
) -> Result<(), String> {
    info("Issues found — running fixer...");
    let fix_prompt = render_prompt(
        "fix.txt",
        &serde_json::json!({
            "PlanPath": plan_path,
            "BaseRef": base_ref.unwrap_or_default(),
            "GitAvailable": git_available,
            "Issues": issues.join("\n\n"),
        }),
    );
    if let Err(e) = r.run(&fix_prompt) {
        err_msg(&format!("Fixer error: {}", e));
        return Err(format!("fixer failed after automatic retries: {}", e));
    }
    Ok(())
}

fn is_clean_review(review: &str) -> bool {
    let normalized = review.trim().to_uppercase();
    normalized.contains("- ZERO FINDINGS")
}

// ── Bare Mode ──

pub fn bare_phase(r: &Runner, request: &str, max_iterations: usize) -> Result<(), String> {
    banner("BARE");
    for iteration in 1..=max_iterations {
        info(&format!("Bare iteration {}/{}", iteration, max_iterations));
        let p = render_prompt("bare.txt", &serde_json::json!({"Request": request}));
        if let Err(e) = r.run(&p) {
            return Err(format!("bare iteration {} failed: {}", iteration, e));
        }
    }
    Ok(())
}

// ── Finalize Phase ──

pub fn finalize_phase(r: &Runner, plan_path: &str, base_ref: &str) -> Result<(), String> {
    banner("FINALIZE");

    let branch = current_branch()?;
    let finalize_target = resolve_finalize_target(base_ref)?;
    let commits_ahead = commit_count_ahead(&finalize_target)?;
    if commits_ahead == 0 {
        return Err(format!(
            "finalize: branch {:?} has no commits to finalize relative to {:?}; run this on your feature branch or pass --base-ref <ref>",
            branch, finalize_target
        ));
    }

    let p = render_prompt(
        "finalize.txt",
        &serde_json::json!({
            "PlanPath": plan_path,
            "FinalizeTarget": finalize_target,
            "FinalizeNeedsFetch": target_needs_fetch(&finalize_target),
        }),
    );

    if let Err(e) = r.run(&p) {
        err_msg(&format!("Finalize error: {}", e));
        return Err(format!("finalize failed after automatic retries: {}", e));
    }
    Ok(())
}

fn current_branch() -> Result<String, String> {
    let branch = git_trimmed_output(&["symbolic-ref", "--short", "HEAD"])?;
    if branch.is_empty() {
        return Err(
            "finalize requires a named branch (detached HEAD is not supported)".to_string(),
        );
    }
    Ok(branch)
}

fn resolve_finalize_target(base_ref: &str) -> Result<String, String> {
    if base_ref != "HEAD" {
        git_trimmed_output(&["rev-parse", "--verify", base_ref])?;
        return Ok(base_ref.to_string());
    }

    if let Ok(target) = git_trimmed_output(&[
        "symbolic-ref",
        "--quiet",
        "--short",
        "refs/remotes/origin/HEAD",
    ]) {
        if !target.is_empty() {
            return Ok(target);
        }
    }

    for candidate in [
        "refs/remotes/origin/main",
        "refs/remotes/origin/master",
        "refs/heads/main",
        "refs/heads/master",
    ] {
        if git_ref_exists(candidate) {
            return Ok(candidate
                .strip_prefix("refs/remotes/")
                .or_else(|| candidate.strip_prefix("refs/heads/"))
                .unwrap_or(candidate)
                .to_string());
        }
    }

    Err(
        "finalize could not resolve a default base branch. Set origin/HEAD, create a local main/master branch, or pass --base-ref <ref>."
            .to_string(),
    )
}

fn commit_count_ahead(base_ref: &str) -> Result<u64, String> {
    let range = format!("{}..HEAD", base_ref);
    let count = git_trimmed_output(&["rev-list", "--count", &range])?;
    count
        .parse::<u64>()
        .map_err(|e| format!("parse git rev-list count {:?}: {}", count, e))
}

fn target_needs_fetch(base_ref: &str) -> bool {
    base_ref.starts_with("origin/")
}

fn git_ref_exists(refname: &str) -> bool {
    Command::new("git")
        .args(["show-ref", "--verify", "--quiet", refname])
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
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
