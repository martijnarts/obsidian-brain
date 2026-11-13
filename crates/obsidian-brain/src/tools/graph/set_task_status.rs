//! `set_task_status`: rewrite the one character between a task's brackets.
//! The task is looked up with the same scan as `list_tasks`, so a `- [ ]`
//! line inside a code fence or the frontmatter is not a task here either.

use anyhow::{Result, bail};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::mcp::tool::ToolSpec;
use crate::resolve::single_note::resolve_single_note;
use crate::tools::support::background_reindex::run_background_reindex;
use crate::util::js;
use crate::vault::tasks::{replace_task_status, scan_tasks, task_state};
use crate::vault::vault_path::{read_note_file, write_file_atomic};

const DESCRIPTION: &str = "Set the status of one markdown task in place. Only the character between the brackets changes; the rest of the file stays byte-identical. Refuses a line that is not a task.";

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
struct Args {
    /// Path or fuzzy match of the note.
    name: String,
    /// 1-based line of the task, as `list_tasks` returns it.
    #[serde(deserialize_with = "super::positive")]
    #[schemars(range(min = 1))]
    line: u64,
    /// `open` (space), `done` (`x`), or one raw status character such as `/` or `-`.
    status: String,
    /// The task text you expect on that line. The write is refused when it differs, e.g. after the note changed.
    expected_text: Option<String>,
}

fn quoted(s: &str) -> String {
    js::json_stringify(&Value::String(s.to_string()))
}

fn to_status_char(status: &str) -> Result<String> {
    match status {
        "open" => return Ok(" ".into()),
        "done" => return Ok("x".into()),
        _ => {}
    }
    if status.chars().count() != 1 || status.contains(['\r', '\n', '[', ']']) {
        bail!(
            "status must be \"open\", \"done\", or one character other than a bracket or line break; got {}",
            quoted(status)
        );
    }
    Ok(status.to_string())
}

pub fn spec() -> ToolSpec {
    ToolSpec::new::<Args, _, _>("set_task_status", DESCRIPTION, |ctx, args| async move {
        let status = to_status_char(&args.status)?;
        let name = args.name.clone();
        let id = ctx
            .db
            .run(move |c| resolve_single_note(&name, c, false))
            .await?;
        let note = read_note_file(&ctx.config.vault_path, &id)?;
        let line = args.line as usize;

        let lines: Vec<&str> = note.content.split('\n').collect();
        if line > lines.len() {
            bail!(
                "Line {line} is past the end of \"{}\" ({} lines).",
                note.rel,
                lines.len()
            );
        }
        let Some(task) = scan_tasks(&note.content)
            .into_iter()
            .find(|t| t.line == line)
        else {
            let text = lines[line - 1];
            let text = text.strip_suffix('\r').unwrap_or(text);
            bail!(
                "Line {line} of \"{}\" is not a task: {}. Use list_tasks to find the line.",
                note.rel,
                quoted(text)
            );
        };
        if let Some(expected) = &args.expected_text
            && js::trim(expected) != js::trim(&task.text)
        {
            bail!(
                "Task text on line {line} of \"{}\" is {}, not {}. Run list_tasks again and retry.",
                note.rel,
                quoted(&task.text),
                quoted(expected)
            );
        }

        let changed = task.status != status;
        if changed {
            let next = replace_task_status(&note.content, line, &status)?;
            write_file_atomic(&note.abs, next)?;
            run_background_reindex(&ctx);
        }
        Ok(json!({
            "path": note.rel,
            "line": line,
            "previousStatus": task.status,
            "status": status,
            "state": task_state(&status),
            "text": task.text,
            "changed": changed,
        }))
    })
}
