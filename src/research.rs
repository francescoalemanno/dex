use crate::core::{dex_path, ensure_dex_dir, git_trimmed_output, render_prompt};
use crate::runner::{track_child, untrack_child, Runner};
use crate::ui::{banner, err_msg, info, phase_detail, prompt_choice, prompt_line, warn};

use regex::Regex;
use serde::{Deserialize, Serialize};
use shared_child::SharedChild;
use std::fs;
use std::io::Read as IoRead;
use std::process::Stdio;
use std::sync::{mpsc, Arc};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const BENCHMARK_TIMEOUT_SECS: u64 = 600;
const MAX_RECENT_HISTORY: usize = 25;
const RECENT_FULL_BODY: usize = 5;
const MAX_DEAD_ENDS: usize = 20;
const MAX_CONSECUTIVE_AGENT_FAILURES: usize = 3;
const MISSING_RESEARCH_SESSION: &str = "no research session found (missing .dex/research.jsonl)";

// ── Types ──

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResearchConfig {
    #[serde(rename = "type")]
    pub(crate) entry_type: String,
    pub(crate) goal: String,
    pub(crate) command: String,
    pub(crate) metric_name: String,
    pub(crate) metric_unit: String,
    pub(crate) direction: String,
    pub(crate) files_in_scope: String,
    pub(crate) constraints: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) checks_command: Option<String>,
}

impl ResearchConfig {
    fn metric_summary(&self) -> String {
        format!("{} ({} is better)", self.metric_name, self.direction)
    }

    fn format_metric_value(&self, value: f64) -> String {
        format!("{}{}", format_metric(value), self.metric_unit)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ResearchEntry {
    run: usize,
    commit: String,
    metric: f64,
    status: String,
    description: String,
    timestamp: u64,
    confidence: Option<f64>,
}

#[derive(Debug, Clone)]
struct ResearchState {
    config: ResearchConfig,
    results: Vec<ResearchEntry>,
}

#[derive(Debug, Clone)]
struct AttemptDetails {
    head_before: String,
    commit_sha: String,
    description: String,
}

struct BenchmarkOutcome {
    exit_code: Option<i32>,
    duration_secs: f64,
    timed_out: bool,
    metrics: Vec<(String, f64)>,
}

// ── Small helpers ──

fn is_better(current: f64, reference: f64, direction: &str) -> bool {
    if direction == "higher" {
        current > reference
    } else {
        current < reference
    }
}

fn now_timestamp() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn slugify(text: &str) -> String {
    let slug: String = text
        .to_lowercase()
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { '-' })
        .collect();
    let joined: String = slug
        .split('-')
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("-");
    let trimmed = if joined.len() > 40 {
        &joined[..40]
    } else {
        &joined
    };
    trimmed.trim_end_matches('-').to_string()
}

// ── Metric formatting ──

fn add_thousands_sep(s: &str) -> String {
    let negative = s.starts_with('-');
    let digits = if negative { &s[1..] } else { s };
    let mut result = String::new();
    for (i, c) in digits.chars().rev().enumerate() {
        if i > 0 && i % 3 == 0 {
            result.push(',');
        }
        result.push(c);
    }
    if negative {
        result.push('-');
    }
    result.chars().rev().collect()
}

fn format_metric(value: f64) -> String {
    if value == value.round() && value.abs() < 1e15 {
        add_thousands_sep(&format!("{}", value as i64))
    } else {
        let int_part = value.trunc() as i64;
        let frac = format!("{:.2}", value.fract().abs());
        format!("{}{}", add_thousands_sep(&int_part.to_string()), &frac[1..])
    }
}

fn format_delta_pct(current: f64, baseline: f64) -> String {
    if baseline == 0.0 {
        return "N/A".to_string();
    }
    let pct = ((current - baseline) / baseline) * 100.0;
    let sign = if pct > 0.0 { "+" } else { "" };
    format!("{}{:.1}", sign, pct)
}

fn confidence_label(conf: f64) -> &'static str {
    if conf >= 2.0 {
        "likely real"
    } else if conf >= 1.0 {
        "marginal"
    } else {
        "within noise"
    }
}

