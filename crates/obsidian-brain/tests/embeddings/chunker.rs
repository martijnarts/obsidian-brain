//! Tests of the markdown chunker.

use obsidian_brain::embeddings::chunker::{
    ChunkerConfig, DEFAULT_CHUNKER_CONFIG, build_chunk_embedding_text, chunk_id, chunk_markdown,
    chunker_config_from_budget,
};

const SMALL: ChunkerConfig = ChunkerConfig {
    chunk_size: 120,
    min_chunk_chars: 1,
    ..DEFAULT_CHUNKER_CONFIG
};

fn utf16_len(s: &str) -> usize {
    s.encode_utf16().count()
}

#[test]
fn empty_input_yields_nothing() {
    assert!(chunk_markdown("", &DEFAULT_CHUNKER_CONFIG).is_empty());
}

#[test]
fn whitespace_only_input_yields_nothing() {
    assert!(chunk_markdown("   \n\n  ", &DEFAULT_CHUNKER_CONFIG).is_empty());
}

#[test]
fn strips_yaml_frontmatter() {
    let md = [
        "---",
        "title: Foo",
        "tags: [a, b]",
        "---",
        "",
        "# Real Heading",
        "",
        &format!("Some body text. {}", "Padding ".repeat(20)),
    ]
    .join("\n");
    let chunks = chunk_markdown(&md, &DEFAULT_CHUNKER_CONFIG);
    assert!(!chunks.is_empty());
    for c in &chunks {
        assert!(!c.content.contains("title: Foo"));
        assert!(!c.content.contains("tags:"));
    }
}

#[test]
fn splits_on_headings() {
    let filler = "Some padding text to clear the minimum length threshold.";
    let md = [
        "# Alpha".to_string(),
        format!("First section body. {filler}"),
        String::new(),
        "# Beta".into(),
        format!("Second section body. {filler}"),
        String::new(),
        "# Gamma".into(),
        format!("Third section body. {filler}"),
    ]
    .join("\n");
    let headings: Vec<Option<String>> = chunk_markdown(&md, &DEFAULT_CHUNKER_CONFIG)
        .into_iter()
        .map(|c| c.heading)
        .collect();
    for h in ["Alpha", "Beta", "Gamma"] {
        assert!(headings.contains(&Some(h.to_string())));
    }
}

#[test]
fn headings_below_the_split_depth_stay_in_the_parent() {
    let filler = "padding padding padding padding padding padding padding";
    let md = format!("# H1\nbody one {filler}\n\n##### H5 should not split\nbody two {filler}");
    let chunks = chunk_markdown(
        &md,
        &ChunkerConfig {
            heading_split_depth: 4,
            ..DEFAULT_CHUNKER_CONFIG
        },
    );
    assert_eq!(chunks.len(), 1);
    assert_eq!(chunks[0].heading.as_deref(), Some("H1"));
}

#[test]
fn splits_an_oversized_section_on_paragraphs() {
    let big = (0..8)
        .map(|i| format!("Paragraph {i} {}", "x".repeat(40)))
        .collect::<Vec<_>>()
        .join("\n\n");
    let chunks = chunk_markdown(&format!("# Big\n\n{big}"), &SMALL);
    assert!(chunks.len() > 1);
    for c in &chunks {
        assert!(utf16_len(&c.content) <= SMALL.chunk_size + 20);
    }
}

#[test]
fn keeps_code_blocks_whole() {
    let fence = format!("```js\n{}```", "console.log(\"hi\");\n".repeat(6));
    let md = format!("# With Code\n\nprefix para.\n\n{fence}\n\nsuffix para.");
    let joined = chunk_markdown(&md, &SMALL)
        .into_iter()
        .map(|c| c.content)
        .collect::<Vec<_>>()
        .join("\n\n");
    assert!(joined.contains(&fence));
    assert_eq!(joined.matches("```js").count(), 1);
}

