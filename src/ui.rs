use std::io::{self, BufRead, Write};
use std::ops::{Deref, DerefMut};
use std::sync::{Mutex, MutexGuard};
use termcolor::{Color, ColorChoice, ColorSpec, StandardStream, WriteColor};
use termimad::{terminal_size, MadSkin};

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

/// Detected (and clamped) usable terminal width in columns.
pub(crate) fn term_width() -> usize {
    let (cols, _) = terminal_size();
    let cols = cols as usize;
    if cols < 40 {
        80
    } else {
        cols.min(120)
    }
}

/// Soft-wrap `text` to fit within the current terminal width.
///
/// - Existing newlines in `text` are preserved as hard breaks.
/// - Continuation lines are prefixed with `indent` spaces, so wrapped output
///   visually aligns under the column where `text` started.
/// - Words longer than the available width are hard-broken at character
///   boundaries.
pub(crate) fn wrap_text(text: &str, indent: usize) -> String {
    let width = term_width();
    let avail = width.saturating_sub(indent).max(20);
    let cont = " ".repeat(indent);
    let mut out = String::new();

    for (li, line) in text.split('\n').enumerate() {
        if li > 0 {
            out.push('\n');
            out.push_str(&cont);
        }

        let mut col: usize = 0;
        let mut first_word = true;
        // Walk grapheme-ish words split on ASCII whitespace.
        for word in line.split_whitespace() {
            let wlen = word.chars().count();
            if first_word {
                if wlen <= avail {
                    out.push_str(word);
                    col = wlen;
                } else {
                    push_hard_wrapped(word, avail, &cont, &mut out);
                    col = avail;
                }
                first_word = false;
            } else if col + 1 + wlen <= avail {
                out.push(' ');
                out.push_str(word);
                col += 1 + wlen;
            } else {
                out.push('\n');
                out.push_str(&cont);
                if wlen <= avail {
                    out.push_str(word);
                    col = wlen;
                } else {
                    push_hard_wrapped(word, avail, &cont, &mut out);
                    col = avail;
                }
            }
        }
    }
    out
}

fn push_hard_wrapped(word: &str, avail: usize, cont: &str, out: &mut String) {
    let chars: Vec<char> = word.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        if i > 0 {
            out.push('\n');
            out.push_str(cont);
        }
        let end = (i + avail).min(chars.len());
        for c in &chars[i..end] {
            out.push(*c);
        }
        i = end;
    }
}

/// Print the application header: a stylized ASCII logo + tagline.
pub fn app_header() {
    let mut stream = locked_stderr();
    let _ = writeln!(stream);

    let logo = [
        "██████╗ ███████╗██╗  ██╗",
        "██╔══██╗██╔════╝╚██╗██╔╝",
        "██║  ██║█████╗   ╚███╔╝ ",
        "██║  ██║██╔══╝   ██╔██╗ ",
        "██████╔╝███████╗██╔╝ ██╗",
        "╚═════╝ ╚══════╝╚═╝  ╚═╝",
    ];

    let mut spec = ColorSpec::new();
    spec.set_fg(Some(Color::Cyan)).set_bold(true);
    let _ = stream.set_color(&spec);
    for line in logo.iter() {
        let _ = writeln!(stream, "  {}", line);
    }
    let _ = stream.reset();

    let mut tag = ColorSpec::new();
    tag.set_fg(Some(Color::Cyan)).set_dimmed(true);
    let _ = stream.set_color(&tag);
    let _ = writeln!(
        stream,
        "  Agentic Orchestrator · v{}",
        REVISION
    );
    let _ = stream.reset();
    let _ = writeln!(stream);
}

