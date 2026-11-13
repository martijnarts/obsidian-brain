//! The unified diff of an `edit_note` preview, byte for byte what the npm
//! `diff` package's `createPatch` writes: its line tokenizer, its Myers
//! search with the same tie-breaks, four lines of context, and its
//! `Index:` header.

const CONTEXT: usize = 4;

/// `createPatch(file_name, old, new, old_header, new_header)`.
pub fn create_patch(
    file_name: &str,
    old: &str,
    new: &str,
    old_header: &str,
    new_header: &str,
) -> String {
    let hunks = structured_patch(old, new);
    let name = quote_file_name_if_needed(file_name);
    let mut ret = vec![
        format!("Index: {file_name}"),
        "===================================================================".to_string(),
        format!("--- {name}{}", header(old_header)),
        format!("+++ {name}{}", header(new_header)),
    ];
    for hunk in hunks {
        // A hunk of zero lines names the line before it.
        let old_start = if hunk.old_lines == 0 {
            hunk.old_start - 1
        } else {
            hunk.old_start
        };
        let new_start = if hunk.new_lines == 0 {
            hunk.new_start - 1
        } else {
            hunk.new_start
        };
        ret.push(format!(
            "@@ -{old_start},{} +{new_start},{} @@",
            hunk.old_lines, hunk.new_lines
        ));
        ret.extend(hunk.lines);
    }
    ret.join("\n") + "\n"
}

fn header(h: &str) -> String {
    if h.is_empty() {
        String::new()
    } else {
        format!("\t{h}")
    }
}

struct Hunk {
    old_start: usize,
    old_lines: usize,
    new_start: usize,
    new_lines: usize,
    lines: Vec<String>,
}

#[derive(Debug, Clone, PartialEq)]
struct Change {
    added: bool,
    removed: bool,
    value: String,
}

fn structured_patch(old: &str, new: &str) -> Vec<Hunk> {
    let mut diff: Vec<(Change, Vec<String>)> = diff_lines(old, new)
        .into_iter()
        .map(|c| {
            let lines = split_lines(&c.value);
            (c, lines)
        })
        .collect();
    // An empty last entry makes the cleanup below simpler.
    diff.push((
        Change {
            added: false,
            removed: false,
            value: String::new(),
        },
        Vec::new(),
    ));
    let context_lines =
        |lines: &[String]| -> Vec<String> { lines.iter().map(|l| format!(" {l}")).collect() };

    let mut hunks = Vec::new();
    let (mut old_range_start, mut new_range_start) = (0usize, 0usize);
    let mut cur_range: Vec<String> = Vec::new();
    let (mut old_line, mut new_line) = (1usize, 1usize);
    for i in 0..diff.len() {
        let (current, lines) = &diff[i];
        if current.added || current.removed {
            // Start with the previous context, if any.
            if old_range_start == 0 {
                old_range_start = old_line;
                new_range_start = new_line;
                if i > 0 {
                    let prev = &diff[i - 1].1;
                    cur_range = context_lines(&prev[prev.len().saturating_sub(CONTEXT)..]);
                    old_range_start -= cur_range.len();
                    new_range_start -= cur_range.len();
                }
            }
            let sign = if current.added { '+' } else { '-' };
            cur_range.extend(lines.iter().map(|l| format!("{sign}{l}")));
            if current.added {
                new_line += lines.len();
            } else {
                old_line += lines.len();
            }
        } else {
            if old_range_start != 0 {
                if lines.len() <= CONTEXT * 2 && i + 2 < diff.len() {
                    // Close enough to the next change: join the hunks.
                    cur_range.extend(context_lines(lines));
                } else {
                    let context_size = lines.len().min(CONTEXT);
                    cur_range.extend(context_lines(&lines[..context_size]));
                    hunks.push(Hunk {
                        old_start: old_range_start,
                        old_lines: old_line - old_range_start + context_size,
                        new_start: new_range_start,
                        new_lines: new_line - new_range_start + context_size,
                        lines: std::mem::take(&mut cur_range),
                    });
                    old_range_start = 0;
                    new_range_start = 0;
                }
            }
            old_line += lines.len();
            new_line += lines.len();
        }
    }

    // Drop each line's newline; a line without one gets the marker.
    for hunk in &mut hunks {
        let mut lines = Vec::with_capacity(hunk.lines.len());
        for line in hunk.lines.drain(..) {
            match line.strip_suffix('\n') {
                Some(stripped) => lines.push(stripped.to_string()),
                None => {
                    lines.push(line);
                    lines.push("\\ No newline at end of file".to_string());
                }
            }
        }
        hunk.lines = lines;
    }
    hunks
}