#[test]
fn keeps_latex_blocks_whole() {
    let latex = format!("$$\n{}$$", "\\int_{0}^{1} x^2 dx\n".repeat(6));
    let md = format!("# Math\n\nsome intro.\n\n{latex}\n\nafterwards.");
    let joined = chunk_markdown(&md, &SMALL)
        .into_iter()
        .map(|c| c.content)
        .collect::<Vec<_>>()
        .join("\n\n");
    assert!(joined.contains(&latex));
}

#[test]
fn content_hash_is_stable() {
    let md = "# One\n\nBody content with enough text to clear the minimum chunk length threshold.";
    let a = chunk_markdown(md, &DEFAULT_CHUNKER_CONFIG);
    let b = chunk_markdown(md, &DEFAULT_CHUNKER_CONFIG);
    assert!(!a.is_empty());
    assert_eq!(a[0].content_hash, b[0].content_hash);
}

#[test]
fn content_hash_is_sha256_over_heading_and_content() {
    let md = "# One\n\nBody content with enough text to clear the minimum chunk length threshold.";
    let c = &chunk_markdown(md, &DEFAULT_CHUNKER_CONFIG)[0];
    use sha2::{Digest, Sha256};
    let want = hex::encode(Sha256::digest(format!("One {}", c.content).as_bytes()));
    assert_eq!(c.content_hash, want);
    assert_eq!((c.start_line, c.end_line), (1, 3));
    assert_eq!(c.heading_level, Some(1));
}

#[test]
fn different_content_gives_a_different_hash() {
    let pad = " lots of extra characters here so we clear the minimum length threshold.";
    let a = chunk_markdown(&format!("# A\n\nalpha body{pad}"), &DEFAULT_CHUNKER_CONFIG);
    let b = chunk_markdown(&format!("# A\n\nbeta body{pad}"), &DEFAULT_CHUNKER_CONFIG);
    assert_ne!(a[0].content_hash, b[0].content_hash);
}

#[test]
fn chunk_index_is_sequential_from_zero() {
    let filler = "padding padding padding padding padding padding";
    let md = format!(
        "# One\nbody one {filler}\n\n# Two\nbody two {filler}\n\n# Three\nbody three {filler}"
    );
    let idx: Vec<usize> = chunk_markdown(&md, &DEFAULT_CHUNKER_CONFIG)
        .iter()
        .map(|c| c.chunk_index)
        .collect();
    assert_eq!(idx, vec![0, 1, 2]);
}

#[test]
fn drops_chunks_below_min_chunk_chars() {
    let md = format!("# Big enough\n\n{}\n\n# Too short\n\nhi", "x".repeat(200));
    let headings: Vec<Option<String>> = chunk_markdown(
        &md,
        &ChunkerConfig {
            min_chunk_chars: 100,
            ..DEFAULT_CHUNKER_CONFIG
        },
    )
    .into_iter()
    .map(|c| c.heading)
    .collect();
    assert!(headings.contains(&Some("Big enough".into())));
    assert!(!headings.contains(&Some("Too short".into())));
}

#[test]
fn embedding_text_prepends_the_heading() {
    let chunks = chunk_markdown(
        "# Widget Theory\n\nBody talks about widgets at considerable length with plenty of filler text.",
        &DEFAULT_CHUNKER_CONFIG,
    );
    let text = build_chunk_embedding_text(&chunks[0]);
    assert!(text.starts_with("Widget Theory"));
    assert!(text.contains("Body talks about widgets"));
}

#[test]
fn embedding_text_is_the_content_without_a_heading() {
    let body =
        "Just body text with no heading at all, long enough to clear the minimum length threshold.";
    let chunks = chunk_markdown(body, &DEFAULT_CHUNKER_CONFIG);
    assert_eq!(build_chunk_embedding_text(&chunks[0]), chunks[0].content);
}

#[test]
fn chunk_id_joins_with_double_colon() {
    assert_eq!(chunk_id("Notes/foo.md", 3), "Notes/foo.md::3");
}

