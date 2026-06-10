//! Structural code protection for translation: protected spans (fenced blocks,
//! inline code — the same definition `compact` uses) are replaced by sentinel
//! placeholders (`⟦C0⟧`, `⟦C1⟧`, …) before the text is sent to the local
//! model, and the original bytes are reinstated afterwards.
//!
//! Why: prompt-side mitigation has a ceiling. A live sweep showed qwen2.5:7b
//! "translating" a `Rewrite this loop` prompt by SOLVING the loop inside the
//! fenced block, and dropping an inline SQL query entirely — even under a
//! translate-not-answer system prompt with few-shot demonstrations. Masking
//! removes the model from the loop for code: it cannot answer, alter, or drop
//! bytes it never sees.
//!
//! If the input already contains the sentinel pattern (`⟦C`), masking is
//! skipped entirely (collision guard) and the text is translated as-is.
//! If the model loses or duplicates a placeholder, restoration FAILS rather
//! than silently shipping a prompt with missing code — consistent with Sigo's
//! "never degrade quietly" rule.

use crate::compact::{segment, Piece};
use crate::error::{Result, SigoError};

/// Result of masking: the text to translate plus the hidden spans in order.
pub(crate) struct Masked {
    pub text: String,
    pub spans: Vec<String>,
}

fn placeholder(i: usize) -> String {
    format!("⟦C{i}⟧")
}

/// Replace protected spans with sentinels. Returns `None` when there is
/// nothing to mask (no protected spans) or when masking would be ambiguous
/// (the input already contains the sentinel pattern).
pub(crate) fn mask_protected(input: &str) -> Option<Masked> {
    if input.contains("⟦C") {
        return None;
    }
    let pieces = segment(input);
    if !pieces.iter().any(|p| matches!(p, Piece::Protected(_))) {
        return None;
    }
    let mut text = String::with_capacity(input.len());
    let mut spans = Vec::new();
    for piece in pieces {
        match piece {
            Piece::Text(t) => text.push_str(t),
            Piece::Protected(p) => {
                text.push_str(&placeholder(spans.len()));
                spans.push(p.to_string());
            }
        }
    }
    Some(Masked { text, spans })
}

/// Reinstate the original spans into the model's output. Each placeholder must
/// appear exactly once; anything else is a translator failure, not a shrug.
pub(crate) fn restore_protected(output: &str, spans: &[String]) -> Result<String> {
    let mut restored = output.to_string();
    for (i, span) in spans.iter().enumerate() {
        let ph = placeholder(i);
        if restored.matches(&ph).count() != 1 {
            return Err(SigoError::Translator(format!(
                "translation lost or duplicated code placeholder {ph}; refusing to ship a prompt with missing code"
            )));
        }
        restored = restored.replacen(&ph, span, 1);
    }
    Ok(restored)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn masks_fenced_and_inline_spans_in_order() {
        let input = "Fix `a b` here:\n```rust\nfn x() {}\n```\nthen `c`.";
        let m = mask_protected(input).expect("has protected spans");
        assert_eq!(m.spans.len(), 3);
        assert_eq!(m.spans[0], "`a b`");
        assert_eq!(m.spans[1], "```rust\nfn x() {}\n```\n");
        assert_eq!(m.spans[2], "`c`");
        assert!(m.text.contains("⟦C0⟧") && m.text.contains("⟦C1⟧") && m.text.contains("⟦C2⟧"));
        assert!(!m.text.contains("fn x"), "code leaked into masked text");
    }

    #[test]
    fn roundtrip_restores_input_bytes() {
        let input = "Rewrite this loop:\n```python\nresult = []\nfor x in items:\n    result.append(x)\n```\nKeep `snake_case` names.";
        let m = mask_protected(input).unwrap();
        assert_eq!(restore_protected(&m.text, &m.spans).unwrap(), input);
    }

    #[test]
    fn no_protected_spans_means_no_masking() {
        assert!(mask_protected("plain prose, no code at all").is_none());
    }

    #[test]
    fn sentinel_collision_skips_masking() {
        assert!(mask_protected("weird input with ⟦C0⟧ and `code`").is_none());
    }

    #[test]
    fn lost_placeholder_is_an_error_not_a_shrug() {
        let spans = vec!["`SELECT 1`".to_string()];
        let err = restore_protected("查询返回0。首先检查什么？", &spans);
        assert!(err.is_err(), "missing placeholder must fail loudly");
        let dup = restore_protected("⟦C0⟧ 和 ⟦C0⟧", &spans);
        assert!(dup.is_err(), "duplicated placeholder must fail loudly");
    }

    #[test]
    fn unclosed_fence_masks_to_end_of_input() {
        let input = "Look:\n```\nraw to the end";
        let m = mask_protected(input).unwrap();
        assert_eq!(m.spans, vec!["```\nraw to the end".to_string()]);
        assert_eq!(restore_protected(&m.text, &m.spans).unwrap(), input);
    }
}
