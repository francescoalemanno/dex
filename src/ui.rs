use std::io::{self, BufRead, Write};
use termcolor::{Color, ColorChoice, ColorSpec, StandardStream, WriteColor};
use termimad::MadSkin;

fn stderr() -> StandardStream {
    StandardStream::stderr(ColorChoice::Auto)
}

fn write_styled(color: Option<Color>, bold: bool, dimmed: bool, text: &str) {
    let mut stream = stderr();
    let mut spec = ColorSpec::new();
    spec.set_fg(color).set_bold(bold).set_dimmed(dimmed);
    let _ = stream.set_color(&spec);
    let _ = write!(stream, "{}", text);
    let _ = stream.reset();
}

pub fn banner(phase: &str) {
    let mut stream = stderr();
    let _ = writeln!(stream);
    write_styled(
        Some(Color::Cyan),
        true,
        false,
        &format!("══════ {} ══════", phase),
    );
    let _ = writeln!(stream);
    let _ = writeln!(stream);
}

pub fn info(msg: &str) {
    write_styled(Some(Color::Green), true, false, &format!("▸ {}", msg));
    let _ = writeln!(stderr());
}

pub fn warn(msg: &str) {
    write_styled(Some(Color::Yellow), true, false, &format!("▸ {}", msg));
    let _ = writeln!(stderr());
}

pub fn err_msg(msg: &str) {
    write_styled(Some(Color::Red), true, false, &format!("▸ {}", msg));
    let _ = writeln!(stderr());
}

pub fn write_dim(stream: &mut StandardStream, text: &str) {
    let mut spec = ColorSpec::new();
    spec.set_bold(true).set_dimmed(true);
    let _ = stream.set_color(&spec);
    let _ = write!(stream, "{}", text);
    let _ = stream.reset();
}

pub fn show_block(title: &str, content: &str) {
    let mut stream = stderr();
    let _ = writeln!(stream);
    write_dim(&mut stream, &format!("── {} ──", title));
    let _ = writeln!(stream);
    let _ = writeln!(stream, "{}", content);
    write_dim(&mut stream, "── end ──");
    let _ = writeln!(stream);
    let _ = writeln!(stream);
}

pub fn show_markdown(title: &str, md: &str) {
    let mut stream = stderr();
    let _ = writeln!(stream);
    write_dim(&mut stream, &format!("── {} ──", title));
    let _ = writeln!(stream);
    let skin = MadSkin::default();
    let rendered = skin.term_text(md);
    let _ = write!(stream, "{}", rendered);
    write_dim(&mut stream, "── end ──");
    let _ = writeln!(stream);
    let _ = writeln!(stream);
}

pub fn prompt_multiline(msg: &str) -> String {
    let mut stream = stderr();
    let mut spec = ColorSpec::new();
    spec.set_bold(true);
    let _ = stream.set_color(&spec);
    let _ = write!(stream, "{}", msg);
    let _ = stream.reset();
    let _ = write!(stream, " ");
    write_dim(&mut stream, "(single .  to finish)");
    let _ = writeln!(stream);
    let _ = stream.flush();

    let stdin = io::stdin();
    let mut lines: Vec<String> = Vec::new();
    for line in stdin.lock().lines() {
        match line {
            Ok(l) => {
                if l.trim() == "." && !lines.is_empty() {
                    break;
                }
                lines.push(l);
            }
            Err(_) => break,
        }
    }
    lines.join("\n")
}

pub fn prompt_choice(msg: &str, choices: &[&str]) -> String {
    let joined = choices.join("/");
    loop {
        let mut stream = stderr();
        let mut spec = ColorSpec::new();
        spec.set_bold(true);
        let _ = stream.set_color(&spec);
        let _ = write!(stream, "{}", msg);
        let _ = stream.reset();
        let _ = write!(stream, " [{}] ", joined);
        let _ = stream.flush();

        let mut input = String::new();
        if io::stdin().read_line(&mut input).is_err() {
            continue;
        }
        let ans = input.trim().to_lowercase();
        for c in choices {
            let cl = c.to_lowercase();
            if cl == ans || (ans.len() == 1 && cl.starts_with(&ans)) {
                return cl;
            }
        }
        eprintln!("Invalid choice, try again.");
    }
}

/// Write a bold+cyan timestamp prefix to stderr. Used by runner.
pub fn write_timestamp(stream: &mut StandardStream, text: &str) {
    let mut spec = ColorSpec::new();
    spec.set_bold(true).set_fg(Some(Color::Cyan));
    let _ = stream.set_color(&spec);
    let _ = write!(stream, "{}", text);
    let _ = stream.reset();
}
