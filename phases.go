package main

import (
	"fmt"
	"os"
	"os/exec"
	"strings"
	"sync"

	udiff "github.com/aymanbagabas/go-udiff"
)

// ── Phase 1: Planning ──

func PlanPhase(r *Runner, userInput string) (string, error) {
	banner("PLANNING")
	if err := ensureDexDir(); err != nil {
		return "", err
	}

	var feedbacks []string
	request := userInput
	planPath := dexPath("plan.md")

	if existing, _ := readDexFile("plan.md"); existing != "" {
		showMarkdown("Existing plan", existing)
		choice := promptChoice("Is your request a revision of this plan, or a new plan?", []string{"revise", "new"})
		switch choice {
		case "new":
			clearPlanState()
		case "revise":
			if orig, _ := readDexFile("request.txt"); orig != "" {
				request = orig
			}
			feedbacks = loadFeedbacks()
			feedbacks = append(feedbacks, userInput)
		}
	}

	savePlanRequest(request)
	saveFeedbacks(feedbacks)

	for iteration := 1; ; iteration++ {
		info(fmt.Sprintf("Planning iteration %d", iteration))

		removeDexFile("questions.md")

		p := renderPrompt("plan.txt", map[string]any{
			"Request":   request,
			"Feedbacks": feedbacks,
		})

		if err := r.Run(p); err != nil {
			errMsg(fmt.Sprintf("CLI error: %v", err))
			choice := promptChoice("Retry or abort?", []string{"retry", "abort"})
			if choice == "abort" {
				return "", fmt.Errorf("aborted by user")
			}
			continue
		}

		questions, _ := readDexFile("questions.md")
		if questions != "" {
			showBlock("Questions from CLI", questions)
			answer := promptMultiline("Your answers:")
			feedbacks = append(feedbacks, fmt.Sprintf("Questions:\n%s\n\nAnswers:\n%s", questions, answer))
			saveFeedbacks(feedbacks)
			continue
		}

		plan, err := readDexFile("plan.md")
		if err != nil {
			return "", err
		}
		if plan == "" {
			warn("CLI did not produce a plan or questions. Retrying...")
			feedbacks = append(feedbacks, "You did not produce a plan in .dex/plan.md or questions in .dex/questions.md. Please do so.")
			saveFeedbacks(feedbacks)
			continue
		}

		showMarkdown("Plan", plan)

		choice := promptChoice("Accept, edit, revise, or reject?", []string{"accept", "edit", "revise", "reject"})
		switch choice {
		case "accept":
			info("Plan accepted!")
			return planPath, nil
		case "reject":
			warn("Plan rejected.")
			return "", nil
		case "edit":
			diff, err := editPlanInEditor(plan)
			if err != nil {
				errMsg(fmt.Sprintf("Editor error: %v", err))
				continue
			}
			if diff == "" {
				continue
			}
			feedback := fmt.Sprintf("user provided feedback in the form of a unified diff: \n\n%s", diff)
			feedbacks = append(feedbacks, feedback)
			saveFeedbacks(feedbacks)
		case "revise":
			feedback := promptMultiline("Your revision feedback:")
			feedbacks = append(feedbacks, feedback)
			saveFeedbacks(feedbacks)
		}
	}
}

func editorCmd() string {
	if v := os.Getenv("VISUAL"); v != "" {
		return v
	}
	if v := os.Getenv("EDITOR"); v != "" {
		return v
	}
	return "vi"
}

func editPlanInEditor(plan string) (string, error) {
	tmp, err := os.CreateTemp("", "dex-plan-*.md")
	if err != nil {
		return "", fmt.Errorf("create temp file: %w", err)
	}
	tmpPath := tmp.Name()
	defer os.Remove(tmpPath)

	if _, err := tmp.WriteString(plan); err != nil {
		tmp.Close()
		return "", fmt.Errorf("write temp file: %w", err)
	}
	tmp.Close()

	editor := editorCmd()
	cmd := exec.Command(editor, tmpPath)
	cmd.Stdin = os.Stdin
	cmd.Stdout = os.Stdout
	cmd.Stderr = os.Stderr
	if err := cmd.Run(); err != nil {
		return "", fmt.Errorf("editor %q: %w", editor, err)
	}

	edited, err := os.ReadFile(tmpPath)
	if err != nil {
		return "", fmt.Errorf("read temp file: %w", err)
	}

	editedStr := string(edited)
	if editedStr == plan {
		warn("No changes detected.")
		return "", nil
	}

	edits := udiff.Lines(plan, editedStr)
	diff, err := udiff.ToUnified("plan.md", "plan.md.edited", plan, edits, 5)
	if err != nil {
		return "", fmt.Errorf("compute diff: %w", err)
	}
	return diff, nil
}