// ── Metric parsing ──

fn parse_metric_lines(output: &str) -> Vec<(String, f64)> {
    let re = Regex::new(r"(?m)^METRIC\s+([\w.µ]+)=(\S+)\s*$").unwrap();
    let mut metrics = Vec::new();
    for caps in re.captures_iter(output) {
        let name = caps[1].to_string();
        if let Ok(value) = caps[2].parse::<f64>() {
            if value.is_finite() {
                if let Some(pos) = metrics.iter().position(|(n, _)| n == &name) {
                    metrics[pos].1 = value;
                } else {
                    metrics.push((name, value));
                }
            }
        }
    }
    metrics
}

fn extract_primary_metric(outcome: &BenchmarkOutcome, metric_name: &str) -> Option<f64> {
    for (name, value) in &outcome.metrics {
        if name == metric_name {
            return Some(*value);
        }
    }
    if metric_name == "duration_s" {
        return Some(outcome.duration_secs);
    }
    None
}

// ── Statistics ──

fn sorted_median(values: &[f64]) -> f64 {
    if values.is_empty() {
        return 0.0;
    }
    let mut sorted: Vec<f64> = values.to_vec();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let mid = sorted.len() / 2;
    if sorted.len().is_multiple_of(2) {
        sorted[mid - 1] + (sorted[mid] - sorted[mid - 1]) / 2.0
    } else {
        sorted[mid]
    }
}

fn best_kept_metric(results: &[ResearchEntry], direction: &str) -> Option<f64> {
    results
        .iter()
        .filter(|r| r.status == "keep" && r.metric > 0.0)
        .map(|r| r.metric)
        .reduce(|best, val| {
            if is_better(val, best, direction) {
                val
            } else {
                best
            }
        })
}

fn compute_confidence(results: &[ResearchEntry], baseline: f64, direction: &str) -> Option<f64> {
    let values: Vec<f64> = results
        .iter()
        .filter(|r| r.metric > 0.0)
        .map(|r| r.metric)
        .collect();
    if values.len() < 3 {
        return None;
    }
    let median = sorted_median(&values);
    let deviations: Vec<f64> = values.iter().map(|v| (v - median).abs()).collect();
    let mad = sorted_median(&deviations);
    if mad == 0.0 {
        return None;
    }
    let best = best_kept_metric(results, direction)?;
    if best == baseline {
        return None;
    }
    Some((best - baseline).abs() / mad)
}

// ── History for prompts ──

fn first_line(s: &str) -> &str {
    s.lines().next().unwrap_or(s)
}

fn build_recent_history(results: &[ResearchEntry], metric_unit: &str) -> String {
    let start = results.len().saturating_sub(MAX_RECENT_HISTORY);
    let recent = &results[start..];
    if recent.is_empty() {
        return String::new();
    }
    let baseline = results.first().filter(|r| r.metric > 0.0).map(|r| r.metric);
    let full_body_start = recent.len().saturating_sub(RECENT_FULL_BODY);
    let mut lines = Vec::new();
    for (i, r) in recent.iter().enumerate().rev() {
        let delta = match baseline {
            Some(b) if b > 0.0 && r.metric > 0.0 => {
                format!(" ({}%)", format_delta_pct(r.metric, b))
            }
            _ => String::new(),
        };
        let desc = if i >= full_body_start {
            &r.description
        } else {
            first_line(&r.description)
        };
        lines.push(format!(
            "#{}: {} — {}{}{}  — {}",
            r.run,
            r.status,
            format_metric(r.metric),
            metric_unit,
            delta,
            desc,
        ));
    }
    lines.join("\n")
}

