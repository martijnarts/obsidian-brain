//! jsdiff 9's `createPatch`: a unified diff of two texts, line by line,
//! with four lines of context. The edit script follows jsdiff's Myers
//! variant, so the hunks match the TypeScript server's byte for byte.

/// One run of equal, added or removed tokens, linked to the run before it.
#[derive(Clone, Copy)]
struct Component {
    count: usize,
    added: bool,
    removed: bool,
    prev: Option<usize>,
}

#[derive(Clone, Copy)]
struct DiffPath {
    old_pos: isize,
    last: Option<usize>,
}

struct Change {
    added: bool,
    removed: bool,
    lines: Vec<String>,
}

struct Differ<'a> {
    old: &'a [&'a str],
    new: &'a [&'a str],
    comps: Vec<Component>,
}

impl Differ<'_> {
    fn add_to_path(&mut self, path: DiffPath, added: bool, removed: bool, inc: isize) -> DiffPath {
        let comp = match path.last.map(|i| self.comps[i]) {
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
        self.comps.push(comp);
        DiffPath {
            old_pos: path.old_pos + inc,
            last: Some(self.comps.len() - 1),
        }
    }

    fn extract_common(&mut self, path: &mut DiffPath, diagonal: isize) -> isize {
        let (old_len, new_len) = (self.old.len() as isize, self.new.len() as isize);
        let mut old_pos = path.old_pos;
        let mut new_pos = old_pos - diagonal;
        let mut common = 0;
        while new_pos + 1 < new_len
            && old_pos + 1 < old_len
            && self.old[(old_pos + 1) as usize] == self.new[(new_pos + 1) as usize]
        {
            new_pos += 1;
            old_pos += 1;
            common += 1;
        }
        if common > 0 {
            self.comps.push(Component {
                count: common,
                added: false,
                removed: false,
                prev: path.last,
            });
            path.last = Some(self.comps.len() - 1);
        }
        path.old_pos = old_pos;
        new_pos
    }

    fn build(&self, last: Option<usize>) -> Vec<Change> {
        let mut chain = Vec::new();
        let mut cur = last;
        while let Some(i) = cur {
            chain.push(self.comps[i]);
            cur = self.comps[i].prev;
        }
        chain.reverse();
        let (mut new_pos, mut old_pos) = (0, 0);
        chain
            .into_iter()
            .map(|c| {
                let tokens = if c.removed {
                    let t = &self.old[old_pos..old_pos + c.count];
                    old_pos += c.count;
                    t
                } else {
                    let t = &self.new[new_pos..new_pos + c.count];
                    new_pos += c.count;
                    if !c.added {
                        old_pos += c.count;
                    }
                    t
                };
                Change {
                    added: c.added,
                    removed: c.removed,
                    lines: tokens.iter().map(|t| t.to_string()).collect(),
                }
            })
            .collect()
    }

    /// jsdiff's `diffLines`, without options.
    fn diff(mut self) -> Vec<Change> {
        let (old_len, new_len) = (self.old.len() as isize, self.new.len() as isize);
        let max_edit = old_len + new_len;
        let offset = max_edit + 2;
        let mut best: Vec<Option<DiffPath>> = vec![None; (2 * offset + 1) as usize];
        let at = |d: isize| (d + offset) as usize;

        let mut seed = DiffPath {
            old_pos: -1,
            last: None,
        };
        let new_pos = self.extract_common(&mut seed, 0);
        if seed.old_pos + 1 >= old_len && new_pos + 1 >= new_len {
            return self.build(seed.last);
        }
        best[at(0)] = Some(seed);

        let (mut min_diag, mut max_diag) = (isize::MIN, isize::MAX);
        let mut edit_length: isize = 1;
        while edit_length <= max_edit {
            let mut d = min_diag.max(-edit_length);
            while d <= max_diag.min(edit_length) {
                let remove_path = best[at(d - 1)];
                let add_path = best[at(d + 1)];
                if remove_path.is_some() {
                    best[at(d - 1)] = None;
                }
                let can_add = add_path.is_some_and(|p| {
                    let pos = p.old_pos - d;
                    0 <= pos && pos < new_len
                });
                let can_remove = remove_path.is_some_and(|p| p.old_pos + 1 < old_len);
                if !can_add && !can_remove {
                    best[at(d)] = None;
                    d += 2;
                    continue;
                }
                // Branch from the path that got furthest into the old text.
                let take_add = !can_remove
                    || (can_add
                        && remove_path
                            .zip(add_path)
                            .is_some_and(|(r, a)| r.old_pos < a.old_pos));
                let mut base = if take_add {
                    self.add_to_path(add_path.expect("can_add"), true, false, 0)
                } else {
                    self.add_to_path(remove_path.expect("can_remove"), false, true, 1)
                };
                let new_pos = self.extract_common(&mut base, d);
                if base.old_pos + 1 >= old_len && new_pos + 1 >= new_len {
                    return self.build(base.last);
                }
                best[at(d)] = Some(base);
                if base.old_pos + 1 >= old_len {
                    max_diag = max_diag.min(d - 1);
                }
                if new_pos + 1 >= new_len {
                    min_diag = min_diag.max(d + 1);
                }
                d += 2;
            }
            edit_length += 1;
        }
        unreachable!("an edit script exists within old + new edits")
    }
}

