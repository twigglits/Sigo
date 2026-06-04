use std::process::Stdio;
use std::time::Duration;
use tokio::process::Command;

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
    pub fn is_pass(&self) -> bool {
        matches!(self, Outcome::Pass)
    }
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
    // raw scan: returns from the def to end-of-string; trailing prose may cause SyntaxError (acceptable for v1)
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
                if l.trim_start().starts_with("```") {
                    break;
                }
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

/// Extract code, run `<code>\n<test>\ncheck(entry_point)` under `python3` with a
/// hard timeout, and classify the result. Runs untrusted model code: callers
/// should run inside a throwaway VM/container for untrusted corpora.
pub async fn evaluate_answer(
    answer: &str,
    test: &str,
    entry_point: &str,
    timeout: Duration,
) -> Outcome {
    let Some(code) = extract_code(answer, entry_point) else {
        return Outcome::NoCodeExtracted;
    };
    let dir = match tempfile::tempdir() {
        Ok(d) => d,
        Err(_) => return Outcome::RuntimeError,
    };
    let runner = format!("{code}\n{test}\ncheck({entry_point})\nprint('SIGO_OK')\n");
    let path = dir.path().join("runner.py");
    if std::fs::write(&path, runner).is_err() {
        return Outcome::RuntimeError;
    }

    let child = Command::new("python3")
        .arg(&path)
        .current_dir(dir.path())
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn();
    let child = match child {
        Ok(c) => c,
        Err(_) => return Outcome::RuntimeError,
    };

    match tokio::time::timeout(timeout, child.wait_with_output()).await {
        Err(_) => Outcome::Timeout, // timeout elapsed → future cancelled → Child dropped → SIGKILL (kill_on_drop)
        Ok(Err(_)) => Outcome::RuntimeError,
        Ok(Ok(out)) => classify(
            out.status.code(),
            &String::from_utf8_lossy(&out.stdout),
            &String::from_utf8_lossy(&out.stderr),
        ),
    }
}

fn classify(code: Option<i32>, stdout: &str, stderr: &str) -> Outcome {
    if code == Some(0) && stdout.contains("SIGO_OK") {
        return Outcome::Pass;
    }
    if stderr.contains("AssertionError") {
        return Outcome::AssertFail;
    }
    if stderr.contains("SyntaxError")
        || stderr.contains("IndentationError")
        || stderr.contains("NameError")
        || stderr.contains("ImportError")
        || stderr.contains("ModuleNotFoundError")
    {
        return Outcome::CompileError;
    }
    Outcome::RuntimeError
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn picks_block_defining_entry_point() {
        let ans =
            "Here:\n```python\nimport os\n```\nand\n```python\ndef foo():\n    return 1\n```\n";
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

    fn python3_available() -> bool {
        std::process::Command::new("python3")
            .arg("--version")
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    }

    #[tokio::test]
    async fn passing_solution_scores_pass() {
        if !python3_available() {
            eprintln!("skip: no python3");
            return;
        }
        let code = "def add(a, b):\n    return a + b\n";
        let test = "def check(candidate):\n    assert candidate(2, 3) == 5\n";
        assert_eq!(
            evaluate_answer(
                &fence(code),
                test,
                "add",
                std::time::Duration::from_secs(10)
            )
            .await,
            Outcome::Pass
        );
    }

    #[tokio::test]
    async fn wrong_solution_scores_assert_fail() {
        if !python3_available() {
            eprintln!("skip: no python3");
            return;
        }
        let code = "def add(a, b):\n    return a - b\n";
        let test = "def check(candidate):\n    assert candidate(2, 3) == 5\n";
        assert_eq!(
            evaluate_answer(
                &fence(code),
                test,
                "add",
                std::time::Duration::from_secs(10)
            )
            .await,
            Outcome::AssertFail
        );
    }

    #[tokio::test]
    async fn syntax_error_scores_compile_error() {
        if !python3_available() {
            eprintln!("skip: no python3");
            return;
        }
        let code = "def add(a, b)\n    return a + b\n"; // missing colon
        let test = "def check(candidate):\n    assert candidate(2, 3) == 5\n";
        assert_eq!(
            evaluate_answer(
                &fence(code),
                test,
                "add",
                std::time::Duration::from_secs(10)
            )
            .await,
            Outcome::CompileError
        );
    }

    #[tokio::test]
    async fn infinite_loop_scores_timeout() {
        if !python3_available() {
            eprintln!("skip: no python3");
            return;
        }
        let code = "def add(a, b):\n    while True:\n        pass\n";
        let test = "def check(candidate):\n    assert candidate(2, 3) == 5\n";
        assert_eq!(
            evaluate_answer(&fence(code), test, "add", std::time::Duration::from_secs(2)).await,
            Outcome::Timeout
        );
    }

    #[tokio::test]
    async fn no_code_scores_no_code_extracted() {
        let test = "def check(candidate):\n    assert candidate(2, 3) == 5\n";
        assert_eq!(
            evaluate_answer("no code", test, "add", std::time::Duration::from_secs(10)).await,
            Outcome::NoCodeExtracted
        );
    }

    fn fence(code: &str) -> String {
        format!("```python\n{code}```\n")
    }
}
