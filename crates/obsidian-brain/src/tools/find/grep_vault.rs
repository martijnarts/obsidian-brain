//! `grep_vault`: line-by-line text search over the note files on disk.

use std::path::Path;
use std::time::Instant;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::mcp::tool::ToolSpec;
use crate::util::js;
use crate::util::js::utf16_cmp;
use crate::util::safe_regex::{UserRegex, compile_user_regex};
use crate::vault::parser::collect_markdown_files;
use crate::vault::vault_path::{read_utf8, resolve_folder};

/// Wall-clock budget for one scan. Past it the tool returns what it has.
pub const GREP_TIME_BUDGET_MS: u64 = 2_000;
/// Returned lines are clipped to this many UTF-16 units (around the match).
const MAX_LINE_CHARS: usize = 400;

const DESCRIPTION: &str = "Search the text of every note file on disk, frontmatter included, for a literal string or a JavaScript regex, line by line like grep. Returns matching lines with 1-based line numbers and surrounding lines; `truncated: true` means the scan stopped early at the file limit or the 2 s time cap.";

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
struct Args {
    /// Literal text, or a regex source without slashes when `regex` is true.
    #[schemars(length(min = 1))]
    query: String,
    /// Treat `query` as a JavaScript regex. Default false.
    regex: Option<bool>,
    /// Default false.
    case_sensitive: Option<bool>,
    /// Only notes under this vault-relative folder.
    folder: Option<String>,
    /// Lines of context before and after each match. Default 1.
    #[schemars(range(min = 0, max = 10))]
    context_lines: Option<u64>,
    /// Max files with matches to return. Default 20.
    #[schemars(range(min = 1, max = 200))]
    limit: Option<u64>,
    /// Max matching lines per file. Default 5.
    #[schemars(range(min = 1, max = 100))]
    max_matches_per_file: Option<u64>,
}

