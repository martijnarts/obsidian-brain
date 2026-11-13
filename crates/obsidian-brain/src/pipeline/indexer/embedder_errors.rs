//! Sorting embed failures into "skip this chunk" and "the embedder is gone".

use std::sync::LazyLock;

use regex::RegexSet;

use crate::util::js;

/// Errors that mean the chunk is too large for the embedder. The chunk is
/// skipped and the pass goes on, as LangChain, LlamaIndex, FastEmbed and
/// Haystack do.
static TOO_LONG: LazyLock<RegexSet> = LazyLock::new(|| {
    RegexSet::new([
        r"(?i)input length exceeds".to_string(),
        r"(?i)context length".to_string(),
        r"(?i)too many tokens".to_string(),
        r"(?i)maximum context length".to_string(),
        format!(r"(?i)HTTP 400{}*length", js::DOT),
        r"(?i)input_too_long".to_string(),
        r"(?i)Cannot broadcast|shape mismatch".to_string(),
    ])
    .expect("valid patterns")
});

/// Errors that mean the embedder itself is unreachable. The whole pass
/// aborts; retrying chunk by chunk makes no sense when the host is down.
///
/// The bare word "network" is not matched: ONNX Runtime reports a chunk
/// that is too long as "neural network input tensor shape mismatch".
static DEAD_EMBEDDER: LazyLock<RegexSet> = LazyLock::new(|| {
    RegexSet::new([
        r"(?i)ECONNREFUSED".to_string(),
        r"(?i)ENOTFOUND".to_string(),
        r"(?i)ECONNRESET".to_string(),
        r"(?i)getaddrinfo".to_string(),
        r"(?i)network (error|down|unreachable)".to_string(),
        r"(?i)EmbedderLoadError".to_string(),
        format!(r"(?i)kind{}*offline", js::DOT),
    ])
    .expect("valid patterns")
});

pub fn is_too_long_error(msg: &str) -> bool {
    TOO_LONG.is_match(msg)
}

pub fn is_dead_embedder_error(msg: &str) -> bool {
    DEAD_EMBEDDER.is_match(msg)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_messages() {
        assert!(is_too_long_error("HTTP 400: input length too big"));
        assert!(is_too_long_error("Cannot broadcast [1, 512] to [1, 256]"));
        assert!(!is_too_long_error("connect ECONNREFUSED"));
        assert!(is_dead_embedder_error(
            "connect ECONNREFUSED 127.0.0.1:11434"
        ));
        assert!(is_dead_embedder_error("Network unreachable"));
        assert!(!is_dead_embedder_error(
            "neural network input tensor shape mismatch"
        ));
    }
}