struct Hunk {
    old_start: usize,
    old_lines: usize,
    new_start: usize,
    new_lines: usize,
    lines: Vec<String>,
}

const CONTEXT: usize = 4;

fn hunks(mut diff: Vec<Change>) -> Vec<Hunk> {
    diff.push(Change {
        added: false,
        removed: false,
        lines: Vec::new(),
    });
    let context_lines =
        |lines: &[String]| -> Vec<String> { lines.iter().map(|l| format!(" {l}")).collect() };
    let mut hunks = Vec::new();
    let (mut old_range_start, mut new_range_start) = (0usize, 0usize);
    let mut cur: Vec<String> = Vec::new();
    let (mut old_line, mut new_line) = (1usize, 1usize);
    for i in 0..diff.len() {
        let current = &diff[i];
        let lines = &current.lines;
        if current.added || current.removed {
            if old_range_start == 0 {
                old_range_start = old_line;
                new_range_start = new_line;
                if i > 0 {
                    let prev = &diff[i - 1].lines;
                    cur = context_lines(&prev[prev.len().saturating_sub(CONTEXT)..]);
                    old_range_start -= cur.len();
                    new_range_start -= cur.len();
                }
            }
            let sign = if current.added { '+' } else { '-' };
            cur.extend(lines.iter().map(|l| format!("{sign}{l}")));
            if current.added {
                new_line += lines.len();
            } else {
                old_line += lines.len();
            }
        } else {
            if old_range_start != 0 {
                if lines.len() <= CONTEXT * 2 && i + 2 < diff.len() {
                    cur.extend(context_lines(lines));
                } else {
                    let size = lines.len().min(CONTEXT);
                    cur.extend(context_lines(&lines[..size]));
                    hunks.push(Hunk {
                        old_start: old_range_start,
                        old_lines: old_line - old_range_start + size,
                        new_start: new_range_start,
                        new_lines: new_line - new_range_start + size,
                        lines: std::mem::take(&mut cur),
                    });
                    old_range_start = 0;
                    new_range_start = 0;
                }
            }
            old_line += lines.len();
            new_line += lines.len();
        }
    }
    for hunk in &mut hunks {
        let mut out = Vec::with_capacity(hunk.lines.len());
        for line in hunk.lines.drain(..) {
            match line.strip_suffix('\n') {
                Some(l) => out.push(l.to_string()),
                None => {
                    out.push(line);
                    out.push("\\ No newline at end of file".into());
                }
            }
        }
        hunk.lines = out;
    }
    hunks
}

