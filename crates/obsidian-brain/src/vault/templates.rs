//! Obsidian core Templates, rendered headless: finds a template in the
//! folder `.obsidian/templates.json` names and fills `{{title}}`,
//! `{{date}}`, `{{time}}` and their `:FORMAT` variants the way the core
//! plugin does. Templater (`<% %>`) is a separate plugin and is not run.

use std::collections::HashMap;
use std::path::Path;
use std::sync::LazyLock;

use anyhow::{Result, bail};
use chrono::{Datelike, NaiveDateTime, Timelike};
use regex::{Captures, Regex};
use serde::Serialize;
use serde_json::Value;

use super::vault_path::{resolve_vault_path, to_vault_relative};
use crate::util::js;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TemplatesConfig {
    pub folder: String,
    pub date_format: String,
    pub time_format: String,
}

pub const DEFAULT_TEMPLATES_FOLDER: &str = "Templates";
pub const DEFAULT_DATE_FORMAT: &str = "YYYY-MM-DD";
pub const DEFAULT_TIME_FORMAT: &str = "HH:mm";

/// Names the core plugin fills itself; `variables` may not reuse them.
pub const BUILTIN_VARIABLES: [&str; 3] = ["title", "date", "time"];

static PLACEHOLDER: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\{\{\s*([^{}:]+?)\s*(?::([^{}]*))?\}\}").expect("valid pattern"));
static TOKEN: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\[([^\]]*)]|YYYY|YY|MMMM|MMM|MM|M|Do|DD|D|dddd|ddd|dd|d|HH|H|hh|h|mm|m|ss|s|A|a").expect("valid pattern")
});

/// Settings from `.obsidian/templates.json`, with Obsidian's defaults for anything unset.
pub fn read_templates_config(vault_path: &Path) -> TemplatesConfig {
    // Missing or unreadable settings: the plugin's defaults apply.
    let raw: serde_json::Map<String, Value> = std::fs::read_to_string(vault_path.join(".obsidian").join("templates.json"))
        .ok()
        .and_then(|s| serde_json::from_str::<Value>(&s).ok())
        .and_then(|v| match v {
            Value::Object(m) => Some(m),
            _ => None,
        })
        .unwrap_or_default();
    let str_of = |key: &str| -> Option<String> {
        match raw.get(key) {
            Some(Value::String(s)) if !js::trim(s).is_empty() => Some(s.clone()),
            _ => None,
        }
    };
    let folder = str_of("folder").unwrap_or_else(|| DEFAULT_TEMPLATES_FOLDER.into());
    TemplatesConfig {
        folder: folder.trim_matches('/').to_string(),
        date_format: str_of("dateFormat").unwrap_or_else(|| DEFAULT_DATE_FORMAT.into()),
        time_format: str_of("timeFormat").unwrap_or_else(|| DEFAULT_TIME_FORMAT.into()),
    }
}

/// Find a template by vault-relative path or by name inside the templates
/// folder, with or without `.md`. Returns the vault-relative path.
pub fn find_template(vault_path: &Path, template: &str, config: &TemplatesConfig) -> Result<String> {
    let names: Vec<String> =
        if template.ends_with(".md") { vec![template.into()] } else { vec![template.into(), format!("{template}.md")] };
    let in_folder = names.iter().map(|n| if config.folder.is_empty() { n.clone() } else { format!("{}/{n}", config.folder) });
    let candidates: Vec<String> = in_folder.chain(names.iter().cloned()).collect();
    for candidate in candidates {
        let resolved = resolve_vault_path(vault_path, &candidate)?;
        if resolved.abs.is_file() {
            return Ok(to_vault_relative(vault_path, &resolved.abs));
        }
    }
    let folder = if config.folder.is_empty() { "/" } else { &config.folder };
    bail!("Template not found: \"{template}\" (looked in \"{folder}\" and the vault root)")
}

pub struct RenderOptions<'a> {
    pub title: &'a str,
    /// Local wall-clock time.
    pub now: NaiveDateTime,
    pub config: &'a TemplatesConfig,
    pub variables: Option<&'a HashMap<String, String>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenderResult {
    pub content: String,
    /// `{{name}}` placeholders left in place because nothing filled them.
    pub unresolved: Vec<String>,
}

