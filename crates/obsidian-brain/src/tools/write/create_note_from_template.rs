//! `create_note_from_template`: render an Obsidian core Templates template
//! and create the note from it, indexed and with its stub migrated as
//! `create_note` does.
//!
//! The rendered template is written as it is, with `title` set in its
//! frontmatter by line, so the template's own quoting, comments and key
//! order survive. The TypeScript server re-serialised the whole block.

use std::collections::HashMap;
use std::sync::LazyLock;

use anyhow::{Result, anyhow, bail};
use chrono::{DateTime, Local, NaiveDate, NaiveDateTime, TimeZone};
use regex::Regex;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Map, Value};

use super::basename_md;
use crate::mcp::tool::ToolSpec;
use crate::store::nodes::{migrate_stub_to_real, upsert_node};
use crate::tools::support::run_background_reindex;
use crate::types::ParsedNode;
use crate::util::errors::node_fs_error_message;
use crate::util::js;
use crate::vault::frontmatter::{self, FrontmatterOp, edit_frontmatter};
use crate::vault::parser::node_title;
use crate::vault::templates::{
    BUILTIN_VARIABLES, RenderOptions, find_template, read_templates_config, render_template,
};
use crate::vault::vault_path::{read_utf8, resolve_vault_path};

const DESCRIPTION: &str = "Create a note from an Obsidian core Templates template, filling `{{title}}`, `{{date}}`, `{{time}}`, `{{date:FORMAT}}`, `{{time:FORMAT}}` and any `variables`. Templater `<% %>` code is not run and stays as written. Fails if the note exists.";

const TEMPLATER_NOTE: &str =
    "Templater syntax (<% %>) was left as written; this server does not run Templater.";

static TEMPLATER_SYNTAX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"<%[\s\S]*?%>").expect("valid pattern"));
static DATE_ONLY: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^(\d{4})-(\d{2})-(\d{2})$").expect("valid pattern"));

#[derive(Debug, Deserialize, JsonSchema)]
struct Args {
    /// Template name in the templates folder, or a vault-relative path. `.md` is optional.
    #[schemars(length(min = 1))]
    template: String,
    /// Note title. Used as the filename base and for `{{title}}`.
    #[schemars(length(min = 1))]
    title: String,
    /// Vault-relative subdirectory to create the note in.
    directory: Option<String>,
    /// Extra `{{key}}` substitutions.
    #[schemars(with = "Option<HashMap<String, String>>")]
    variables: Option<Map<String, Value>>,
    /// ISO date or date-time to use instead of now.
    date: Option<String>,
}

