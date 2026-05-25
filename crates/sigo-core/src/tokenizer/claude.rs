use crate::error::{Result, SigoError};
use super::Tokenizer;

pub struct ClaudeTokenizer;

impl ClaudeTokenizer {
    pub fn new() -> Result<Self> {
        // The claude-tokenizer crate handles initialization internally.
        // If it requires an explicit init step, do it here.
        Ok(Self)
    }
}

impl Tokenizer for ClaudeTokenizer {
    fn count_tokens(&self, text: &str) -> Result<u32> {
        // Use the claude-tokenizer crate to tokenize and return the count.
        // Adapt this body to the crate's actual API.
        let tokens = claude_tokenizer::tokenize(text)
            .map_err(|e| SigoError::Tokenizer(format!("{e:?}")))?;
        Ok(tokens.len() as u32)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_string_counts_zero() {
        let t = ClaudeTokenizer::new().unwrap();
        assert_eq!(t.count_tokens("").unwrap(), 0);
    }

    #[test]
    fn english_short_phrase_has_few_tokens() {
        let t = ClaudeTokenizer::new().unwrap();
        let count = t.count_tokens("Hello, world!").unwrap();
        assert!(count > 0 && count < 10, "got {count}");
    }

    #[test]
    fn chinese_short_phrase_has_few_tokens() {
        let t = ClaudeTokenizer::new().unwrap();
        let count = t.count_tokens("你好，世界！").unwrap();
        assert!(count > 0 && count < 20, "got {count}");
    }

    #[test]
    fn longer_text_has_more_tokens_than_shorter() {
        let t = ClaudeTokenizer::new().unwrap();
        let short = t.count_tokens("Hi.").unwrap();
        let long = t.count_tokens("The quick brown fox jumps over the lazy dog.").unwrap();
        assert!(long > short);
    }
}