// ── Phase 2: Implementation ──

func ImplPhase(r *Runner, planPath string) error {
	banner("IMPLEMENTATION")

	for iteration := 1; ; iteration++ {
		task, err := NextOpenTask(planPath)
		if err != nil {
			return err
		}
		if task == nil {
			info("All tasks complete!")
			return nil
		}

		header := task.Header
		if header == "" {
			header = "(unnamed task)"
		}
		info(fmt.Sprintf("Iteration %d — working on: %s (%d/%d steps open)",
			iteration, header, task.Open, task.Open+task.Done))

		p := renderPrompt("impl.txt", map[string]any{
			"PlanPath":   planPath,
			"TaskHeader": task.Header,
			"TaskBody":   task.String(),
		})

		if err := r.Run(p); err != nil {
			errMsg(fmt.Sprintf("CLI error: %v", err))
			choice := promptChoice("Retry or abort?", []string{"retry", "abort"})
			if choice == "abort" {
				return fmt.Errorf("aborted by user")
			}
			continue
		}

		done, err := AllTasksDone(planPath)
		if err != nil {
			return err
		}
		if done {
			info("All tasks complete!")
			return nil
		}
	}
}

// ── Phase 3: Review ──

type ReviewRole struct {
	Name   string
	Scope  string
	Prompt string
}

// broadReviewers run once for a comprehensive first pass.
var broadReviewers = []ReviewRole{
	{
		Name:  "quality",
		Scope: "bugs, security, correctness, simplicity",
		Prompt: `Focus on:
- logic errors
- edge cases
- error handling
- resource management
- concurrency issues
- input validation and security issues
- unnecessary abstraction or over-engineering when a simpler solution would work`,
	},
	{
		Name:  "implementation",
		Scope: "goal coverage, wiring, completeness, logic flow",
		Prompt: `Focus on:
- requirement coverage — does the code actually achieve the plan's goal?
- correctness of the chosen approach
- wiring and integration between components
- completeness — are any requirements missing?
- logic flow and edge cases`,
	},
	{
		Name:  "simplification",
		Scope: "unnecessary complexity, over-engineering",
		Prompt: `Focus on:
- excessive abstraction layers
- premature generalization
- unnecessary indirection
- unused extension points
- unnecessary fallbacks
- premature optimization`,
	},
	{
		Name:  "testing",
		Scope: "coverage, test quality, edge cases",
		Prompt: `Focus on:
- missing tests for changed code
- untested error paths
- weak assertions
- fake tests that do not verify behavior
- missing edge-case coverage
- test independence`,
	},
	{
		Name:  "documentation",
		Scope: "README, internal docs, plan alignment",
		Prompt: `Focus on:
- missing README updates for new features, flags, configuration, APIs, or changed behavior
- missing internal documentation updates for new patterns, commands, or architecture
- plan file drift that should be corrected while addressing documentation gaps`,
	},
}

// focusedReviewers run in a loop after the broad pass, targeting only critical/major issues.
var focusedReviewers = []ReviewRole{
	{
		Name:  "quality",
		Scope: "critical and major correctness, security, reliability",
		Prompt: `Review code only for critical and major bugs, security issues, and correctness problems.
Ignore style issues and minor suggestions.
Focus on:
- logic errors that cause incorrect behavior
- security vulnerabilities
- data loss or corruption risks
- concurrency bugs`,
	},
	{
		Name:  "implementation",
		Scope: "critical and major goal coverage, integration, completeness",
		Prompt: `Review whether any critical or major requirement-coverage or integration issues remain.
Ignore style issues and minor suggestions.
Focus on:
- requirements that are not implemented at all
- integration bugs between components
- critical logic flow errors`,
	},
}

const maxFocusedRounds = 3

func ReviewPhase(r *Runner, planPath, baseRef string) error {
	// ── Broad pass: all reviewers, once ──
	issues := runReviewFanout(r, planPath, baseRef, broadReviewers, "broad", 1, 1)
	if issues != nil {
		if err := runFixer(r, planPath, baseRef, issues); err != nil {
			return err
		}
	}

	// ── Focused pass: critical/major reviewers, loop till clean ──
	for round := 1; round <= maxFocusedRounds; round++ {
		issues := runReviewFanout(r, planPath, baseRef, focusedReviewers, "focused", round, maxFocusedRounds)
		if issues == nil {
			info("All focused reviewers report ZERO ISSUES. Review phase complete!")
			return nil
		}
		if err := runFixer(r, planPath, baseRef, issues); err != nil {
			return err
		}
	}

	warn(fmt.Sprintf("Focused review cap of %d rounds reached, accepting current state.", maxFocusedRounds))
	return nil
}