/// Lines with their trailing `\n`, where present.
fn split_lines(text: &str) -> Vec<String> {
    let mut result: Vec<String> = text.split('\n').map(|l| format!("{l}\n")).collect();
    if text.ends_with('\n') {
        result.pop();
    } else if let Some(last) = result.last_mut() {
        last.pop();
    }
    result
}

/// `value.split(/(\n|\r\n)/)` with each separator merged into its line.
fn tokenize(value: &str) -> Vec<&str> {
    let mut lines = Vec::new();
    let mut start = 0;
    let bytes = value.as_bytes();
    for (i, &b) in bytes.iter().enumerate() {
        if b == b'\n' {
            lines.push(&value[start..=i]);
            start = i + 1;
        }
    }
    if start < value.len() {
        lines.push(&value[start..]);
    }
    lines
}

/// One component of the edit script, linked to the one before it.
struct Component {
    count: usize,
    added: bool,
    removed: bool,
    prev: Option<usize>,
}

#[derive(Clone, Copy)]
struct PathState {
    old_pos: isize,
    last: Option<usize>,
}

/// `diffLines(old, new)`: the Myers search of the `diff` package, with its
/// diagonal pruning and its preference for removals over additions.
fn diff_lines(old: &str, new: &str) -> Vec<Change> {
    let old_tokens = tokenize(old);
    let new_tokens = tokenize(new);
    let old_len = old_tokens.len() as isize;
    let new_len = new_tokens.len() as isize;
    let mut arena: Vec<Component> = Vec::new();

    let extract_common = |arena: &mut Vec<Component>, path: &mut PathState, diagonal: isize| {
        let mut old_pos = path.old_pos;
        let mut new_pos = old_pos - diagonal;
        let mut common = 0;
        while new_pos + 1 < new_len
            && old_pos + 1 < old_len
            && old_tokens[(old_pos + 1) as usize] == new_tokens[(new_pos + 1) as usize]
        {
            new_pos += 1;
            old_pos += 1;
            common += 1;
        }
        if common > 0 {
            arena.push(Component {
                count: common,
                added: false,
                removed: false,
                prev: path.last,
            });
            path.last = Some(arena.len() - 1);
        }
        path.old_pos = old_pos;
        new_pos
    };
    let add_to_path =
        |arena: &mut Vec<Component>, path: PathState, added: bool, removed: bool, inc: isize| {
            let comp = match path.last.map(|i| &arena[i]) {
                Some(last) if last.added == added && last.removed == removed => Component {
                    count: last.count + 1,
                    added,
                    removed,
                    prev: last.prev,
                },
                _ => Component {
                    count: 1,
                    added,
                    removed,
                    prev: path.last,
                },
            };
            arena.push(comp);
            PathState {
                old_pos: path.old_pos + inc,
                last: Some(arena.len() - 1),
            }
        };

    let offset = old_len + new_len + 1;
    let mut best: Vec<Option<PathState>> = vec![None; (2 * offset + 1) as usize];
    let slot = |d: isize| (d + offset) as usize;

    let mut seed = PathState {
        old_pos: -1,
        last: None,
    };
    let new_pos = extract_common(&mut arena, &mut seed, 0);
    if seed.old_pos + 1 >= old_len && new_pos + 1 >= new_len {
        return build_values(&arena, seed.last, &new_tokens, &old_tokens);
    }
    best[slot(0)] = Some(seed);

    let mut min_diagonal = isize::MIN;
    let mut max_diagonal = isize::MAX;
    let max_edit_length = new_len + old_len;
    let mut edit_length: isize = 1;
    while edit_length <= max_edit_length {
        let mut diagonal = min_diagonal.max(-edit_length);
        while diagonal <= max_diagonal.min(edit_length) {
            let remove_path = best[slot(diagonal - 1)];
            let add_path = best[slot(diagonal + 1)];
            if remove_path.is_some() {
                best[slot(diagonal - 1)] = None;
            }
            let can_add = add_path.is_some_and(|p| {
                let add_new_pos = p.old_pos - diagonal;
                0 <= add_new_pos && add_new_pos < new_len
            });
            let can_remove = remove_path.is_some_and(|p| p.old_pos + 1 < old_len);
            if !can_add && !can_remove {
                best[slot(diagonal)] = None;
                diagonal += 2;
                continue;
            }
            // Branch from the path that got furthest into the old text.
            let take_add = match (remove_path, add_path) {
                (Some(r), Some(a)) => !can_remove || (can_add && r.old_pos < a.old_pos),
                _ => !can_remove,
            };
            let mut base = match (take_add, remove_path, add_path) {
                (true, _, Some(a)) => add_to_path(&mut arena, a, true, false, 0),
                (false, Some(r), _) => add_to_path(&mut arena, r, false, true, 1),
                _ => unreachable!("the chosen path exists"),
            };
            let new_pos = extract_common(&mut arena, &mut base, diagonal);
            if base.old_pos + 1 >= old_len && new_pos + 1 >= new_len {
                return build_values(&arena, base.last, &new_tokens, &old_tokens);
            }
            best[slot(diagonal)] = Some(base);
            if base.old_pos + 1 >= old_len {
                max_diagonal = max_diagonal.min(diagonal - 1);
            }
            if new_pos + 1 >= new_len {
                min_diagonal = min_diagonal.max(diagonal + 1);
            }
            diagonal += 2;
        }
        edit_length += 1;
    }
    unreachable!("the edit script never exceeds old_len + new_len")
}

