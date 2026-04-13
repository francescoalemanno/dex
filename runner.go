package main

import (
	"fmt"
	"os"
	"os/exec"
	"strings"
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
		Args:  []string{"run", "--thinking", "--format", "default"},
		Stdin: true,
		Env: map[string]string{
			"OPENCODE_CONFIG_CONTENT": `{"$schema":"https://opencode.ai/config.json","permission":"allow","lsp":false}`,
		},
	},
	"codex":  {Cmd: "codex", Args: []string{"exec", "--dangerously-bypass-approvals-and-sandbox", "--ephemeral"}, Stdin: true},
	"claude": {Cmd: "claude", Args: []string{"--dangerously-skip-permissions", "--allow-dangerously-skip-permissions", "-p"}, Stdin: false},
	"droid":  {Cmd: "droid", Args: []string{"exec", "--skip-permissions-unsafe"}, Stdin: false},
	"gemini": {Cmd: "gemini", Args: []string{"-y", "-p"}, Stdin: false},
	"pi":     {Cmd: "pi", Args: []string{"--no-session", "-p"}, Stdin: false},
	"raijin": {Cmd: "raijin", Args: []string{"-ephemeral", "-no-echo", "-no-thinking"}, Stdin: false},
}

type Runner struct {
	cfg CLIConfig
}

func NewRunner(name string) (*Runner, error) {
	cfg, ok := cliConfigs[name]
	if !ok {
		names := make([]string, 0, len(cliConfigs))
		for k := range cliConfigs {
			names = append(names, k)
		}
		return nil, fmt.Errorf("unknown CLI %q, available: %s", name, strings.Join(names, ", "))
	}
	return &Runner{cfg: cfg}, nil
}

func (r *Runner) Run(prompt string) error {
	args := append([]string{}, r.cfg.Args...)
	if !r.cfg.Stdin {
		args = append(args, prompt)
	}

	cmd := exec.Command(r.cfg.Cmd, args...)
	cmd.Stdout = os.Stdout
	cmd.Stderr = os.Stderr
	if len(r.cfg.Env) > 0 {
		cmd.Env = os.Environ()
		for k, v := range r.cfg.Env {
			cmd.Env = append(cmd.Env, k+"="+v)
		}
	}

	if r.cfg.Stdin {
		cmd.Stdin = strings.NewReader(prompt)
	}

	return cmd.Run()
}