/// Print a phase banner: a horizontal rule with the phase name highlighted.
///
/// Example:
///   ── ▶ PLANNING ─────────────────────────────────────
pub fn banner(phase: &str) {
    let mut stream = locked_stderr();
    let width = term_width();
    let label = phase.to_uppercase();

    let _ = writeln!(stream);

    // Leading rule
    let mut dim_spec = ColorSpec::new();
    dim_spec.set_fg(Some(Color::Cyan)).set_dimmed(true);
    let _ = stream.set_color(&dim_spec);
    let _ = write!(stream, "── ");
    let _ = stream.reset();

    // Phase label
    let mut label_spec = ColorSpec::new();
    label_spec.set_fg(Some(Color::Magenta)).set_bold(true);
    let _ = stream.set_color(&label_spec);
    let _ = write!(stream, "▶ {}", label);
    let _ = stream.reset();

    // Trailing rule
    let used = 3 + 2 + label.chars().count() + 1; // "── " + "▶ " + label + " "
    let pad = width.saturating_sub(used);
    let _ = stream.set_color(&dim_spec);
    let _ = write!(stream, " ");
    for _ in 0..pad {
        let _ = write!(stream, "─");
    }
    let _ = stream.reset();
    let _ = writeln!(stream);
}

/// Print an indented "key: value" detail line under a phase.
pub fn phase_detail(key: &str, value: &str) {
    let mut stream = locked_stderr();

    let mut gutter = ColorSpec::new();
    gutter.set_fg(Some(Color::Cyan)).set_dimmed(true);
    let _ = stream.set_color(&gutter);
    let _ = write!(stream, "  │ ");
    let _ = stream.reset();

    let mut key_spec = ColorSpec::new();
    key_spec.set_fg(Some(Color::Blue));
    let _ = stream.set_color(&key_spec);
    let _ = write!(stream, "{}:", key);
    let _ = stream.reset();

    // Indent for continuation = "  │ " (4) + key + ": " (2) + leading space (1).
    let indent = 4 + key.chars().count() + 3;
    let _ = writeln!(stream, " {}", wrap_text(value, indent));
}

/// Print a success message: ✓ msg in green+bold.
pub fn info(msg: &str) {
    let mut stream = locked_stderr();
    // Symbol "✓ " then message; continuation indents by 2 visible columns.
    let wrapped = wrap_text(msg, 2);
    write_styled_to(
        &mut stream,
        Some(Color::Green),
        true,
        false,
        &format!("\u{2713} {}", wrapped),
    );
    let _ = writeln!(stream);
}

/// Print a warning: ⚠ msg in yellow+bold.
pub fn warn(msg: &str) {
    let mut stream = locked_stderr();
    let wrapped = wrap_text(msg, 2);
    write_styled_to(
        &mut stream,
        Some(Color::Yellow),
        true,
        false,
        &format!("\u{26a0} {}", wrapped),
    );
    let _ = writeln!(stream);
}

