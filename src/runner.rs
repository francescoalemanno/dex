use serde_json::Value;
use shared_child::SharedChild;
use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, Instant};
use termcolor::StandardStream;

use crate::ui::{err_msg, locked_stderr, phase_detail, show_block, warn, write_timestamp};

static VERBOSE: AtomicBool = AtomicBool::new(false);

pub fn set_verbose(v: bool) {
    VERBOSE.store(v, Ordering::Relaxed);
}

fn is_verbose() -> bool {
    VERBOSE.load(Ordering::Relaxed)
}

static CHILDREN: Mutex<Vec<Arc<SharedChild>>> = Mutex::new(Vec::new());

pub(crate) fn track_child(child: &Arc<SharedChild>) {
    if let Ok(mut children) = CHILDREN.lock() {
        children.push(Arc::clone(child));
    }
}

pub(crate) fn untrack_child(child: &Arc<SharedChild>) {
    if let Ok(mut children) = CHILDREN.lock() {
        children.retain(|c| c.id() != child.id());
    }
}

/// Kill all tracked child processes. Called on exit / signal.
pub fn kill_all_children() {
    let children = match CHILDREN.lock() {
        Ok(children) => children.clone(),
        Err(e) => e.into_inner().clone(),
    };
    for child in &children {
        let _ = child.kill();
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputFormat {
    Plain,
    JsonND,
    PiJsonND,
}

pub struct CLIConfig {
    pub cmd: &'static str,
    pub args: &'static [&'static str],
    pub stdin: bool,
    pub env: &'static [(&'static str, &'static str)],
    pub output_format: OutputFormat,
}

pub static CLI_CONFIGS: &[(&str, CLIConfig)] = &[
    (
        "opencode",
        CLIConfig {
            cmd: "opencode",
            args: &["run", "--thinking", "--format", "json"],
            stdin: true,
            env: &[(
                "OPENCODE_CONFIG_CONTENT",
                r#"{"$schema":"https://opencode.ai/config.json","permission":"allow","lsp":false}"#,
            )],
            output_format: OutputFormat::JsonND,
        },
    ),
    (
        "codex",
        CLIConfig {
            cmd: "codex",
            args: &["exec", "--yolo", "--ephemeral", "--json"],
            stdin: true,
            env: &[],
            output_format: OutputFormat::JsonND,
        },
    ),
    (
        "claude",
        CLIConfig {
            cmd: "claude",
            args: &[
                "--dangerously-skip-permissions",
                "--allow-dangerously-skip-permissions",
                "-p",
            ],
            stdin: false,
            env: &[],
            output_format: OutputFormat::Plain,
        },
    ),
    (
        "droid",
        CLIConfig {
            cmd: "droid",
            args: &["exec", "--skip-permissions-unsafe"],
            stdin: false,
            env: &[],
            output_format: OutputFormat::Plain,
        },
    ),
    (
        "gemini",
        CLIConfig {
            cmd: "gemini",
            args: &["-y", "-p"],
            stdin: false,
            env: &[],
            output_format: OutputFormat::Plain,
        },
    ),
    (
        "pi",
        CLIConfig {
            cmd: "pi",
            args: &["--no-session", "-p", "--mode", "json"],
            stdin: false,
            env: &[],
            output_format: OutputFormat::PiJsonND,
        },
    ),
    (
        "raijin",
        CLIConfig {
            cmd: "raijin",
            args: &["-ephemeral", "-no-echo", "-no-thinking"],
            stdin: false,
            env: &[],
            output_format: OutputFormat::Plain,
        },
    ),
];

pub fn dex_available_agents() -> Vec<&'static str> {
    CLI_CONFIGS
        .iter()
        .filter_map(|(name, config)| which::which(config.cmd).is_ok().then_some(*name))
        .collect()
}

pub fn validate_cli_name(name: &str) -> Result<(), String> {
    let known = CLI_CONFIGS.iter().any(|(k, _)| *k == name);
    if !known {
        let all_names: Vec<&str> = CLI_CONFIGS.iter().map(|(k, _)| *k).collect();
        return Err(format!(
            "unknown CLI {:?}; supported agents: {}",
            name,
            all_names.join(", ")
        ));
    }
    let cfg = &CLI_CONFIGS.iter().find(|(k, _)| *k == name).unwrap().1;
    if which::which(cfg.cmd).is_err() {
        return Err(format!(
            "CLI {:?} is not available in PATH (command {:?} not found)",
            name, cfg.cmd
        ));
    }
    Ok(())
}

