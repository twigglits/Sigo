//! System prompts for the local translator. Two EN→ZH registers exist because
//! the o200k-proxy measurements (and a live qwen2.5:7b A/B) showed faithful
//! *fluent* Chinese costs MORE tokens than the English original (+10% on a
//! realistic prompt), while meaning-preserving *terse* Chinese costs decidedly
//! less (−22%..−51% live). Terse is the product default; fluent is retained so
//! paired benchmark runs can attribute savings to the register, not to
//! translation per se. The ZH→EN prompt stays fluent: it produces the displayed
//! answer and feeds the English control arm, neither of which may be compressed.

/// Token-minimizing register: concise written Chinese with an explicit
/// constraint-recall clause. A unit test pins the contract text; only live
/// tests and the bench can speak to actual model behavior.
pub const EN_TO_ZH_TERSE_SYSTEM: &str = "\
You are a translator from English to Simplified Chinese. \
Rewrite the user's input as maximally concise written Chinese (简练书面语): \
preserve every fact, constraint, number, name, negation, and the full intent; \
drop politeness, filler, and redundant function words; prefer compact constructions. \
Output ONLY the Chinese text, no explanations, no quotes, no preamble. \
Preserve the following EXACTLY as-is without translating them: \
fenced code blocks (```), inline code (single backticks), file paths, URLs, command-line invocations, \
ALL_CAPS identifiers, snake_case_identifiers, camelCaseIdentifiers, and HTML/XML tags.";

pub const EN_TO_ZH_FLUENT_SYSTEM: &str = "\
You are a translator from English to Simplified Chinese. \
Translate the user's input faithfully. Output ONLY the translated text, no explanations, no quotes, no preamble. \
Preserve the following EXACTLY as-is without translating them: \
fenced code blocks (```), inline code (single backticks), file paths, URLs, command-line invocations, \
ALL_CAPS identifiers, snake_case_identifiers, camelCaseIdentifiers, and HTML/XML tags. \
Translate everything else into natural, fluent Simplified Chinese.";

pub const ZH_TO_EN_SYSTEM: &str = "\
You are a translator from Simplified Chinese to English. \
Translate the user's input faithfully. Output ONLY the translated text, no explanations, no quotes, no preamble. \
Preserve the following EXACTLY as-is without translating them: \
fenced code blocks (```), inline code (single backticks), file paths, URLs, command-line invocations, \
ALL_CAPS identifiers, snake_case_identifiers, camelCaseIdentifiers, and HTML/XML tags. \
Translate everything else into natural, fluent English.";

#[cfg(test)]
mod tests {
    use super::*;

    // These are change detectors over contract text: they pin what the prompt
    // ASKS FOR, not what any model does. Behavior claims belong to the
    // feature="live" tests and the bench harness.

    #[test]
    fn terse_prompt_pins_required_clauses() {
        let preserve = [
            "fenced code blocks",
            "inline code",
            "file paths",
            "URLs",
            "command-line invocations",
            "ALL_CAPS identifiers",
            "snake_case_identifiers",
            "camelCaseIdentifiers",
            "HTML/XML tags",
        ];
        for clause in preserve {
            assert!(
                EN_TO_ZH_TERSE_SYSTEM.contains(clause),
                "missing preserve clause: {clause}"
            );
        }
        let recall = [
            "concise",
            "fact",
            "constraint",
            "number",
            "name",
            "negation",
        ];
        for clause in recall {
            assert!(
                EN_TO_ZH_TERSE_SYSTEM.contains(clause),
                "missing terseness/recall clause: {clause}"
            );
        }
        assert!(
            !EN_TO_ZH_TERSE_SYSTEM.contains("natural, fluent"),
            "terse prompt must not request fluent prose"
        );
    }

    #[test]
    fn fluent_prompts_unchanged_for_paired_baselines() {
        assert!(EN_TO_ZH_FLUENT_SYSTEM.contains("natural, fluent Simplified Chinese"));
        assert!(ZH_TO_EN_SYSTEM.contains("natural, fluent English"));
    }
}