pub fn spec() -> ToolSpec {
    ToolSpec::new::<Args, _, _>(
        "create_note_from_template",
        DESCRIPTION,
        |ctx, args| async move {
            // `Object.keys` order: index-like keys first.
            let mut variables = args.variables.unwrap_or_default();
            js::js_object_order(&mut variables);
            // The schema made every value a string.
            let vars: HashMap<String, String> = variables
                .iter()
                .filter_map(|(k, v)| Some((k.clone(), v.as_str()?.to_string())))
                .collect();

            let title = args.title;
            let directory = args
                .directory
                .as_deref()
                .map(|d| d.trim_matches('/').to_string())
                .filter(|d| !d.is_empty());

            if title.contains(['/', '\\']) {
                bail!("Title cannot contain \"/\" or \"\\\"; use `directory` for folders");
            }
            let reserved: Vec<&str> = variables
                .keys()
                .filter(|k| BUILTIN_VARIABLES.contains(&k.to_lowercase().as_str()))
                .map(String::as_str)
                .collect();
            if !reserved.is_empty() {
                bail!(
                    "`variables` cannot set {}; these are filled from the arguments",
                    reserved.join(", ")
                );
            }
            let now = parse_date(args.date.as_deref())?;

            let vault = ctx.config.vault_path.clone();
            let target = match &directory {
                Some(dir) => format!("{dir}/{title}.md"),
                None => format!("{title}.md"),
            };
            if resolve_vault_path(&vault, &target)?.abs.exists() {
                bail!("File already exists: {target}");
            }

            let config = read_templates_config(&vault);
            let template_path = find_template(&vault, &args.template, &config)?;
            let template_abs = resolve_vault_path(&vault, &template_path)?.abs;
            let raw = read_utf8(&template_abs)
                .map_err(|e| anyhow!(node_fs_error_message(&e, "open", &template_abs, None)))?;
            let rendered = render_template(
                &raw,
                &RenderOptions {
                    title: &title,
                    now,
                    config: &config,
                    variables: Some(&vars),
                },
            );

            let parsed = frontmatter::parse(&rendered.content).map_err(|e| {
                anyhow!("Template {template_path} has invalid YAML frontmatter: {e}")
            })?;
            // As `create_note`: inject `title` unless the template sets one; a
            // `title: null` opts out and is dropped.
            let ops = match parsed.data.get("title") {
                None => vec![FrontmatterOp::Set(
                    "title".into(),
                    Value::String(title.clone()),
                )],
                Some(Value::Null) => vec![FrontmatterOp::Remove("title".into())],
                Some(_) => Vec::new(),
            };
            let mut text = edit_frontmatter(&rendered.content, &ops)
                .map_err(|e| anyhow!("Template {template_path} has invalid YAML frontmatter: {e}"))?
                .text;
            // `matter.stringify` ends the note with a newline.
            if !text.ends_with('\n') {
                text.push('\n');
            }

            let job = ctx.clone();
            let path = ctx
                .db
                .run(move |conn| -> Result<String> {
                    let path = write_new_note(&job.config.vault_path, conn, &target, &text)?;
                    let stem = basename_md(&path);
                    if !stem.is_empty() {
                        migrate_stub_to_real(conn, &format!("_stub/{stem}.md"), &path)?;
                    }
                    Ok(path)
                })
                .await?;

            run_background_reindex(&ctx);

            let mut out = Map::new();
            out.insert("path".into(), path.into());
            out.insert("title".into(), title.into());
            out.insert("template".into(), template_path.into());
            if !rendered.unresolved.is_empty() {
                out.insert("unresolved".into(), rendered.unresolved.into());
            }
            if TEMPLATER_SYNTAX.is_match(&raw) {
                out.insert("templater".into(), TEMPLATER_NOTE.into());
            }
            Ok(Value::Object(out))
        },
    )
}

/// Write `text` to the new note `target` and index it, as
/// `VaultWriter::create_node` does for a note it renders itself.
fn write_new_note(
    vault: &std::path::Path,
    conn: &rusqlite::Connection,
    target: &str,
    text: &str,
) -> Result<String> {
    let resolved = resolve_vault_path(vault, target)?;
    if let Some(parent) = resolved.abs.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| anyhow!(node_fs_error_message(&e, "mkdir", parent, None)))?;
    }
    if resolved.abs.exists() {
        bail!("File already exists: {}", resolved.rel);
    }
    std::fs::write(&resolved.abs, text)
        .map_err(|e| anyhow!(node_fs_error_message(&e, "open", &resolved.abs, None)))?;

    let (fm, content) = match frontmatter::parse(text) {
        Ok(p) => (p.data, p.content.to_string()),
        Err(_) => (Map::new(), text.to_string()),
    };
    upsert_node(
        conn,
        &ParsedNode {
            id: resolved.rel.clone(),
            title: node_title(&fm, &resolved.rel),
            content,
            frontmatter: fm,
        },
    )?;
    Ok(resolved.rel)
}