#[derive(Debug, Serialize)]
struct LineMatch {
    line: usize,
    text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    before: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    after: Option<Vec<String>>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct FileMatches {
    path: String,
    matches: Vec<LineMatch>,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    more_matches: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
enum StoppedBy {
    Limit,
    Time,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct GrepResult {
    files: Vec<FileMatches>,
    scanned_files: usize,
    total_files: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    truncated: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    stopped_by: Option<StoppedBy>,
}

/// Clip a long line to a window that keeps the unit at `at` visible.
fn clip(line: &str, at: usize) -> String {
    let len = js::utf16_len(line);
    if len <= MAX_LINE_CHARS {
        return line.to_string();
    }
    let start = (at as i64 - (MAX_LINE_CHARS / 4) as i64)
        .min((len - MAX_LINE_CHARS) as i64)
        .max(0) as usize;
    let body = js::utf16_slice(line, start, start + MAX_LINE_CHARS);
    format!(
        "{}{body}{}",
        if start > 0 { "…" } else { "" },
        if start + MAX_LINE_CHARS < len {
            "…"
        } else {
            ""
        }
    )
}

pub fn spec() -> ToolSpec {
    ToolSpec::new::<Args, _, _>("grep_vault", DESCRIPTION, |ctx, args| async move {
        let root = ctx.config.vault_path.clone();
        let origin = Instant::now();
        tokio::task::spawn_blocking(move || {
            grep(&root, &args, || origin.elapsed().as_millis() as u64)
        })
        .await?
    })
}

/// The scan. `now` is the clock in milliseconds, read where the TS read
/// `Date.now()`, so a test can drive the time budget.
fn grep(root: &Path, args: &Args, mut now: impl FnMut() -> u64) -> anyhow::Result<Value> {
    let flags = if args.case_sensitive == Some(true) {
        ""
    } else {
        "i"
    };
    let pattern = compile_user_regex(&args.query, flags, args.regex != Some(true))?;
    let context_lines = args.context_lines.unwrap_or(1) as usize;
    let limit = args.limit.unwrap_or(20) as usize;
    let max_matches = args.max_matches_per_file.unwrap_or(5) as usize;

    let folder = resolve_folder(root, args.folder.as_deref())?;
    let mut paths = collect_markdown_files(root, &folder.rel)?;
    paths.sort_by(|a, b| utf16_cmp(a, b));

    let deadline = now() + GREP_TIME_BUDGET_MS;
    let mut files: Vec<FileMatches> = Vec::new();
    let mut scanned = 0;
    let mut stopped_by = None;

    'scan: for rel in &paths {
        if files.len() >= limit {
            stopped_by = Some(StoppedBy::Limit);
            break;
        }
        if now() > deadline {
            stopped_by = Some(StoppedBy::Time);
            break;
        }
        let Ok(raw) = read_utf8(&root.join(rel)) else {
            continue; // deleted or unreadable since the directory walk
        };
        let lines: Vec<&str> = raw
            .split('\n')
            .map(|l| l.strip_suffix('\r').unwrap_or(l))
            .collect();
        let mut matches = Vec::new();
        let mut more = false;
        for (i, line) in lines.iter().enumerate() {
            if (i & 255) == 255 && now() > deadline {
                if !matches.is_empty() {
                    files.push(FileMatches {
                        path: rel.clone(),
                        matches,
                        more_matches: false,
                    });
                }
                stopped_by = Some(StoppedBy::Time);
                break 'scan;
            }
            let Some(at) = first_match(&pattern, line) else {
                continue;
            };
            if matches.len() >= max_matches {
                more = true;
                break;
            }
            let mut hit = LineMatch {
                line: i + 1,
                text: clip(line, at),
                before: None,
                after: None,
            };
            if context_lines > 0 {
                let from = i.saturating_sub(context_lines);
                hit.before = Some(lines[from..i].iter().map(|l| clip(l, 0)).collect());
                let to = (i + 1 + context_lines).min(lines.len());
                hit.after = Some(lines[i + 1..to].iter().map(|l| clip(l, 0)).collect());
            }
            matches.push(hit);
        }
        scanned += 1;
        if !matches.is_empty() {
            files.push(FileMatches {
                path: rel.clone(),
                matches,
                more_matches: more,
            });
        }
    }

    Ok(serde_json::to_value(GrepResult {
        files,
        scanned_files: scanned,
        total_files: paths.len(),
        truncated: stopped_by.map(|_| true),
        stopped_by,
    })?)
}

/// The UTF-16 index of the first match in `line`, as `exec(line).index`.
/// A match that hits the backtrack limit counts as no match.
fn first_match(pattern: &UserRegex, line: &str) -> Option<usize> {
    let m = pattern.regex.find(line).ok()??;
    Some(js::byte_to_utf16(line, m.start()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn write(root: &Path, files: &[(&str, &str)]) {
        for (rel, content) in files {
            let p = root.join(rel);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, content).unwrap();
        }
    }

    fn vault() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        write(
            dir.path(),
            &[
                (
                    "Alpha.md",
                    "---\nstatus: draft\n---\n# Alpha\nfirst line\nthe Widget is here\nlast line",
                ),
                (
                    "Projects/Beta.md",
                    "widget one\r\nwidget two\r\nwidget three",
                ),
                ("Projects/Deep/Gamma.md", "no match here\n"),
            ],
        );
        dir
    }

    fn args(query: &str) -> Args {
        Args {
            query: query.into(),
            regex: None,
            case_sensitive: None,
            folder: None,
            context_lines: None,
            limit: None,
            max_matches_per_file: None,
        }
    }

    #[test]
    fn stops_at_the_time_budget_and_returns_what_it_has() {
        let dir = vault();
        let mut t = 1_000_000;
        let out = grep(dir.path(), &args("widget"), || {
            t += GREP_TIME_BUDGET_MS / 2 + 1;
            t
        })
        .unwrap();
        assert_eq!(out["truncated"], true);
        assert_eq!(out["stoppedBy"], "time");
        assert!(out["scannedFiles"].as_u64() < out["totalFiles"].as_u64());
    }

    #[test]
    fn keeps_the_matches_of_a_file_cut_off_mid_scan() {
        let dir = vault();
        let lines: Vec<&str> = (0..600)
            .map(|i| if i == 3 { "needle" } else { "filler" })
            .collect();
        write(dir.path(), &[("AAA.md", &lines.join("\n"))]);
        let mut calls = 0;
        let out = grep(dir.path(), &args("needle"), || {
            calls += 1;
            if calls > 3 { 1_000_000_000_000 } else { 0 }
        })
        .unwrap();
        assert_eq!(out["stoppedBy"], "time");
        assert_eq!(
            out["files"],
            json!([{ "path": "AAA.md", "matches": [
                { "line": 4, "text": "needle", "before": ["filler"], "after": ["filler"] }
            ] }])
        );
        assert_eq!(out["scannedFiles"], 0);
    }

    #[test]
    fn clip_counts_utf16_units() {
        // Each `😀` is two units: 300 of them are 600 units, so the line is clipped.
        let line = format!("{}NEEDLE", "😀".repeat(300));
        let at = 600;
        let out = clip(&line, at);
        assert!(out.starts_with('…'));
        assert!(out.ends_with("NEEDLE"));
        assert_eq!(js::utf16_len(&out), 401);
        assert_eq!(
            first_match(&compile_user_regex("needle", "i", true).unwrap(), &line),
            Some(600)
        );
    }
}
