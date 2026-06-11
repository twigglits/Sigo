//! Terminal multiple-choice picker for AskUserQuestion passthrough.
//!
//! Pure rendering and input parsing with injected I/O so every path is
//! unit-testable. The picker displays the (already translated) question and
//! options, and returns a semantic outcome — which option indexes were
//! picked, free text the user typed instead, or a decline.

use std::io::{BufRead, Write};

/// A question prepared for display (fields already translated to English,
/// falling back to the original Chinese when translation failed).
#[derive(Debug, Clone)]
pub struct DisplayQuestion {
    /// The question text to show.
    pub question: String,
    /// Short chip/tag label (may be empty).
    pub header: String,
    /// The selectable options.
    pub options: Vec<DisplayOption>,
    /// Whether multiple options may be picked.
    pub multi_select: bool,
}

/// One displayed option.
#[derive(Debug, Clone)]
pub struct DisplayOption {
    /// Display label.
    pub label: String,
    /// Display description (may be empty).
    pub description: String,
}

/// The user's choice for one question.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PickOutcome {
    /// Indexes (0-based) of the picked options.
    Picked(Vec<usize>),
    /// The user typed a custom answer instead of picking an option.
    FreeText(String),
    /// The user declined to answer.
    Declined,
}

/// Render the question, its numbered options, and the input hint.
pub fn render(q: &DisplayQuestion) -> String {
    let mut out = String::new();
    out.push('\n');
    if q.header.is_empty() {
        out.push_str(&format!("❓ {}\n", q.question));
    } else {
        out.push_str(&format!("❓ [{}] {}\n", q.header, q.question));
    }
    for (i, opt) in q.options.iter().enumerate() {
        if opt.description.is_empty() {
            out.push_str(&format!("  {}) {}\n", i + 1, opt.label));
        } else {
            out.push_str(&format!(
                "  {}) {} — {}\n",
                i + 1,
                opt.label,
                opt.description
            ));
        }
    }
    let n = q.options.len();
    if q.multi_select {
        out.push_str(&format!(
            "(pick one or more of 1-{n}, e.g. 1,3 · or type your own answer · 'skip' to decline) "
        ));
    } else {
        out.push_str(&format!(
            "(pick 1-{n} · or type your own answer · 'skip' to decline) "
        ));
    }
    out
}

/// Parse one line of user input against the option count.
///
/// Returns `None` for input that is neither a valid selection, free text,
/// nor a decline — the caller should re-prompt. Purely-numeric input is
/// always treated as a selection attempt (so an out-of-range "7" re-prompts
/// instead of being sent as a free-text answer).
pub fn parse_selection(input: &str, n_options: usize, multi: bool) -> Option<PickOutcome> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return None;
    }
    if trimmed.eq_ignore_ascii_case("skip") {
        return Some(PickOutcome::Declined);
    }
    // Selection attempt: tokens of digits separated by commas/whitespace.
    let tokens: Vec<&str> = trimmed
        .split(|c: char| c == ',' || c.is_whitespace())
        .filter(|t| !t.is_empty())
        .collect();
    let all_numeric = tokens.iter().all(|t| t.chars().all(|c| c.is_ascii_digit()));
    if all_numeric {
        let mut picked: Vec<usize> = Vec::new();
        for t in &tokens {
            let n: usize = t.parse().ok()?;
            if n < 1 || n > n_options {
                return None; // out of range → re-prompt, never free text
            }
            if !picked.contains(&(n - 1)) {
                picked.push(n - 1);
            }
        }
        if picked.is_empty() {
            return None;
        }
        if !multi && picked.len() > 1 {
            return None;
        }
        return Some(PickOutcome::Picked(picked));
    }
    Some(PickOutcome::FreeText(trimmed.to_string()))
}

