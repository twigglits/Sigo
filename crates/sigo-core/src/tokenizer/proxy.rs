use crate::error::Result;
use super::Tokenizer;
use tiktoken_rs::CoreBPE;

/// English-optimised BPE (GPT-4o `o200k_base`) used as an OFFLINE PROXY for
/// Claude's non-public tokenizer. Reported as a proxy, never as authoritative.
pub struct TokenizerProxy {
    bpe: &'static CoreBPE,
}

impl TokenizerProxy {
    pub fn new() -> Result<Self> {
        // Reuse the library's lazily-built singleton so the ~200k-entry BPE is
        // constructed once per process, not once per instance.
        Ok(Self { bpe: tiktoken_rs::o200k_base_singleton() })
    }

    /// Human-readable label for reports.
    pub fn label() -> &'static str {
        "proxy (o200k_base)"
    }
}

impl Tokenizer for TokenizerProxy {
    fn count_tokens(&self, text: &str) -> Result<u32> {
        Ok(self.bpe.encode_ordinary(text).len() as u32)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_string_counts_zero() {
        let t = TokenizerProxy::new().unwrap();
        assert_eq!(t.count_tokens("").unwrap(), 0);
    }

    #[test]
    fn counts_are_monotonic_in_length() {
        // Guards the wiring, NOT the ZH-vs-EN hypothesis: do not assume a
        // direction between languages here — that is what the benchmark measures.
        let t = TokenizerProxy::new().unwrap();
        let short = t.count_tokens("Hi.").unwrap();
        let long = t.count_tokens("The quick brown fox jumps over the lazy dog.").unwrap();
        assert!(long > short, "short={short} long={long}");
    }

    #[test]
    fn chinese_text_tokenizes_to_a_plausible_count() {
        let t = TokenizerProxy::new().unwrap();
        let n = t.count_tokens("你好，世界！").unwrap();
        assert!(n > 0 && n < 20, "got {n}");
    }
}