fn build_dead_ends(results: &[ResearchEntry]) -> String {
    let dead: Vec<&ResearchEntry> = results
        .iter()
        .filter(|r| r.status == "discard" || r.status == "crash" || r.status == "checks_failed")
        .collect();
    let start = dead.len().saturating_sub(MAX_DEAD_ENDS);
    let recent_dead = &dead[start..];
    if recent_dead.is_empty() {
        return String::new();
    }
    recent_dead
        .iter()
        .map(|r| format!("- {} ({})", r.description, r.status))
        .collect::<Vec<_>>()
        .join("\n")
}

fn build_research_prompt(
    config: &ResearchConfig,
    results: &[ResearchEntry],
    baseline: f64,
    confidence: Option<f64>,
    iteration: usize,
    max_iterations: Option<usize>,
) -> String {
    let best = best_kept_metric(results, &config.direction).filter(|&value| value != baseline);
    let recent_history = build_recent_history(results, &config.metric_unit);
    let dead_ends = build_dead_ends(results);

    render_prompt(
        "research.txt",
        &serde_json::json!({
            "Goal": config.goal,
            "Command": config.command,
            "MetricName": config.metric_name,
            "Direction": config.direction,
            "Baseline": config.format_metric_value(baseline),
            "BestMetric": best.map(|value| config.format_metric_value(value)),
            "DeltaPct": best.map(|value| format_delta_pct(value, baseline)),
            "Confidence": confidence
                .map(|value| format!("{value:.1}×"))
                .unwrap_or_else(|| "N/A".to_string()),
            "Iteration": iteration,
            "MaxIterations": max_iterations,
            "RecentHistory": if recent_history.is_empty() {
                None
            } else {
                Some(recent_history)
            },
            "DeadEnds": if dead_ends.is_empty() { None } else { Some(dead_ends) },
            "FilesInScope": config.files_in_scope,
            "Constraints": if config.constraints.is_empty() {
                None
            } else {
                Some(&config.constraints)
            },
        }),
    )
}

// ── JSONL persistence ──

fn jsonl_path() -> String {
    dex_path("research.jsonl")
}

fn append_result(entry: &ResearchEntry) -> Result<(), String> {
    use std::io::Write;
    let line = serde_json::to_string(entry).map_err(|e| format!("serialize result: {}", e))?;
    let mut file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(jsonl_path())
        .map_err(|e| format!("open research.jsonl: {}", e))?;
    writeln!(file, "{}", line).map_err(|e| format!("append research.jsonl: {}", e))
}

fn load_state() -> Result<Option<ResearchState>, String> {
    let content = match fs::read_to_string(jsonl_path()) {
        Ok(c) => c,
        Err(_) => return Ok(None),
    };

    let mut config: Option<ResearchConfig> = None;
    let mut results: Vec<ResearchEntry> = Vec::new();
    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }

        let val: serde_json::Value =
            serde_json::from_str(line).map_err(|e| format!("parse jsonl line: {}", e))?;
        if val.get("type").and_then(|v| v.as_str()) == Some("config") {
            config = Some(serde_json::from_value(val).map_err(|e| format!("parse config: {}", e))?);
        } else {
            results.push(serde_json::from_value(val).map_err(|e| format!("parse result: {}", e))?);
        }
    }

    Ok(config.map(|config| ResearchState { config, results }))
}

fn load_existing_state() -> Result<ResearchState, String> {
    load_state()?.ok_or_else(|| MISSING_RESEARCH_SESSION.to_string())
}

fn save_research_config(config: &ResearchConfig) -> Result<(), String> {
    let line = serde_json::to_string(config).map_err(|e| format!("serialize config: {}", e))?;
    fs::write(jsonl_path(), format!("{}\n", line))
        .map_err(|e| format!("write research.jsonl: {}", e))
}

