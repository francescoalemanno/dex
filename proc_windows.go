//go:build windows

package main

import "os/exec"

func setProcGroup(cmd *exec.Cmd) {
	// No process-group support on Windows; fall back to default behavior.
}

func killProcessTree(cmd *exec.Cmd) {
	cmd.Process.Kill()
}
