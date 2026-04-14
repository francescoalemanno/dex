package main

import (
	"encoding/json"
	"flag"
	"fmt"
	"os"
	"os/exec"
	"strings"
	"time"
)

type Config struct {
	CLI      string `json:"cli"`
	Plan     string `json:"plan"`
	NoReview bool   `json:"no_review"`
	BaseRef  string `json:"base_ref"`
}

func loadConfig() Config {
	cfg := Config{CLI: "opencode", BaseRef: "HEAD"}
	data, err := os.ReadFile(dexPath("config.json"))
	if err != nil {
		return cfg
	}
	json.Unmarshal(data, &cfg)
	return cfg
}

func saveConfig(cfg Config) {
	ensureDexDir()
	data, _ := json.MarshalIndent(cfg, "", "  ")
	os.WriteFile(dexPath("config.json"), append(data, '\n'), 0o644)
}

func main() {
	defaults := loadConfig()

	cliName := flag.String("cli", defaults.CLI, "coding CLI to use")
	planFile := flag.String("plan", defaults.Plan, "skip planning, use existing plan file")
	skipReview := flag.Bool("no-review", defaults.NoReview, "skip the review phase")
	baseRef := flag.String("base-ref", defaults.BaseRef, "base git ref for review diffs")
	timeout := flag.Duration("timeout", 20*time.Minute, "kill agent after this idle duration")
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

	// Persist final flag values
	saveConfig(Config{
		CLI:      *cliName,
		Plan:     *planFile,
		NoReview: *skipReview,
		BaseRef:  *baseRef,
	})

	runner, err := NewRunner(*cliName, *timeout)
	if err != nil {
		fmt.Fprintln(os.Stderr, err)
		os.Exit(1)
	}

	request := strings.Join(flag.Args(), " ")
	if request == "" && *planFile == "" {
		request = promptMultiline("Enter your request:")
		if strings.TrimSpace(request) == "" {
			flag.Usage()
			os.Exit(1)
		}
	}

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

	// Snapshot base ref before implementation if using default
	if *baseRef == "HEAD" {
		if out, err := exec.Command("git", "rev-parse", "HEAD").Output(); err == nil {
			*baseRef = strings.TrimSpace(string(out))
		}
		saveConfig(Config{
			CLI:      *cliName,
			Plan:     *planFile,
			NoReview: *skipReview,
			BaseRef:  *baseRef,
		})
	}

	// Phase 2: Implementation
	if err := ImplPhase(runner, planPath); err != nil {
		errMsg(err.Error())
		os.Exit(1)
	}

	// Phase 3: Review
	if !*skipReview {
		if err := ReviewPhase(runner, planPath, *baseRef); err != nil {
			errMsg(err.Error())
			os.Exit(1)
		}
	}

	banner("DONE")
	info("All phases complete.")
}
