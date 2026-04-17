use serde::Deserialize;
use similar::TextDiff;
use std::fs;
use std::process::Command;

use crate::core::{
    append_progress, dex_path, ensure_dex_dir, git_trimmed_output, read_dex_file, remove_dex_file,
    render_prompt, save_feedbacks, save_plan_request,
};
use crate::plan::{all_tasks_done, next_open_task, plan_step_counts};
use crate::runner::Runner;
use crate::ui::{
    banner, err_msg, info, phase_detail, prompt_choice, prompt_multiline, show_block,
    show_markdown, warn,
};

// ── Phase 1: Planning ──

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

        loop {
            let choice = prompt_choice(
                "Accept, edit, revise, or reject?",
                &["accept", "edit", "revise", "reject"],
            );
            match choice.as_str() {
                "accept" => {
                    info("Plan accepted!");
                    return Ok(Some(plan_path.clone()));
                }
                "reject" => {
                    warn("Plan rejected.");
                    return Ok(None);
                }
                "edit" => match edit_plan_in_editor(&plan) {
                    Ok(Some(diff)) => {
                        let feedback = format!(
                            "user provided feedback in the form of a unified diff: \n\n{}",
                            diff
                        );
                        feedbacks.push(feedback);
                        save_feedbacks(&feedbacks);
                        break;
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
                    save_feedbacks(&feedbacks);
                    break;
                }
                _ => {}
            }
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

        let (plan_steps_open, plan_steps_total) = plan_step_counts(plan_path)?;
        let header = format_task_label(&task.header);
        let iteration_detail = format!(
            "{} of {} plan steps remaining",
            plan_steps_open, plan_steps_total
        );
        phase_detail(&format!("Iteration {}", iteration), &iteration_detail);
        phase_detail("Job", &header);
        append_progress(
            &format!("Implementation — iteration {}", iteration),
            &format!(
                "Iteration {}: {}\nJob: {}",
                iteration, iteration_detail, header
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

pub fn review_phase(
    r: &Runner,
    plan_path: &str,
    base_ref: Option<&str>,
    git_available: bool,
    parallel: Option<usize>,
) -> Result<(), String> {
    let reviewers = load_reviewers();

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
        &reviewers.broad,
        "broad",
        1,
        1,
        parallel,
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
            &reviewers.focused,
            "focused",
            round,
            MAX_FOCUSED_ROUNDS,
            parallel,
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
    parallel: Option<usize>,
) -> Option<Vec<String>> {
    banner(&format!(
        "{}-review | round {}/{}",
        label, round, max_rounds
    ));

    for rv in reviewers {
        remove_dex_file(&format!("review-{}.md", rv.name));
    }

    // Run reviewers in parallel using threads, respecting concurrency limit
    let max_concurrent = parallel.unwrap_or(reviewers.len()).max(1);

    let prepared: Vec<_> = reviewers
        .iter()
        .map(|rv| {
            let plan_path = plan_path.to_string();
            let base_ref = base_ref.map(str::to_string);
            let role_name = rv.name.to_string();
            let role_scope = rv.scope.to_string();
            let role_prompt = rv.prompt.to_string();

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

            (p, role_name, role_scope)
        })
        .collect();

    for batch in prepared.chunks(max_concurrent) {
        let handles: Vec<_> = batch
            .iter()
            .map(|(p, role_name, role_scope)| {
                info(&format!(
                    "[parallel:{}] running {} review",
                    role_name, role_scope
                ));

                let lr = r.labeled(role_name);
                let p = p.clone();
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

        for handle in handles {
            let _ = handle.join();
        }
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

fn run_fixer(
    r: &Runner,
    plan_path: &str,
    base_ref: Option<&str>,
    git_available: bool,
    issues: &[String],
) -> Result<(), String> {
    warn("Running fixer...");
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
    let re = regex::Regex::new(r"(?i)[-*]\s*(zero|no)\s+(findings|issues)").unwrap();
    re.is_match(review)
}

#[cfg(test)]
mod tests {
    use super::{format_task_label, is_clean_review, read_bare_request_file, Reviewers};
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

        assert_eq!(request.unwrap(), Some("hello from file".to_string()));
    }

    #[test]
    fn bare_request_file_stops_on_empty_trimmed_content() {
        let path = write_temp_file(" \n\t ");
        let request = read_bare_request_file(path.to_str().unwrap());
        let _ = fs::remove_file(&path);

        assert_eq!(request.unwrap(), None);
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

        assert_eq!(
            read_bare_request_file(path.to_str().unwrap()).unwrap(),
            None
        );
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

// ── Bare Mode ──

fn read_bare_request_file(path: &str) -> Result<Option<String>, String> {
    let request = match fs::read_to_string(path) {
        Ok(request) => request,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(format!("read bare request {:?}: {}", path, e)),
    };
    let request = request.trim().to_string();
    if request.is_empty() {
        return Ok(None);
    }
    Ok(Some(request))
}

pub fn bare_phase(r: &Runner, request_file: &str, max_iterations: usize) -> Result<(), String> {
    banner("BARE");
    for iteration in 1..=max_iterations {
        phase_detail("iteration", &format!("{}/{}", iteration, max_iterations));
        let request = match read_bare_request_file(request_file)? {
            Some(request) => request,
            None => {
                info("Bare loop stopped due to missing request.");
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