fn current_attempt(head_before: String) -> AttemptDetails {
    let commit_sha = git_trimmed_output(&["rev-parse", "--short=7", "HEAD"])
        .unwrap_or_else(|_| "???????".to_string());
    let description = git_trimmed_output(&["log", "-1", "--format=%B"])
        .unwrap_or_else(|_| "(no message)".to_string())
        .trim_start_matches("research:")
        .trim_start_matches("research ")
        .trim()
        .to_string();

    AttemptDetails {
        head_before,
        commit_sha,
        description,
    }
}

fn record_result(
    results: &mut Vec<ResearchEntry>,
    attempt: &AttemptDetails,
    revert: bool,
    metric: f64,
    status: &str,
    description: String,
    confidence: Option<f64>,
) -> Result<(), String> {
    if revert {
        let _ = git_revert_to(&attempt.head_before);
    }

    let entry = ResearchEntry {
        run: results.len() + 1,
        commit: attempt.commit_sha.clone(),
        metric,
        status: status.to_string(),
        description,
        timestamp: now_timestamp(),
        confidence,
    };
    append_result(&entry)?;
    results.push(entry);
    Ok(())
}

// ── Benchmark execution ──

fn run_benchmark(command: &str, timeout: Duration) -> Result<BenchmarkOutcome, String> {
    let start = Instant::now();

    #[cfg(target_os = "windows")]
    let mut cmd = {
        let mut cmd = std::process::Command::new("cmd");
        cmd.args(["/C", command]);
        cmd
    };
    #[cfg(not(target_os = "windows"))]
    let mut cmd = {
        let mut cmd = std::process::Command::new("sh");
        cmd.args(["-c", command]);
        cmd
    };
    cmd.stdout(Stdio::piped()).stderr(Stdio::piped());

    let child = SharedChild::spawn(&mut cmd).map_err(|e| format!("spawn benchmark: {}", e))?;
    let child = Arc::new(child);
    track_child(&child);

    let child_for_timeout = Arc::clone(&child);
    let timed_out = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let timed_out_flag = Arc::clone(&timed_out);
    let (done_tx, done_rx) = mpsc::channel();
    let timeout_thread = std::thread::spawn(move || {
        if done_rx.recv_timeout(timeout).is_err() {
            timed_out_flag.store(true, std::sync::atomic::Ordering::SeqCst);
            let _ = child_for_timeout.kill();
        }
    });

    let stdout_pipe = child.take_stdout().ok_or("no stdout from benchmark")?;
    let stderr_pipe = child.take_stderr().ok_or("no stderr from benchmark")?;

    let stdout_thread = std::thread::spawn(move || {
        let mut buf = String::new();
        let mut reader = std::io::BufReader::new(stdout_pipe);
        let _ = reader.read_to_string(&mut buf);
        buf
    });
    let stderr_thread = std::thread::spawn(move || {
        let mut buf = String::new();
        let mut reader = std::io::BufReader::new(stderr_pipe);
        let _ = reader.read_to_string(&mut buf);
        buf
    });

    let stdout_output = stdout_thread.join().unwrap_or_default();
    let stderr_output = stderr_thread.join().unwrap_or_default();

    let status = child.wait().map_err(|e| format!("wait benchmark: {}", e))?;
    let duration_secs = start.elapsed().as_secs_f64();
    let _ = done_tx.send(());
    let _ = timeout_thread.join();
    untrack_child(&child);

    let did_timeout = timed_out.load(std::sync::atomic::Ordering::SeqCst);
    let combined = format!("{}\n{}", stdout_output, stderr_output);
    let metrics = parse_metric_lines(&combined);

    Ok(BenchmarkOutcome {
        exit_code: status.code(),
        duration_secs,
        timed_out: did_timeout,
        metrics,
    })
}

// ── Git helpers ──

