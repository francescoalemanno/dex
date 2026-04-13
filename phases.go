package main

import (
	"bytes"
	"embed"
	"fmt"
	"os"
	"path/filepath"
	"strings"
	"sync"
	"text/template"
)

//go:embed prompts/*.txt
var promptFS embed.FS

var prompts = template.Must(
	template.New("").
		Funcs(template.FuncMap{"inc": func(i int) int { return i + 1 }}).
		ParseFS(promptFS, "prompts/*.txt"),
)

func renderPrompt(name string, data any) string {
	var buf bytes.Buffer
	if err := prompts.ExecuteTemplate(&buf, name, data); err != nil {
		panic(fmt.Sprintf("template %q: %v", name, err))
	}
	return buf.String()
}

const dexDir = ".dex"

func ensureDexDir() error {
	if err := os.MkdirAll(dexDir, 0o755); err != nil {
		return err
	}
	gitignore := filepath.Join(dexDir, ".gitignore")
	if _, err := os.Stat(gitignore); os.IsNotExist(err) {
		return os.WriteFile(gitignore, []byte("*\n"), 0o644)
	}
	return nil
}

func dexPath(name string) string {
	return filepath.Join(dexDir, name)
}

func readDexFile(name string) (string, error) {
	data, err := os.ReadFile(dexPath(name))
	if err != nil {
		if os.IsNotExist(err) {
			return "", nil
		}
		return "", err
	}
	return strings.TrimSpace(string(data)), nil
}

func removeDexFile(name string) {
	os.Remove(dexPath(name))
}

// ── Phase 1: Planning ──

func PlanPhase(r *Runner, request string) (string, error) {
	banner("PLANNING")
	if err := ensureDexDir(); err != nil {
		return "", err
	}

	var feedbacks []string
	planPath := dexPath("plan.md")

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
			continue
		}

		plan, err := readDexFile("plan.md")
		if err != nil {
			return "", err
		}
		if plan == "" {
			warn("CLI did not produce a plan or questions. Retrying...")
			feedbacks = append(feedbacks, "You did not produce a plan in .dex/plan.md or questions in .dex/questions.md. Please do so.")
			continue
		}

		showBlock("Plan", plan)

		choice := promptChoice("Accept, revise, or reject?", []string{"accept", "revise", "reject"})
		switch choice {
		case "accept":
			info("Plan accepted!")
			return planPath, nil
		case "reject":
			warn("Plan rejected.")
			return "", nil
		case "revise":
			feedback := promptMultiline("Your revision feedback:")
			feedbacks = append(feedbacks, feedback)
		}
	}
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
	Prompt string
}

var defaultReviewers = []ReviewRole{
	{
		Name: "quality",
		Prompt: `Review the codebase for correctness and quality.
Look for: bugs, logic errors, missing error handling at boundaries, edge cases, broken functionality.
Do NOT suggest style changes or refactoring unless it hides a bug.`,
	},
	{
		Name: "simplicity",
		Prompt: `Review the codebase for overengineering and unnecessary complexity.
Look for: YAGNI violations, premature abstractions, unnecessary indirection, over-complicated solutions.
Only flag things that should be simplified or removed.`,
	},
	{
		Name: "security",
		Prompt: `Review the codebase for security issues.
Look for: exposed secrets, injection vulnerabilities, unsafe input handling, insecure defaults.
Only flag actual security risks, not theoretical concerns.`,
	},
}

func ReviewPhase(r *Runner, planPath string) error {
	banner("REVIEW")

	for round := 1; ; round++ {
		info(fmt.Sprintf("Review round %d — running %d reviewers in parallel", round, len(defaultReviewers)))

		for _, rv := range defaultReviewers {
			removeDexFile(fmt.Sprintf("review-%s.md", rv.Name))
		}

		var wg sync.WaitGroup
		errs := make([]error, len(defaultReviewers))
		for i, rv := range defaultReviewers {
			wg.Add(1)
			go func(idx int, role ReviewRole) {
				defer wg.Done()
				p := renderPrompt("review.txt", map[string]any{
					"PlanPath":   planPath,
					"RoleName":   role.Name,
					"RolePrompt": role.Prompt,
					"ReviewPath": dexPath(fmt.Sprintf("review-%s.md", role.Name)),
				})
				errs[idx] = r.Run(p)
			}(i, rv)
		}
		wg.Wait()

		for i, err := range errs {
			if err != nil {
				errMsg(fmt.Sprintf("Reviewer %q failed: %v", defaultReviewers[i].Name, err))
			}
		}

		allClean := true
		var issues []string
		for _, rv := range defaultReviewers {
			review, _ := readDexFile(fmt.Sprintf("review-%s.md", rv.Name))
			if review == "" {
				warn(fmt.Sprintf("Reviewer %q produced no output", rv.Name))
				allClean = false
				continue
			}
			showBlock(fmt.Sprintf("Review: %s", rv.Name), review)
			if !isCleanReview(review) {
				allClean = false
				issues = append(issues, fmt.Sprintf("── %s ──\n%s", rv.Name, review))
			}
		}

		if allClean {
			info("All reviewers report ZERO ISSUES. Review phase complete!")
			return nil
		}

		info("Issues found — running fixer...")

		fixPrompt := renderPrompt("fix.txt", map[string]any{
			"PlanPath": planPath,
			"Issues":   strings.Join(issues, "\n\n"),
		})
		if err := r.Run(fixPrompt); err != nil {
			errMsg(fmt.Sprintf("Fixer error: %v", err))
			choice := promptChoice("Retry round or abort?", []string{"retry", "abort"})
			if choice == "abort" {
				return fmt.Errorf("aborted by user")
			}
		}
	}
}

func isCleanReview(review string) bool {
	normalized := strings.ToUpper(strings.TrimSpace(review))
	return strings.Contains(normalized, "ZERO ISSUES")
}