/// Fill a template's placeholders. Unknown `{{name}}` placeholders stay as written.
pub fn render_template(template: &str, opts: &RenderOptions) -> RenderResult {
    let mut unresolved: Vec<String> = Vec::new();
    let content = PLACEHOLDER.replace_all(template, |c: &Captures| {
        let raw_name = &c[1];
        let format = c.get(2).map(|m| m.as_str());
        let name = raw_name.to_lowercase();
        let pick = |f: Option<&str>, default: &str| -> String {
            match f.map(js::trim) {
                Some(f) if !f.is_empty() => f.to_string(),
                _ => default.to_string(),
            }
        };
        if name == "title" && format.is_none() {
            return opts.title.to_string();
        }
        if name == "date" {
            return format_date(&opts.now, &pick(format, &opts.config.date_format));
        }
        if name == "time" {
            return format_date(&opts.now, &pick(format, &opts.config.time_format));
        }
        if format.is_none()
            && let Some(v) = opts.variables.and_then(|vars| vars.get(raw_name))
        {
            return v.clone();
        }
        let whole = c[0].to_string();
        if !unresolved.contains(&whole) {
            unresolved.push(whole.clone());
        }
        whole
    });
    RenderResult { content: content.into_owned(), unresolved }
}

const MONTHS: [&str; 12] = [
    "January", "February", "March", "April", "May", "June", "July", "August", "September", "October", "November", "December",
];
const WEEKDAYS: [&str; 7] = ["Sunday", "Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday"];

/// A small subset of moment.js formatting, in English: years, months, days
/// of month and week, 12/24-hour clock, minutes, seconds, AM/PM, and
/// `[literal]` text. Other characters pass through. `date` is local time.
pub fn format_date(date: &NaiveDateTime, format: &str) -> String {
    let pad = |n: u32| format!("{n:02}");
    let hours = date.hour();
    let hours12 = if hours % 12 == 0 { 12 } else { hours % 12 };
    let month = date.month0() as usize;
    let weekday = date.weekday().num_days_from_sunday() as usize;
    TOKEN
        .replace_all(format, |c: &Captures| {
            if let Some(literal) = c.get(1) {
                return literal.as_str().to_string();
            }
            match &c[0] {
                "YYYY" => format!("{:04}", date.year()),
                "YY" => pad(date.year().rem_euclid(100) as u32),
                "MMMM" => MONTHS[month].to_string(),
                "MMM" => MONTHS[month][..3].to_string(),
                "MM" => pad(date.month()),
                "M" => date.month().to_string(),
                "Do" => ordinal(date.day()),
                "DD" => pad(date.day()),
                "D" => date.day().to_string(),
                "dddd" => WEEKDAYS[weekday].to_string(),
                "ddd" => WEEKDAYS[weekday][..3].to_string(),
                "dd" => WEEKDAYS[weekday][..2].to_string(),
                "d" => weekday.to_string(),
                "HH" => pad(hours),
                "H" => hours.to_string(),
                "hh" => pad(hours12),
                "h" => hours12.to_string(),
                "mm" => pad(date.minute()),
                "m" => date.minute().to_string(),
                "ss" => pad(date.second()),
                "s" => date.second().to_string(),
                "A" => if hours < 12 { "AM" } else { "PM" }.to_string(),
                _ => if hours < 12 { "am" } else { "pm" }.to_string(),
            }
        })
        .into_owned()
}