fn require_clean_worktree() -> Result<(), String> {
    let status = git_trimmed_output(&["status", "--porcelain"])?;
    if !status.is_empty() {
        return Err(
            "working tree is dirty — commit or stash your changes before starting research.\n\
             Dirty files:\n"
                .to_string()
                + &status,
        );
    }
    Ok(())
}

fn git_revert_to(sha: &str) -> Result<(), String> {
    git_trimmed_output(&["reset", "--hard", sha])?;
    git_trimmed_output(&["clean", "-fd"])?;
    Ok(())
}

fn git_clean_working_tree() -> Result<(), String> {
    let toplevel = git_trimmed_output(&["rev-parse", "--show-toplevel"]).unwrap_or_default();
    let pathspec = if toplevel.is_empty() { "." } else { &toplevel };
    git_trimmed_output(&["checkout", "--", pathspec])?;
    git_trimmed_output(&["clean", "-fd", pathspec])?;
    Ok(())
}

// ── Interactive setup ──

pub fn interactive_setup(goal: &str) -> Result<ResearchConfig, String> {
    let command = prompt_line("Benchmark command:", "");
    if command.is_empty() {
        return Err("benchmark command is required".to_string());
    }

    let metric_name = prompt_line("Primary metric name:", "(default: duration_s)");
    let metric_name = if metric_name.is_empty() {
        "duration_s".to_string()
    } else {
        metric_name
    };

    let metric_unit = prompt_line("Metric unit:", "(e.g. µs, ms, s, kb — blank for none)");

    let direction = prompt_choice(
        "Optimization direction — which is better?",
        &["lower", "higher"],
    );

    let files_in_scope = prompt_line("Files in scope:", "(comma-separated, blank for all)");
    let files_in_scope = if files_in_scope.is_empty() {
        "(all project files)".to_string()
    } else {
        files_in_scope
    };

    let constraints = prompt_line("Constraints:", "(e.g. tests must pass — blank for none)");

    let checks_input = prompt_line("Checks command:", "(runs after benchmark, blank for none)");
    let checks_command = if checks_input.is_empty() {
        None
    } else {
        Some(checks_input)
    };

    Ok(ResearchConfig {
        entry_type: "config".to_string(),
        goal: goal.to_string(),
        command,
        metric_name,
        metric_unit,
        direction,
        files_in_scope,
        constraints,
        checks_command,
    })
}

// ── Status / Clear ──

pub fn research_status() -> Result<(), String> {
    let ResearchState { config, results } = load_existing_state()?;

    banner("RESEARCH STATUS");
    phase_detail("goal", &config.goal);
    phase_detail("metric", &config.metric_summary());

    if results.is_empty() {
        phase_detail("runs", "0");
        return Ok(());
    }

    let baseline = results.first().map(|r| r.metric).unwrap_or(0.0);
    phase_detail("baseline", &config.format_metric_value(baseline));

    if let Some(best) =
        best_kept_metric(&results, &config.direction).filter(|&best| best != baseline)
    {
        phase_detail(
            "best",
            &format!(
                "{} ({}%)",
                config.format_metric_value(best),
                format_delta_pct(best, baseline)
            ),
        );
    }

    if let Some(confidence) = compute_confidence(&results, baseline, &config.direction) {
        phase_detail(
            "confidence",
            &format!("{confidence:.1}× ({})", confidence_label(confidence)),
        );
    }

    let kept = results.iter().filter(|r| r.status == "keep").count();
    let discarded = results.iter().filter(|r| r.status == "discard").count();
    let crashed = results.iter().filter(|r| r.status == "crash").count();
    let checks_failed = results
        .iter()
        .filter(|r| r.status == "checks_failed")
        .count();
    let mut detail = format!("{} total, {} kept", results.len(), kept);
    if discarded > 0 {
        detail.push_str(&format!(", {} discarded", discarded));
    }
    if crashed > 0 {
        detail.push_str(&format!(", {} crashed", crashed));
    }
    if checks_failed > 0 {
        detail.push_str(&format!(", {} checks_failed", checks_failed));
    }
    phase_detail("runs", &detail);
    Ok(())
}

