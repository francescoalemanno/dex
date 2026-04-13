package main

import "testing"

func TestParseTasks(t *testing.T) {
	plan := `# My Plan

## Setup Database
- [x] Create schema
- [ ] Write migrations
- [ ] Add seed data

Some notes here.

## Build API
- [ ] Create router
- [ ] Add handlers
- [ ] Write tests

## Documentation
- [x] Write README
- [x] Add examples
`

	groups := parseTasks(plan)
	if len(groups) != 3 {
		t.Fatalf("expected 3 groups, got %d", len(groups))
	}

	// Group 1: Setup Database
	if groups[0].Header != "## Setup Database" {
		t.Errorf("group 0 header = %q", groups[0].Header)
	}
	if groups[0].Open != 2 || groups[0].Done != 1 {
		t.Errorf("group 0: open=%d done=%d", groups[0].Open, groups[0].Done)
	}
	if groups[0].IsComplete() {
		t.Error("group 0 should not be complete")
	}

	// Group 2: Build API
	if groups[1].Header != "## Build API" {
		t.Errorf("group 1 header = %q", groups[1].Header)
	}
	if groups[1].Open != 3 || groups[1].Done != 0 {
		t.Errorf("group 1: open=%d done=%d", groups[1].Open, groups[1].Done)
	}

	// Group 3: Documentation (all done)
	if groups[2].Header != "## Documentation" {
		t.Errorf("group 2 header = %q", groups[2].Header)
	}
	if !groups[2].IsComplete() {
		t.Error("group 2 should be complete")
	}
}

func TestParseTasksEmpty(t *testing.T) {
	groups := parseTasks("no checkboxes here")
	if len(groups) != 0 {
		t.Fatalf("expected 0 groups, got %d", len(groups))
	}
}

func TestParseTasksAllDone(t *testing.T) {
	plan := `## Done
- [x] a
- [x] b
`
	groups := parseTasks(plan)
	if len(groups) != 1 {
		t.Fatalf("expected 1 group, got %d", len(groups))
	}
	if !groups[0].IsComplete() {
		t.Error("should be complete")
	}
}