/// Run the interactive prompt loop: render, read a line, parse; re-prompt on
/// invalid input. EOF (Ctrl-D) declines.
pub fn pick(q: &DisplayQuestion, input: &mut dyn BufRead, output: &mut dyn Write) -> PickOutcome {
    let _ = output.write_all(render(q).as_bytes());
    let _ = output.flush();
    loop {
        let mut line = String::new();
        match input.read_line(&mut line) {
            Ok(0) | Err(_) => return PickOutcome::Declined, // EOF
            Ok(_) => {}
        }
        match parse_selection(&line, q.options.len(), q.multi_select) {
            Some(outcome) => return outcome,
            None => {
                let _ = output.write_all("invalid choice, try again: ".as_bytes());
                let _ = output.flush();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn q(multi: bool) -> DisplayQuestion {
        DisplayQuestion {
            question: "What is your favorite color?".into(),
            header: "Color".into(),
            options: vec![
                DisplayOption {
                    label: "Blue".into(),
                    description: "Cool and calming".into(),
                },
                DisplayOption {
                    label: "Red".into(),
                    description: String::new(),
                },
            ],
            multi_select: multi,
        }
    }

    #[test]
    fn render_shows_header_question_numbered_options_and_hint() {
        let s = render(&q(false));
        assert!(s.contains("[Color] What is your favorite color?"), "{s}");
        assert!(s.contains("1) Blue — Cool and calming"), "{s}");
        assert!(s.contains("2) Red"), "{s}");
        assert!(s.contains("pick 1-2"), "{s}");
        assert!(s.contains("skip"), "{s}");
    }

    #[test]
    fn render_multi_select_hint_differs() {
        let s = render(&q(true));
        assert!(s.contains("one or more"), "{s}");
        assert!(s.contains("1,3"), "{s}");
    }

    #[test]
    fn single_selection_parses() {
        assert_eq!(
            parse_selection("1", 2, false),
            Some(PickOutcome::Picked(vec![0]))
        );
        assert_eq!(
            parse_selection(" 2 ", 2, false),
            Some(PickOutcome::Picked(vec![1]))
        );
    }

    #[test]
    fn out_of_range_or_zero_reprompts_never_free_text() {
        assert_eq!(parse_selection("7", 2, false), None);
        assert_eq!(parse_selection("0", 2, false), None);
        assert_eq!(parse_selection("3", 2, true), None);
    }

    #[test]
    fn empty_input_reprompts() {
        assert_eq!(parse_selection("", 2, false), None);
        assert_eq!(parse_selection("   \n", 2, false), None);
    }

    #[test]
    fn multi_select_accepts_lists_and_dedupes() {
        assert_eq!(
            parse_selection("1,2", 2, true),
            Some(PickOutcome::Picked(vec![0, 1]))
        );
        assert_eq!(
            parse_selection("2 1", 2, true),
            Some(PickOutcome::Picked(vec![1, 0]))
        );
        assert_eq!(
            parse_selection("1,1,1", 2, true),
            Some(PickOutcome::Picked(vec![0]))
        );
    }

    #[test]
    fn single_select_rejects_lists() {
        assert_eq!(parse_selection("1,2", 2, false), None);
    }

    #[test]
    fn skip_declines_case_insensitively() {
        assert_eq!(
            parse_selection("skip", 2, false),
            Some(PickOutcome::Declined)
        );
        assert_eq!(
            parse_selection("SKIP", 2, true),
            Some(PickOutcome::Declined)
        );
    }

    #[test]
    fn non_numeric_input_is_free_text() {
        assert_eq!(
            parse_selection("dark teal, please", 2, false),
            Some(PickOutcome::FreeText("dark teal, please".into()))
        );
        // mixed numeric+text is free text too
        assert_eq!(
            parse_selection("1 but darker", 2, false),
            Some(PickOutcome::FreeText("1 but darker".into()))
        );
    }

    #[test]
    fn pick_loop_reprompts_until_valid() {
        let mut input = std::io::Cursor::new(b"9\nblah blah\n".to_vec());
        // "9" is invalid (re-prompt), "blah blah" is free text (accepted)
        let mut out = Vec::new();
        let got = pick(&q(false), &mut input, &mut out);
        assert_eq!(got, PickOutcome::FreeText("blah blah".into()));
        let printed = String::from_utf8(out).unwrap();
        assert!(printed.contains("invalid choice"), "{printed}");
    }

    #[test]
    fn pick_eof_declines() {
        let mut input = std::io::Cursor::new(Vec::<u8>::new());
        let mut out = Vec::new();
        assert_eq!(pick(&q(false), &mut input, &mut out), PickOutcome::Declined);
    }
}