/// The local wall-clock time to render with. A date-only ISO string means
/// local midnight, not UTC midnight.
fn parse_date(value: Option<&str>) -> Result<NaiveDateTime> {
    let Some(value) = value else {
        return Ok(Local::now().naive_local());
    };
    let parsed = match DATE_ONLY.captures(value) {
        Some(c) => local_date(c[1].parse().ok(), c[2].parse().ok(), c[3].parse().ok()),
        None => parse_js_date(value),
    };
    parsed.ok_or_else(|| anyhow!("Invalid date: {value}"))
}

/// `new Date(y, m - 1, d)`: months and days past their end roll over, and
/// a year below 100 means 19xx.
fn local_date(year: Option<i32>, month: Option<i64>, day: Option<i64>) -> Option<NaiveDateTime> {
    let (mut year, month, day) = (year?, month?, day?);
    if (0..=99).contains(&year) {
        year += 1900;
    }
    let months = month - 1;
    let y = year + months.div_euclid(12) as i32;
    let m = months.rem_euclid(12) as u32 + 1;
    let first = NaiveDate::from_ymd_opt(y, m, 1)?;
    let date = first.checked_add_signed(chrono::Duration::days(day - 1))?;
    date.and_hms_opt(0, 0, 0)
}

/// The ISO forms `new Date(value)` reads: a date-time without an offset
/// is local time; with `Z` or an offset it is converted to local time.
/// `YYYY` and `YYYY-MM` alone are UTC. RFC 2822 dates are read as well.
fn parse_js_date(value: &str) -> Option<NaiveDateTime> {
    let to_local = |dt: DateTime<chrono::FixedOffset>| dt.with_timezone(&Local).naive_local();
    if let Ok(dt) = DateTime::parse_from_rfc3339(value) {
        return Some(to_local(dt));
    }
    for fmt in ["%Y-%m-%dT%H:%M%#z", "%Y-%m-%dT%H:%M:%S%.f%#z"] {
        if let Ok(dt) = DateTime::parse_from_str(&value.replace('Z', "+00:00"), fmt) {
            return Some(to_local(dt));
        }
    }
    for fmt in [
        "%Y-%m-%dT%H:%M:%S%.f",
        "%Y-%m-%dT%H:%M:%S",
        "%Y-%m-%dT%H:%M",
    ] {
        if let Ok(naive) = NaiveDateTime::parse_from_str(value, fmt) {
            return Local
                .from_local_datetime(&naive)
                .earliest()
                .map(|dt| dt.naive_local());
        }
    }
    let utc_start = |y: i32, m: u32| {
        let naive = NaiveDate::from_ymd_opt(y, m, 1)?.and_hms_opt(0, 0, 0)?;
        Some(
            chrono::Utc
                .from_utc_datetime(&naive)
                .with_timezone(&Local)
                .naive_local(),
        )
    };
    if value.len() == 4 && value.bytes().all(|b| b.is_ascii_digit()) {
        return utc_start(value.parse().ok()?, 1);
    }
    if value.len() == 7
        && value.as_bytes()[4] == b'-'
        && let (Ok(y), Ok(m)) = (value[..4].parse(), value[5..].parse())
    {
        return utc_start(y, m);
    }
    DateTime::parse_from_rfc2822(value).ok().map(to_local)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn date_only_is_local_midnight_and_rolls_over() {
        let d = |s: &str| parse_date(Some(s)).unwrap().to_string();
        assert_eq!(d("2026-03-05"), "2026-03-05 00:00:00");
        assert_eq!(d("2026-13-01"), "2027-01-01 00:00:00");
        assert_eq!(d("2026-02-30"), "2026-03-02 00:00:00");
        assert_eq!(d("2026-03-05T09:07:03"), "2026-03-05 09:07:03");
        assert_eq!(d("2026-03-05T09:07"), "2026-03-05 09:07:00");
    }

    #[test]
    fn an_unreadable_date_is_an_error() {
        let err = parse_date(Some("soon")).unwrap_err().to_string();
        assert_eq!(err, "Invalid date: soon");
    }
}
