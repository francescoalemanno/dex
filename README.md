# dex

A structured, multi-phase orchestrator for AI coding agents. Give it a request — it plans, implements, and reviews the work automatically using your preferred coding CLI.

**dex** takes the core ideas from Geoffrey Huntley's [Ralph Wiggum Technique](https://ghuntley.com/ralph/) — an autonomous plan → build → iterate loop for AI agents — and wraps them in a deterministic, three-phase pipeline with built-in retry logic, parallel code review, and checkpoint-based task tracking.

## Why dex?

The classic Ralph Wiggum loop is beautifully simple: plan in a conversation, then let the agent build in a `while true` shell loop with two prompts and a markdown checklist. It works — but it's heuristic. The agent decides what's done, the loop is a bash script, retries are manual, and review is "run it again and hope."

dex keeps the same philosophy — markdown plans, checkbox progress, autonomous iteration — but adds the guardrails you'd want for longer or riskier tasks:

- **Human-in-the-loop planning** — accept, revise, or reject the plan before any code is written
- **Structured task parsing** — checkboxes are tracked programmatically, not by vibes
- **Automatic retries with backoff** — transient failures don't require babysitting
- **Parallel multi-reviewer code review** — five specialized reviewers run concurrently, a fixer resolves confirmed issues, and focused rounds repeat until clean
- **CLI-agnostic** — swap between 7 supported coding agents with a flag

## Installation

Requires Go 1.24+ and at least one supported coding CLI installed.

```bash
go install github.com/francescoalemanno/dex@latest
```

Or build from source:

```bash
git clone https://github.com/francescoalemanno/dex.git
cd dex
go build -o dex .
```

## Usage

```bash
dex "add a /health endpoint that returns JSON with uptime and version"
```

Or launch interactively (prompts for input):

```bash
dex
```

### Flags

| Flag | Default | Description |
|------|---------|-------------|
| `-cli` | `opencode` | Coding CLI to use |
| `-plan` | | Skip planning, use an existing plan file |
| `-no-review` | `false` | Skip the review phase |
| `-base-ref` | `HEAD` | Base git ref for review diffs |
| `-timeout` | `20m` | Kill the agent after this idle duration |

Flag values are persisted in `.dex/config.json` across runs.

### Supported CLIs

| CLI | Notes |
|-----|-------|
| `opencode` | Default. JSON output mode, LSP disabled. |
| `codex` | OpenAI Codex CLI. |
| `claude` | Anthropic Claude Code. |
| `droid` | Droid CLI. |
| `gemini` | Google Gemini CLI. |
| `pi` | Pi CLI. |
| `raijin` | Raijin CLI. |

## The Three Phases

### Phase 1: Planning

The agent explores your codebase, optionally asks clarifying questions (written to `.dex/questions.md`), and produces a structured markdown plan at `.dex/plan.md` with checkbox tasks. You review the plan and choose to **accept**, **revise** (with feedback), or **reject** it. The loop repeats until you're satisfied or you walk away.

### Phase 2: Implementation

The plan's checkbox tasks are executed one group at a time. Each iteration, the agent picks the first incomplete task group, implements it, runs tests, marks the checkboxes done, and commits. This continues until every checkbox is checked.

### Phase 3: Review

A fan-out of parallel reviewers examines the diff between the base ref and HEAD:

- **Quality** — bugs, security, correctness, error handling
- **Implementation** — goal coverage, wiring, completeness
- **Simplification** — over-engineering, unnecessary abstraction
- **Testing** — coverage, test quality, edge cases
- **Documentation** — README updates, internal docs

Each reviewer writes findings to `.dex/review-<name>.md`. If any issues are found, a fixer agent resolves confirmed problems and commits the fixes. A focused review loop (quality + implementation only) then runs up to 3 additional rounds until the codebase is clean.

## Project Structure

```
main.go      — CLI entry point, flag parsing, phase orchestration
plan.go      — Markdown plan parser (checkbox task groups)
phases.go    — Planning, implementation, and review phase logic
runner.go    — CLI runner with retry, timeout, and output processing
ui.go        — Terminal UI (banners, prompts, markdown rendering)
prompts/     — Embedded Go templates for agent prompts
.dex/        — Working directory (gitignored): plan, config, reviews
```

## Credits

dex is a structured take on the **Ralph Wiggum Technique** created by [Geoffrey Huntley](https://ghuntley.com/ralph/) ([@GeoffreyHuntley](https://x.com/GeoffreyHuntley)). The original technique pioneered the autonomous plan → build → iterate loop for AI coding agents. The playbook and community discussion are documented at [ghuntley/how-to-ralph-wiggum](https://github.com/ghuntley/how-to-ralph-wiggum).

## License

[MIT](LICENSE)
