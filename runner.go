package main

import (
	"bufio"
	"encoding/json"
	"fmt"
	"os"
	"os/exec"
	"strings"
	"sync"
	"time"

	"charm.land/lipgloss/v2"
)

type CLIConfig struct {
	Cmd   string
	Args  []string
	Stdin bool              // prompt goes to stdin (vs last arg)
	Env   map[string]string // extra env vars for this CLI
}

var cliConfigs = map[string]CLIConfig{
	"opencode": {
		Cmd:   "opencode",
		Args:  []string{"run", "--thinking", "--format", "json"},
		Stdin: true,
		Env: map[string]string{
			"OPENCODE_CONFIG_CONTENT": `{"$schema":"https://opencode.ai/config.json","permission":"allow","lsp":false}`,
		},
	},
	"codex":  {Cmd: "codex", Args: []string{"exec", "--yolo", "--ephemeral", "--json"}, Stdin: true},
	"claude": {Cmd: "claude", Args: []string{"--dangerously-skip-permissions", "--allow-dangerously-skip-permissions", "-p"}, Stdin: false},
	"droid":  {Cmd: "droid", Args: []string{"exec", "--skip-permissions-unsafe"}, Stdin: false},
	"gemini": {Cmd: "gemini", Args: []string{"-y", "-p"}, Stdin: false},
	"pi":     {Cmd: "pi", Args: []string{"--no-session", "-p"}, Stdin: false},
	"raijin": {Cmd: "raijin", Args: []string{"-ephemeral", "-no-echo", "-no-thinking"}, Stdin: false},
}

type Runner struct {
	cfg     CLIConfig
	timeout time.Duration
	label   string
}

func (r *Runner) Labeled(label string) *Runner {
	return &Runner{cfg: r.cfg, timeout: r.timeout, label: label}
}

func NewRunner(name string, timeout time.Duration) (*Runner, error) {
	cfg, ok := cliConfigs[name]
	if !ok {
		names := make([]string, 0, len(cliConfigs))
		for k := range cliConfigs {
			names = append(names, k)
		}
		return nil, fmt.Errorf("unknown CLI %q, available: %s", name, strings.Join(names, ", "))
	}
	return &Runner{cfg: cfg, timeout: timeout}, nil
}

func (r *Runner) Run(prompt string) error {
	delay := time.Second
	for attempt := 0; attempt <= 5; attempt++ {
		if attempt > 0 {
			warn(fmt.Sprintf("Retry %d/5 after %.0fs delay", attempt, delay.Seconds()))
			time.Sleep(delay)
			next := delay * 8
			if next > time.Hour {
				next = time.Hour
			}
			delay = next
		}
		err := r.runOnce(prompt)
		if err == nil {
			return nil
		}
		errMsg(fmt.Sprintf("Agent failed: %v", err))
	}
	return fmt.Errorf("agent failed after 5 retries")
}

func (r *Runner) runOnce(prompt string) error {
	args := append([]string{}, r.cfg.Args...)
	if !r.cfg.Stdin {
		args = append(args, prompt)
	}

	cmd := exec.Command(r.cfg.Cmd, args...)
	setProcGroup(cmd)
	if len(r.cfg.Env) > 0 {
		cmd.Env = os.Environ()
		for k, v := range r.cfg.Env {
			cmd.Env = append(cmd.Env, k+"="+v)
		}
	}

	stdout, err := cmd.StdoutPipe()
	if err != nil {
		return fmt.Errorf("stdout pipe: %w", err)
	}
	stderr, err := cmd.StderrPipe()
	if err != nil {
		return fmt.Errorf("stderr pipe: %w", err)
	}

	if r.cfg.Stdin {
		cmd.Stdin = strings.NewReader(prompt)
	}

	start := time.Now()
	lastOutput := start
	if err := cmd.Start(); err != nil {
		return err
	}

	type line struct {
		text     string
		isStdout bool
	}

	lines := make(chan line)
	var wg sync.WaitGroup

	scan := func(s *bufio.Scanner, isStdout bool) {
		defer wg.Done()
		for s.Scan() {
			lines <- line{text: s.Text(), isStdout: isStdout}
		}
	}

	wg.Add(2)
	go scan(bufio.NewScanner(stdout), true)
	go scan(bufio.NewScanner(stderr), false)
	go func() { wg.Wait(); close(lines) }()

	timer := time.NewTimer(r.timeout)
	defer timer.Stop()

	for {
		select {
		case l, ok := <-lines:
			if !ok {
				return cmd.Wait()
			}
			timer.Reset(r.timeout)
			if l.isStdout {
				if processStdoutLine(l.text, start, r.label) {
					lastOutput = time.Now()
				} else if time.Since(lastOutput) >= time.Minute {
					lipgloss.Printf("%s %s\n", formatPrefix(start, r.label), "Working on it")
					lastOutput = time.Now()
				}
			} else {
				if r.label != "" {
					fmt.Fprintf(os.Stderr, "[%s] %s\n", r.label, l.text)
				} else {
					fmt.Fprintln(os.Stderr, l.text)
				}
			}
		case <-timer.C:
			killProcessTree(cmd)
			cmd.Wait()
			return fmt.Errorf("agent idle timeout after %v", r.timeout)
		}
	}
}

func formatPrefix(start time.Time, label string) string {
	ts := formatElapsed(time.Since(start))
	if label != "" {
		return fmt.Sprintf("%s [%s]", ts, label)
	}
	return ts
}

func processStdoutLine(text string, start time.Time, label string) bool {
	prefix := formatPrefix(start, label)

	var obj any
	if json.Unmarshal([]byte(text), &obj) == nil {
		if m, ok := obj.(map[string]any); ok {
			texts := extractTexts(m)
			if len(texts) > 0 {
				for _, t := range texts {
					lipgloss.Printf("%s %s\n", prefix, t)
				}
				return true
			}
		}
		return false
	}
	lipgloss.Printf("%s %s\n", prefix, text)
	return true
}

func extractTexts(v any) []string {
	var out []string
	walkJSON(v, &out)
	return out
}

func walkJSON(v any, texts *[]string) {
	switch val := v.(type) {
	case map[string]any:
		for k, child := range val {
			if k == "text" {
				if s, ok := child.(string); ok {
					*texts = append(*texts, s)
					continue
				}
			}
			walkJSON(child, texts)
		}
	case []any:
		for _, item := range val {
			walkJSON(item, texts)
		}
	}
}

var styleTimestamp = lipgloss.NewStyle().Bold(true).Foreground(lipgloss.Color("6"))

func formatElapsed(d time.Duration) string {
	h := int(d.Hours())
	m := int(d.Minutes()) % 60
	s := int(d.Seconds()) % 60
	return styleTimestamp.Render(fmt.Sprintf("[%02d:%02d:%02d]", h, m, s))
}