func runReviewFanout(r *Runner, planPath, baseRef string, reviewers []ReviewRole, label string, round, maxRounds int) []string {
	banner(fmt.Sprintf("%s-review | round %d/%d", label, round, maxRounds))

	for _, rv := range reviewers {
		removeDexFile(fmt.Sprintf("review-%s.md", rv.Name))
	}

	var wg sync.WaitGroup
	errs := make([]error, len(reviewers))
	for i, rv := range reviewers {
		wg.Add(1)
		go func(idx int, role ReviewRole) {
			defer wg.Done()
			info(fmt.Sprintf("[parallel:%s] running %s review", role.Name, role.Scope))
			p := renderPrompt("review.txt", map[string]any{
				"PlanPath":   planPath,
				"BaseRef":    baseRef,
				"RoleName":   role.Name,
				"RoleScope":  role.Scope,
				"RolePrompt": role.Prompt,
				"ReviewPath": dexPath(fmt.Sprintf("review-%s.md", role.Name)),
			})
			errs[idx] = r.Run(p)
			if errs[idx] != nil {
				errMsg(fmt.Sprintf("[parallel:%s] done %s review (exit=1)", role.Name, role.Scope))
			} else {
				info(fmt.Sprintf("[parallel:%s] done %s review (exit=0)", role.Name, role.Scope))
			}
		}(i, rv)
	}
	wg.Wait()

	allClean := true
	var issues []string
	for _, rv := range reviewers {
		review, _ := readDexFile(fmt.Sprintf("review-%s.md", rv.Name))
		if review == "" {
			warn(fmt.Sprintf("Reviewer %q produced no output", rv.Name))
			allClean = false
			continue
		}
		showMarkdown(fmt.Sprintf("Review: %s", rv.Name), review)
		if !isCleanReview(review) {
			allClean = false
			issues = append(issues, fmt.Sprintf("── %s ──\n%s", rv.Name, review))
		}
	}

	if allClean {
		return nil
	}
	return issues
}

func runFixer(r *Runner, planPath, baseRef string, issues []string) error {
	info("Issues found — running fixer...")
	fixPrompt := renderPrompt("fix.txt", map[string]any{
		"PlanPath": planPath,
		"BaseRef":  baseRef,
		"Issues":   strings.Join(issues, "\n\n"),
	})
	if err := r.Run(fixPrompt); err != nil {
		errMsg(fmt.Sprintf("Fixer error: %v", err))
		choice := promptChoice("Retry or abort?", []string{"retry", "abort"})
		if choice == "abort" {
			return fmt.Errorf("aborted by user")
		}
	}
	return nil
}

func isCleanReview(review string) bool {
	normalized := strings.ToUpper(strings.TrimSpace(review))
	return strings.Contains(normalized, "ZERO ISSUES")
}

// ── Bare Mode ──

func BarePhase(r *Runner, request string, maxIterations int) error {
	banner("BARE")
	for iteration := 1; iteration <= maxIterations; iteration++ {
		info(fmt.Sprintf("Bare iteration %d/%d", iteration, maxIterations))
		p := renderPrompt("bare.txt", map[string]any{
			"Request": request,
		})
		if err := r.Run(p); err != nil {
			return fmt.Errorf("bare iteration %d failed: %w", iteration, err)
		}
	}
	return nil
}

// ── Finalize Phase ──

func FinalizePhase(r *Runner, planPath, baseRef string) error {
	banner("FINALIZE")
	branchOut, err := exec.Command("git", "symbolic-ref", "--short", "HEAD").Output()
	if err != nil || strings.TrimSpace(string(branchOut)) == "" {
		return fmt.Errorf("finalize requires a named branch (detached HEAD is not supported)")
	}
	branch := strings.TrimSpace(string(branchOut))
	headRev, _ := exec.Command("git", "rev-parse", "HEAD").Output()
	baseRev, _ := exec.Command("git", "rev-parse", baseRef).Output()
	if strings.TrimSpace(string(headRev)) == strings.TrimSpace(string(baseRev)) {
		return fmt.Errorf("finalize: current branch %q points to the same commit as base ref %q; switch to a feature branch first", branch, baseRef)
	}
	p := renderPrompt("finalize.txt", map[string]any{
		"PlanPath": planPath,
		"BaseRef":  baseRef,
	})
	if err := r.Run(p); err != nil {
		errMsg(fmt.Sprintf("Finalize error: %v", err))
		choice := promptChoice("Retry or abort?", []string{"retry", "abort"})
		if choice == "abort" {
			return fmt.Errorf("aborted by user")
		}
		return r.Run(p)
	}
	return nil
}