pub fn research_clear() -> Result<(), String> {
    let mut removed = false;
    for name in &["research.jsonl"] {
        if fs::remove_file(dex_path(name)).is_ok() {
            removed = true;
        }
    }
    if removed {
        info("Research session cleared.");
    } else {
        info("No research session files to clear.");
    }
    Ok(())
}

// ── Entry points ──

pub fn research_new(
    runner: &Runner,
    config: ResearchConfig,
    max_iterations: Option<usize>,
) -> Result<(), String> {
    require_clean_worktree()?;

    banner("RESEARCH SETUP");
    phase_detail("goal", &config.goal);
    phase_detail("command", &config.command);
    phase_detail("metric", &config.metric_summary());

    let branch_name = format!("research/{}-{}", slugify(&config.goal), now_timestamp());
    git_trimmed_output(&["checkout", "-b", &branch_name])?;
    info(&format!("Branch: {}", branch_name));

    ensure_dex_dir();
    save_research_config(&config)?;

    info("Running baseline benchmark...");
    let outcome = run_benchmark(&config.command, Duration::from_secs(BENCHMARK_TIMEOUT_SECS))?;
    if outcome.timed_out {
        return Err("baseline benchmark timed out".to_string());
    }
    if outcome.exit_code != Some(0) {
        return Err(format!(
            "baseline benchmark failed (exit {:?})",
            outcome.exit_code
        ));
    }

    let primary = extract_primary_metric(&outcome, &config.metric_name).ok_or(format!(
        "baseline produced no METRIC line for {:?}",
        config.metric_name
    ))?;

    let baseline_entry = ResearchEntry {
        run: 1,
        commit: git_trimmed_output(&["rev-parse", "--short=7", "HEAD"])
            .unwrap_or_else(|_| "0000000".to_string()),
        metric: primary,
        status: "keep".to_string(),
        description: "baseline".to_string(),
        timestamp: now_timestamp(),
        confidence: None,
    };
    append_result(&baseline_entry)?;

    info(&format!(
        "Baseline: {} = {}",
        config.metric_name,
        config.format_metric_value(primary)
    ));

    research_loop(runner, config, vec![baseline_entry], max_iterations)
}

pub fn research_resume(runner: &Runner, max_iterations: Option<usize>) -> Result<(), String> {
    let ResearchState { config, results } = load_existing_state()?;

    require_clean_worktree()?;

    banner("RESEARCH RESUME");
    phase_detail("goal", &config.goal);
    phase_detail("runs so far", &results.len().to_string());

    if let Some(best) = best_kept_metric(&results, &config.direction) {
        let baseline = results.first().map(|r| r.metric).unwrap_or(0.0);
        phase_detail(
            "current best",
            &format!(
                "{} ({}% from baseline)",
                config.format_metric_value(best),
                format_delta_pct(best, baseline)
            ),
        );
    }

    research_loop(runner, config, results, max_iterations)
}

// ── The loop ──

