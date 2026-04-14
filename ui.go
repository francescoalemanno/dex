package main

import (
	"bufio"
	"fmt"
	"os"
	"strings"

	"charm.land/glamour/v2"
	"charm.land/lipgloss/v2"
)

var (
	styleBanner = lipgloss.NewStyle().Bold(true).Foreground(lipgloss.Color("6"))  // cyan
	styleInfo   = lipgloss.NewStyle().Bold(true).Foreground(lipgloss.Color("2"))  // green
	styleWarn   = lipgloss.NewStyle().Bold(true).Foreground(lipgloss.Color("3"))  // yellow
	styleErr    = lipgloss.NewStyle().Bold(true).Foreground(lipgloss.Color("1"))  // red
	styleDim    = lipgloss.NewStyle().Bold(true).Faint(true)

	glamourStyle = func() string {
		if lipgloss.HasDarkBackground(os.Stdin, os.Stdout) {
			return "dark"
		}
		return "light"
	}()
)

func banner(phase string) {
	lipgloss.Printf("\n%s\n\n", styleBanner.Render(fmt.Sprintf("══════ %s ══════", phase)))
}

func info(msg string) {
	lipgloss.Println(styleInfo.Render("▸ " + msg))
}

func warn(msg string) {
	lipgloss.Println(styleWarn.Render("▸ " + msg))
}

func errMsg(msg string) {
	lipgloss.Println(styleErr.Render("▸ " + msg))
}

func showBlock(title, content string) {
	lipgloss.Printf("\n%s\n", styleDim.Render(fmt.Sprintf("── %s ──", title)))
	fmt.Println(content)
	lipgloss.Printf("%s\n\n", styleDim.Render("── end ──"))
}

func showMarkdown(title, md string) {
	lipgloss.Printf("\n%s\n", styleDim.Render(fmt.Sprintf("── %s ──", title)))
	rendered, err := glamour.Render(md, glamourStyle)
	if err != nil {
		fmt.Println(md)
	} else {
		lipgloss.Print(rendered)
	}
	lipgloss.Printf("%s\n\n", styleDim.Render("── end ──"))
}

func promptMultiline(msg string) string {
	lipgloss.Printf("%s %s\n", lipgloss.NewStyle().Bold(true).Render(msg), styleDim.Render("(single .  to finish)"))
	scanner := bufio.NewScanner(os.Stdin)
	scanner.Buffer(make([]byte, 1024*1024), 1024*1024)
	var lines []string
	for scanner.Scan() {
		line := scanner.Text()
		if strings.TrimSpace(line) == "." && len(lines) > 0 {
			break
		}
		lines = append(lines, line)
	}
	return strings.Join(lines, "\n")
}

func promptChoice(msg string, choices []string) string {
	for {
		lipgloss.Printf("%s [%s] ", lipgloss.NewStyle().Bold(true).Render(msg), strings.Join(choices, "/"))
		scanner := bufio.NewScanner(os.Stdin)
		if scanner.Scan() {
			ans := strings.ToLower(strings.TrimSpace(scanner.Text()))
			for _, c := range choices {
				if strings.ToLower(c) == ans || (len(ans) == 1 && strings.HasPrefix(strings.ToLower(c), ans)) {
					return strings.ToLower(c)
				}
			}
		}
		fmt.Println("Invalid choice, try again.")
	}
}