/// Print an error: ✗ msg in red+bold.
pub fn err_msg(msg: &str) {
    let mut stream = locked_stderr();
    let wrapped = wrap_text(msg, 2);
    write_styled_to(
        &mut stream,
        Some(Color::Red),
        true,
        false,
        &format!("\u{2717} {}", wrapped),
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

/// Render a markdown block framed by a titled rule line and a closing rule.
pub fn show_markdown(title: &str, md: &str) {
    let mut stream = locked_stderr();
    let width = term_width();
    let _ = writeln!(stream);

    // Top rule with title pill: ── Title ───────────
    let mut rule = ColorSpec::new();
    rule.set_fg(Some(Color::Cyan)).set_dimmed(true);
    let _ = stream.set_color(&rule);
    let _ = write!(stream, "── ");
    let _ = stream.reset();

    let mut title_spec = ColorSpec::new();
    title_spec.set_fg(Some(Color::Cyan)).set_bold(true);
    let _ = stream.set_color(&title_spec);
    let _ = write!(stream, "{}", title);
    let _ = stream.reset();

    let used = 3 + title.chars().count() + 1;
    let pad = width.saturating_sub(used);
    let _ = stream.set_color(&rule);
    let _ = write!(stream, " ");
    for _ in 0..pad {
        let _ = write!(stream, "─");
    }
    let _ = stream.reset();
    let _ = writeln!(stream);

    // Body
    let skin = MadSkin::default();
    let rendered = skin.term_text(md);
    let _ = write!(stream, "{}", rendered);

    // Bottom rule
    let _ = stream.set_color(&rule);
    for _ in 0..width {
        let _ = write!(stream, "─");
    }
    let _ = stream.reset();
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
    write_dim(&mut stream, "(end with a single \".\" on its own line)");
    let _ = writeln!(stream);
    let _ = stream.flush();

    let stdin = io::stdin();
    let mut lines: Vec<String> = Vec::new();
    for line in stdin.lock().lines().map_while(Result::ok) {
        if line.trim() == "." {
            break;
        }
        lines.push(line);
    }
    lines.join("\n")
}

/// Show a list of choices and read the user's selection.
///
/// Each choice can be entered as its number (1-based), the full word, or its
/// first letter (case-insensitive) when that letter is unambiguous among the
/// available choices.
pub fn prompt_choice(msg: &str, choices: &[&str]) -> String {
    // Compute first-letter shortcuts that are unambiguous among the choices.
    let shortcuts: Vec<Option<char>> = choices
        .iter()
        .enumerate()
        .map(|(i, choice)| {
            let first = choice.chars().next()?.to_ascii_lowercase();
            let unique = choices
                .iter()
                .enumerate()
                .all(|(j, other)| {
                    j == i
                        || other
                            .chars()
                            .next()
                            .map(|c| c.to_ascii_lowercase() != first)
                            .unwrap_or(true)
                });
            unique.then_some(first)
        })
        .collect();

    loop {
        let mut stream = StandardStream::stderr(ColorChoice::Auto);

        let mut q_spec = ColorSpec::new();
        q_spec.set_fg(Some(Color::Yellow)).set_bold(true);
        let _ = stream.set_color(&q_spec);
        let _ = writeln!(stream, "? {}", msg);
        let _ = stream.reset();

        // Render options inline with [n] markers in cyan.
        for (i, choice) in choices.iter().enumerate() {
            let mut idx_spec = ColorSpec::new();
            idx_spec.set_fg(Some(Color::Cyan)).set_bold(true);
            let _ = stream.set_color(&idx_spec);
            let _ = write!(stream, "  [{}]", i + 1);
            let _ = stream.reset();

            let _ = write!(stream, " ");

            // Highlight the shortcut letter inside the word, if available.
            match shortcuts[i] {
                Some(letter) => {
                    let mut chars = choice.chars();
                    let first = chars.next().unwrap_or(letter);
                    let rest: String = chars.collect();

                    let mut letter_spec = ColorSpec::new();
                    letter_spec.set_fg(Some(Color::Yellow)).set_bold(true).set_underline(true);
                    let _ = stream.set_color(&letter_spec);
                    let _ = write!(stream, "{}", first);
                    let _ = stream.reset();
                    let _ = write!(stream, "{}", rest);
                }
                None => {
                    let _ = write!(stream, "{}", choice);
                }
            }
            let _ = writeln!(stream);
        }

        let mut prompt_spec = ColorSpec::new();
        prompt_spec.set_fg(Some(Color::Yellow)).set_bold(true);
        let _ = stream.set_color(&prompt_spec);
        let _ = write!(stream, "  > ");
        let _ = stream.reset();
        let _ = stream.flush();

        let mut input = String::new();
        if io::stdin().read_line(&mut input).is_err() {
            continue;
        }
        let ans = input.trim().to_lowercase();

        if ans.is_empty() {
            err_msg("Please enter a choice.");
            continue;
        }

        // Numeric selection.
        if let Ok(num) = ans.parse::<usize>() {
            if (1..=choices.len()).contains(&num) {
                return choices[num - 1].to_lowercase();
            }
        }

        // Full word match.
        if let Some(choice) = choices
            .iter()
            .copied()
            .find(|choice| choice.eq_ignore_ascii_case(&ans))
        {
            return choice.to_lowercase();
        }

        // Single-letter shortcut.
        if ans.chars().count() == 1 {
            let c = ans.chars().next().unwrap();
            for (i, sc) in shortcuts.iter().enumerate() {
                if *sc == Some(c) {
                    return choices[i].to_lowercase();
                }
            }
        }

        err_msg(&format!("Invalid choice {:?}. Try a number or one of the listed options.", ans));
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
