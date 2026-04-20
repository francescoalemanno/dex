use std::io::{self, BufRead, Write};
use std::ops::{Deref, DerefMut};
use std::sync::{Mutex, MutexGuard};
use termcolor::{Color, ColorChoice, ColorSpec, StandardStream, WriteColor};
use termimad::MadSkin;

/// Global lock that serialises all terminal output so parallel threads
/// never interleave their prints.
static PRINT_LOCK: Mutex<()> = Mutex::new(());

/// A guard that holds the global print lock and a `StandardStream` to stderr.
/// Dereferences to `StandardStream` so it can be used directly as a writer.
pub(crate) struct Term {
    _lock: MutexGuard<'static, ()>,
    stream: StandardStream,
}

impl Deref for Term {
    type Target = StandardStream;
    fn deref(&self) -> &StandardStream {
        &self.stream
    }
}

impl DerefMut for Term {
    fn deref_mut(&mut self) -> &mut StandardStream {
        &mut self.stream
    }
}

/// Acquire the global print lock and return a locked stderr writer.
pub(crate) fn locked_stderr() -> Term {
    let guard = match PRINT_LOCK.lock() {
        Ok(g) => g,
        Err(e) => e.into_inner(),
    };
    Term {
        _lock: guard,
        stream: StandardStream::stderr(ColorChoice::Auto),
    }
}
const REVISION: &str = env!("CARGO_PKG_VERSION");

/// Print the application header box in cyan with Unicode box-drawing characters.
pub fn app_header() {
    let mut stream = locked_stderr();
    let line1 = "DEX v".to_owned() + REVISION;
    let line2 = "Agentic Orchestrator";
    let width = line2.len().max(line1.len()) + 4; // padding
    let _ = writeln!(stream);
    let mut spec = ColorSpec::new();
    spec.set_fg(Some(Color::Cyan)).set_bold(true);
    let _ = stream.set_color(&spec);
    let _ = write!(stream, "  \u{250c}");
    for _ in 0..width {
        let _ = write!(stream, "\u{2500}");
    }
    let _ = writeln!(stream, "\u{2510}");
    // line1 centered
    let pad1 = (width.saturating_sub(line1.len())) / 2;
    let _ = write!(stream, "  \u{2502}");
    for _ in 0..pad1 {
        let _ = write!(stream, " ");
    }
    let _ = write!(stream, "{}", line1);
    for _ in 0..width.saturating_sub(pad1 + line1.len()) {
        let _ = write!(stream, " ");
    }
    let _ = writeln!(stream, "\u{2502}");
    // line2 centered
    let pad2 = (width.saturating_sub(line2.len())) / 2;
    let _ = write!(stream, "  \u{2502}");
    for _ in 0..pad2 {
        let _ = write!(stream, " ");
    }
    let _ = write!(stream, "{}", line2);
    for _ in 0..width.saturating_sub(pad2 + line2.len()) {
        let _ = write!(stream, " ");
    }
    let _ = writeln!(stream, "\u{2502}");
    let _ = write!(stream, "  \u{2514}");
    for _ in 0..width {
        let _ = write!(stream, "\u{2500}");
    }
    let _ = writeln!(stream, "\u{2518}");
    let _ = stream.reset();
    let _ = writeln!(stream);
}

/// Print a phase banner: ▸ PHASE_NAME in magenta+bold.
pub fn banner(phase: &str) {
    let mut stream = locked_stderr();
    let _ = writeln!(stream);
    let mut spec = ColorSpec::new();
    spec.set_fg(Some(Color::Magenta)).set_bold(true);
    let _ = stream.set_color(&spec);
    let _ = write!(stream, "\u{25b8} {}", phase.to_uppercase());
    let _ = stream.reset();
    let _ = writeln!(stream);
}

/// Print an indented key: value detail line under a phase.
/// The key (including colon) is blue; the value is white.
pub fn phase_detail(key: &str, value: &str) {
    let mut stream = locked_stderr();
    let mut key_spec = ColorSpec::new();
    key_spec.set_fg(Some(Color::Blue));
    let _ = stream.set_color(&key_spec);
    let _ = write!(stream, "    {}: ", key);
    let _ = stream.reset();
    let _ = writeln!(stream, "{}", value);
}

/// Print a success message: ✓ msg in green+bold.
pub fn info(msg: &str) {
    let mut stream = locked_stderr();
    write_styled_to(
        &mut stream,
        Some(Color::Green),
        true,
        false,
        &format!("\u{2713} {}", msg),
    );
    let _ = writeln!(stream);
}

