use std::time::Duration;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    Pass,
    AssertFail,
    CompileError,
    Timeout,
    RuntimeError,
    NoCodeExtracted,
}

impl Outcome {
    pub fn is_pass(&self) -> bool { matches!(self, Outcome::Pass) }
    pub fn label(&self) -> &'static str {
        match self {
            Outcome::Pass => "pass",
            Outcome::AssertFail => "assert_fail",
            Outcome::CompileError => "compile_error",
            Outcome::Timeout => "timeout",
            Outcome::RuntimeError => "runtime_error",
            Outcome::NoCodeExtracted => "no_code",
        }
    }
}

/// Pull the Python solution out of a model answer. Prefers the fenced block that
/// defines `entry_point`; else the longest fenced block; else a raw `def` scan.
pub fn extract_code(answer: &str, entry_point: &str) -> Option<String> {
    let needle = format!("def {entry_point}");
    let blocks = fenced_blocks(answer);
    if let Some(b) = blocks.iter().find(|b| b.contains(&needle)) {
        return Some(b.clone());
    }
    if let Some(b) = blocks.into_iter().max_by_key(|b| b.len()) {
        return Some(b);
    }
    if let Some(idx) = answer.find(&needle) {
        return Some(answer[idx..].to_string());
    }
    None
}

/// Return the bodies of all ```...``` fenced blocks (language tag stripped).
fn fenced_blocks(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut lines = text.lines();
    while let Some(line) = lines.next() {
        if line.trim_start().starts_with("```") {
            let mut body = String::new();
            for l in lines.by_ref() {
                if l.trim_start().starts_with("```") { break; }
                body.push_str(l);
                body.push('\n');
            }
            if !body.trim().is_empty() {
                out.push(body);
            }
        }
    }
    out
}

// TEMPORARY stub — real implementation (python3 execution) lands in the next task.
pub async fn evaluate_answer(_answer: &str, _test: &str, _entry_point: &str, _timeout: Duration) -> Outcome {
    Outcome::NoCodeExtracted
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn picks_block_defining_entry_point() {
        let ans = "Here:\n```python\nimport os\n```\nand\n```python\ndef foo():\n    return 1\n```\n";
        let code = extract_code(ans, "foo").unwrap();
        assert!(code.contains("def foo"));
        assert!(!code.contains("import os"));
    }

    #[test]
    fn falls_back_to_longest_block() {
        let ans = "```\na = 1\n```\n```\nb = 2\nc = 3\nd = 4\n```\n";
        let code = extract_code(ans, "missing").unwrap();
        assert!(code.contains("b = 2"));
    }

    #[test]
    fn raw_scan_when_unfenced() {
        let ans = "def bar():\n    return 7\n";
        assert!(extract_code(ans, "bar").unwrap().contains("return 7"));
    }

    #[test]
    fn none_when_no_code() {
        assert_eq!(extract_code("no code here at all", "baz"), None);
    }

    #[test]
    fn handles_chinese_prose_around_code() {
        let ans = "这是答案：\n```python\ndef add(a, b):\n    return a + b\n```\n完成。";
        assert!(extract_code(ans, "add").unwrap().contains("return a + b"));
    }
}
