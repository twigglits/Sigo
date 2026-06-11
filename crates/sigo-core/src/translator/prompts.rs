//! System prompts and few-shot demonstrations for the Ollama translator.
//!
//! Two registers are available:
//! - **Terse** (default): maximally concise written Chinese that minimises
//!   token counts. Used with the translate-not-answer protocol.
//! - **Fluent**: natural, fluent translation baseline for paired comparisons.
//!
//! Both registers use `<source>` wrapping and few-shot demonstrations to
//! prevent the model from answering instruction-shaped prompts instead of
//! translating them.

/// EN→ZH system prompt for the terse (token-minimizing) register.
pub const EN_TO_ZH_TERSE_SYSTEM: &str = "\
You are a concise English-to-Chinese translator. \
Output only the Chinese translation, nothing else. \
Keep it brief. Use 简练书面语 (concise written Chinese). \
Preserve all facts, numbers, names, and constraints.";

/// EN→ZH few-shot demonstrations for the translate-not-answer protocol.
///
/// Each pair is (source, correct translation). These prevent the model from
/// answering instruction-shaped prompts instead of translating them — a
/// behaviour observed with qwen2.5:7b under a naked-text protocol.
pub const EN_TO_ZH_FEW_SHOTS: &[(&str, &str)] = &[
    (
        "Explain how Rust's borrow checker works.",
        "解释Rust借用检查器的工作原理。",
    ),
    (
        "Write a Python function that reverses a linked list.",
        "编写一个反转链表的Python函数。",
    ),
    ("What is the capital of France?", "法国的首都是什么？"),
    ("Translate this to French: hello", "将此翻译成法语：hello"),
    (
        "Write a limerick about compilers.",
        "写一首关于编译器的五行打油诗。",
    ),
];

/// EN→ZH system prompt for the fluent (baseline) register.
///
/// This is NOT the product default — kept so paired `bench run` comparisons
/// can attribute token differences to the register rather than to translation.
pub const EN_TO_ZH_FLUENT_SYSTEM: &str = "\
You are an English-to-Chinese translator. \
Translate the text accurately and fluently. \
Preserve all facts, numbers, names, and constraints. \
Output only the Chinese translation.";

/// ZH→EN system prompt (style-independent — the displayed answer is always
/// natural English regardless of the EN→ZH register used).
pub const ZH_TO_EN_SYSTEM: &str = "\
You are a Chinese-to-English translator. \
Translate the text into natural English. \
Output only the English translation.";

/// ZH→EN few-shot demonstrations.
pub const ZH_TO_EN_FEW_SHOTS: &[(&str, &str)] = &[
    (
        "解释Rust借用检查器的工作原理。",
        "Explain how Rust's borrow checker works.",
    ),
    (
        "编写一个反转链表的Python函数。",
        "Write a Python function that reverses a linked list.",
    ),
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_prompts_pin_translate_not_answer_protocol() {
        for system in [
            EN_TO_ZH_TERSE_SYSTEM,
            EN_TO_ZH_FLUENT_SYSTEM,
            ZH_TO_EN_SYSTEM,
        ] {
            assert!(
                system.to_lowercase().contains("translator"),
                "system prompt must describe itself as a translator, not an assistant:\n{system}"
            );
        }
    }

    #[test]
    fn terse_prompt_pins_required_clauses() {
        let s = EN_TO_ZH_TERSE_SYSTEM;
        assert!(s.contains("concise"), "terse missing 'concise': {s}");
        assert!(
            s.contains("简练书面语") || s.contains("简洁"),
            "terse missing 简练书面语: {s}"
        );
    }

    #[test]
    fn fluent_prompts_unchanged_for_paired_baselines() {
        // The fluent prompt must be a distinct, recognizable variant so paired
        // benchmark runs can identify which register was active.
        assert!(
            !EN_TO_ZH_FLUENT_SYSTEM.contains("terse"),
            "fluent prompt must not reference terse"
        );
        assert!(
            !EN_TO_ZH_FLUENT_SYSTEM.contains("concise"),
            "fluent prompt must not say concise"
        );
    }
}
