//! Markdown task lines (`- [ ] text`, `* [x] text`, `1. [/] text`) read
//! straight from file content, without Obsidian's metadata cache.

use std::sync::LazyLock;

use anyhow::{Result, bail};
use regex::Regex;
use serde::Serialize;

use super::markdown_lines::{non_body_lines, strip_cr};
use crate::util::js;

static TASK: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^([ \t]*)([-*+]|[0-9]+[.)])([ \t]+)\[(.)\](?:[ \t]+(.*))?$").expect("valid pattern"));
static LIST_ITEM: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^([ \t]*)(?:[-*+]|[0-9]+[.)])(?:[ \t]|$)").expect("valid pattern"));
static DUE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"📅\s*([0-9]{4}-[0-9]{2}-[0-9]{2})").expect("valid pattern"));
const TAB_WIDTH: usize = 4;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum TaskState {
    Open,
    Done,
    Cancelled,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Task {
    /// 1-based line number, counted from the start of the file.
    pub line: usize,
    /// The raw character between the brackets.
    pub status: String,
    pub state: TaskState,
    pub text: String,
    /// Nesting depth among list items: 0 for a top-level item.
    pub indent: usize,
    /// Line of the nearest enclosing task, when the task is nested under one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_line: Option<usize>,
    /// Obsidian Tasks due date (`📅 YYYY-MM-DD`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub due: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskLine {
    /// Leading whitespace, list marker and the space before `[`.
    pub prefix: String,
    pub status: String,
    pub text: String,
}

pub fn parse_task_line(line: &str) -> Option<TaskLine> {
    let m = TASK.captures(strip_cr(line))?;
    Some(TaskLine {
        prefix: format!("{}{}{}", &m[1], &m[2], &m[3]),
        status: m[4].to_string(),
        text: js::trim_end(m.get(5).map_or("", |t| t.as_str())).to_string(),
    })
}

pub fn task_state(status: &str) -> TaskState {
    match status {
        "x" | "X" => TaskState::Done,
        "-" => TaskState::Cancelled,
        _ => TaskState::Open,
    }
}

fn indent_width(ws: &str) -> usize {
    ws.chars().map(|c| if c == '\t' { TAB_WIDTH } else { 1 }).sum()
}

struct Open {
    width: usize,
    line: usize,
    task: bool,
}

/// Every task in `content`, skipping frontmatter and fenced code.
pub fn scan_tasks(content: &str) -> Vec<Task> {
    let lines: Vec<&str> = content.split('\n').collect();
    let skip = non_body_lines(&lines);
    let mut tasks = Vec::new();
    // Open list items above the current line, outermost first.
    let mut stack: Vec<Open> = Vec::new();

    for (i, line) in lines.iter().enumerate() {
        if skip[i] {
            continue;
        }
        let raw = strip_cr(line);
        let Some(item) = LIST_ITEM.captures(raw) else {
            // A flush-left line that is not a list item ends the list; blank and
            // indented continuation lines keep it open.
            if !js::trim(raw).is_empty() && !raw.starts_with([' ', '\t']) {
                stack.clear();
            }
            continue;
        };

        let width = indent_width(&item[1]);
        while stack.last().is_some_and(|s| s.width >= width) {
            stack.pop();
        }
        let parent = stack.iter().rev().find(|s| s.task).map(|s| s.line);
        let parsed = parse_task_line(raw);
        stack.push(Open { width, line: i + 1, task: parsed.is_some() });
        let Some(parsed) = parsed else { continue };

        let due = DUE.captures(&parsed.text).map(|m| m[1].to_string());
        tasks.push(Task {
            line: i + 1,
            state: task_state(&parsed.status),
            status: parsed.status,
            text: parsed.text,
            indent: stack.len() - 1,
            parent_line: parent,
            due,
        });
    }
    tasks
}

/// `content` with the status character of the task on `line` (1-based)
/// replaced. Every other byte stays as it was.
pub fn replace_task_status(content: &str, line: usize, status: &str) -> Result<String> {
    let mut start = 0;
    for _ in 1..line {
        // `indexOf` returns -1 past the last line, which wraps `start` to 0.
        start = content[start..].find('\n').map_or(0, |p| start + p + 1);
    }
    let end = content[start..].find('\n').map_or(content.len(), |p| start + p);
    let text = &content[start..end];
    let Some(parsed) = parse_task_line(text) else {
        bail!("Line {line} is not a task");
    };
    let at = start + parsed.prefix.len() + 1;
    Ok(format!("{}{}{}", &content[..at], status, &content[at + parsed.status.len()..]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn skips_tasks_in_an_unclosed_fence_and_treats_unclosed_frontmatter_as_body() {
        let lines: Vec<usize> = scan_tasks("---\n- [ ] body task\n```\n- [ ] code").iter().map(|t| t.line).collect();
        assert_eq!(lines, vec![2]);
    }

    #[test]
    fn resets_nesting_after_a_flush_left_paragraph_and_keeps_it_over_indented_text() {
        let tasks = scan_tasks("- [ ] a\n  continued\n  - [ ] b\nPara\n  - [ ] c");
        let got: Vec<_> = tasks.iter().map(|t| (t.line, t.indent, t.parent_line)).collect();
        assert_eq!(got, vec![(1, 0, None), (3, 1, Some(1)), (5, 0, None)]);
    }

    #[test]
    fn accepts_plus_markers_and_paren_numbering() {
        let states: Vec<_> = scan_tasks("+ [ ] plus\n2) [x] paren").iter().map(|t| t.state).collect();
        assert_eq!(states, vec![TaskState::Open, TaskState::Done]);
    }

    #[test]
    fn reads_due_dates_and_serializes_like_the_ts() {
        let tasks = scan_tasks("- [-] ship 📅 2024-05-01");
        assert_eq!(tasks[0].due.as_deref(), Some("2024-05-01"));
        assert_eq!(
            serde_json::to_value(&tasks[0]).unwrap(),
            serde_json::json!({"line": 1, "status": "-", "state": "cancelled", "text": "ship 📅 2024-05-01", "indent": 0, "due": "2024-05-01"})
        );
    }

    #[test]
    fn changes_one_character_and_errors_on_a_non_task_line() {
        assert_eq!(replace_task_status("a\n- [ ] b\nc", 2, "x").unwrap(), "a\n- [x] b\nc");
        assert_eq!(replace_task_status("- [🔥] hot", 1, " ").unwrap(), "- [ ] hot");
        assert!(replace_task_status("plain", 1, "x").unwrap_err().to_string().contains("not a task"));
    }
}
