use regex::Regex;
use std::fs;

#[derive(Debug, Clone)]
pub struct TaskGroup {
    pub header: String,
    pub lines: Vec<String>,
    pub open: usize,
    pub done: usize,
}

impl TaskGroup {
    pub fn is_complete(&self) -> bool {
        self.open == 0
    }

    pub fn body(&self) -> String {
        self.lines.join("\n")
    }

    fn has_checkboxes(&self) -> bool {
        self.open + self.done > 0
    }
}

fn push_group(groups: &mut Vec<TaskGroup>, mut group: TaskGroup) {
    while matches!(group.lines.last(), Some(line) if line.trim().is_empty()) {
        group.lines.pop();
    }

    if group.has_checkboxes() {
        groups.push(group);
    }
}

pub fn parse_plan(path: &str) -> Result<Vec<TaskGroup>, String> {
    let data = fs::read_to_string(path).map_err(|e| format!("read plan: {}", e))?;
    Ok(parse_tasks(&data))
}

pub fn parse_tasks(content: &str) -> Vec<TaskGroup> {
    let checkbox_re = Regex::new(r"^(\s*)-\s+\[([ xX])\]\s+(.*)$").unwrap();
    let heading_re = Regex::new(r"^(#{1,6})\s+.*$").unwrap();
    let mut groups: Vec<TaskGroup> = Vec::new();
    let mut cur = TaskGroup {
        header: String::new(),
        lines: Vec::new(),
        open: 0,
        done: 0,
    };

    for line in content.split('\n') {
        let trimmed = line.trim();

        if heading_re.is_match(trimmed) {
            push_group(&mut groups, cur);
            cur = TaskGroup {
                header: trimmed.to_string(),
                lines: Vec::new(),
                open: 0,
                done: 0,
            };
            continue;
        }

        cur.lines.push(line.to_string());
        if let Some(caps) = checkbox_re.captures(line) {
            if &caps[2] == " " {
                cur.open += 1;
            } else {
                cur.done += 1;
            }
        }
    }
    push_group(&mut groups, cur);
    groups
}

pub fn all_tasks_done(path: &str) -> Result<bool, String> {
    let groups = parse_plan(path)?;
    Ok(groups.iter().all(|g| g.is_complete()))
}

pub fn plan_step_counts(path: &str) -> Result<(usize, usize), String> {
    let groups = parse_plan(path)?;
    let open = groups.iter().map(|g| g.open).sum();
    let total = groups.iter().map(|g| g.open + g.done).sum();
    Ok((open, total))
}

pub fn next_open_task(path: &str) -> Result<Option<TaskGroup>, String> {
    let groups = parse_plan(path)?;
    Ok(groups.into_iter().find(|g| !g.is_complete()))
}

pub fn validate_candidate_plan(path: &str) -> Result<(), String> {
    match next_open_task(path)? {
        Some(_) => Ok(()),
        None => Err(format!(
            "candidate plan {:?} does not contain any open task",
            path
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn write_temp_plan(contents: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "dex-plan-test-{}-{}.md",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        fs::write(&path, contents).unwrap();
        path
    }

    #[test]
    fn test_parse_tasks() {
        let plan = "# My Plan\n\n## Overview\nSome background\n- [x] Scope confirmed\n\n## Implementation Steps\n### Task 1: Setup Database\n**Files:**\n- Modify: `src/db.rs`\n\n- [x] Create schema\n\nSome notes here.\n\n- [ ] Write migrations\n- [ ] Add seed data\n\n### Task 2: Build API\n- [ ] Create router\n- [ ] Add handlers\n- [ ] Write tests\n\n## Documentation\n- [x] Write README\n- [x] Add examples\n";

        let groups = parse_tasks(plan);
        assert_eq!(groups.len(), 4);

        assert_eq!(groups[0].header, "## Overview");
        assert_eq!(groups[0].open, 0);
        assert_eq!(groups[0].done, 1);
        assert_eq!(groups[0].body(), "Some background\n- [x] Scope confirmed");

        assert_eq!(groups[1].header, "### Task 1: Setup Database");
        assert_eq!(groups[1].open, 2);
        assert_eq!(groups[1].done, 1);
        assert!(groups[1].body().contains("**Files:**"));
        assert!(groups[1].body().contains("- Modify: `src/db.rs`"));
        assert!(groups[1].body().contains("Some notes here."));
        assert!(!groups[1].is_complete());

        assert_eq!(groups[2].header, "### Task 2: Build API");
        assert_eq!(groups[2].open, 3);
        assert_eq!(groups[2].done, 0);

        assert_eq!(groups[3].header, "## Documentation");
        assert!(groups[3].is_complete());
    }

    #[test]
    fn test_parse_tasks_empty() {
        let groups = parse_tasks("no checkboxes here");
        assert_eq!(groups.len(), 0);
    }

    #[test]
    fn test_parse_tasks_all_done() {
        let plan = "## Done\n- [x] a\n- [x] b\n";
        let groups = parse_tasks(plan);
        assert_eq!(groups.len(), 1);
        assert!(groups[0].is_complete());
    }

    #[test]
    fn test_parse_tasks_keeps_entire_section_together() {
        let plan =
            "## Build\n- [ ] first\n\nSome notes here.\n\n- [ ] second\n\n## Next\n- [ ] third\n";
        let groups = parse_tasks(plan);

        assert_eq!(groups.len(), 2);
        assert_eq!(groups[0].header, "## Build");
        assert_eq!(groups[0].open, 2);
        assert_eq!(
            groups[0].body(),
            "- [ ] first\n\nSome notes here.\n\n- [ ] second"
        );
    }

    #[test]
    fn test_parse_tasks_splits_on_nested_headers() {
        let plan = "## Parent\n- [ ] parent step\n### Child\n- [ ] child step\n";
        let groups = parse_tasks(plan);

        assert_eq!(groups.len(), 2);
        assert_eq!(groups[0].header, "## Parent");
        assert_eq!(groups[0].open, 1);
        assert_eq!(groups[1].header, "### Child");
        assert_eq!(groups[1].open, 1);
    }

    #[test]
    fn test_plan_step_counts_are_plan_wide() {
        let path = write_temp_plan(
            "## Overview\n- [x] aligned scope\n## Build\n- [ ] first\n- [x] second\n### Verify\n- [ ] third\n",
        );
        let counts = plan_step_counts(path.to_str().unwrap());
        let _ = fs::remove_file(&path);

        assert_eq!(counts.unwrap(), (2, 4));
    }

    #[test]
    fn candidate_plan_validation_accepts_open_tasks() {
        let path = write_temp_plan("## Build\n- [ ] implement feature\n");
        let result = validate_candidate_plan(path.to_str().unwrap());
        let _ = fs::remove_file(&path);

        assert!(result.is_ok());
    }

    #[test]
    fn candidate_plan_validation_rejects_completed_plans() {
        let path = write_temp_plan("## Done\n- [x] already finished\n");
        let result = validate_candidate_plan(path.to_str().unwrap());
        let _ = fs::remove_file(&path);

        assert_eq!(
            result.unwrap_err(),
            format!(
                "candidate plan {:?} does not contain any open task",
                path.to_str().unwrap()
            )
        );
    }
}
