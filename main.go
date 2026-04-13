package main

import (
	"flag"
	"fmt"
	"os"
	"strings"
)

func main() {
	cliName := flag.String("cli", "opencode", "coding CLI to use")
	planFile := flag.String("plan", "", "skip planning, use existing plan file")
	skipReview := flag.Bool("no-review", false, "skip the review phase")
	flag.Usage = func() {
		fmt.Fprintf(os.Stderr, "Usage: dex [flags] <request...>\n\nFlags:\n")
		flag.PrintDefaults()
		fmt.Fprintf(os.Stderr, "\nSupported CLIs: ")
		names := make([]string, 0, len(cliConfigs))
		for k := range cliConfigs {
			names = append(names, k)
		}
		fmt.Fprintln(os.Stderr, strings.Join(names, ", "))
	}
	flag.Parse()

	if flag.NArg() == 0 && *planFile == "" {
		flag.Usage()
		os.Exit(1)
	}

	runner, err := NewRunner(*cliName)
	if err != nil {
		fmt.Fprintln(os.Stderr, err)
		os.Exit(1)
	}

	request := strings.Join(flag.Args(), " ")

	// Phase 1: Planning
	planPath := *planFile
	if planPath == "" {
		var err error
		planPath, err = PlanPhase(runner, request)
		if err != nil {
			errMsg(err.Error())
			os.Exit(1)
		}
		if planPath == "" {
			os.Exit(0)
		}
	}

	// Phase 2: Implementation
	if err := ImplPhase(runner, planPath); err != nil {
		errMsg(err.Error())
		os.Exit(1)
	}

	// Phase 3: Review
	if !*skipReview {
		if err := ReviewPhase(runner, planPath); err != nil {
			errMsg(err.Error())
			os.Exit(1)
		}
	}

	banner("DONE")
	info("All phases complete.")
}
