use std::path::Path;
use std::process::Stdio;
use std::sync::OnceLock;
use std::time::Duration;
use tokio::process::Command;

/// In-process Python hardening prepended to every runner. Best-effort: it nulls the
/// common shell/file/exec entry points and caps address space so a buggy or hostile
/// solution can't trivially trash the host or OOM a long run.
///
/// On Linux, bubblewrap provides additional network + filesystem isolation when
/// available. On macOS / Windows (and Linux without bwrap), this preamble is the
/// primary sandbox — it is always active regardless of platform or bwrap status.
const SANDBOX_PREAMBLE: &str = r#"# --- sigo sandbox preamble ---
import sys as _sys
try:
    import resource as _resource
    _MAX = 2147483648  # 2 GiB address-space cap
    for _lim in (_resource.RLIMIT_AS, _resource.RLIMIT_DATA):
        try:
            _resource.setrlimit(_lim, (_MAX, _MAX))
        except Exception:
            pass
except Exception:
    pass
try:
    import faulthandler as _fh
    _fh.disable()
except Exception:
    pass
try:
    import os as _os
    _os.environ['OMP_NUM_THREADS'] = '1'
    for _n in ('system','popen','kill','killpg','fork','forkpty','remove','removedirs','rmdir','unlink','rename','renames','replace','truncate','chmod','chown','chroot','chdir','setuid','putenv','execv','execve','execvp','execvpe'):
        if hasattr(_os, _n):
            try:
                setattr(_os, _n, None)
            except Exception:
                pass
except Exception:
    pass
try:
    import shutil as _shutil
    _shutil.rmtree = None
    _shutil.move = None
except Exception:
    pass
try:
    import subprocess as _subprocess
    _subprocess.Popen = None
except Exception:
    pass
import builtins as _builtins
_builtins.exit = None
_builtins.quit = None
for _m in ('tkinter', 'psutil', 'resource'):
    _sys.modules[_m] = None
# --- end sigo sandbox preamble ---
"#;

/// Secret-bearing env vars scrubbed from the child before running untrusted code.
const SCRUBBED_ENV: &[&str] = &[
    "ANTHROPIC_API_KEY",
    "OPENAI_API_KEY",
    "AWS_SECRET_ACCESS_KEY",
    "AWS_SESSION_TOKEN",
    "GITHUB_TOKEN",
];

/// Base bubblewrap arguments: fresh namespaces (incl. network), system read-only,
/// a private /tmp, die-with-parent. Per-run bind/chdir are appended by the caller.
const BWRAP_BASE: &[&str] = &[
    "--unshare-all",
    "--die-with-parent",
    "--ro-bind-try",
    "/usr",
    "/usr",
    "--ro-bind-try",
    "/bin",
    "/bin",
    "--ro-bind-try",
    "/sbin",
    "/sbin",
    "--ro-bind-try",
    "/lib",
    "/lib",
    "--ro-bind-try",
    "/lib64",
    "/lib64",
    "--ro-bind-try",
    "/etc/alternatives",
    "/etc/alternatives",
    "--proc",
    "/proc",
    "--dev",
    "/dev",
    "--tmpfs",
    "/tmp",
];

/// Whether bubblewrap is present AND actually works here. Probed once: nested
/// namespaces (some CI/containers) can have bwrap installed yet fail to set up the
/// sandbox, in which case we must NOT use it or every task would error spuriously.
pub fn bwrap_works() -> bool {
    static OK: OnceLock<bool> = OnceLock::new();
    *OK.get_or_init(|| {
        std::process::Command::new("bwrap")
            .args(BWRAP_BASE)
            .args(["python3", "-c", "pass"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    })
}

/// Command that runs `runner` (absolute, inside `workdir`): bubblewrap-wrapped when it
/// works, else bare `python3` (still hardened by the in-process preamble).
fn runner_command(workdir: &Path, runner: &Path) -> Command {
    if bwrap_works() {
        let mut c = Command::new("bwrap");
        c.args(BWRAP_BASE)
            .arg("--bind")
            .arg(workdir)
            .arg(workdir)
            .arg("--chdir")
            .arg(workdir)
            .arg("python3")
            .arg(runner);
        c
    } else {
        let mut c = Command::new("python3");
        c.arg(runner);
        c
    }
}

/// Result of executing a model-generated solution against its test suite.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// All tests passed.
    Pass,
    /// Code compiled/ran but an assertion failed.
    AssertFail,
    /// The generated code had a syntax error (SyntaxError, IndentationError, TabError).
    CompileError,
    /// Execution timed out.
    Timeout,
    /// A non-specific runtime error occurred.
    RuntimeError,
    /// No code block could be extracted from the model's answer.
    NoCodeExtracted,
}

impl Outcome {
    /// Whether this outcome counts as a passing result.
    pub fn is_pass(&self) -> bool {
        matches!(self, Outcome::Pass)
    }
    /// Short machine-readable label for this outcome.
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

/// Extract code, run `<preamble>\n<code>\n<test>\ncheck(entry_point)` with a hard
/// timeout, and classify the result. The model code is untrusted: it runs under
/// bubblewrap when available (no network, read-only system) and always behind the
/// in-process [`SANDBOX_PREAMBLE`]. The guard is best-effort, not a security boundary —
/// for a genuinely untrusted corpus, install `bwrap` or use a throwaway VM/container.
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
    let runner =
        format!("{SANDBOX_PREAMBLE}\n{code}\n{test}\ncheck({entry_point})\nprint('SIGO_OK')\n");
    let path = dir.path().join("runner.py");
    if std::fs::write(&path, runner).is_err() {
        return Outcome::RuntimeError;
    }

    let mut cmd = runner_command(dir.path(), &path);
    cmd.current_dir(dir.path())
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    for var in SCRUBBED_ENV {
        cmd.env_remove(var);
    }
    let child = cmd.spawn();
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
    // Only true parse-time failures are "compile" errors. NameError / ImportError /
    // ModuleNotFoundError are raised at run time and belong in the runtime bucket.
    if stderr.contains("SyntaxError")
        || stderr.contains("IndentationError")
        || stderr.contains("TabError")
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
    async fn sandbox_neutralizes_dangerous_calls() {
        if !python3_available() {
            eprintln!("skip: no python3");
            return;
        }
        // PASSES only if the reliability guard nulled these before the model code ran.
        let code = "import os, subprocess\ndef add(a, b):\n    assert os.system is None\n    assert os.remove is None\n    assert subprocess.Popen is None\n    return a + b\n";
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
    async fn name_error_scores_runtime_not_compile() {
        if !python3_available() {
            eprintln!("skip: no python3");
            return;
        }
        // Valid syntax, but an undefined name is referenced at run time — NameError is
        // a runtime exception, not a parse/compile failure.
        let code = "def add(a, b):\n    return a + undefined_name\n";
        let test = "def check(candidate):\n    assert candidate(2, 3) == 5\n";
        assert_eq!(
            evaluate_answer(
                &fence(code),
                test,
                "add",
                std::time::Duration::from_secs(10)
            )
            .await,
            Outcome::RuntimeError
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