/// Print a warning: ▸ msg in yellow+bold.
pub fn warn(msg: &str) {
    let mut stream = locked_stderr();
    write_styled_to(
        &mut stream,
        Some(Color::Yellow),
        true,
        false,
        &format!("\u{25b8} {}", msg),
    );
    let _ = writeln!(stream);
}

/// Print an error: ✗ msg in red+bold.
pub fn err_msg(msg: &str) {
    let mut stream = locked_stderr();
    write_styled_to(
        &mut stream,
        Some(Color::Red),
        true,
        false,
        &format!("\u{2717} {}", msg),
    );
    let _ = writeln!(stream);
}

fn write_styled_to(
    stream: &mut StandardStream,
    color: Option<Color>,
    bold: bool,
    dimmed: bool,
    text: &str,
) {
    let mut spec = ColorSpec::new();
    spec.set_fg(color).set_bold(bold).set_dimmed(dimmed);
    let _ = stream.set_color(&spec);
    let _ = write!(stream, "{}", text);
    let _ = stream.reset();
}

pub fn write_dim(stream: &mut StandardStream, text: &str) {
    let mut spec = ColorSpec::new();
    spec.set_bold(true).set_dimmed(true);
    let _ = stream.set_color(&spec);
    let _ = write!(stream, "{}", text);
    let _ = stream.reset();
}

pub fn show_markdown(title: &str, md: &str) {
    let mut stream = locked_stderr();
    let _ = writeln!(stream);
    write_dim(
        &mut stream,
        &format!("\u{2500}\u{2500} {} \u{2500}\u{2500}", title),
    );
    let _ = writeln!(stream);
    let skin = MadSkin::default();
    let rendered = skin.term_text(md);
    let _ = write!(stream, "{}", rendered);
    write_dim(&mut stream, "\u{2500}\u{2500} end \u{2500}\u{2500}");
    let _ = writeln!(stream);
    let _ = writeln!(stream);
}

pub fn prompt_multiline(msg: &str) -> String {
    let mut stream = StandardStream::stderr(ColorChoice::Auto);
    let mut spec = ColorSpec::new();
    spec.set_fg(Some(Color::Yellow)).set_bold(true);
    let _ = stream.set_color(&spec);
    let _ = write!(stream, "? {}", msg);
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
                if l.trim() == "." {
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
    loop {
        let mut stream = StandardStream::stderr(ColorChoice::Auto);
        // Question line
        let mut q_spec = ColorSpec::new();
        q_spec.set_fg(Some(Color::Yellow)).set_bold(true);
        let _ = stream.set_color(&q_spec);
        let _ = writeln!(stream, "? {}", msg);
        // Numbered choices
        for (i, c) in choices.iter().enumerate() {
            let _ = stream.set_color(&q_spec);
            let _ = writeln!(stream, "  {}) {}", i + 1, c);
        }
        let _ = stream.set_color(&q_spec);
        let _ = write!(stream, "  > ");
        let _ = stream.reset();
        let _ = stream.flush();

        let mut input = String::new();
        if io::stdin().read_line(&mut input).is_err() {
            continue;
        }
        let ans = input.trim().to_lowercase();
        // Accept by number
        if let Ok(num) = ans.parse::<usize>() {
            if num >= 1 && num <= choices.len() {
                return choices[num - 1].to_lowercase();
            }
        }
        // Accept by name
        for c in choices {
            let cl = c.to_lowercase();
            if cl == ans {
                return cl;
            }
        }
        eprintln!("Invalid choice, try again.");
    }
}

pub fn prompt_line(msg: &str, hint: &str) -> String {
    let mut stream = StandardStream::stderr(ColorChoice::Auto);
    let mut spec = ColorSpec::new();
    spec.set_fg(Some(Color::Yellow)).set_bold(true);
    let _ = stream.set_color(&spec);
    let _ = write!(stream, "? {}", msg);
    let _ = stream.reset();
    if !hint.is_empty() {
        let _ = write!(stream, " ");
        write_dim(&mut stream, hint);
    }
    let _ = write!(stream, " ");
    let _ = stream.flush();

    let mut input = String::new();
    if io::stdin().read_line(&mut input).is_err() {
        return String::new();
    }
    input.trim().to_string()
}

/// Write a dim-gray timestamp prefix to stderr. Used by runner.
pub fn write_timestamp(stream: &mut StandardStream, text: &str) {
    let mut spec = ColorSpec::new();
    spec.set_dimmed(true);
    let _ = stream.set_color(&spec);
    let _ = write!(stream, "{}", text);
    let _ = stream.reset();
}
