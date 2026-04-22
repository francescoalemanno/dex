use serde::Deserialize;
use similar::TextDiff;
use std::fs;
use std::process::Command;

use crate::core::{
    append_impl_commits, dex_path, ensure_dex_dir, git_commits_between, git_head,
    git_trimmed_output, impl_commit_history_summary, read_dex_file, remove_dex_file, render_prompt,
    save_feedbacks, save_plan_request,
};
use crate::plan::{next_open_task, plan_step_counts};
use crate::runner::Runner;
use crate::ui::{
    banner, err_msg, info, phase_detail, prompt_choice, prompt_multiline, show_markdown, warn,
};

const IMPLEMENTATION_STALEMATE_LIMIT: usize = 4;

// ── Phase 1: Planning ──

pub fn resume_plan(r: &Runner) -> Result<Option<String>, String> {
    let plan_path = dex_path("plan.md");
    let plan = match read_dex_file("plan.md") {
        Some(p) => p,
        None => return Ok(None),
    };

    banner("EXISTING PLAN");
    show_markdown("Plan", &plan);

    let request = read_dex_file("request.txt").unwrap_or_default();
    let mut feedbacks = crate::core::load_feedbacks();

    match plan_review_loop(&plan, &mut feedbacks)? {
        PlanReviewResult::Accepted => Ok(Some(plan_path)),
        PlanReviewResult::Rejected => Ok(None),
        PlanReviewResult::Loop => run_planning_loop(r, request, feedbacks, plan_path),
    }
}

pub fn plan_phase(
    r: &Runner,
    request: &str,
    feedbacks: Vec<String>,
) -> Result<Option<String>, String> {
    banner("PLANNING");
    ensure_dex_dir();

    let plan_path = dex_path("plan.md");
    save_plan_request(request);
    save_feedbacks(&feedbacks);

    run_planning_loop(r, request.to_string(), feedbacks, plan_path)
}

enum PlanReviewResult {
    Accepted,
    Rejected,
    Loop,
}

fn plan_review_loop(plan: &str, feedbacks: &mut Vec<String>) -> Result<PlanReviewResult, String> {
    loop {
        let choice = prompt_choice(
            "Accept, edit, revise, or reject?",
            &["accept", "edit", "revise", "reject"],
        );
        match choice.as_str() {
            "accept" => {
                info("Plan accepted!");
                return Ok(PlanReviewResult::Accepted);
            }
            "reject" => {
                warn("Plan rejected.");
                return Ok(PlanReviewResult::Rejected);
            }
            "edit" => match edit_plan_in_editor(plan) {
                Ok(Some(diff)) => {
                    let feedback = format!(
                        "user provided feedback in the form of a unified diff: \n\n{}",
                        diff
                    );
                    feedbacks.push(feedback);
                    save_feedbacks(feedbacks);
                    return Ok(PlanReviewResult::Loop);
                }
                Ok(None) => {
                    info("No changes detected in the plan.");
                }
                Err(e) => {
                    err_msg(&format!("Editor error: {}", e));
                }
            },
            "revise" => {
                let feedback = prompt_multiline("Your revision feedback:");
                feedbacks.push(feedback);
                save_feedbacks(feedbacks);
                return Ok(PlanReviewResult::Loop);
            }
            _ => {}
        }
    }
}