pub struct Runner {
    config_idx: usize,
    timeout: Duration,
    label: String,
}

enum StreamLine {
    Stdout(String),
    Stderr(String),
    Done,
}

impl Runner {
    pub fn new(name: &str, timeout: Duration) -> Result<Self, String> {
        validate_cli_name(name)?;
        let idx = CLI_CONFIGS
            .iter()
            .position(|(k, _)| *k == name)
            .ok_or_else(|| format!("unknown CLI {:?}", name))?;
        Ok(Runner {
            config_idx: idx,
            timeout,
            label: String::new(),
        })
    }

    fn cfg(&self) -> &CLIConfig {
        &CLI_CONFIGS[self.config_idx].1
    }

    pub fn labeled(&self, label: &str) -> Self {
        Runner {
            config_idx: self.config_idx,
            timeout: self.timeout,
            label: label.to_string(),
        }
    }

    pub fn run(&self, prompt: &str) -> Result<(), String> {
        let mut delay = Duration::from_secs(1);
        let mut last_err = String::new();
        for attempt in 0..=5 {
            if attempt > 0 {
                warn(&format!(
                    "Retry {}/5 after {:.0}s delay",
                    attempt,
                    delay.as_secs_f64()
                ));
                std::thread::sleep(delay);
                let next = delay * 8;
                delay = if next > Duration::from_secs(3600) {
                    Duration::from_secs(3600)
                } else {
                    next
                };
            }
            match self.run_once(prompt) {
                Ok(()) => return Ok(()),
                Err(e) => {
                    err_msg(&format!("Agent failed: {}", e));
                    last_err = e;
                }
            }
        }
        Err(format!("agent failed after 5 retries: {}", last_err))
    }

    fn run_once(&self, prompt: &str) -> Result<(), String> {
        let cfg = self.cfg();
        let mut args: Vec<&str> = cfg.args.to_vec();
        if !cfg.stdin {
            args.push(prompt);
        }

        // Show the exec command (without the prompt argument for readability)
        let display_args: Vec<&str> = cfg.args.to_vec();
        phase_detail("exec", &format!("{} {}", cfg.cmd, display_args.join(" ")));

        if is_verbose() {
            show_block("Prompt", prompt);
        }

        let mut cmd = Command::new(cfg.cmd);
        cmd.args(&args);
        cmd.stdout(Stdio::piped());
        cmd.stderr(Stdio::piped());

        if !cfg.env.is_empty() {
            for (k, v) in cfg.env {
                cmd.env(k, v);
            }
        }

        if cfg.stdin {
            cmd.stdin(Stdio::piped());
        } else {
            cmd.stdin(Stdio::null());
        }

        let child =
            SharedChild::spawn(&mut cmd).map_err(|e| format!("spawn {}: {}", cfg.cmd, e))?;
        let child = Arc::new(child);

        track_child(&child);

        if cfg.stdin {
            use std::io::Write;
            if let Some(mut stdin) = child.take_stdin() {
                let _ = stdin.write_all(prompt.as_bytes());
            }
        }

        let stdout = child.take_stdout().ok_or("no stdout")?;
        let stderr = child.take_stderr().ok_or("no stderr")?;

        let (tx, rx) = mpsc::channel();
        let start = Instant::now();
        let mut last_display = start;

        let tx_out = tx.clone();
        let stdout_thread = std::thread::spawn(move || {
            let reader = BufReader::new(stdout);
            for line in reader.lines() {
                match line {
                    Ok(text) => {
                        if tx_out.send(StreamLine::Stdout(text)).is_err() {
                            break;
                        }
                    }
                    Err(_) => break,
                }
            }
            let _ = tx_out.send(StreamLine::Done);
        });

        let tx_err = tx.clone();
        let stderr_thread = std::thread::spawn(move || {
            let reader = BufReader::new(stderr);
            for line in reader.lines() {
                match line {
                    Ok(text) => {
                        if tx_err.send(StreamLine::Stderr(text)).is_err() {
                            break;
                        }
                    }
                    Err(_) => break,
                }
            }
            let _ = tx_err.send(StreamLine::Done);
        });

        let mut done_count = 0;
        loop {
            match rx.recv_timeout(self.timeout) {
                Ok(StreamLine::Stdout(text)) => {
                    if process_stdout_line(&text, start, &self.label, cfg.output_format) {
                        last_display = Instant::now();
                    } else if last_display.elapsed() >= Duration::from_secs(60) {
                        let mut stream = locked_stderr();
                        write_prefix(&mut stream, start, &self.label);
                        let _ = writeln!(stream, " Working on it");
                        last_display = Instant::now();
                    }
                }
                Ok(StreamLine::Stderr(text)) => {
                    let _guard = locked_stderr();
                    if self.label.is_empty() {
                        eprintln!("{}", text);
                    } else {
                        eprintln!("[{}] {}", self.label, text);
                    }
                }
                Ok(StreamLine::Done) => {
                    done_count += 1;
                    if done_count >= 2 {
                        break;
                    }
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    let _ = child.kill();
                    let _ = child.wait();
                    untrack_child(&child);
                    return Err(format!("agent idle timeout after {:?}", self.timeout));
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    break;
                }
            }
        }

        let _ = stdout_thread.join();
        let _ = stderr_thread.join();

        let status = child.wait().map_err(|e| format!("wait: {}", e))?;
        untrack_child(&child);
        if status.success() {
            Ok(())
        } else {
            Err(format!("exit code: {}", status))
        }
    }
}