fn research_loop(
    runner: &Runner,
    config: ResearchConfig,
    mut results: Vec<ResearchEntry>,
    max_iterations: Option<usize>,
) -> Result<(), String> {
    let baseline = results.first().map(|r| r.metric).unwrap_or(0.0);
    let starting_run = results.len();
    let mut consecutive_failures: usize = 0;
    let timeout = Duration::from_secs(BENCHMARK_TIMEOUT_SECS);

    let _ = git_clean_working_tree();
    banner("RESEARCH");

    loop {
        let iteration = results.len() - starting_run + 1;
        if let Some(max) = max_iterations {
            if iteration > max {
                info(&format!("Reached max iterations ({}).", max));
                break;
            }
            phase_detail("iteration", &format!("{}/{}", iteration, max));
        } else {
            phase_detail("iteration", &iteration.to_string());
        }

        let reference = best_kept_metric(&results, &config.direction).unwrap_or(baseline);
        let confidence = compute_confidence(&results, baseline, &config.direction);
        let prompt = build_research_prompt(
            &config,
            &results,
            baseline,
            confidence,
            iteration,
            max_iterations,
        );

        let head_before =
            git_trimmed_output(&["rev-parse", "HEAD"]).map_err(|e| format!("git: {}", e))?;

        phase_detail("agent", "running...");
        if let Err(e) = runner.run(&prompt) {
            consecutive_failures += 1;
            warn(&format!(
                "Agent failed ({}/{}): {}",
                consecutive_failures, MAX_CONSECUTIVE_AGENT_FAILURES, e
            ));
            let _ = git_revert_to(&head_before);
            if consecutive_failures >= MAX_CONSECUTIVE_AGENT_FAILURES {
                return Err(format!(
                    "research aborted: agent failed {} times in a row",
                    MAX_CONSECUTIVE_AGENT_FAILURES
                ));
            }
            continue;
        }
        consecutive_failures = 0;

        let _ = git_clean_working_tree();

        let head_after = git_trimmed_output(&["rev-parse", "HEAD"]).unwrap_or_default();
        if head_after == head_before {
            warn("Agent made no changes. Skipping benchmark.");
            continue;
        }

        let attempt = current_attempt(head_before);
        let description = attempt.description.clone();

        phase_detail("benchmark", &format!("running {}...", config.command));
        let outcome = match run_benchmark(&config.command, timeout) {
            Ok(outcome) if !outcome.timed_out && outcome.exit_code == Some(0) => outcome,
            Ok(outcome) => {
                let reason = if outcome.timed_out {
                    "timeout"
                } else {
                    "benchmark failed"
                };
                warn(&format!("Benchmark {}: reverting.", reason));
                record_result(
                    &mut results,
                    &attempt,
                    true,
                    0.0,
                    "crash",
                    format!("{} ({})", description, reason),
                    None,
                )?;
                continue;
            }
            Err(e) => {
                err_msg(&format!("Benchmark spawn error: {}", e));
                record_result(
                    &mut results,
                    &attempt,
                    true,
                    0.0,
                    "crash",
                    format!("{} (spawn error)", description),
                    None,
                )?;
                continue;
            }
        };

        let primary = match extract_primary_metric(&outcome, &config.metric_name) {
            Some(value) => value,
            None => {
                warn(&format!(
                    "No METRIC line for {:?} in output: reverting.",
                    config.metric_name
                ));
                record_result(
                    &mut results,
                    &attempt,
                    true,
                    0.0,
                    "crash",
                    format!("{} (metric not found)", description),
                    None,
                )?;
                continue;
            }
        };

        if let Some(ref checks_cmd) = config.checks_command {
            phase_detail("checks", &format!("running {}...", checks_cmd));
            match run_benchmark(checks_cmd, timeout) {
                Ok(outcome) if outcome.exit_code == Some(0) && !outcome.timed_out => {
                    info("Checks passed.");
                }
                Ok(_) => {
                    warn("Checks failed: reverting.");
                    record_result(
                        &mut results,
                        &attempt,
                        true,
                        primary,
                        "checks_failed",
                        format!("{} (checks failed)", description),
                        confidence,
                    )?;
                    continue;
                }
                Err(e) => {
                    warn(&format!("Checks error: {}", e));
                    record_result(
                        &mut results,
                        &attempt,
                        true,
                        primary,
                        "checks_failed",
                        format!("{} (checks error)", description),
                        None,
                    )?;
                    continue;
                }
            }
        }

        let improved = is_better(primary, reference, &config.direction);
        let status = if improved { "keep" } else { "discard" };
        record_result(
            &mut results,
            &attempt,
            !improved,
            primary,
            status,
            description.clone(),
            confidence,
        )?;

        let summary = format!(
            "#{} {} — {} ({}% from baseline) — {}",
            results.len(),
            status.to_uppercase(),
            config.format_metric_value(primary),
            format_delta_pct(primary, baseline),
            description,
        );
        if improved {
            info(&summary);
        } else {
            warn(&summary);
        }

        if let Some(confidence) = confidence {
            phase_detail(
                "confidence",
                &format!("{confidence:.1}× ({})", confidence_label(confidence)),
            );
        }
    }

    banner("RESEARCH DONE");
    let _ = research_status();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(
        run: usize,
        commit: &str,
        metric: f64,
        status: &str,
        description: &str,
    ) -> ResearchEntry {
        ResearchEntry {
            run,
            commit: commit.into(),
            metric,
            status: status.into(),
            description: description.into(),
            timestamp: 0,
            confidence: None,
        }
    }

    #[test]
    fn parse_metric_lines_cases() {
        for (output, expected) in [
            (
                "some output\nMETRIC total_us=15200\nMETRIC compile_us=4200\nother\n",
                vec![
                    ("total_us".to_string(), 15200.0),
                    ("compile_us".to_string(), 4200.0),
                ],
            ),
            (
                "METRIC x=100\nMETRIC x=200\n",
                vec![("x".to_string(), 200.0)],
            ),
            (
                "METRIC a=NaN\nMETRIC b=inf\nMETRIC c=42\n",
                vec![("c".to_string(), 42.0)],
            ),
        ] {
            assert_eq!(parse_metric_lines(output), expected);
        }
    }

    #[test]
    fn is_better_cases() {
        for (current, reference, direction, expected) in [
            (90.0, 100.0, "lower", true),
            (110.0, 100.0, "lower", false),
            (110.0, 100.0, "higher", true),
            (90.0, 100.0, "higher", false),
        ] {
            assert_eq!(is_better(current, reference, direction), expected);
        }
    }

    #[test]
    fn format_metric_cases() {
        for (value, expected) in [
            (15200.0, "15,200"),
            (0.0, "0"),
            (999.0, "999"),
            (1000.0, "1,000"),
            (15.23, "15.23"),
            (1234.5, "1,234.50"),
        ] {
            assert_eq!(format_metric(value), expected);
        }
    }

    #[test]
    fn format_delta_pct_cases() {
        for (current, baseline, expected) in [
            (90.0, 100.0, "-10.0"),
            (120.0, 100.0, "+20.0"),
            (42.0, 0.0, "N/A"),
        ] {
            assert_eq!(format_delta_pct(current, baseline), expected);
        }
    }

    #[test]
    fn sorted_median_cases() {
        for (values, expected) in [
            (&[3.0, 1.0, 2.0][..], 2.0),
            (&[1.0, 2.0, 3.0, 4.0][..], 2.5),
        ] {
            assert_eq!(sorted_median(values), expected);
        }
    }

    #[test]
    fn slugify_basic() {
        assert_eq!(slugify("Optimize Test Runtime"), "optimize-test-runtime");
        assert_eq!(slugify("  hello  world  "), "hello-world");
    }

    #[test]
    fn confidence_insufficient_data() {
        let results = vec![
            entry(1, "a", 100.0, "keep", "baseline"),
            entry(2, "b", 95.0, "keep", "opt"),
        ];
        assert!(compute_confidence(&results, 100.0, "lower").is_none());
    }

    #[test]
    fn confidence_with_data() {
        let results = vec![
            entry(1, "a", 100.0, "keep", "baseline"),
            entry(2, "b", 95.0, "keep", "opt1"),
            entry(3, "c", 98.0, "discard", "opt2"),
            entry(4, "d", 90.0, "keep", "opt3"),
        ];
        let conf = compute_confidence(&results, 100.0, "lower");
        assert!(conf.is_some());
        assert!(conf.unwrap() > 0.0);
    }
}