#[test]
fn preserve_code_blocks_off_lets_a_fence_split() {
    let fence_body = "abc ".repeat(200);
    let md = format!("Preamble text.\n\n```\n{fence_body}\n```\n");
    let cfg = ChunkerConfig {
        chunk_size: 200,
        min_chunk_chars: 1,
        preserve_code_blocks: false,
        ..DEFAULT_CHUNKER_CONFIG
    };
    let chunks = chunk_markdown(&md, &cfg);
    assert!(chunks.len() > 1);
    assert!(!chunks.iter().any(|c| c.content.contains(fence_body.trim())));
}

#[test]
fn preserve_latex_blocks_off_lets_latex_split() {
    let latex = "\\sum_{i=0}^{N} x_i ".repeat(60);
    let md = format!("Intro paragraph.\n\n$$\n{latex}\n$$\n");
    let cfg = ChunkerConfig {
        chunk_size: 200,
        min_chunk_chars: 1,
        preserve_latex_blocks: false,
        ..DEFAULT_CHUNKER_CONFIG
    };
    let chunks = chunk_markdown(&md, &cfg);
    assert!(chunks.len() > 1);
    assert!(!chunks.iter().any(|c| c.content.contains(latex.trim())));
}

#[test]
fn preserve_code_blocks_on_keeps_the_fence() {
    let fence_body = "abc ".repeat(200);
    let md = format!("Preamble text.\n\n```\n{fence_body}\n```\n");
    let cfg = ChunkerConfig {
        chunk_size: 200,
        min_chunk_chars: 1,
        preserve_code_blocks: true,
        ..DEFAULT_CHUNKER_CONFIG
    };
    let chunks = chunk_markdown(&md, &cfg);
    assert!(chunks.iter().any(|c| c.content.contains(fence_body.trim())));
}

#[test]
fn splits_a_long_paragraph_on_sentences() {
    let sentences = (0..40)
        .map(|i| format!("This is sentence number {i}."))
        .collect::<Vec<_>>()
        .join(" ");
    let cfg = ChunkerConfig {
        chunk_size: 150,
        min_chunk_chars: 1,
        ..DEFAULT_CHUNKER_CONFIG
    };
    let chunks = chunk_markdown(&sentences, &cfg);
    assert!(chunks.len() > 1);
    for c in &chunks {
        let t = c.content.trim();
        assert!(t.ends_with(['.', '!', '?']), "{t:?}");
    }
}

#[test]
fn config_from_budget_sets_only_chunk_size() {
    let cfg = chunker_config_from_budget(3500);
    assert_eq!(
        cfg,
        ChunkerConfig {
            chunk_size: 3500,
            ..DEFAULT_CHUNKER_CONFIG
        }
    );
}

#[test]
fn chunks_respect_the_budget() {
    let cfg = chunker_config_from_budget(300);
    let body = (0..20)
        .map(|i| format!("Sentence number {i} ends here."))
        .collect::<Vec<_>>()
        .join(" ");
    let chunks = chunk_markdown(&format!("# Heading\n\n{body}"), &cfg);
    assert!(!chunks.is_empty());
    for c in &chunks {
        assert!(utf16_len(&c.content) <= 350);
    }
}

#[test]
fn embedding_text_puts_a_blank_line_after_the_heading() {
    let md = "# Getting Started\n\nThis is the intro content for the section. It has enough text to clear the minimum chunk length threshold easily.";
    let chunk = &chunk_markdown(md, &DEFAULT_CHUNKER_CONFIG)[0];
    assert_eq!(chunk.heading.as_deref(), Some("Getting Started"));
    let text = build_chunk_embedding_text(chunk);
    assert!(text.starts_with("Getting Started\n\n"));
    assert!(text.contains("intro content"));
}

#[test]
fn embedding_text_without_heading_has_no_leading_newline() {
    let body = "Preamble with no heading at all, plenty of characters to clear minimum threshold.";
    let chunk = &chunk_markdown(body, &DEFAULT_CHUNKER_CONFIG)[0];
    assert_eq!(chunk.heading, None);
    let text = build_chunk_embedding_text(chunk);
    assert_eq!(text, chunk.content);
    assert!(!text.starts_with('\n'));
}

