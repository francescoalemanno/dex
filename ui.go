package main

import (
	"bufio"
	"fmt"
	"os"
	"strings"

	"github.com/charmbracelet/glamour"
)

const (
	colorReset  = "\033[0m"
	colorBold   = "\033[1m"
	colorCyan   = "\033[36m"
	colorGreen  = "\033[32m"
	colorYellow = "\033[33m"
	colorRed    = "\033[31m"
	colorDim    = "\033[2m"
)

func banner(phase string) {
	fmt.Printf("\n%s%s══════ %s ══════%s\n\n", colorBold, colorCyan, phase, colorReset)
}

func info(msg string) {
	fmt.Printf("%s%s▸ %s%s\n", colorBold, colorGreen, msg, colorReset)
}

func warn(msg string) {
	fmt.Printf("%s%s▸ %s%s\n", colorBold, colorYellow, msg, colorReset)
}

func errMsg(msg string) {
	fmt.Printf("%s%s▸ %s%s\n", colorBold, colorRed, msg, colorReset)
}

func showBlock(title, content string) {
	fmt.Printf("\n%s%s── %s ──%s\n", colorBold, colorDim, title, colorReset)
	fmt.Println(content)
	fmt.Printf("%s%s── end ──%s\n\n", colorBold, colorDim, colorReset)
}

func showMarkdown(title, md string) {
	fmt.Printf("\n%s%s── %s ──%s\n", colorBold, colorDim, title, colorReset)
	rendered, err := glamour.Render(md, "auto")
	if err != nil {
		fmt.Println(md)
	} else {
		fmt.Print(rendered)
	}
	fmt.Printf("%s%s── end ──%s\n\n", colorBold, colorDim, colorReset)
}

func promptMultiline(msg string) string {
	fmt.Printf("%s%s%s %s(single .  to finish)%s\n", colorBold, msg, colorReset, colorDim, colorReset)
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
		fmt.Printf("%s%s%s [%s] ", colorBold, msg, colorReset, strings.Join(choices, "/"))
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