fn write_prefix(stream: &mut StandardStream, start: Instant, label: &str) {
    let d = start.elapsed();
    let h = d.as_secs() / 3600;
    let m = (d.as_secs() % 3600) / 60;
    let s = d.as_secs() % 60;
    write_timestamp(stream, &format!("[{:02}:{:02}:{:02}]", h, m, s));
    if !label.is_empty() {
        let _ = write!(stream, " [{}]", label);
    }
}

fn process_stdout_line(text: &str, start: Instant, label: &str, format: OutputFormat) -> bool {
    match format {
        OutputFormat::Plain => display_plain(text, start, label),
        OutputFormat::JsonND => display_jsonnd(text, start, label),
        OutputFormat::PiJsonND => display_pi_jsonnd(text, start, label),
    }
}

fn display_plain(text: &str, start: Instant, label: &str) -> bool {
    let mut stream = locked_stderr();
    write_prefix(&mut stream, start, label);
    let _ = writeln!(stream, " {}", text);
    true
}

fn display_jsonnd(text: &str, start: Instant, label: &str) -> bool {
    if let Ok(obj) = serde_json::from_str::<Value>(text) {
        if let Some(map) = obj.as_object() {
            let mut texts: Vec<String> = Vec::new();
            walk_json(&Value::Object(map.clone()), &mut texts);
            if !texts.is_empty() {
                let mut stream = locked_stderr();
                for t in &texts {
                    write_prefix(&mut stream, start, label);
                    let _ = writeln!(stream, " {}", t);
                }
                return true;
            }
        }
        return false;
    }
    display_plain(text, start, label)
}

fn display_pi_jsonnd(text: &str, start: Instant, label: &str) -> bool {
    if let Ok(obj) = serde_json::from_str::<Value>(text) {
        if obj.get("type").and_then(|v| v.as_str()) == Some("message_end") {
            if let Some(msg) = obj.get("message") {
                if msg.get("role").and_then(|v| v.as_str()) == Some("assistant") {
                    if let Some(content) = msg.get("content").and_then(|v| v.as_array()) {
                        let mut displayed = false;
                        let mut stream = locked_stderr();
                        for item in content {
                            if item.get("type").and_then(|v| v.as_str()) == Some("text") {
                                if let Some(s) = item.get("text").and_then(|v| v.as_str()) {
                                    if !s.is_empty() {
                                        write_prefix(&mut stream, start, label);
                                        let _ = writeln!(stream, " {}", s);
                                        displayed = true;
                                    }
                                }
                            }
                        }
                        return displayed;
                    }
                }
            }
        }
        return false;
    }
    display_plain(text, start, label)
}

fn walk_json(v: &Value, texts: &mut Vec<String>) {
    match v {
        Value::Object(map) => {
            for (k, child) in map {
                if k == "text" {
                    if let Some(s) = child.as_str() {
                        texts.push(s.to_string());
                        continue;
                    }
                }
                walk_json(child, texts);
            }
        }
        Value::Array(arr) => {
            for item in arr {
                walk_json(item, texts);
            }
        }
        _ => {}
    }
}
