use similar::TextDiff;
use std::fs;
use std::process::Command;

use crate::core::{
    append_progress, dex_path, ensure_dex_dir, load_feedbacks, read_dex_file, remove_dex_file,
    render_prompt, reset_dex_runtime_artifacts, save_feedbacks, save_plan_request,
};
use crate::plan::{all_tasks_done, next_open_task};
use crate::runner::Runner;
use crate::ui::{
    banner, err_msg, info, prompt_choice, prompt_multiline, show_block, show_markdown, warn,
};

// ── Phase 1: Planning ──

const EXISTING_PLAN_CHOICE_PROMPT: &str =
    "Is your request a revision of this plan, a new plan, or should dex quit? Resume the current plan by running dex without a request.";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlanPhaseResult {
    pub plan_path: String,
    pub created_new_plan: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlanPhaseMode {
    Standard,
    ReviseImportedDraft,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PlanningSeed {
    request: String,
    feedbacks: Vec<String>,
    created_new_plan: bool,
}

fn should_refresh_review_base_ref_after_planning(
    has_existing_plan: bool,
    choice: Option<&str>,
) -> bool {
    !has_existing_plan || matches!(choice, Some("new"))
}

pub fn plan_phase(
    r: &Runner,
    user_input: &str,
    mode: PlanPhaseMode,
) -> Result<Option<PlanPhaseResult>, String> {
    banner("PLANNING");
    ensure_dex_dir();

    let plan_path = dex_path("plan.md");
    let seed = match mode {
        PlanPhaseMode::Standard => prepare_standard_plan_seed(user_input)?,
        PlanPhaseMode::ReviseImportedDraft => prepare_imported_plan_seed(user_input)?,
    };
    let PlanningSeed {
        request,
        feedbacks,
        created_new_plan,
    } = match seed {
        Some(seed) => seed,
        None => return Ok(None),
    };

    save_plan_request(&request);
    save_feedbacks(&feedbacks);

    run_planning_loop(r, request, feedbacks, plan_path, created_new_plan)
}

fn prepare_standard_plan_seed(user_input: &str) -> Result<Option<PlanningSeed>, String> {
    let mut feedbacks: Vec<String> = Vec::new();
    let mut request = user_input.to_string();
    let existing_plan = read_dex_file("plan.md");
    let mut created_new_plan =
        should_refresh_review_base_ref_after_planning(existing_plan.is_some(), None);

    if let Some(existing) = existing_plan {
        show_markdown("Existing plan", &existing);
        let choice = prompt_choice(EXISTING_PLAN_CHOICE_PROMPT, &["revise", "new", "quit"]);
        created_new_plan =
            should_refresh_review_base_ref_after_planning(true, Some(choice.as_str()));
        match choice.as_str() {
            "new" => reset_dex_runtime_artifacts(),
            "revise" => {
                if let Some(orig) = read_dex_file("request.txt") {
                    request = orig;
                }
                feedbacks = load_feedbacks();
                feedbacks.push(user_input.to_string());
            }
            "quit" => {
                info("Exiting without changing the current plan.");
                return Ok(None);
            }
            _ => {}
        }
    }

    Ok(Some(PlanningSeed {
        request,
        feedbacks,
        created_new_plan,
    }))
}

fn prepare_imported_plan_seed(user_input: &str) -> Result<Option<PlanningSeed>, String> {
    if read_dex_file("plan.md").is_none() {
        return Err(format!(
            "imported plan revision requested, but {} is missing",
            dex_path("plan.md")
        ));
    }

    Ok(Some(PlanningSeed {
        request: user_input.to_string(),
        feedbacks: Vec::new(),
        created_new_plan: true,
    }))
}

fn run_planning_loop(
    r: &Runner,
    request: String,
    mut feedbacks: Vec<String>,
    plan_path: String,
    created_new_plan: bool,
) -> Result<Option<PlanPhaseResult>, String> {
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
                feedbacks.push(format!(
                    "You did not produce a plan in {} or questions in {}. Please do so.",
                    dex_path("plan.md"),
                    dex_path("questions.md")
                ));
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
                return Ok(Some(PlanPhaseResult {
                    plan_path: plan_path.clone(),
                    created_new_plan,
                }));
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

    let editor = std::env::var("VISUAL")
        .or_else(|_| std::env::var("EDITOR"))
        .unwrap_or_else(|_| "vi".to_string());
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

    let progress_path = dex_path("progress.txt");
    let mut iteration = 1;
    loop {
        let task = next_open_task(plan_path)?;
        let task = match task {
            Some(t) => t,
            None => {
                append_progress("Implementation", "All tasks complete.");
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
        append_progress(
            &format!("Implementation — iteration {}", iteration),
            &format!(
                "Working on: {} ({}/{} steps open)",
                header,
                task.open,
                task.open + task.done
            ),
        );

        let p = render_prompt(
            "impl.txt",
            &serde_json::json!({
                "PlanPath": plan_path,
                "ProgressFile": progress_path,
                "TaskHeader": task.header,
                "TaskBody": task.body(),
            }),
        );

        if let Err(e) = r.run(&p) {
            append_progress(
                &format!("Implementation — iteration {}", iteration),
                &format!("FAILED: {}", e),
            );
            err_msg(&format!("CLI error: {}", e));
            return Err(format!(
                "implementation failed after automatic retries: {}",
                e
            ));
        }

        append_progress(
            &format!("Implementation — iteration {}", iteration),
            &format!("Completed: {}", header),
        );

        if all_tasks_done(plan_path)? {
            append_progress("Implementation", "All tasks complete.");
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
        prompt: "Review code for bugs, security issues, and quality problems.\n\nCorrectness Review:\n- Logic errors — off-by-one errors, incorrect conditionals, wrong operators\n- Edge cases — empty inputs, nil/null values, boundary conditions, concurrent access\n- Error handling — all errors checked, appropriate error wrapping, no silent failures\n- Resource management — proper cleanup, no leaks, correct resource release\n- Concurrency issues — race conditions, deadlocks, thread leaks\n- Data integrity — validation, sanitization, consistent state management\n\nSecurity Analysis:\n- Input validation — all user inputs validated and sanitized\n- Authentication/authorization — proper checks in place\n- Injection vulnerabilities — SQL, command, path traversal\n- Secret exposure — no hardcoded credentials or keys\n- Information disclosure — error messages, logs, debug info\n\nSimplicity Assessment:\n- Direct solutions first — if simple approach works, do not use complex pattern\n- No enterprise patterns for simple problems\n- Question every abstraction — each interface/abstraction must solve real problem\n- No scope creep — changes solve only the stated problem\n- No premature optimization\n\nFocus on defects that would cause runtime failures, security vulnerabilities, or maintainability problems.\nReport problems only — no positive observations.",
    },
    ReviewRole {
        name: "implementation",
        scope: "goal coverage, wiring, completeness, logic flow",
        prompt: "Review whether the implementation achieves the stated goal/requirement.\n\nCore Review Responsibilities:\n- Requirement coverage — does implementation address all aspects of the stated requirement? Are there edge cases or scenarios not handled?\n- Correctness of approach — is the chosen approach actually solving the right problem? Could it fail to achieve the goal in certain conditions?\n- Wiring and integration — is everything connected properly? Are new components registered, routes added, handlers wired, configs updated?\n- Completeness — are there missing pieces that would prevent the feature from working? Missing imports, unimplemented interfaces, incomplete migrations?\n- Logic flow — does data flow correctly from input to output? Are transformations correct? Is state managed properly?\n- Edge cases — are boundary conditions handled? Empty inputs, null values, concurrent access, error paths?\n\nFocus on correctness of approach, not code style.\nReport problems only — no positive observations.",
    },
    ReviewRole {
        name: "simplification",
        scope: "unnecessary complexity, over-engineering",
        prompt: "Detect over-engineered and overcomplicated code — code that works but is more complex than necessary.\n\n- Excessive abstraction layers — wrapper adds nothing, factory for single implementation, layer cake anti-pattern, DTO/mapper overkill\n- Premature generalization — generic solution for specific problem, config objects for 2-3 options, plugin architecture for fixed functionality\n- Unnecessary indirection — pass-through wrappers, excessive method chaining, interface wrapping primitives\n- Future-proofing excess — unused extension points, versioned internal APIs, feature flags for permanent decisions\n- Unnecessary fallbacks — fallback that never triggers, legacy mode kept just in case, dual implementations, silent fallbacks hiding problems\n- Premature optimization — caching rarely-accessed data, custom data structures when arrays/maps work, worker pools for occasional tasks\n\nFor each finding report: location, which over-engineering pattern detected, why it adds unnecessary complexity, what simpler code would look like.\nReport problems only — no positive observations.",
    },
    ReviewRole {
        name: "testing",
        scope: "coverage, test quality, edge cases",
        prompt: "Review test coverage and quality.\n\nTest Existence and Coverage:\n- Missing tests — new code paths without corresponding tests\n- Untested error paths — error conditions not verified\n- Coverage gaps — functions or branches without test coverage\n\nTest Quality:\n- Tests verify behavior, not implementation details\n- Each test is independent, can run in any order\n- Both success and error paths tested\n- Edge cases and boundary conditions covered\n\nFake Test Detection:\n- Tests that always pass regardless of code changes\n- Tests checking hardcoded values instead of actual output\n- Tests verifying mock behavior instead of code using the mock\n- Ignored errors with _ or empty error checks\n- Conditional assertions that always pass\n\nTest Independence:\n- No shared mutable state between tests\n- Proper setup and teardown\n- No order dependencies between tests\n- Resources properly cleaned up\n\nEdge Case Coverage:\n- Empty inputs and collections\n- Null/nil values\n- Boundary values (zero, max, min)\n- Concurrent access scenarios\n\nReport problems only — no positive observations.",
    },
    ReviewRole {
        name: "documentation",
        scope: "README, internal docs, plan alignment",
        prompt: "Review code changes and identify missing documentation updates.\n\nREADME.md (Human Documentation) — must document:\n- New features or capabilities\n- New CLI flags or command-line options\n- New API endpoints or interfaces\n- New configuration options\n- Changed behavior that affects users\n- New dependencies or system requirements\n- Breaking changes\nSkip: internal refactoring with no user-visible changes, bug fixes that restore documented behavior, test additions, code style changes.\n\nInternal docs — must document:\n- New architectural patterns discovered/established\n- New conventions or coding standards\n- New build/test commands\n- New libraries or tools integrated\n- Project structure changes\n- Workflow changes\nSkip: standard code additions following existing patterns, simple bug fixes, test additions using existing patterns.\n\nPlan Files:\n- Mark completed items as done\n- Update plan status if needed\n- Note which plan items this change addresses\n\nReport problems only — no positive observations.",
    },
];

static FOCUSED_REVIEWERS: &[ReviewRole] = &[
    ReviewRole {
        name: "critical-correctness",
        scope: "critical and major correctness, security, reliability",
        prompt: "Review code only for critical and major bugs, security issues, and correctness problems.\nFocus only on critical and major issues. Ignore style/minor issues.\n\n- Logic errors that cause incorrect behavior\n- Security vulnerabilities — injection, XSS, secrets exposure, improper validation\n- Data loss or corruption risks\n- Race conditions — concurrent access, shared state, missing synchronization\n- Resource leaks — unclosed handles, missing cleanup\n- Error handling gaps — silent failures, ignored errors\n\nReport problems only — no positive observations.",
    },
    ReviewRole {
        name: "critical-coverage",
        scope: "critical and major goal coverage, integration, completeness",
        prompt: "Review whether any critical or major requirement-coverage or integration issues remain.\nFocus only on critical and major issues. Ignore style/minor issues.\n\n- Requirements that are not implemented at all\n- Integration bugs between components — missing wiring, unregistered handlers, broken data flow\n- Critical logic flow errors\n- Missing error paths that could cause crashes or data loss\n\nReport problems only — no positive observations.",
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
    append_progress(
        "Review — broad pass",
        "Starting broad review with all reviewers.",
    );
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
    if let Some(ref issues) = issues {
        append_progress(
            "Review — broad pass",
            &format!("Issues found by {} reviewers, running fixer.", issues.len()),
        );
        run_fixer(r, plan_path, base_ref, git_available, issues)?;
    } else {
        append_progress("Review — broad pass", "No issues found.");
    }

    // Focused pass: critical/major reviewers, loop till clean
    for round in 1..=MAX_FOCUSED_ROUNDS {
        append_progress(
            &format!(
                "Review — focused pass round {}/{}",
                round, MAX_FOCUSED_ROUNDS
            ),
            "Starting focused review (critical/major issues only).",
        );
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
                append_progress(
                    &format!(
                        "Review — focused pass round {}/{}",
                        round, MAX_FOCUSED_ROUNDS
                    ),
                    "ZERO ISSUES. Review phase complete.",
                );
                info("All focused reviewers report ZERO ISSUES. Review phase complete!");
                return Ok(());
            }
            Some(ref issues) => {
                append_progress(
                    &format!(
                        "Review — focused pass round {}/{}",
                        round, MAX_FOCUSED_ROUNDS
                    ),
                    &format!("Issues found by {} reviewers, running fixer.", issues.len()),
                );
                run_fixer(r, plan_path, base_ref, git_available, issues)?;
            }
        }
    }

    append_progress(
        "Review",
        &format!(
            "Focused review cap of {} rounds reached, accepting current state.",
            MAX_FOCUSED_ROUNDS
        ),
    );
    warn(&format!(
        "Focused review cap of {} rounds reached, accepting current state.",
        MAX_FOCUSED_ROUNDS
    ));
    Ok(())
}

#[allow(clippy::too_many_arguments)]
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
                    "ReviewName": format!("review-{}.md", rv.name),
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

#[cfg(test)]
mod tests {
    use super::{
        should_refresh_review_base_ref_after_planning, BROAD_REVIEWERS,
        EXISTING_PLAN_CHOICE_PROMPT, FOCUSED_REVIEWERS,
    };

    #[test]
    fn existing_plan_prompt_mentions_quit_and_resume_behavior() {
        assert!(EXISTING_PLAN_CHOICE_PROMPT.contains("quit"));
        assert!(EXISTING_PLAN_CHOICE_PROMPT.contains("without a request"));
    }

    #[test]
    fn review_base_ref_refreshes_only_for_new_plans() {
        assert!(should_refresh_review_base_ref_after_planning(false, None));
        assert!(should_refresh_review_base_ref_after_planning(
            true,
            Some("new")
        ));
        assert!(!should_refresh_review_base_ref_after_planning(
            true,
            Some("revise")
        ));
        assert!(!should_refresh_review_base_ref_after_planning(
            true,
            Some("quit")
        ));
    }

    #[test]
    fn focused_reviewer_names_do_not_overlap_with_broad_reviewers() {
        for focused in FOCUSED_REVIEWERS {
            for broad in BROAD_REVIEWERS {
                assert_ne!(focused.name, broad.name);
                assert!(!focused.name.contains(broad.name));
                assert!(!broad.name.contains(focused.name));
            }
        }
    }
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

pub fn finalize_phase(r: &Runner, plan_path: &str, finalize_target: &str) -> Result<(), String> {
    banner("FINALIZE");

    let branch = current_branch()?;
    let finalize_target = resolve_finalize_target(finalize_target)?;
    let commits_ahead = commit_count_ahead(&finalize_target)?;
    if commits_ahead == 0 {
        return Err(format!(
            "finalize: branch {:?} has no commits to finalize relative to {:?}; run this on your feature branch or choose a different target",
            branch, finalize_target
        ));
    }

    let p = render_prompt(
        "finalize.txt",
        &serde_json::json!({
            "PlanPath": plan_path,
            "FinalizeTarget": finalize_target,
            "FinalizeNeedsFetch": finalize_target.starts_with("origin/"),
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

fn resolve_finalize_target(finalize_target: &str) -> Result<String, String> {
    if finalize_target.trim().is_empty() {
        return Err(
            "finalize requires a rebase target: dex --finalize <target-for-rebase>".to_string(),
        );
    }

    git_trimmed_output(&["rev-parse", "--verify", finalize_target])?;
    Ok(finalize_target.to_string())
}

fn commit_count_ahead(finalize_target: &str) -> Result<u64, String> {
    let range = format!("{}..HEAD", finalize_target);
    let count = git_trimmed_output(&["rev-list", "--count", &range])?;
    count
        .parse::<u64>()
        .map_err(|e| format!("parse git rev-list count {:?}: {}", count, e))
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