#[test]
fn a_token_budget_from_the_env_formula_is_honoured() {
    // chunkBudgetChars = floor(floor(0.9 * 200) * 2.5) = 450
    let cfg = chunker_config_from_budget(450);
    let body = "word ".repeat(500);
    for c in chunk_markdown(&body, &cfg) {
        assert!(utf16_len(&c.content) <= cfg.chunk_size + 50);
    }
}

#[test]
fn lengths_count_utf16_units() {
    // Each emoji is two UTF-16 units: 60 of them is 120 units, which fits
    // SMALL exactly; 61 does not and is hard-cut.
    let fits = "😀".repeat(60);
    assert_eq!(chunk_markdown(&fits, &SMALL).len(), 1);
    let over = "😀".repeat(61);
    let chunks = chunk_markdown(&over, &SMALL);
    assert_eq!(chunks.len(), 2);
    assert_eq!(utf16_len(&chunks[0].content), 120);
}

// ---------------------------------------------------------------------------
// Properties, over generated documents. A small xorshift generator stands
// in for fast-check.
// ---------------------------------------------------------------------------

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
    fn text(&mut self, max: u64) -> String {
        const ALPHABET: &[char] = &[
            'a', 'b', 'c', ' ', ' ', '.', '!', '?', '\n', '#', '$', '`', 'é', '日', '😀', '-', '*',
            '\t',
        ];
        let len = self.below(max + 1);
        let s: String = (0..len)
            .map(|_| ALPHABET[self.below(ALPHABET.len() as u64) as usize])
            .collect();
        let mut s = s;
        while s.contains("```") || s.contains("$$") {
            s = s.replace("```", "").replace("$$", "");
        }
        s
    }
    fn paragraph(&mut self) -> String {
        let lines = 1 + self.below(4);
        (0..lines)
            .map(|_| self.text(200))
            .collect::<Vec<_>>()
            .join("\n")
    }
    fn fence(&mut self) -> String {
        format!("```\n{}\n```", self.text(120))
    }
    fn latex(&mut self) -> String {
        format!("$$\n{}\n$$", self.text(80))
    }
    fn heading(&mut self) -> String {
        loop {
            let t = self.text(40).replace('\n', "");
            if !t.trim().is_empty() {
                return format!("{} {}", "#".repeat(1 + self.below(4) as usize), t.trim());
            }
        }
    }
    fn document(&mut self) -> String {
        let parts = 1 + self.below(12);
        (0..parts)
            .map(|_| match self.below(4) {
                0 => self.paragraph(),
                1 => self.fence(),
                2 => self.latex(),
                _ => self.heading(),
            })
            .collect::<Vec<_>>()
            .join("\n\n")
    }
}

const PROP_CFG: ChunkerConfig = ChunkerConfig {
    chunk_size: 250,
    min_chunk_chars: 1,
    ..DEFAULT_CHUNKER_CONFIG
};

#[test]
fn property_chunk_index_is_contiguous() {
    let mut rng = Rng(0x9E3779B97F4A7C15);
    for _ in 0..200 {
        let doc = rng.document();
        for (i, c) in chunk_markdown(&doc, &PROP_CFG).iter().enumerate() {
            assert_eq!(c.chunk_index, i, "{doc:?}");
        }
    }
}

#[test]
fn property_no_sentinel_leaks() {
    let mut rng = Rng(0xD1B54A32D192ED03);
    for _ in 0..200 {
        let doc = rng.document();
        for c in chunk_markdown(&doc, &PROP_CFG) {
            assert!(
                !c.content.contains('\u{E000}') && !c.content.contains('\u{E001}'),
                "{doc:?}"
            );
        }
    }
}

#[test]
fn property_a_fence_lands_whole_in_exactly_one_chunk() {
    let mut rng = Rng(0x2545F4914F6CDD1D);
    for _ in 0..100 {
        let fence = rng.fence();
        let body = rng.paragraph();
        let doc = format!("{body}\n\n{fence}\n\n{body}");
        let matching = chunk_markdown(&doc, &PROP_CFG)
            .into_iter()
            .filter(|c| c.content.contains(&fence))
            .count();
        assert_eq!(matching, 1, "{doc:?}");
    }
}