/// C-style quoting of a file name, as Git and GNU diff print it.
fn quote_file_name(s: &str) -> String {
    let needs = s
        .encode_utf16()
        .any(|u| !(0x20..=0x7e).contains(&u) || u == u16::from(b'"') || u == u16::from(b'\\'));
    if !needs {
        return s.to_string();
    }
    let mut out = String::from("\"");
    for &b in s.as_bytes() {
        match b {
            0x07 => out.push_str("\\a"),
            0x08 => out.push_str("\\b"),
            0x09 => out.push_str("\\t"),
            0x0a => out.push_str("\\n"),
            0x0b => out.push_str("\\v"),
            0x0c => out.push_str("\\f"),
            0x0d => out.push_str("\\r"),
            0x22 => out.push_str("\\\""),
            0x5c => out.push_str("\\\\"),
            0x20..=0x7e => out.push(b as char),
            _ => out.push_str(&format!("\\{b:03o}")),
        }
    }
    out.push('"');
    out
}

/// `createPatch(fileName, oldStr, newStr, oldHeader, newHeader)`.
pub fn create_patch(
    file_name: &str,
    old: &str,
    new: &str,
    old_header: &str,
    new_header: &str,
) -> String {
    let old_tokens: Vec<&str> = old.split_inclusive('\n').collect();
    let new_tokens: Vec<&str> = new.split_inclusive('\n').collect();
    let diff = Differ {
        old: &old_tokens,
        new: &new_tokens,
        comps: Vec::new(),
    }
    .diff();
    // A run's value split into lines keeps each line's `\n`, which is what
    // the tokens already are.
    let mut ret = vec![
        format!("Index: {file_name}"),
        "===================================================================".to_string(),
    ];
    let header = |h: &str| {
        if h.is_empty() {
            String::new()
        } else {
            format!("\t{h}")
        }
    };
    ret.push(format!(
        "--- {}{}",
        quote_file_name(file_name),
        header(old_header)
    ));
    ret.push(format!(
        "+++ {}{}",
        quote_file_name(file_name),
        header(new_header)
    ));
    for hunk in hunks(diff) {
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

#[cfg(test)]
mod tests {
    use super::create_patch;

    #[test]
    fn a_one_line_change_in_the_middle() {
        let old = "a\nb\nc\nd\ne\nf\ng\nh\ni\nj\n";
        let new = "a\nb\nc\nd\ne\nF\ng\nh\ni\nj\n";
        assert_eq!(
            create_patch("n.md", old, new, "original", "proposed"),
            "Index: n.md\n\
             ===================================================================\n\
             --- n.md\toriginal\n\
             +++ n.md\tproposed\n\
             @@ -2,9 +2,9 @@\n b\n c\n d\n e\n-f\n+F\n g\n h\n i\n j\n"
        );
    }

    #[test]
    fn a_last_line_without_a_newline() {
        assert_eq!(
            create_patch("n.md", "x\ny", "x\ny ^id", "original", "proposed"),
            "Index: n.md\n\
             ===================================================================\n\
             --- n.md\toriginal\n\
             +++ n.md\tproposed\n\
             @@ -1,2 +1,2 @@\n x\n-y\n\\ No newline at end of file\n+y ^id\n\\ No newline at end of file\n"
        );
    }

    #[test]
    fn an_insertion_into_an_empty_text_and_two_far_apart_hunks() {
        assert_eq!(
            create_patch("n.md", "", "a\n", "", ""),
            "Index: n.md\n\
             ===================================================================\n\
             --- n.md\n+++ n.md\n@@ -0,0 +1,1 @@\n+a\n"
        );
        let old: String = (1..=20).map(|i| format!("{i}\n")).collect();
        let new: String = (1..=20)
            .map(|i| match i {
                2 => "two\n".to_string(),
                19 => "nineteen\n".to_string(),
                _ => format!("{i}\n"),
            })
            .collect();
        let patch = create_patch("n.md", &old, &new, "o", "p");
        assert!(patch.contains("@@ -1,6 +1,6 @@\n 1\n-2\n+two\n 3\n"));
        assert!(patch.contains("@@ -15,6 +15,6 @@\n 15\n 16\n 17\n 18\n-19\n+nineteen\n 20\n"));
    }

    #[test]
    fn quotes_unusual_file_names() {
        let patch = create_patch("a \"b\"é.md", "x\n", "y\n", "o", "p");
        assert!(patch.starts_with("Index: a \"b\"é.md\n"));
        assert!(patch.contains("--- \"a \\\"b\\\"\\303\\251.md\"\to\n"));
    }
}