fn ordinal(n: u32) -> String {
    let tens = n % 100;
    if (11..=13).contains(&tens) {
        return format!("{n}th");
    }
    let suffix = ["th", "st", "nd", "rd"].get((n % 10) as usize).unwrap_or(&"th");
    format!("{n}{suffix}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::NaiveDate;

    fn at(y: i32, mo: u32, d: u32, h: u32, mi: u32, s: u32) -> NaiveDateTime {
        NaiveDate::from_ymd_opt(y, mo, d).unwrap().and_hms_opt(h, mi, s).unwrap()
    }

    fn fixed() -> NaiveDateTime {
        at(2026, 3, 5, 9, 7, 3)
    }

    #[test]
    fn formats_tokens() {
        for (format, want) in [
            ("YYYY-MM-DD", "2026-03-05"),
            ("YY/M/D", "26/3/5"),
            ("HH:mm:ss", "09:07:03"),
            ("H:m:s", "9:7:3"),
            ("dddd, MMMM Do", "Thursday, March 5th"),
            ("ddd MMM DD", "Thu Mar 05"),
            ("dd d", "Th 4"),
            ("hh:mm A", "09:07 AM"),
            ("h a", "9 am"),
            ("[Week of] YYYY", "Week of 2026"),
            ("YYYY_MM", "2026_03"),
        ] {
            assert_eq!(format_date(&fixed(), format), want, "{format}");
        }
    }

    #[test]
    fn handles_afternoon_midnight_and_ordinal_edge_cases() {
        assert_eq!(format_date(&at(2026, 1, 1, 0, 0, 0), "h A Do"), "12 AM 1st");
        assert_eq!(format_date(&at(2026, 1, 12, 15, 0, 0), "h:mm a Do"), "3:00 pm 12th");
        assert_eq!(format_date(&at(2026, 1, 22, 0, 0, 0), "Do"), "22nd");
        assert_eq!(format_date(&at(2026, 1, 23, 0, 0, 0), "Do"), "23rd");
        assert_eq!(format_date(&at(2026, 1, 13, 0, 0, 0), "Do"), "13th");
    }

    fn config() -> TemplatesConfig {
        TemplatesConfig { folder: "Templates".into(), date_format: "YYYY-MM-DD".into(), time_format: "HH:mm".into() }
    }

    #[test]
    fn fills_built_ins_case_insensitively_and_keeps_unknown_placeholders() {
        let vars = HashMap::from([("who".to_string(), "me".to_string())]);
        let cfg = config();
        let out = render_template(
            "{{Title}} {{ date }} {{TIME}} {{date:YYYY}} {{time:HH}} {{who}} {{title:x}}",
            &RenderOptions { title: "T", now: fixed(), config: &cfg, variables: Some(&vars) },
        );
        assert_eq!(out.content, "T 2026-03-05 09:07 2026 09 me {{title:x}}");
        assert_eq!(out.unresolved, ["{{title:x}}"]);
    }

    #[test]
    fn matches_variables_case_sensitively() {
        let vars = HashMap::from([("who".to_string(), "me".to_string())]);
        let cfg = config();
        let out = render_template("{{Who}}", &RenderOptions { title: "T", now: fixed(), config: &cfg, variables: Some(&vars) });
        assert_eq!(out.content, "{{Who}}");
        assert_eq!(out.unresolved, ["{{Who}}"]);
    }

    #[test]
    fn read_templates_config_defaults_trims_and_ignores_corrupt_files() {
        let vault = tempfile::tempdir().unwrap();
        assert_eq!(
            read_templates_config(vault.path()),
            TemplatesConfig { folder: "Templates".into(), date_format: "YYYY-MM-DD".into(), time_format: "HH:mm".into() }
        );
        std::fs::create_dir(vault.path().join(".obsidian")).unwrap();
        let settings = vault.path().join(".obsidian").join("templates.json");
        std::fs::write(&settings, r#"{"folder":"/Meta/Tpl/","dateFormat":"DD.MM.YYYY","timeFormat":""}"#).unwrap();
        assert_eq!(
            read_templates_config(vault.path()),
            TemplatesConfig { folder: "Meta/Tpl".into(), date_format: "DD.MM.YYYY".into(), time_format: "HH:mm".into() }
        );
        std::fs::write(&settings, "{").unwrap();
        assert_eq!(read_templates_config(vault.path()).folder, "Templates");
    }

    #[test]
    fn finds_templates_in_the_folder_or_the_root() {
        let vault = tempfile::tempdir().unwrap();
        std::fs::create_dir(vault.path().join("Templates")).unwrap();
        std::fs::write(vault.path().join("Templates").join("Daily.md"), "x").unwrap();
        std::fs::write(vault.path().join("Root.md"), "x").unwrap();
        let cfg = config();
        assert_eq!(find_template(vault.path(), "Daily", &cfg).unwrap(), "Templates/Daily.md");
        assert_eq!(find_template(vault.path(), "Root.md", &cfg).unwrap(), "Root.md");
        assert_eq!(
            find_template(vault.path(), "Gone", &cfg).unwrap_err().to_string(),
            "Template not found: \"Gone\" (looked in \"Templates\" and the vault root)"
        );
        assert!(find_template(vault.path(), "../x", &cfg).unwrap_err().to_string().contains("outside the vault"));
    }
}
