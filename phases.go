package main

import (
	"fmt"
	"os"
	"path/filepath"
	"strings"
	"sync"
)

const dexDir = ".dex"

func ensureDexDir() error {
	return os.MkdirAll(dexDir, 0o755)
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

		// clean transient files
		removeDexFile("questions.md")

		// build prompt
		p := buildPlanPrompt(request, feedbacks)

		// run CLI
		if err := r.Run(p); err != nil {
			errMsg(fmt.Sprintf("CLI error: %v", err))
			choice := promptChoice("Retry or abort?", []string{"retry", "abort"})
			if choice == "abort" {
				return "", fmt.Errorf("aborted by user")
			}
			continue
		}

		// check for questions
		questions, _ := readDexFile("questions.md")
		if questions != "" {
			showBlock("Questions from CLI", questions)
			answer := promptMultiline("Your answers:")
			feedbacks = append(feedbacks, fmt.Sprintf("Questions:\n%s\n\nAnswers:\n%s", questions, answer))
			continue
		}

		// check for plan
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

func buildPlanPrompt(request string, feedbacks []string) string {
	var sb strings.Builder
	sb.WriteString("You are in the PLANNING phase. Do NOT implement anything.\n\n")
	sb.WriteString("The user wants:\n")
	sb.WriteString(request)
	sb.WriteString("\n\n")
	sb.WriteString("Instructions:\n")
	sb.WriteString("1. If you need to ask clarifying questions, write them to .dex/questions.md (one per line).\n")
	sb.WriteString("2. Otherwise, write a detailed implementation plan to .dex/plan.md\n\n")
	sb.WriteString("The plan MUST use markdown checkboxes grouped into logical tasks:\n\n")
	sb.WriteString("## Task Name\n")
	sb.WriteString("- [ ] Step 1\n")
	sb.WriteString("- [ ] Step 2\n")
	sb.WriteString("- [ ] Step 3\n\n")
	sb.WriteString("Each task should have 3-7 steps. Separate tasks with headings.\n")
	sb.WriteString("Do NOT write any code. Only produce the plan or questions.\n")

	if len(feedbacks) > 0 {
		sb.WriteString("\n── Previous feedback from the user ──\n")
		for i, f := range feedbacks {
			sb.WriteString(fmt.Sprintf("\n[Round %d]\n%s\n", i+1, f))
		}
		sb.WriteString("\nRevise your plan in .dex/plan.md based on ALL feedback above.\n")
	}

	return sb.String()
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

		p := buildImplPrompt(planPath, task)

		if err := r.Run(p); err != nil {
			errMsg(fmt.Sprintf("CLI error: %v", err))
			choice := promptChoice("Retry or abort?", []string{"retry", "abort"})
			if choice == "abort" {
				return fmt.Errorf("aborted by user")
			}
			continue
		}

		// verify progress
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

func buildImplPrompt(planPath string, task *TaskGroup) string {
	var sb strings.Builder
	sb.WriteString("You are in the IMPLEMENTATION phase.\n\n")
	sb.WriteString(fmt.Sprintf("Read the full plan at %s.\n\n", planPath))
	sb.WriteString("Execute ONLY the following task (the first open task group):\n\n")
	if task.Header != "" {
		sb.WriteString(task.Header + "\n")
	}
	sb.WriteString(task.String())
	sb.WriteString("\n\n")
	sb.WriteString(fmt.Sprintf("After completing each step, update %s and mark the step as \"- [x]\".\n", planPath))
	sb.WriteString("Do NOT work on any other task. Stop after this task group is done.\n")
	return sb.String()
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

		// clean old reviews
		for _, rv := range defaultReviewers {
			removeDexFile(fmt.Sprintf("review-%s.md", rv.Name))
		}

		// run reviewers in parallel
		var wg sync.WaitGroup
		errs := make([]error, len(defaultReviewers))
		for i, rv := range defaultReviewers {
			wg.Add(1)
			go func(idx int, role ReviewRole) {
				defer wg.Done()
				p := buildReviewPrompt(planPath, role)
				errs[idx] = r.Run(p)
			}(i, rv)
		}
		wg.Wait()

		// check for CLI errors
		for i, err := range errs {
			if err != nil {
				errMsg(fmt.Sprintf("Reviewer %q failed: %v", defaultReviewers[i].Name, err))
			}
		}

		// collect reviews
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

		fixPrompt := buildFixPrompt(planPath, issues)
		if err := r.Run(fixPrompt); err != nil {
			errMsg(fmt.Sprintf("Fixer error: %v", err))
			choice := promptChoice("Retry round or abort?", []string{"retry", "abort"})
			if choice == "abort" {
				return fmt.Errorf("aborted by user")
			}
		}
	}
}

func buildReviewPrompt(planPath string, role ReviewRole) string {
	var sb strings.Builder
	sb.WriteString("You are a code REVIEWER.\n\n")
	sb.WriteString(fmt.Sprintf("The implementation plan is at %s. Read it to understand what was built.\n\n", planPath))
	sb.WriteString(fmt.Sprintf("Your role: %s\n\n", role.Name))
	sb.WriteString(role.Prompt)
	sb.WriteString("\n\n")
	sb.WriteString(fmt.Sprintf("Write your review to %s\n\n", dexPath(fmt.Sprintf("review-%s.md", role.Name))))
	sb.WriteString("If you find NO issues at all, write exactly: ZERO ISSUES\n")
	sb.WriteString("Otherwise, list each issue with file path and description.\n")
	sb.WriteString("Do NOT fix anything. Only review.\n")
	return sb.String()
}

func buildFixPrompt(planPath string, issues []string) string {
	var sb strings.Builder
	sb.WriteString("You are a code FIXER.\n\n")
	sb.WriteString(fmt.Sprintf("The implementation plan is at %s.\n\n", planPath))
	sb.WriteString("The following issues were found by reviewers. Fix ALL of them:\n\n")
	sb.WriteString(strings.Join(issues, "\n\n"))
	sb.WriteString("\n\nDeduplicate overlapping issues. Fix each one.\n")
	sb.WriteString("Do NOT introduce new features or refactoring beyond what the issues require.\n")
	return sb.String()
}

func isCleanReview(review string) bool {
	normalized := strings.ToUpper(strings.TrimSpace(review))
	return strings.Contains(normalized, "ZERO ISSUES")
}