fn run_planning_loop(
    r: &Runner,
    request: String,
    mut feedbacks: Vec<String>,
    plan_path: String,
) -> Result<Option<String>, String> {
    let mut iteration = 1;
    loop {
        phase_detail("iteration", &iteration.to_string());
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
            show_markdown("Questions from CLI", &questions);
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

        match plan_review_loop(&plan, &mut feedbacks)? {
            PlanReviewResult::Accepted => return Ok(Some(plan_path)),
            PlanReviewResult::Rejected => return Ok(None),
            PlanReviewResult::Loop => continue,
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

fn format_task_label(header: &str) -> String {
    let header = header.trim();
    if header.is_empty() {
        return "(unnamed task)".to_string();
    }

    let label = header.trim_start_matches('#').trim();
    if label.is_empty() {
        "(unnamed task)".to_string()
    } else {
        label.to_string()
    }
}

pub fn impl_phase(r: &Runner, plan_path: &str) -> Result<(), String> {
    banner("IMPLEMENTATION");

    let mut iteration = 1;
    let mut unchanged_iterations = 0;
    loop {
        let task = next_open_task(plan_path)?;
        let task = match task {
            Some(t) => t,
            None => {
                info("All tasks complete!");
                return Ok(());
            }
        };

        let (plan_steps_open, plan_steps_total) = plan_step_counts(plan_path)?;
        let header = format_task_label(&task.header);
        let iteration_detail = format!(
            "{} of {} plan steps remaining",
            plan_steps_open, plan_steps_total
        );
        phase_detail(&format!("Iteration {}", iteration), &iteration_detail);
        phase_detail("Job", &header);

        let commit_history = impl_commit_history_summary().unwrap_or_default();
        let state_a = git_head().ok();

        let p = render_prompt(
            "impl.txt",
            &serde_json::json!({
                "PlanPath": plan_path,
                "TaskHeader": task.header,
                "TaskBody": task.body(),
                "CommitHistory": commit_history,
            }),
        );

        if let Err(e) = r.run(&p) {
            err_msg(&format!("CLI error: {}", e));
            return Err(format!(
                "implementation failed after automatic retries: {}",
                e
            ));
        }

        if let Some(ref before) = state_a {
            if let Ok(after) = git_head() {
                let new_commits = git_commits_between(before, &after);
                append_impl_commits(&new_commits);
            }
        }

        let after_counts = plan_step_counts(plan_path)?;
        if after_counts.0 == 0 {
            info("All tasks complete!");
            return Ok(());
        }

        unchanged_iterations = if (plan_steps_open, plan_steps_total) == after_counts {
            unchanged_iterations + 1
        } else {
            0
        };
        if unchanged_iterations >= IMPLEMENTATION_STALEMATE_LIMIT {
            return Err(format!(
                "STALEMATE: total plan steps ({}) and remaining plan steps ({}) were unchanged for {} consecutive implementation iterations.",
                after_counts.1, after_counts.0, IMPLEMENTATION_STALEMATE_LIMIT
            ));
        }

        iteration += 1;
    }
}

// ── Phase 3: Review ──

#[derive(Deserialize)]
struct ReviewRole {
    name: String,
    scope: String,
    prompt: String,
}

#[derive(Deserialize)]
pub struct Reviewers {
    broad: Vec<ReviewRole>,
    focused: Vec<ReviewRole>,
}

impl Reviewers {
    pub fn builtin() -> Self {
        serde_json::from_str(include_str!("../prompts/reviewers.json"))
            .expect("invalid built-in reviewers.json")
    }
}

fn load_reviewers() -> Reviewers {
    let path = dex_path("reviewers.json");
    match fs::read_to_string(&path) {
        Ok(data) => serde_json::from_str(&data).unwrap_or_else(|_| {
            warn("Invalid .dex/reviewers.json, falling back to defaults.");
            Reviewers::builtin()
        }),
        Err(_) => Reviewers::builtin(),
    }
}

const MAX_FOCUSED_ROUNDS: usize = 3;

struct PreparedReview {
    prompt: String,
    role_name: String,
    role_scope: String,
}

pub fn review_phase(
    r: &Runner,
    plan_path: &str,
    base_ref: &str,
    parallel: Option<usize>,
) -> Result<(), String> {
    let reviewers = load_reviewers();

    let issues = run_review_fanout(
        r,
        plan_path,
        base_ref,
        &reviewers.broad,
        "broad",
        1,
        1,
        parallel,
    );
    if let Some(ref issues) = issues {
        run_fixer(r, plan_path, base_ref, issues)?;
    }

    for round in 1..=MAX_FOCUSED_ROUNDS {
        let issues = run_review_fanout(
            r,
            plan_path,
            base_ref,
            &reviewers.focused,
            "focused",
            round,
            MAX_FOCUSED_ROUNDS,
            parallel,
        );
        match issues {
            None => {
                info("All focused reviewers report ZERO ISSUES. Review phase complete!");
                return Ok(());
            }
            Some(ref issues) => run_fixer(r, plan_path, base_ref, issues)?,
        }
    }

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
    base_ref: &str,
    reviewers: &[ReviewRole],
    label: &str,
    round: usize,
    max_rounds: usize,
    parallel: Option<usize>,
) -> Option<Vec<String>> {
    banner(&format!(
        "{}-review | round {}/{}",
        label, round, max_rounds
    ));

    for rv in reviewers {
        remove_dex_file(&review_output_name(&rv.name));
    }

    let prepared = prepare_review_jobs(plan_path, base_ref, reviewers);
    run_review_batches(r, &prepared, parallel.unwrap_or(reviewers.len()).max(1));
    collect_review_issues(reviewers)
}

fn review_output_name(role_name: &str) -> String {
    format!("review-{}.md", role_name)
}

fn prepare_review_jobs(
    plan_path: &str,
    base_ref: &str,
    reviewers: &[ReviewRole],
) -> Vec<PreparedReview> {
    reviewers
        .iter()
        .map(|rv| PreparedReview {
            prompt: render_prompt(
                "review.txt",
                &serde_json::json!({
                    "PlanPath": plan_path,
                    "BaseRef": base_ref,
                    "RoleName": rv.name,
                    "RoleScope": rv.scope,
                    "RolePrompt": rv.prompt,
                    "ReviewName": review_output_name(&rv.name),
                }),
            ),
            role_name: rv.name.clone(),
            role_scope: rv.scope.clone(),
        })
        .collect()
}

fn run_review_batches(r: &Runner, prepared: &[PreparedReview], max_concurrent: usize) {
    for batch in prepared.chunks(max_concurrent) {
        run_review_batch(r, batch);
    }
}

fn run_review_batch(r: &Runner, batch: &[PreparedReview]) {
    let handles: Vec<_> = batch
        .iter()
        .map(|review| {
            info(&format!(
                "[parallel:{}] running {} review",
                review.role_name, review.role_scope
            ));

            let runner = r.labeled(&review.role_name);
            let prompt = review.prompt.clone();
            let role_name = review.role_name.clone();
            let role_scope = review.role_scope.clone();
            std::thread::spawn(move || {
                let result = runner.run(&prompt);
                match &result {
                    Ok(()) => info(&format!(
                        "[parallel:{}] done {} review (exit=0)",
                        role_name, role_scope
                    )),
                    Err(_) => err_msg(&format!(
                        "[parallel:{}] done {} review (exit=1)",
                        role_name, role_scope
                    )),
                }
                result
            })
        })
        .collect();

    for (review, handle) in batch.iter().zip(handles) {
        if handle.join().is_err() {
            err_msg(&format!(
                "[parallel:{}] review thread panicked",
                review.role_name
            ));
        }
    }
}

fn collect_review_issues(reviewers: &[ReviewRole]) -> Option<Vec<String>> {
    let mut all_clean = true;
    let mut issues = Vec::new();

    for rv in reviewers {
        let review = read_dex_file(&review_output_name(&rv.name));
        match review {
            None => {
                warn(&format!("Reviewer {:?} produced no output", rv.name));
                all_clean = false;
            }
            Some(review) => {
                if is_clean_review(&review) {
                    info(&format!("[{}] ZERO ISSUES", rv.name));
                } else {
                    err_msg(&format!("[{}] issues found", rv.name));
                    show_markdown(&format!("Review: {}", rv.name), &review);
                    all_clean = false;
                    issues.push(format!(
                        "\u{2500}\u{2500} {} \u{2500}\u{2500}\n{}",
                        rv.name, review
                    ));
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

fn run_fixer(r: &Runner, plan_path: &str, base_ref: &str, issues: &[String]) -> Result<(), String> {
    warn("Running fixer...");
    let fix_prompt = render_prompt(
        "fix.txt",
        &serde_json::json!({
            "PlanPath": plan_path,
            "BaseRef": base_ref,
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
    let re = regex::Regex::new(r"(?i)[-*]\s*(zero|no)\s+(findings|issues)").unwrap();
    re.is_match(review)
}

// ── Bare Mode ──

enum BareRequestFile {
    Ready(String),
    Missing,
    Empty,
}

fn read_bare_request_file(path: &str) -> Result<BareRequestFile, String> {
    let request = match fs::read_to_string(path) {
        Ok(request) => request,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(BareRequestFile::Missing),
        Err(e) => return Err(format!("read bare request {:?}: {}", path, e)),
    };

    let request = request.trim().to_string();
    if request.is_empty() {
        return Ok(BareRequestFile::Empty);
    }

    Ok(BareRequestFile::Ready(request))
}

pub fn bare_phase(r: &Runner, request_file: &str, max_iterations: usize) -> Result<(), String> {
    banner("BARE");
    for iteration in 1..=max_iterations {
        phase_detail("iteration", &format!("{}/{}", iteration, max_iterations));
        let request = match read_bare_request_file(request_file)? {
            BareRequestFile::Ready(request) => request,
            BareRequestFile::Missing => {
                info(&format!(
                    "Bare loop stopped: request file {:?} is missing.",
                    request_file
                ));
                return Ok(());
            }
            BareRequestFile::Empty => {
                info(&format!(
                    "Bare loop stopped: request file {:?} is empty after trimming.",
                    request_file
                ));
                return Ok(());
            }
        };

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

#[cfg(test)]
mod tests {
    use super::{
        format_task_label, is_clean_review, read_bare_request_file, BareRequestFile, Reviewers,
    };
    use std::fs;
    use std::path::PathBuf;

    fn write_temp_file(contents: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "dex-phases-test-{}-{}.txt",
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
    fn focused_reviewer_names_do_not_overlap_with_broad_reviewers() {
        let r = Reviewers::builtin();
        for focused in &r.focused {
            for broad in &r.broad {
                assert_ne!(focused.name, broad.name);
                assert!(!focused.name.contains(&broad.name));
                assert!(!broad.name.contains(&focused.name));
            }
        }
    }

    #[test]
    fn is_clean_review_accepts_variations() {
        assert!(is_clean_review("- ZERO FINDINGS"));
        assert!(is_clean_review("- zero findings"));
        assert!(is_clean_review("- Zero Findings"));
        assert!(is_clean_review("* No issues"));
        assert!(is_clean_review("- No findings"));
        assert!(is_clean_review("* ZERO ISSUES"));
        assert!(is_clean_review("  - zero  findings  "));
    }

    #[test]
    fn is_clean_review_rejects_dirty() {
        assert!(!is_clean_review("Found 3 issues"));
        assert!(!is_clean_review("Some problems detected"));
        assert!(!is_clean_review(""));
    }

    #[test]
    fn bare_request_file_is_trimmed() {
        let path = write_temp_file("  hello from file  \n");
        let request = read_bare_request_file(path.to_str().unwrap());
        let _ = fs::remove_file(&path);

        assert!(matches!(
            request.unwrap(),
            BareRequestFile::Ready(request) if request == "hello from file"
        ));
    }

    #[test]
    fn bare_request_file_stops_on_empty_trimmed_content() {
        let path = write_temp_file(" \n\t ");
        let request = read_bare_request_file(path.to_str().unwrap());
        let _ = fs::remove_file(&path);

        assert!(matches!(request.unwrap(), BareRequestFile::Empty));
    }

    #[test]
    fn bare_request_file_stops_on_missing_file() {
        let path = std::env::temp_dir().join(format!(
            "dex-phases-missing-{}-{}.txt",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));

        assert!(matches!(
            read_bare_request_file(path.to_str().unwrap()).unwrap(),
            BareRequestFile::Missing
        ));
    }

    #[test]
    fn format_task_label_strips_markdown_heading_prefix() {
        assert_eq!(
            format_task_label("### Task 2: Build API"),
            "Task 2: Build API"
        );
        assert_eq!(format_task_label("## Overview"), "Overview");
        assert_eq!(format_task_label(""), "(unnamed task)");
    }
}
