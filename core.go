package main

import (
	"bytes"
	"embed"
	"encoding/json"
	"fmt"
	"os"
	"path/filepath"
	"strings"
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

func savePlanRequest(request string) {
	ensureDexDir()
	os.WriteFile(dexPath("request.txt"), []byte(request), 0o644)
}

func saveFeedbacks(feedbacks []string) {
	ensureDexDir()
	data, _ := json.MarshalIndent(feedbacks, "", "  ")
	os.WriteFile(dexPath("feedbacks.json"), data, 0o644)
}

func loadFeedbacks() []string {
	data, err := os.ReadFile(dexPath("feedbacks.json"))
	if err != nil {
		return nil
	}
	var fb []string
	json.Unmarshal(data, &fb)
	return fb
}

func clearPlanState() {
	removeDexFile("plan.md")
	removeDexFile("request.txt")
	removeDexFile("feedbacks.json")
	removeDexFile("questions.md")
}
