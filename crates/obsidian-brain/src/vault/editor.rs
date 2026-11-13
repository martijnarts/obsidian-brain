//! In-place edits against a single `.md` file inside the vault.
//!
//! Ported from the patch and window-edit logic in aaronsb/obsidian-mcp-plugin,
//! with these simplifications:
//!   - No Obsidian metadata cache; heading lookup is a regex over lines.
//!   - No block-id (`^blockId`) targeting.
//!   - No content-buffer recovery; the caller handles errors.
//!   - Frontmatter edits change only the lines of the keys they touch.

use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use anyhow::{Result, anyhow, bail};
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use super::frontmatter::{self, FrontmatterOp};
use super::fuzzy::fuzzy_find;
use super::vault_path::{read_utf8, write_file_atomic};
use crate::types::Frontmatter;
use crate::util::errors::node_fs_error_message;
use crate::util::js;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum HeadingOp {
    #[default]
    Replace,
    After,
    Before,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum HeadingScope {
    #[default]
    Section,
    Body,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LineOp {
    Before,
    #[default]
    After,
    Replace,
}

/// One edit, shaped like the TypeScript `EditMode` (`{kind: 'append', ...}`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", rename_all_fields = "camelCase")]
pub enum EditMode {
    Append {
        content: String,
    },
    Prepend {
        content: String,
    },
    ReplaceWindow {
        search: String,
        content: String,
        #[serde(default)]
        fuzzy: bool,
        #[serde(default)]
        fuzzy_threshold: Option<f64>,
    },
    PatchHeading {
        heading: String,
        content: String,
        #[serde(default)]
        op: HeadingOp,
        #[serde(default)]
        scope: HeadingScope,
        /// 0-based position among matching headings, top to bottom. Required
        /// when the heading text matches more than once.
        #[serde(default)]
        heading_index: Option<i64>,
    },
    PatchFrontmatter {
        key: String,
        /// `None` or `Value::Null` removes the key.
        #[serde(default)]
        value: Option<Value>,
    },
    AtLine {
        /// 1-based, counted from the start of the file, frontmatter included.
        line: i64,
        content: String,
        #[serde(default)]
        op: LineOp,
    },
}

impl EditMode {
    /// The TypeScript `kind` string.
    pub fn kind(&self) -> &'static str {
        match self {
            EditMode::Append { .. } => "append",
            EditMode::Prepend { .. } => "prepend",
            EditMode::ReplaceWindow { .. } => "replace_window",
            EditMode::PatchHeading { .. } => "patch_heading",
            EditMode::PatchFrontmatter { .. } => "patch_frontmatter",
            EditMode::AtLine { .. } => "at_line",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct HeadingMatch {
    /// The full heading line as it appears in the file.
    pub heading: String,
    /// 1-based line number of the heading.
    pub line: usize,
}

/// `patch_heading` found the heading text more than once and the caller
/// passed no `heading_index`. `matches` lists every occurrence. Callers
/// find it with `err.downcast_ref::<MultipleMatchesError>()`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MultipleMatchesError {
    pub matches: Vec<HeadingMatch>,
}

impl fmt::Display for MultipleMatchesError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let n = self.matches.len();
        let preview = self.matches.iter().map(|m| format!("  [{}] {}", m.line, m.heading)).collect::<Vec<_>>().join("\n");
        write!(f, "[patch_heading] MultipleMatches: {n} headings match — pass headingIndex (0..{}) to disambiguate:\n{preview}", n - 1)
    }
}

impl std::error::Error for MultipleMatchesError {}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Diff {
    pub before: String,
    pub after: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EditResult {
    pub path: PathBuf,
    pub bytes_written: usize,
    /// Bytes of existing content this edit removed before inserting its
    /// content: 0 for insert-only modes (`append`, `prepend`, `at_line`
    /// before/after, `patch_heading` before/after, `patch_frontmatter`). It
    /// lets `patch_heading` callers see when the default `section` scope
    /// consumed more than they expected.
    pub removed_len: usize,
    pub diff: Diff,
}

/// The result of applying one edit to a string.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Apply {
    pub next: String,
    /// Where the edit happened, in UTF-16 code units as in the TS. It is an
    /// offset into both the original and `next`.
    pub at: usize,
    /// The inserted content's length in UTF-16 code units.
    pub len: usize,
    /// Bytes of original text the edit replaced (UTF-8).
    pub removed_len: usize,
}

const CTX: usize = 500;

fn ctx(s: &str, at: usize, len: usize) -> String {
    let half = CTX / 2;
    let total = js::utf16_len(s);
    let from = js::utf16_to_byte(s, at.saturating_sub(half));
    let to = js::utf16_to_byte(s, (at + len + half).min(total));
    s[from..to.max(from)].to_string()
}

fn line_offset(lines: &[String], idx: usize) -> usize {
    lines[..idx].iter().map(|l| js::utf16_len(l) + 1).sum()
}

/// Line endings and trailing whitespace do not count as a difference.
fn normalize_for_compare(text: &str) -> String {
    let unified = text.replace("\r\n", "\n").replace('\r', "\n");
    let lines: Vec<&str> = unified.split('\n').map(js::trim_end).collect();
    js::trim(&lines.join("\n")).to_string()
}

/// Write precondition for an edit that replaces text: `expected` must equal
/// the text the edit removes, so an edit planned from a stale read fails
/// instead of destroying what someone else wrote since. Errors when the
/// text differs, or when the edit replaces nothing.
pub fn check_expected_content(original: &str, res: &Apply, expected: &str) -> Result<()> {
    if res.removed_len == 0 {
        bail!(
            "expectedContent applies only to edits that replace text (replace_window, patch_heading with headingOp replace, at_line with lineOp replace)."
        );
    }
    let start = js::utf16_to_byte(original, res.at);
    let mut end = (start + res.removed_len).min(original.len());
    while !original.is_char_boundary(end) {
        end -= 1;
    }
    let current = &original[start..end];
    if normalize_for_compare(current) != normalize_for_compare(expected) {
        bail!(
            "The text this edit replaces no longer matches expectedContent: the note changed since it was read. Read it again and redo the edit. Nothing was written. Current text:\n{current}"
        );
    }
    Ok(())
}

fn read_note(abs: &Path) -> Result<String> {
    read_utf8(abs).map_err(|e| anyhow!(node_fs_error_message(&e, "open", abs, None)))
}

/// Apply one edit to the note at `vault_path/file_rel_path` and write it
/// atomically. The path is joined as given; callers guard it first.
pub fn edit_note(vault_path: &Path, file_rel_path: &str, mode: &EditMode, expected_content: Option<&str>) -> Result<EditResult> {
    let abs = vault_path.join(file_rel_path);
    let original = read_note(&abs)?;
    let res = apply_edit(&original, mode)?;
    if let Some(expected) = expected_content {
        check_expected_content(&original, &res, expected)?;
    }
    write_file_atomic(&abs, &res.next)?;
    Ok(EditResult {
        bytes_written: res.next.len(),
        removed_len: res.removed_len,
        diff: Diff { before: ctx(&original, res.at, res.len), after: ctx(&res.next, res.at, res.len) },
        path: abs,
    })
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BulkEditResult {
    pub path: PathBuf,
    pub edits_applied: usize,
    pub bytes_written: usize,
    pub before: String,
    pub after: String,
}

/// Apply every edit in order and write once; when one fails, nothing is
/// written.
pub fn bulk_edit_note(vault_path: &Path, file_rel_path: &str, modes: &[EditMode]) -> Result<BulkEditResult> {
    let abs = vault_path.join(file_rel_path);
    let original = read_note(&abs)?;
    let mut current = original.clone();
    for (i, mode) in modes.iter().enumerate() {
        match apply_edit(&current, mode) {
            Ok(res) => current = res.next,
            Err(err) => bail!("[bulk edit] edits[{i}] ({}) failed: {err}. No edits were applied.", mode.kind()),
        }
    }
    if current == original {
        return Ok(BulkEditResult { path: abs, edits_applied: modes.len(), bytes_written: 0, before: original, after: current });
    }
    write_file_atomic(&abs, &current)?;
    Ok(BulkEditResult { path: abs, edits_applied: modes.len(), bytes_written: current.len(), before: original, after: current })
}

pub fn apply_edit(s: &str, mode: &EditMode) -> Result<Apply> {
    match mode {
        EditMode::Append { content } => {
            // Start the content on a new line so the last byte of the file does
            // not run into it.
            let prefix = if !s.is_empty() && !s.ends_with('\n') { "\n" } else { "" };
            Ok(Apply {
                next: format!("{s}{prefix}{content}"),
                at: js::utf16_len(s) + prefix.len(),
                len: js::utf16_len(content),
                removed_len: 0,
            })
        }
        EditMode::Prepend { content } => {
            // After a YAML frontmatter block, so the fence stays intact.
            let insert_at = prepend_offset(s);
            Ok(Apply {
                next: format!("{}{content}{}", &s[..insert_at], &s[insert_at..]),
                at: js::byte_to_utf16(s, insert_at),
                len: js::utf16_len(content),
                removed_len: 0,
            })
        }
        EditMode::ReplaceWindow { search, content, fuzzy, fuzzy_threshold } => {
            replace_window(s, search, content, *fuzzy, *fuzzy_threshold)
        }
        EditMode::PatchHeading { heading, content, op, scope, heading_index } => {
            patch_heading(s, heading, content, *op, *scope, *heading_index)
        }
        EditMode::PatchFrontmatter { key, value } => patch_frontmatter(s, key, value.as_ref()),
        EditMode::AtLine { line, content, op } => at_line(s, *line, content, *op),
    }
}

/// The end of `/^---\r?\n[\s\S]*?\r?\n---\r?\n/`, or 0.
fn prepend_offset(s: &str) -> usize {
    let open = if s.starts_with("---\n") {
        4
    } else if s.starts_with("---\r\n") {
        5
    } else {
        return 0;
    };
    let mut from = open;
    while let Some(i) = s[from..].find("\n---") {
        let at = from + i + 4;
        if s[at..].starts_with('\n') {
            return at + 1;
        }
        if s[at..].starts_with("\r\n") {
            return at + 2;
        }
        from = from + i + 1;
    }
    0
}

fn replace_window(s: &str, search: &str, content: &str, fuzzy: bool, fuzzy_threshold: Option<f64>) -> Result<Apply> {
    if !fuzzy {
        let Some(first) = s.find(search) else {
            bail!("[replace_window] NoMatch: search text not found");
        };
        if s[first + search.len()..].contains(search) {
            bail!("[replace_window] MultipleMatches: exact search matched more than once");
        }
        return Ok(Apply {
            next: format!("{}{content}{}", &s[..first], &s[first + search.len()..]),
            at: js::byte_to_utf16(s, first),
            len: js::utf16_len(content),
            removed_len: search.len(),
        });
    }
    let threshold = fuzzy_threshold.unwrap_or(0.7);
    let matches = fuzzy_find(s, search, threshold);
    if matches.is_empty() {
        bail!("[replace_window] NoMatch: needle not found (fuzzy, threshold={})", js::number_to_string(threshold));
    }
    if matches.len() > 1 {
        let preview = matches
            .iter()
            .take(3)
            .map(|m| {
                let text = js::trim(&m.text);
                let cut = js::utf16_to_byte(text, 40);
                format!("\"{}\" @{}", &text[..cut], js::byte_to_utf16(s, m.start))
            })
            .collect::<Vec<_>>()
            .join(", ");
        bail!("[replace_window] MultipleMatches: {} candidates near {preview}", matches.len());
    }
    let h = &matches[0];
    // When the query has no trailing sentence terminator but the match ends
    // just before one ("foo" matched "foo."), swallow it so a replacement
    // that ends in its own terminator does not double it.
    let mut end = h.end;
    let is_term = |c: u8| c == b'.' || c == b'?' || c == b'!';
    let last_q = js::trim(search).as_bytes().last().copied();
    if !last_q.is_some_and(is_term) && s.as_bytes().get(end).is_some_and(|c| is_term(*c)) {
        end += 1;
    }
    Ok(Apply {
        next: format!("{}{content}{}", &s[..h.start], &s[end..]),
        at: js::byte_to_utf16(s, h.start),
        len: js::utf16_len(content),
        removed_len: s[h.start..end].len(),
    })
}

static HEADING_HASHES: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^(#+)\s+").expect("valid pattern"));

fn patch_heading(
    s: &str,
    heading: &str,
    content: &str,
    op: HeadingOp,
    scope: HeadingScope,
    heading_index: Option<i64>,
) -> Result<Apply> {
    let mut lines: Vec<String> = s.split('\n').map(str::to_string).collect();
    let re = Regex::new(&format!(r"^(#+)\s+{}\s*$", regex::escape(js::trim(heading)))).expect("escaped pattern");

    let hits: Vec<(usize, usize)> =
        lines.iter().enumerate().filter_map(|(i, l)| re.captures(l).map(|m| (i, m[1].len()))).collect();
    if hits.is_empty() {
        bail!("[patch_heading] Heading not found: {heading}");
    }
    let (hit_idx, hit_level) = if hits.len() == 1 {
        hits[0]
    } else {
        match heading_index {
            None => {
                let matches = hits.iter().map(|&(i, _)| HeadingMatch { heading: lines[i].clone(), line: i + 1 }).collect();
                return Err(MultipleMatchesError { matches }.into());
            }
            Some(n) if n < 0 || n as usize >= hits.len() => {
                bail!(
                    "[patch_heading] headingIndex={n} out of range; {} matches found (valid: 0..{})",
                    hits.len(),
                    hits.len() - 1
                );
            }
            Some(n) => hits[n as usize],
        }
    };

    let content_len = js::utf16_len(content);
    match op {
        HeadingOp::Before => {
            lines.insert(hit_idx, content.to_string());
            return Ok(Apply { at: line_offset(&lines, hit_idx), next: lines.join("\n"), len: content_len, removed_len: 0 });
        }
        HeadingOp::After => {
            lines.insert(hit_idx + 1, content.to_string());
            return Ok(Apply { at: line_offset(&lines, hit_idx + 1), next: lines.join("\n"), len: content_len, removed_len: 0 });
        }
        HeadingOp::Replace => {}
    }

    // The end of the region to replace:
    // - `section`: up to the next heading of the same or a higher level, or
    //   the end of the file. On the last heading this takes everything below;
    //   `removed_len` lets callers notice.
    // - `body`: up to the first blank line after content, or the next
    //   same-or-higher heading, whichever comes first. The boundary blank is
    //   swallowed too, so the content's own trailing `\n` is the separator.
    let same_or_higher = |l: &str| HEADING_HASHES.captures(l).is_some_and(|m| m[1].len() <= hit_level);
    let mut end_idx = lines.len();
    if scope == HeadingScope::Body {
        let mut seen_content = false;
        for (j, l) in lines.iter().enumerate().skip(hit_idx + 1) {
            if same_or_higher(l) {
                end_idx = j;
                break;
            }
            let blank = js::trim(l).is_empty();
            if !blank {
                seen_content = true;
            }
            if seen_content && blank {
                end_idx = j;
                break;
            }
        }
        if end_idx < lines.len() && js::trim(&lines[end_idx]).is_empty() {
            end_idx += 1;
        }
    } else if let Some(j) = lines.iter().enumerate().skip(hit_idx + 1).find(|(_, l)| same_or_higher(l)).map(|(j, _)| j) {
        end_idx = j;
    }

    // Keep the blank line right after the heading when there is one:
    // `# H\n\nold\n` becomes `# H\n\nnew\n`. Without one, none is added.
    let mut content_start = hit_idx + 1;
    if content_start < lines.len() && js::trim(&lines[content_start]).is_empty() && content_start < end_idx {
        content_start += 1;
    }

    let removed_len = lines[content_start..end_idx].join("\n").len();
    lines.splice(content_start..end_idx, [content.to_string()]);
    Ok(Apply { at: line_offset(&lines, content_start), next: lines.join("\n"), len: content_len, removed_len })
}

fn patch_frontmatter(s: &str, key: &str, value: Option<&Value>) -> Result<Apply> {
    let op = match value {
        None | Some(Value::Null) => FrontmatterOp::Remove(key.to_string()),
        Some(v) => FrontmatterOp::Set(key.to_string(), v.clone()),
    };
    let next = frontmatter::edit_frontmatter(s, &[op])?.text;
    let at = next.find(&format!("{key}:")).map_or(0, |b| js::byte_to_utf16(&next, b));
    let len = match value {
        None | Some(Value::Null) => 0,
        Some(v) => js::utf16_len(&js::to_js_string(v)),
    };
    Ok(Apply { next, at, len, removed_len: 0 })
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FrontmatterUpdate {
    pub next: String,
    pub frontmatter: Frontmatter,
    pub set: Vec<String>,
    pub removed: Vec<String>,
    pub not_present: Vec<String>,
}

/// Set and remove several frontmatter keys in one write. Removals run
/// first, so `set` wins for a key named in both. A null value in `set`
/// removes the key, as in `patch_frontmatter`. An empty result drops the
/// frontmatter block. Only the lines of the keys that change are touched.
pub fn update_frontmatter(s: &str, set: &Map<String, Value>, remove: &[String]) -> Result<FrontmatterUpdate> {
    let parsed = frontmatter::parse(s)?;
    let mut data = parsed.data.clone();
    let mut ops = Vec::new();
    let mut written = Vec::new();
    let mut removed = Vec::new();
    let mut not_present = Vec::new();
    let mut drop_key = |key: &str, data: &mut Frontmatter, ops: &mut Vec<FrontmatterOp>| {
        if data.shift_remove(key).is_some() {
            removed.push(key.to_string());
            ops.push(FrontmatterOp::Remove(key.to_string()));
        } else {
            not_present.push(key.to_string());
        }
    };
    for key in remove {
        if !set.contains_key(key) {
            drop_key(key, &mut data, &mut ops);
        }
    }
    // `Object.entries(set)` visits index-like keys first.
    let mut ordered = set.clone();
    js::js_object_order(&mut ordered);
    for (key, value) in &ordered {
        if value.is_null() {
            drop_key(key, &mut data, &mut ops);
        } else {
            data.insert(key.clone(), value.clone());
            written.push(key.clone());
            ops.push(FrontmatterOp::Set(key.clone(), value.clone()));
        }
    }
    js::js_object_order(&mut data);
    // Nothing changed: keep the file byte for byte.
    let next = if written.is_empty() && removed.is_empty() {
        s.to_string()
    } else {
        frontmatter::edit_frontmatter(s, &ops)?.text
    };
    Ok(FrontmatterUpdate { next, frontmatter: data, set: written, removed, not_present })
}

fn at_line(s: &str, line: i64, content: &str, op: LineOp) -> Result<Apply> {
    let mut lines: Vec<String> = s.split('\n').map(str::to_string).collect();
    if line < 1 || line as usize > lines.len() + 1 {
        bail!("[at_line] Invalid line {line}; file has {} lines", lines.len());
    }
    let idx = line as usize - 1;
    let len = js::utf16_len(content);
    match op {
        LineOp::Replace => {
            let removed_len = lines.get(idx).map_or(0, String::len);
            if idx < lines.len() {
                lines[idx] = content.to_string();
            } else {
                lines.push(content.to_string());
            }
            Ok(Apply { at: line_offset(&lines, idx), next: lines.join("\n"), len, removed_len })
        }
        LineOp::Before => {
            lines.insert(idx, content.to_string());
            Ok(Apply { at: line_offset(&lines, idx), next: lines.join("\n"), len, removed_len: 0 })
        }
        LineOp::After => {
            // Line `len + 1` inserts past the end, as `splice` clamps.
            let at_idx = (idx + 1).min(lines.len());
            lines.insert(at_idx, content.to_string());
            Ok(Apply { at: line_offset(&lines, (idx + 1).min(lines.len())), next: lines.join("\n"), len, removed_len: 0 })
        }
    }
}

#[cfg(test)]
mod tests;
