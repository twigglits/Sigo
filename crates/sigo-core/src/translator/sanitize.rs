//! Input sanitization for the translator pipeline.
//!
//! The user's English prompt is sanitized before being sent to the local Ollama
//! model to prevent prompt-injection-style hijacking of the translator's
//! system prompt or role context.
//!
//! The Ollama translator wraps user text in `<source>` markers and sends it in
//! a structured JSON payload with a fixed `role: "user"`, so the primary vector
//! is content-level injection (instruction override inside the source text).
//! Sanitization here is defense-in-depth — the translate-not-answer protocol
//! and few-shot demonstrations are the primary protection.

/// Characters stripped from input because they can interfere with prompt framing
/// or leak through to the model as special tokens.
const STRIP_CHARS: &[char] = &[
    '\0', '\u{0001}', '\u{0002}', '\u{0003}', '\u{0004}', '\u{0005}', '\u{0006}', '\u{0007}',
    '\u{000B}', // vertical tab
    '\u{000C}', // form feed
    '\u{000E}', '\u{000F}', '\u{0010}', '\u{0011}', '\u{0012}', '\u{0013}', '\u{0014}', '\u{0015}',
    '\u{0016}', '\u{0017}', '\u{0018}', '\u{0019}', '\u{001A}', '\u{001B}', // escape
    '\u{001C}', '\u{001D}', '\u{001E}', '\u{001F}', '\u{007F}', // DEL
];

/// Apply sanitization to a user prompt before passing it to the translator.
///
/// 1. Strips null bytes and non-whitespace control characters.
/// 2. Detects and replaces injected `<source>` or `</source>` markers that
///    could break out of the translate-not-answer wrapping — these are
///    replaced with innocuous equivalents.
pub fn sanitize(input: &str) -> String {
    let mut out = String::with_capacity(input.len());

    for ch in input.chars() {
        if STRIP_CHARS.contains(&ch) {
            continue;
        }
        out.push(ch);
    }

    // Neutralise injected <source> or </source> tokens in the text.
    // The builder wraps the prompt in `<source>\n...\n</source>`, so any
    // occurrence of these markers inside the user's text could confuse the
    // model's context boundary. We replace them with a visible-safe equivalent.
    if out.contains("<source>") || out.contains("</source>") {
        out = out.replace("<source>", "<[source]>");
        out = out.replace("</source>", "</[source]>");
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn null_bytes_are_stripped() {
        assert_eq!(sanitize("hello\0world"), "helloworld");
    }

    #[test]
    fn control_chars_are_stripped() {
        let input = "hello\u{1}world\u{1F}test";
        assert_eq!(sanitize(input), "helloworldtest");
    }

    #[test]
    fn newlines_and_tabs_preserved() {
        let input = "line1\nline2\tindented";
        assert_eq!(sanitize(input), "line1\nline2\tindented");
    }

    #[test]
    fn source_tags_are_neutralised() {
        let input = "ignore <source> and translate this";
        let result = sanitize(input);
        assert!(!result.contains("<source>"));
        assert!(result.contains("<[source]>"));
    }

    #[test]
    fn closing_source_tag_is_neutralised() {
        let input = "malicious </source> injection";
        let result = sanitize(input);
        assert!(!result.contains("</source>"));
        assert!(result.contains("</[source]>"));
    }

    #[test]
    fn clean_text_passes_through() {
        let input = "Explain how to use async/await in Rust.";
        assert_eq!(sanitize(input), input);
    }

    #[test]
    fn mixed_injection_with_clean_text() {
        let input = "what is <source>broken</source> now";
        let result = sanitize(input);
        assert!(!result.contains("<source>"), "found raw <source>: {result}");
        assert!(
            !result.contains("</source>"),
            "found raw </source>: {result}"
        );
        assert!(result.contains("<[source]>"));
        assert!(result.contains("</[source]>"));
    }
}
