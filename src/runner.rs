use serde_json::Value;
use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};
use termcolor::{ColorChoice, StandardStream};

use crate::ui::{err_msg, warn, write_timestamp};

pub struct CLIConfig {
    pub cmd: &'static str,
    pub args: &'static [&'static str],
    pub stdin: bool,
    pub env: &'static [(&'static str, &'static str)],
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
        },
    ),
    (
        "codex",
        CLIConfig {
            cmd: "codex",
            args: &["exec", "--yolo", "--ephemeral", "--json"],
            stdin: true,
            env: &[],
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
        },
    ),
    (
        "droid",
        CLIConfig {
            cmd: "droid",
            args: &["exec", "--skip-permissions-unsafe"],
            stdin: false,
            env: &[],
        },
    ),
    (
        "gemini",
        CLIConfig {
            cmd: "gemini",
            args: &["-y", "-p"],
            stdin: false,
            env: &[],
        },
    ),
    (
        "pi",
        CLIConfig {
            cmd: "pi",
            args: &["--no-session", "-p"],
            stdin: false,
            env: &[],
        },
    ),
    (
        "raijin",
        CLIConfig {
            cmd: "raijin",
            args: &["-ephemeral", "-no-echo", "-no-thinking"],
            stdin: false,
            env: &[],
        },
    ),
];

fn builtin_agent_names() -> Vec<&'static str> {
    CLI_CONFIGS.iter().map(|(name, _)| *name).collect()
}

fn dex_available_agents_with<F>(mut is_available: F) -> Vec<&'static str>
where
    F: FnMut(&str) -> bool,
{
    CLI_CONFIGS
        .iter()
        .filter_map(|(name, config)| is_available(config.cmd).then_some(*name))
        .collect()
}

pub fn dex_available_agents() -> Vec<&'static str> {
    dex_available_agents_with(|cmd| which::which(cmd).is_ok())
}

fn validate_cli_name_with_available(name: &str, available: &[&str]) -> Result<(), String> {
    let is_builtin = CLI_CONFIGS.iter().any(|(candidate, _)| *candidate == name);
    if !is_builtin {
        let supported = builtin_agent_names().join(", ");
        return Err(match available {
            [] => format!(
                "unknown CLI {:?}; supported agents: {}; none are currently available in PATH",
                name, supported
            ),
            _ => format!(
                "unknown CLI {:?}; choose one of the agents currently available in PATH: {}",
                name,
                available.join(", ")
            ),
        });
    }

    if available.iter().any(|candidate| *candidate == name) {
        return Ok(());
    }

    Err(match available {
        [] => format!(
            "CLI {:?} is supported but not currently available in PATH; no supported agents were found",
            name
        ),
        _ => format!(
            "CLI {:?} is not currently available in PATH; available agents: {}",
            name,
            available.join(", ")
        ),
    })
}

pub fn validate_cli_name(name: &str) -> Result<(), String> {
    let available = dex_available_agents();
    validate_cli_name_with_available(name, &available)
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

        // Set process group on unix
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            cmd.process_group(0);
        }

        let mut child = cmd
            .spawn()
            .map_err(|e| format!("spawn {}: {}", cfg.cmd, e))?;

        if cfg.stdin {
            use std::io::Write;
            if let Some(mut stdin) = child.stdin.take() {
                let _ = stdin.write_all(prompt.as_bytes());
                // stdin is dropped here, closing the pipe
            }
        }

        let stdout = child.stdout.take().ok_or("no stdout")?;
        let stderr = child.stderr.take().ok_or("no stderr")?;

        let (tx, rx) = mpsc::channel();
        let start = Instant::now();
        let mut last_output = start;

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
                    if process_stdout_line(&text, start, &self.label) {
                        last_output = Instant::now();
                    } else if last_output.elapsed() >= Duration::from_secs(60) {
                        let mut stream = StandardStream::stderr(ColorChoice::Auto);
                        write_prefix(&mut stream, start, &self.label);
                        let _ = writeln!(stream, " Working on it");
                        last_output = Instant::now();
                    }
                }
                Ok(StreamLine::Stderr(text)) => {
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
                    kill_process(&mut child);
                    let _ = child.wait();
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
        if status.success() {
            Ok(())
        } else {
            Err(format!("exit code: {}", status))
        }
    }
}

fn kill_process(child: &mut std::process::Child) {
    #[cfg(unix)]
    {
        let pid = child.id() as i32;
        let _ = nix_kill_pg(pid);
    }
    #[cfg(not(unix))]
    {
        let _ = child.kill();
    }
}

#[cfg(unix)]
fn nix_kill_pg(pid: i32) -> Result<(), ()> {
    // Use libc::kill with negative pid for process group
    // This is safe: kill() is a POSIX function, not unsafe Rust memory access.
    // We wrap it because std::process doesn't expose process group killing.
    let ret = std::process::Command::new("kill")
        .args(["--", &format!("-{}", pid)])
        .status();
    match ret {
        Ok(s) if s.success() => Ok(()),
        _ => {
            let _ = std::process::Command::new("kill")
                .args([&pid.to_string()])
                .status();
            Ok(())
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

fn process_stdout_line(text: &str, start: Instant, label: &str) -> bool {
    let mut stream = StandardStream::stderr(ColorChoice::Auto);

    if let Ok(obj) = serde_json::from_str::<Value>(text) {
        if let Some(map) = obj.as_object() {
            let texts = extract_texts(&Value::Object(map.clone()));
            if !texts.is_empty() {
                for t in &texts {
                    write_prefix(&mut stream, start, label);
                    let _ = writeln!(stream, " {}", t);
                }
                return true;
            }
        }
        return false;
    }
    write_prefix(&mut stream, start, label);
    let _ = writeln!(stream, " {}", text);
    true
}

fn extract_texts(v: &Value) -> Vec<String> {
    let mut out = Vec::new();
    walk_json(v, &mut out);
    out
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

#[cfg(test)]
mod tests {
    use super::{dex_available_agents_with, validate_cli_name_with_available};

    #[test]
    fn filters_builtins_by_runtime_availability() {
        let available = dex_available_agents_with(|cmd| matches!(cmd, "codex" | "gemini"));

        assert_eq!(available, vec!["codex", "gemini"]);
    }

    #[test]
    fn rejects_supported_but_missing_agent() {
        let err = validate_cli_name_with_available("claude", &["codex", "gemini"]).unwrap_err();

        assert_eq!(
            err,
            "CLI \"claude\" is not currently available in PATH; available agents: codex, gemini"
        );
    }

    #[test]
    fn rejects_unknown_agent_with_available_choices() {
        let err = validate_cli_name_with_available("unknown", &["codex", "gemini"]).unwrap_err();

        assert_eq!(
            err,
            "unknown CLI \"unknown\"; choose one of the agents currently available in PATH: codex, gemini"
        );
    }
}
