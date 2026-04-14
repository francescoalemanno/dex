package main

import (
	"fmt"
	"io"
	"os"
	"strings"
	"testing"
	"time"
)

func TestRunnerHandlesLongJSONLines(t *testing.T) {
	r := helperRunner(t, "long-json-line", false, 2*time.Second)
	if err := r.runOnce(""); err != nil {
		t.Fatalf("runOnce failed for long JSON line: %v", err)
	}
}

func TestRunnerHandlesLongJSONLineViaStdin(t *testing.T) {
	r := helperRunner(t, "long-json-line-stdin", true, 2*time.Second)
	if err := r.runOnce(strings.Repeat("p", 128)); err != nil {
		t.Fatalf("runOnce failed for stdin-driven long JSON line: %v", err)
	}
}

func helperRunner(t *testing.T, scenario string, stdin bool, timeout time.Duration) *Runner {
	t.Helper()

	exe, err := os.Executable()
	if err != nil {
		t.Fatalf("resolve test binary: %v", err)
	}

	args := []string{"-test.run=TestHelperProcess", "--", scenario}
	return &Runner{
		cfg: CLIConfig{
			Cmd:   exe,
			Args:  args,
			Stdin: stdin,
			Env: map[string]string{
				"DEX_HELPER_PROCESS": "1",
			},
		},
		timeout: timeout,
	}
}

func TestHelperProcess(t *testing.T) {
	if os.Getenv("DEX_HELPER_PROCESS") != "1" {
		return
	}

	args := os.Args
	for i, arg := range args {
		if arg != "--" || i+1 >= len(args) {
			continue
		}
		switch args[i+1] {
		case "long-json-line":
			fmt.Printf("{\"kind\":\"event\",\"payload\":\"%s\"}\n", strings.Repeat("x", 256*1024))
			os.Exit(0)
		case "long-json-line-stdin":
			data, err := io.ReadAll(os.Stdin)
			if err != nil {
				fmt.Fprintln(os.Stderr, err)
				os.Exit(1)
			}
			fmt.Printf("{\"kind\":\"event\",\"stdin_bytes\":%d,\"payload\":\"%s\"}\n", len(data), strings.Repeat("y", 256*1024))
			os.Exit(0)
		default:
			fmt.Fprintf(os.Stderr, "unknown helper scenario %q\n", args[i+1])
			os.Exit(2)
		}
	}

	fmt.Fprintln(os.Stderr, "missing helper scenario")
	os.Exit(2)
}
