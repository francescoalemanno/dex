use serde_json::Value;
use shared_child::SharedChild;
use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, Instant};
use termcolor::{ColorSpec, StandardStream, WriteColor};

use crate::core::{CliConfig, OutputFormat};
use crate::ui::{err_msg, locked_stderr, phase_detail, show_markdown, warn};

static VERBOSE: AtomicBool = AtomicBool::new(false);

pub fn set_verbose(v: bool) {
    VERBOSE.store(v, Ordering::Relaxed);
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
pub struct Runner {
    config: CliConfig,
    timeout: Duration,
    label: String,
}

enum StreamLine {
    Stdout(String),
    Stderr(String),
    Done,
}

impl Runner {
    pub fn new(config: CliConfig, timeout: Duration) -> Self {
        Runner {
            config,
            timeout,
            label: String::new(),
        }
    }

    pub fn labeled(&self, label: &str) -> Self {
        Runner {
            config: self.config.clone(),
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
                delay = (delay * 8).min(Duration::from_secs(3600));
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
        let cfg = &self.config;
        let mut args = cfg.args.clone();
        if !cfg.stdin {
            args.push(prompt.to_string());
        }

        // Show the exec command (without the prompt argument for readability)
        let display = if cfg.args.is_empty() {
            cfg.command.clone()
        } else {
            format!("{} {}", cfg.command, cfg.args.join(" "))
        };
        phase_detail("exec", &display);

        if VERBOSE.load(Ordering::Relaxed) {
            show_markdown("Prompt", prompt);
        }

        let mut cmd = Command::new(&cfg.command);
        cmd.args(&args);
        cmd.stdout(Stdio::piped());
        cmd.stderr(Stdio::piped());

        if !cfg.env.is_empty() {
            cmd.envs(&cfg.env);
        }

        cmd.stdin(if cfg.stdin {
            Stdio::piped()
        } else {
            Stdio::null()
        });

        let child =
            SharedChild::spawn(&mut cmd).map_err(|e| format!("spawn {}: {}", cfg.command, e))?;
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
                    let displayed = match cfg.output_format {
                        OutputFormat::Plain => display_plain(&text, start, &self.label),
                        OutputFormat::JsonNd => display_jsonnd(&text, start, &self.label),
                        OutputFormat::PiJsonNd => display_pi_jsonnd(&text, start, &self.label),
                    };
                    if displayed {
                        last_display = Instant::now();
                    } else if last_display.elapsed() >= Duration::from_secs(60) {
                        let silent_secs = last_display.elapsed().as_secs();
                        let mut stream = locked_stderr();
                        write_prefix(&mut stream, start, &self.label);
                        let mut spec = ColorSpec::new();
                        spec.set_dimmed(true);
                        let _ = stream.set_color(&spec);
                        let _ = writeln!(stream, " · still working ({}s silent)", silent_secs);
                        let _ = stream.reset();
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
    let mut spec = ColorSpec::new();
    spec.set_dimmed(true);
    let _ = stream.set_color(&spec);
    if h > 0 {
        let _ = write!(stream, "[{:02}:{:02}:{:02}]", h, m, s);
    } else {
        let _ = write!(stream, "[{:02}:{:02}]", m, s);
    }
    let _ = stream.reset();
    if !label.is_empty() {
        let mut label_spec = ColorSpec::new();
        label_spec.set_fg(Some(termcolor::Color::Magenta)).set_bold(true);
        let _ = stream.set_color(&label_spec);
        let _ = write!(stream, " [{}]", label);
        let _ = stream.reset();
    }
}

fn display_plain(text: &str, start: Instant, label: &str) -> bool {
    let mut stream = locked_stderr();
    write_prefix(&mut stream, start, label);
    let _ = writeln!(stream, " {}", text);
    true
}

fn display_jsonnd(text: &str, start: Instant, label: &str) -> bool {
    let Ok(obj) = serde_json::from_str::<Value>(text) else {
        return display_plain(text, start, label);
    };
    if !obj.is_object() {
        return false;
    }

    let mut texts = Vec::new();
    walk_json(&obj, &mut texts);
    if texts.is_empty() {
        return false;
    }

    let mut stream = locked_stderr();
    for t in &texts {
        write_prefix(&mut stream, start, label);
        let _ = writeln!(stream, " {}", t);
    }
    true
}

fn display_pi_jsonnd(text: &str, start: Instant, label: &str) -> bool {
    let Ok(obj) = serde_json::from_str::<Value>(text) else {
        return display_plain(text, start, label);
    };
    if obj.get("type").and_then(|v| v.as_str()) != Some("message_end") {
        return false;
    }
    let Some(content) = obj
        .get("message")
        .filter(|msg| msg.get("role").and_then(|v| v.as_str()) == Some("assistant"))
        .and_then(|msg| msg.get("content"))
        .and_then(|v| v.as_array())
    else {
        return false;
    };

    let mut displayed = false;
    let mut stream = locked_stderr();
    for item in content {
        if item.get("type").and_then(|v| v.as_str()) != Some("text") {
            continue;
        }
        let Some(text) = item.get("text").and_then(|v| v.as_str()) else {
            continue;
        };
        if text.is_empty() {
            continue;
        }
        write_prefix(&mut stream, start, label);
        let _ = writeln!(stream, " {}", text);
        displayed = true;
    }
    displayed
}

fn walk_json(v: &Value, texts: &mut Vec<String>) {
    match v {
        Value::Object(map) => {
            for (k, child) in map {
                if matches!(k.as_str(), "text" | "thinking") {
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
