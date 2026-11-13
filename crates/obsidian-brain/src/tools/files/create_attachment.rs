//! `create_attachment`: write a binary file from base64. The index holds
//! only notes, so no reindex follows: a link to the new file was already a
//! plain edge or stub, and stays one.

use anyhow::{anyhow, bail};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::json;

use crate::mcp::tool::ToolSpec;
use crate::util::errors::node_fs_error_message;
use crate::util::js;
use crate::vault::vault_path::{resolve_vault_path, write_file_atomic};

const MAX_BYTES: i64 = 10 * 1024 * 1024;

const DESCRIPTION: &str = "Create a binary file (image, PDF, ...) from base64 content, with any missing parent folders. Max 10 MB decoded. Use `create_note` for markdown.";

#[derive(Debug, Deserialize, JsonSchema)]
struct Args {
    /// Vault-relative file path with extension, e.g. `assets/diagram.png`.
    path: String,
    /// File bytes, base64-encoded.
    content: String,
    /// Replace an existing file. Default false.
    overwrite: Option<bool>,
}

pub fn spec() -> ToolSpec {
    ToolSpec::new::<Args, _, _>("create_attachment", DESCRIPTION, |ctx, args| async move {
        let resolved = resolve_vault_path(&ctx.config.vault_path, &args.path)?;
        let (rel, abs) = (resolved.rel, resolved.abs);
        if rel.is_empty() {
            bail!("Path is the vault root: give a file path");
        }
        if rel.to_lowercase().ends_with(".md") {
            bail!("Refusing to write a .md file: use create_note for notes");
        }

        let b64: String = args
            .content
            .chars()
            .filter(|&c| !js::is_js_whitespace(c))
            .collect();
        if !is_base64_shape(&b64) || b64.len() % 4 == 1 {
            bail!("content is not valid base64");
        }
        let padding = if b64.ends_with("==") {
            2
        } else if b64.ends_with('=') {
            1
        } else {
            0
        };
        let decoded_size = (b64.len() as i64 * 3) / 4 - padding;
        if decoded_size > MAX_BYTES {
            bail!(
                "content is {decoded_size} bytes decoded; the limit is {MAX_BYTES} bytes (10 MB)"
            );
        }

        let existing = std::fs::metadata(&abs).ok();
        if existing.as_ref().is_some_and(|st| st.is_dir()) {
            bail!("{rel} is a folder");
        }
        if existing.is_some() && args.overwrite != Some(true) {
            bail!("File already exists: {rel}. Pass overwrite: true to replace it.");
        }

        let bytes = decode_base64(&b64);
        if let Some(parent) = abs.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| anyhow!(node_fs_error_message(&e, "mkdir", parent, None)))?;
        }
        write_file_atomic(&abs, &bytes)
            .map_err(|e| anyhow!(node_fs_error_message(&e, "open", &abs, None)))?;

        Ok(json!({
            "path": rel,
            "bytesWritten": bytes.len(),
            "overwritten": existing.is_some(),
        }))
    })
}

/// `/^[A-Za-z0-9+/]*={0,2}$/`.
fn is_base64_shape(s: &str) -> bool {
    let body = s.trim_end_matches('=');
    s.len() - body.len() <= 2
        && body
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'+' || b == b'/')
}

/// `Buffer.from(s, 'base64')` for a string of the shape above: padding is
/// optional and a trailing partial byte is dropped.
fn decode_base64(s: &str) -> Vec<u8> {
    let mut out = Vec::with_capacity(s.len() * 3 / 4);
    let (mut acc, mut bits) = (0u32, 0u32);
    for b in s.bytes() {
        let v = match b {
            b'A'..=b'Z' => b - b'A',
            b'a'..=b'z' => b - b'a' + 26,
            b'0'..=b'9' => b - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            _ => break,
        };
        acc = (acc << 6) | u32::from(v);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
            acc &= (1 << bits) - 1;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_like_node() {
        assert_eq!(decode_base64("QUJD"), b"ABC");
        assert_eq!(decode_base64("QQ=="), b"A");
        assert_eq!(decode_base64("QQ"), b"A");
        assert_eq!(decode_base64("QUI="), b"AB");
        assert_eq!(decode_base64("QUJDR"), b"ABC");
        assert!(is_base64_shape("QQ=="));
        assert!(!is_base64_shape("QQ==="));
        assert!(!is_base64_shape("ab=c"));
        assert!(!is_base64_shape("not base64!"));
    }
}
