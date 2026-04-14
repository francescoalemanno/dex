# Dex - a Ralph loop that works

[![Go](https://img.shields.io/badge/Go-1.24+-00ADD8?logo=go&logoColor=white)](https://go.dev)
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](LICENSE)
[![GitHub](https://img.shields.io/github/stars/francescoalemanno/dex?style=social)](https://github.com/francescoalemanno/dex)

**A structured orchestrator for AI coding agents.** Give it a request — it plans, implements, and reviews the work automatically using your preferred coding CLI.

You describe what you want. dex turns it into a plan you approve, executes it one task at a time with fresh context each round, then runs parallel code reviews and fixes what the reviewers catch. You stay in control without doing the grunt work.

## Why not just loop?

The [Ralph Wiggum Technique](https://ghuntley.com/ralph/) proved something important: a dumb bash loop feeding prompts to an AI agent can build real software autonomously. Plan in a conversation, let the agent build in a `while true` loop, use markdown checklists as shared state. It works.

But it's heuristic. The agent decides when it's done. Retries are you hitting Ctrl+C and rerunning. Review is "run it again and hope." And if the plan drifts mid-session, you're along for the ride.

dex keeps the same philosophy — markdown plans, checkbox progress, one task per fresh context window — but adds the structure you actually want when the task is longer than a quick prototype:

- **You approve the plan before any code runs.** Accept it, revise it with feedback, edit it in your `$EDITOR`, or reject it entirely. The agent doesn't touch code until you say go.
- **Task progress is tracked programmatically.** Checkboxes are parsed, not vibed. dex knows exactly which task group is next and when everything is done.
- **Failures don't need babysitting.** Transient crashes retry automatically with exponential backoff. An idle agent gets killed after a configurable timeout.
- **Code review is built in, not bolted on.** Five specialized reviewers run in parallel, a fixer resolves confirmed issues, and focused rounds repeat until the codebase is clean — or until you've hit the cap.
- **Any agent, same workflow.** Swap between seven supported coding CLIs with a flag. The orchestration stays identical.

## Quick start

```bash
go install github.com/francescoalemanno/dex@latest
```

Or build from source:

```bash
git clone https://github.com/francescoalemanno/dex.git && cd dex && go build -o dex .
```

You need Go 1.24+ and at least one supported coding CLI installed (opencode, claude, codex, gemini, droid, pi, or raijin).

Then just run it:

```bash
dex "refactor the database layer to use connection pooling instead of per-request connections"
```

dex will explore your codebase, draft a plan, and ask you to approve it before writing a single line of code.

## Real-world examples

**Migrate an API from REST to gRPC:**

```bash
dex "convert the user-facing REST API in server/api/ to gRPC, \
     generate proto definitions from the existing route signatures, \
     keep the HTTP gateway for backwards compatibility"
```

**Add observability to an existing service:**

```bash
dex "instrument all database queries and HTTP handlers in cmd/server \
     with OpenTelemetry tracing, add a /metrics endpoint exposing \
     request latency histograms and error rates in Prometheus format"
```

**Use Claude instead of the default agent, skip review for a quick task:**

```bash
dex -cli claude -no-review "add structured JSON logging to the worker package, \
     replace all fmt.Printf calls with slog"
```

**Resume from an existing plan after a crash or interruption:**

```bash
dex -plan .dex/plan.md
```

**Raw agent loop for open-ended work (10 iterations):**

```bash
dex -b 10 "explore the codebase and improve test coverage \
     for any file under 60% branch coverage"
```

**Finalize a feature branch for merge:**

```bash
dex -finalize -base-ref main
```

## How it works

dex runs your request through three phases. Each phase invokes the coding CLI as a subprocess — dex itself never reads or writes your source code.

### Phase 1: Planning

The agent explores your codebase and drafts a structured markdown plan with checkbox tasks. If it needs clarification, it writes questions to `.dex/questions.md` and dex shows them to you inline.

You review the plan and choose one of four options:

- **accept** — lock the plan, move to implementation
- **revise** — give natural-language feedback, the agent refines the plan
- **edit** — open the plan in `$EDITOR`, dex computes a unified diff and feeds it back as feedback
- **reject** — throw it away, no code touched

This loop repeats until you're satisfied. The agent never touches code during planning.

### Phase 2: Implementation

dex parses the plan's checkboxes into task groups. Each iteration, it picks the first incomplete group, hands it to the agent with the plan as context, and lets the agent implement, test, and commit. Then the CLI process exits, context is cleared, and the next iteration starts fresh.

This is the Ralph insight at work: one task per context window keeps the agent in its "smart zone." dex just makes the task selection deterministic instead of leaving it to the model.

### Phase 3: Review

Five specialized reviewers run concurrently, each in its own agent process:

- **Quality** — bugs, security, correctness, concurrency issues
- **Implementation** — requirement coverage, wiring, completeness
- **Simplification** — unnecessary abstraction, over-engineering
- **Testing** — coverage gaps, weak assertions, missing edge cases
- **Documentation** — README drift, missing docs for new behavior

Each writes findings to `.dex/review-<name>.md`. If any issues are found, a fixer agent reads all findings, verifies them against the actual code (filtering false positives), and commits fixes.

Then a focused review loop runs — only quality and implementation reviewers — for up to 3 additional rounds. The phase ends when both report zero issues, or the cap is reached.

## Flags

| Flag | Default | Description |
|------|---------|-------------|
| `-cli` | `opencode` | Coding CLI to use |
| `-plan` | | Skip planning, use an existing plan file |
| `-no-review` | `false` | Skip the review phase |
| `-base-ref` | `HEAD` | Base git ref for review diffs |
| `-timeout` | `20m` | Kill the agent after this idle duration |
| `-b N` | | Bare mode: send request straight to the agent for N iterations |
| `-finalize` | `false` | Rebase, tidy commits, rerun checks |

Flag values persist across runs in `.dex/config.json`, so you don't have to repeat `-cli claude` every time.

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

All CLIs run with their respective "auto-approve" flags so they can operate autonomously inside dex's loop. Make sure you understand the security implications — dex runs agents with full permissions on your filesystem.

## The `.dex/` directory

dex stores all working state in a `.dex/` directory at your project root. It's gitignored by default (dex creates `.dex/.gitignore` with `*` on first run).

| File | Purpose | Created by |
|------|---------|------------|
| `config.json` | Persisted flag values across runs | dex |
| `plan.md` | The current plan (checkbox tasks) | agent |
| `request.txt` | Original user request (for revisions) | dex |
| `questions.md` | Clarifying questions from the agent | agent |
| `feedbacks.json` | Accumulated revision feedback | dex |
| `review-*.md` | Review findings per reviewer | agent |

You can safely delete the entire `.dex/` directory to start fresh. dex recreates it on the next run.

## Origins

dex builds on the **Ralph Wiggum Technique** created by [Geoffrey Huntley](https://ghuntley.com/ralph/) ([@GeoffreyHuntley](https://x.com/GeoffreyHuntley)), which pioneered the autonomous plan → build → iterate loop for AI coding agents. The key insight — that a fresh context window per task keeps the model sharp, and that a dumb outer loop with file-based state is all you need for continuity — is the foundation dex stands on.

Clayton Farr's [playbook](https://github.com/ghuntley/how-to-ralph-wiggum) documented the methodology in depth and proposed enhancements that influenced dex's multi-reviewer design and backpressure philosophy.

dex's contribution is wrapping that loop in a deterministic orchestrator: programmatic task tracking, human-gated planning, parallel review fanout, automatic retries, and CLI-agnostic execution — so the technique scales to tasks where a bare bash loop starts to feel fragile.

## License

[MIT](LICENSE)
