package main

import (
	"os"
	"regexp"
	"strings"
)

var checkboxRe = regexp.MustCompile(`^(\s*)-\s+\[([ xX])\]\s+(.*)$`)

type TaskGroup struct {
	Header string   // nearest heading above the group, if any
	Lines  []string // raw lines of the group
	Open   int      // count of unchecked items
	Done   int      // count of checked items
}

func (t TaskGroup) IsComplete() bool { return t.Open == 0 }

func (t TaskGroup) String() string { return strings.Join(t.Lines, "\n") }

func ParsePlan(path string) ([]TaskGroup, error) {
	data, err := os.ReadFile(path)
	if err != nil {
		return nil, err
	}
	return parseTasks(string(data)), nil
}

func parseTasks(content string) []TaskGroup {
	lines := strings.Split(content, "\n")
	var groups []TaskGroup
	var cur *TaskGroup
	lastHeader := ""

	for _, line := range lines {
		trimmed := strings.TrimSpace(line)

		// track headings
		if strings.HasPrefix(trimmed, "#") {
			lastHeader = trimmed
		}

		if checkboxRe.MatchString(line) {
			if cur == nil {
				cur = &TaskGroup{Header: lastHeader}
			}
			cur.Lines = append(cur.Lines, line)
			m := checkboxRe.FindStringSubmatch(line)
			if m[2] == " " {
				cur.Open++
			} else {
				cur.Done++
			}
		} else {
			if cur != nil {
				groups = append(groups, *cur)
				cur = nil
			}
		}
	}
	if cur != nil {
		groups = append(groups, *cur)
	}
	return groups
}

func AllTasksDone(path string) (bool, error) {
	groups, err := ParsePlan(path)
	if err != nil {
		return false, err
	}
	for _, g := range groups {
		if !g.IsComplete() {
			return false, nil
		}
	}
	return true, nil
}

func NextOpenTask(path string) (*TaskGroup, error) {
	groups, err := ParsePlan(path)
	if err != nil {
		return nil, err
	}
	for _, g := range groups {
		if !g.IsComplete() {
			return &g, nil
		}
	}
	return nil, nil
}
