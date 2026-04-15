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
}

pub fn parse_plan(path: &str) -> Result<Vec<TaskGroup>, String> {
    let data = fs::read_to_string(path).map_err(|e| format!("read plan: {}", e))?;
    Ok(parse_tasks(&data))
}

pub fn parse_tasks(content: &str) -> Vec<TaskGroup> {
    let checkbox_re = Regex::new(r"^(\s*)-\s+\[([ xX])\]\s+(.*)$").unwrap();
    let lines: Vec<&str> = content.split('\n').collect();
    let mut groups: Vec<TaskGroup> = Vec::new();
    let mut cur: Option<TaskGroup> = None;
    let mut last_header = String::new();

    for line in &lines {
        let trimmed = line.trim();

        if trimmed.starts_with('#') {
            last_header = trimmed.to_string();
        }

        if checkbox_re.is_match(line) {
            let group = cur.get_or_insert_with(|| TaskGroup {
                header: last_header.clone(),
                lines: Vec::new(),
                open: 0,
                done: 0,
            });
            group.lines.push(line.to_string());
            if let Some(caps) = checkbox_re.captures(line) {
                if &caps[2] == " " {
                    group.open += 1;
                } else {
                    group.done += 1;
                }
            }
        } else if let Some(g) = cur.take() {
            groups.push(g);
        }
    }
    if let Some(g) = cur {
        groups.push(g);
    }
    groups
}

pub fn all_tasks_done(path: &str) -> Result<bool, String> {
    let groups = parse_plan(path)?;
    Ok(groups.iter().all(|g| g.is_complete()))
}

pub fn next_open_task(path: &str) -> Result<Option<TaskGroup>, String> {
    let groups = parse_plan(path)?;
    Ok(groups.into_iter().find(|g| !g.is_complete()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_tasks() {
        let plan = "# My Plan\n\n## Setup Database\n- [x] Create schema\n- [ ] Write migrations\n- [ ] Add seed data\n\nSome notes here.\n\n## Build API\n- [ ] Create router\n- [ ] Add handlers\n- [ ] Write tests\n\n## Documentation\n- [x] Write README\n- [x] Add examples\n";

        let groups = parse_tasks(plan);
        assert_eq!(groups.len(), 3);

        // Group 1: Setup Database
        assert_eq!(groups[0].header, "## Setup Database");
        assert_eq!(groups[0].open, 2);
        assert_eq!(groups[0].done, 1);
        assert!(!groups[0].is_complete());

        // Group 2: Build API
        assert_eq!(groups[1].header, "## Build API");
        assert_eq!(groups[1].open, 3);
        assert_eq!(groups[1].done, 0);

        // Group 3: Documentation (all done)
        assert_eq!(groups[2].header, "## Documentation");
        assert!(groups[2].is_complete());
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
}