fn build_values(
    arena: &[Component],
    last: Option<usize>,
    new_tokens: &[&str],
    old_tokens: &[&str],
) -> Vec<Change> {
    let mut order = Vec::new();
    let mut next = last;
    while let Some(i) = next {
        order.push(i);
        next = arena[i].prev;
    }
    order.reverse();
    let (mut new_pos, mut old_pos) = (0, 0);
    order
        .into_iter()
        .map(|i| {
            let c = &arena[i];
            let value = if !c.removed {
                let v = new_tokens[new_pos..new_pos + c.count].concat();
                new_pos += c.count;
                if !c.added {
                    old_pos += c.count;
                }
                v
            } else {
                let v = old_tokens[old_pos..old_pos + c.count].concat();
                old_pos += c.count;
                v
            };
            Change {
                added: c.added,
                removed: c.removed,
                value,
            }
        })
        .collect()
}

/// Git's C-style quoting, for a name with a control character, a
/// non-ASCII character, `"` or `\`.
fn quote_file_name_if_needed(s: &str) -> String {
    let needs = s
        .encode_utf16()
        .any(|u| !(0x20..=0x7e).contains(&u) || u == u16::from(b'"') || u == u16::from(b'\\'));
    if !needs {
        return s.to_string();
    }
    let mut out = String::from("\"");
    for b in s.bytes() {
        match b {
            0x07 => out.push_str("\\a"),
            0x08 => out.push_str("\\b"),
            0x09 => out.push_str("\\t"),
            0x0a => out.push_str("\\n"),
            0x0b => out.push_str("\\v"),
            0x0c => out.push_str("\\f"),
            0x0d => out.push_str("\\r"),
            b'"' => out.push_str("\\\""),
            b'\\' => out.push_str("\\\\"),
            0x20..=0x7e => out.push(b as char),
            _ => out.push_str(&format!("\\{b:03o}")),
        }
    }
    out.push('"');
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    /// Cases captured from `createPatch` of the npm `diff` package 9.0.0.
    #[test]
    fn matches_the_npm_diff_package() {
        let cases: Vec<Value> = serde_json::from_str(include_str!("diff_cases.json")).unwrap();
        assert!(!cases.is_empty());
        for case in cases {
            let got = create_patch(
                case["file"].as_str().unwrap(),
                case["old"].as_str().unwrap(),
                case["new"].as_str().unwrap(),
                "original",
                "proposed",
            );
            assert_eq!(got, case["patch"].as_str().unwrap(), "case {case}");
        }
    }
}
