# Dex - a Ralph-inspired loop that actually works

[![Rust](https://img.shields.io/badge/Rust-stable-000000?logo=rust&logoColor=white)](https://www.rust-lang.org)
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](LICENSE)
[![GitHub](https://img.shields.io/github/stars/francescoalemanno/dex?style=social)](https://github.com/francescoalemanno/dex)

**A structured orchestrator for AI coding agents.** Give it a request, and dex turns it into a plan you can amend, apply, and review using your preferred coding CLI.

You describe what you want. dex turns it into a plan you approve, executes it one task at a time with fresh context each round, then runs parallel code reviews and fixes what the reviewers catch. You stay in control without doing the grunt work.

The current implementation is written in Rust and ships release binaries for Linux and macOS on both `amd64` and `arm64`, plus Windows on `amd64`.

## Why not just loop?

The [Ralph Wiggum Technique](https://ghuntley.com/ralph/) proved something important: a dumb bash loop feeding prompts to an AI agent can build real software autonomously. Plan in a conversation, let the agent build in a `while true` loop, use markdown checklists as shared state. It works.

But it's heuristic. The agent decides when it's done. Retries are you hitting Ctrl+C and rerunning. Review is "run it again and hope." And if the plan drifts mid-session, you're along for the ride.

dex keeps the same philosophy: markdown plans, checkbox progress, and one task per fresh context window. It adds the structure you actually want when the task is longer than a quick prototype:

- **You approve the plan before any code runs.** Accept it, revise it with feedback, edit it in your `$EDITOR`, or reject it entirely. The agent doesn't touch code until you say go.
- **Task progress is tracked programmatically.** Checkboxes are parsed, not vibed. dex knows exactly which task group is next and when everything is done.
- **Failures don't need babysitting.** Transient crashes retry automatically with exponential backoff. An idle agent gets killed after a configurable timeout.
- **Code review is built in, not bolted on.** Five specialized reviewers run in parallel, a fixer resolves confirmed issues, and focused rounds repeat until the codebase is clean, or until you've hit the cap.
- **Any agent, same workflow.** Swap between seven supported coding CLIs with a flag. The orchestration stays identical.

## Quick start

macOS and Linux:

```bash
curl -sSfL https://raw.githubusercontent.com/francescoalemanno/dex/main/install.sh | bash
```

Windows PowerShell:

```powershell
irm https://raw.githubusercontent.com/francescoalemanno/dex/main/install.ps1 | iex
```

Install with Cargo:

```bash
cargo install --git https://github.com/francescoalemanno/dex --locked
```

Build from source:

```bash
git clone https://github.com/francescoalemanno/dex.git
cd dex
cargo build --release
```

You need at least one supported coding CLI installed (`opencode`, `claude`, `codex`, `gemini`, `droid`, `pi`, or `raijin`). You only need Rust if you're building dex from source.

Then start with a plan:

```bash
dex plan "refactor the database layer to use connection pooling instead of per-request connections"
dex apply
dex review
```

dex will explore your codebase, draft a plan, and ask you to approve it before writing a single line of code. Once the plan is locked, `dex apply` implements it and `dex review` runs the reviewer loop.

## Real-world examples

**Migrate an API from REST to gRPC:**

```bash
dex plan "convert the user-facing REST API in server/api/ to gRPC, \
     generate proto definitions from the existing route signatures, \
     keep the HTTP gateway for backwards compatibility"
dex apply
dex review
```

**Add observability to an existing service:**

```bash
dex plan "instrument all database queries and HTTP handlers in cmd/server \
     with OpenTelemetry tracing, add a /metrics endpoint exposing \
     request latency histograms and error rates in Prometheus format"
dex apply
dex review
```

**Use Claude instead of the default agent:**

```bash
dex --cli claude plan "add structured JSON logging to the worker package, \
     replace all fmt.Printf calls with slog"
```

**Amend an existing plan with new feedback:**

```bash
dex amend "use a different database library"
```

**Apply the current plan after approving it:**

```bash
dex apply
```

**Review the current implementation with two reviewers at a time:**

```bash
dex review --parallel 2
```

**Import a prepared plan:**

```bash
dex import myplan.md
```

**Raw agent loop for open-ended work (10 iterations):**

```bash
cat > bare-request.txt <<'EOF'
explore the codebase and improve test coverage for any file under 60% branch coverage
EOF
dex bare 10 bare-request.txt
```

**Finalize a feature branch for merge:**

```bash
dex finalize --onto main
```

**Force overwrite an existing plan:**

```bash
dex plan --force "rewrite the auth module from scratch"
```

## How it works

dex organizes work into three phases. Each phase invokes the coding CLI as a subprocess; dex itself never edits your source files directly.

### Phase 1: Planning

`dex plan` explores your codebase and drafts a structured markdown plan with checkbox tasks. If it needs clarification, it writes questions to `.dex/questions.md` and dex shows them to you inline.

You review the plan and choose one of four options:

- **accept**: lock the plan and move to implementation
- **revise**: give natural-language feedback and let the agent refine the plan
- **edit**: open the plan in `$EDITOR`; dex computes a unified diff and feeds it back as feedback
- **reject**: throw it away and touch no code

This loop repeats until you're satisfied. The agent never touches code during planning. `dex amend` re-enters the same planning loop later using the existing plan plus your new feedback.

### Phase 2: Implementation

`dex apply` parses markdown sections that contain checkbox items into task groups. Each iteration, it picks the first incomplete section, hands it to the agent with the plan as context, and lets the agent implement, test, and commit. Then the CLI process exits, context is cleared, and the next iteration starts fresh.

This is the Ralph insight at work: one task per context window keeps the agent in its smart zone. dex just makes the task selection deterministic instead of leaving it to the model.

### Phase 3: Review

`dex review` runs five specialized reviewers concurrently, each in its own agent process:

- **Quality**: bugs, security, correctness, concurrency issues
- **Implementation**: requirement coverage, wiring, completeness
- **Simplification**: unnecessary abstraction, over-engineering
- **Testing**: coverage gaps, weak assertions, missing edge cases
- **Documentation**: README drift and missing docs for new behavior

Each writes findings to `.dex/review-<name>.md`. The review diff base is loaded from `.dex/review-base-ref.txt`, which dex snapshots before implementation begins so the full implementation can still be reviewed after interruptions or resumes. If any issues are found, a fixer agent reads all findings, verifies them against the actual code, filters false positives, and commits fixes.

Then a focused review loop runs with only quality and implementation reviewers for up to 3 additional rounds. The phase ends when both report zero issues, or the cap is reached.

## Subcommands

| Subcommand | Description |
|------------|-------------|
| `plan [--force] <request>` | Create or replace the current plan from a request. |
| `import [--force] <file>` | Install a markdown plan file as the current plan. |
| `amend <feedback>` | Revise the current plan using natural-language feedback. |
| `apply` | Implement the current plan. |
| `review [--parallel <n>]` | Review the current implementation. |
| `bare <iterations> <request-file>` | Send a request file straight to the agent for N iterations, re-reading the file each round. |
| `finalize --onto <target>` | Rebase, tidy commits, and rerun checks against the given target. |

## Global options

| Option | Default | Description |
|--------|---------|-------------|
| `--cli <name>` | auto-detected | Coding CLI to use; must be available in PATH |
| `--timeout <seconds>` | `1200` | Kill the agent after this many idle seconds |
| `--version` | | Print version and exit |

`--cli` persists across runs in `.dex/config.json`, so you don't have to repeat `--cli claude` every time. When no `--cli` is given and no config exists, dex picks the first available agent it finds in PATH.

## Supported CLIs

| CLI | Key | Notes |
|-----|-----|-------|
| OpenCode | `opencode` | Default. JSON output, auto-permissions. |
| Claude | `claude` | Anthropic's CLI. Skips permissions. |
| Codex | `codex` | OpenAI's CLI. Ephemeral, no sandbox. |
| Gemini | `gemini` | Google's CLI. |
| Droid | `droid` | Skips permissions. |
| Pi | `pi` | No-session mode. |
| Raijin | `raijin` | Ephemeral, no echo. |

All CLIs run with their respective auto-approve flags so they can operate autonomously inside dex's loop. Make sure you understand the security implications: dex runs agents with full permissions on your filesystem.

## The `.dex/` directory

dex stores all working state in a `.dex/` directory at your project root. It's gitignored by default; on first run dex creates `.dex/.gitignore` with `*`.

| File | Purpose | Created by |
|------|---------|------------|
| `config.json` | Persisted CLI preference across runs | dex |
| `plan.md` | The current plan with checkbox tasks | agent |
| `request.txt` | Original request or imported-plan label used for later amendments | dex |
| `questions.md` | Clarifying questions from the agent | agent |
| `feedbacks.json` | Accumulated revision feedback | dex |
| `review-base-ref.txt` | Durable review diff base captured before implementation | dex |
| `review-*.md` | Review findings per reviewer | agent |

You can safely delete the entire `.dex/` directory to start fresh. dex recreates it on the next run.

## Origins

dex builds on the **Ralph Wiggum Technique** created by [Geoffrey Huntley](https://ghuntley.com/ralph/) ([@GeoffreyHuntley](https://x.com/GeoffreyHuntley)), which pioneered the autonomous plan -> build -> iterate loop for AI coding agents. The key insight, that a fresh context window per task keeps the model sharp and that a dumb outer loop with file-based state is enough for continuity, is the foundation dex stands on.

Clayton Farr's [playbook](https://github.com/ghuntley/how-to-ralph-wiggum) documented the methodology in depth and proposed enhancements that influenced dex's multi-reviewer design and backpressure philosophy.

dex's contribution is wrapping that loop in a deterministic orchestrator: programmatic task tracking, human-gated planning, parallel review fanout, automatic retries, and CLI-agnostic execution, so the technique scales to tasks where a bare bash loop starts to feel fragile.

## License

[MIT](LICENSE)
