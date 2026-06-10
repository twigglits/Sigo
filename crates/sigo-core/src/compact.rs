//! Deterministic whitespace compaction for the Chinese prompt sent to Claude.
//!
//! The translated ZH prompt is the billed artifact (it is sent, recorded, and
//! replayed as history every turn), so removing whitespace the o200k-style BPE
//! would otherwise tokenize is a pure win — but only where it cannot change
//! meaning. The rules are therefore deliberately narrow:
//!
//! - **Protected spans are byte-identical.** Fenced code blocks (CommonMark-ish:
//!   opened by up to 3 leading spaces + a run of 3+ backticks with no backtick in
//!   the info string; closed by a line of up to 3 leading spaces + a run at least
//!   as long; unclosed runs to end of input) and inline code spans (a run of N
//!   backticks closes only on the next run of exactly N; unmatched runs are
//!   literal text) pass through untouched.
//! - **Only lines containing a CJK ideograph are edited.** Code, English prose,
//!   and anything else without CJK is byte-identical by construction — unfenced
//!   indented code (e.g. the bundled HumanEval prompts), string literals with
//!   significant spaces, and Makefile tabs are all safe because they contain no
//!   CJK. On CJK lines the compactor only ever DELETES whitespace, never any
//!   other character.
//!
//! Edits on CJK lines, in order:
//! 1. trim trailing whitespace (including `\r`) before the newline;
//! 2. collapse runs of 3+ newlines to exactly 2, only when the lines on both
//!    sides of the run contain CJK (so blank runs inside unfenced code survive);
//! 3. collapse interior runs of 2+ ASCII spaces to one (never line-leading
//!    indentation; tabs are never collapsed, so TSV columns survive);
//! 4. delete a single ASCII space exactly when one neighbor is a CJK ideograph
//!    and the other is ASCII alphanumeric (`在 Rust 中` → `在Rust中`), unless the
//!    non-CJK side's whitespace-delimited token contains `/` or `\\` (URLs and
//!    paths keep their boundaries). CJK–CJK spaces are never removed (quoted
//!    literals like `"初始 提交"` and filenames keep their internal spaces).
//!
//! Known, intentional limitations: markdown hard line breaks (two trailing
//! spaces) on CJK lines are removed; full-width characters are never normalized
//! (their identity can be the subject of the prompt); ideographic space U+3000
//! is left untouched. This module never sees assistant responses — compaction
//! applies to the outbound prompt only (rewriting replayed assistant turns would
//! both falsify the record and defeat prompt caching). The fence policy here is
//! intentionally stricter than the display-side `SentenceBuffer` (which splits
//! Claude's *response* stream); the two never process the same text.
//!
//! `compact_zh` is idempotent, and on any input without CJK ideographs it
//! returns the input unchanged.

/// Compact the Chinese prompt text. See module docs for the exact rules.
pub fn compact_zh(input: &str) -> String {
    if !has_cjk(input) {
        return input.to_string();
    }
    let mut out = String::with_capacity(input.len());
    for piece in segment(input) {
        match piece {
            Piece::Protected(s) => out.push_str(s),
            Piece::Text(s) => out.push_str(&compact_text(s)),
        }
    }
    out
}

const fn is_cjk_ideograph(c: char) -> bool {
    matches!(c,
        '\u{4E00}'..='\u{9FFF}'     // CJK Unified Ideographs
        | '\u{3400}'..='\u{4DBF}'   // Extension A
        | '\u{F900}'..='\u{FAFF}'   // Compatibility Ideographs
        | '\u{20000}'..='\u{2EBEF}' // Extensions B..F
    )
}

fn has_cjk(s: &str) -> bool {
    s.chars().any(is_cjk_ideograph)
}

pub(crate) enum Piece<'a> {
    Protected(&'a str),
    Text(&'a str),
}

/// Split into protected spans (fenced blocks first, then inline code within the
/// remaining text) and editable text runs. Concatenating the pieces in order
/// reproduces the input exactly. Shared with `translator::mask`, which uses the
/// same protected-span definition to hide code from the local model entirely.
pub(crate) fn segment(input: &str) -> Vec<Piece<'_>> {
    let mut pieces = Vec::new();
    for block in split_fences(input) {
        match block {
            Piece::Protected(s) => pieces.push(Piece::Protected(s)),
            Piece::Text(t) => split_inline_code(t, &mut pieces),
        }
    }
    pieces
}

/// A fence opens on a line of up to 3 leading spaces + a run of 3+ backticks
/// whose info string contains no backtick; it closes on a line of up to 3
/// leading spaces + a run at least as long with only blanks after. An unclosed
/// fence protects through end of input.
fn split_fences(input: &str) -> Vec<Piece<'_>> {
    let mut out = Vec::new();
    let mut text_start = 0usize;
    let mut fence_start = 0usize;
    let mut fence_len = 0usize;
    let mut in_fence = false;
    let mut pos = 0usize;
    for line in input.split_inclusive('\n') {
        let line_start = pos;
        pos += line.len();
        let content = line.strip_suffix('\n').unwrap_or(line);
        if in_fence {
            if fence_close_matches(content, fence_len) {
                out.push(Piece::Protected(&input[fence_start..pos]));
                in_fence = false;
                text_start = pos;
            }
        } else if let Some(n) = fence_open_len(content) {
            if line_start > text_start {
                out.push(Piece::Text(&input[text_start..line_start]));
            }
            fence_start = line_start;
            fence_len = n;
            in_fence = true;
        }
    }
    if in_fence {
        out.push(Piece::Protected(&input[fence_start..]));
    } else if text_start < input.len() {
        out.push(Piece::Text(&input[text_start..]));
    }
    out
}

fn fence_open_len(content: &str) -> Option<usize> {
    let indent = content.bytes().take_while(|b| *b == b' ').count();
    if indent > 3 {
        return None;
    }
    let rest = &content[indent..];
    let n = rest.bytes().take_while(|b| *b == b'`').count();
    if n < 3 || rest[n..].contains('`') {
        return None;
    }
    Some(n)
}

fn fence_close_matches(content: &str, open_len: usize) -> bool {
    let indent = content.bytes().take_while(|b| *b == b' ').count();
    if indent > 3 {
        return false;
    }
    let rest = &content[indent..];
    let n = rest.bytes().take_while(|b| *b == b'`').count();
    n >= open_len && rest[n..].bytes().all(|b| matches!(b, b' ' | b'\t' | b'\r'))
}

/// CommonMark backtick-string rule: a run of N backticks is closed only by the
/// next run of exactly N; unmatched runs are literal text. Scanning bytes is
/// UTF-8 safe because a backtick byte never occurs inside a multibyte char.
fn split_inline_code<'a>(t: &'a str, out: &mut Vec<Piece<'a>>) {
    let bytes = t.as_bytes();
    let mut text_start = 0usize;
    let mut i = 0usize;
    while i < bytes.len() {
        if bytes[i] != b'`' {
            i += 1;
            continue;
        }
        let run_start = i;
        while i < bytes.len() && bytes[i] == b'`' {
            i += 1;
        }
        let n = i - run_start;
        let mut j = i;
        let mut close_end: Option<usize> = None;
        while j < bytes.len() {
            if bytes[j] == b'`' {
                let s = j;
                while j < bytes.len() && bytes[j] == b'`' {
                    j += 1;
                }
                if j - s == n {
                    close_end = Some(j);
                    break;
                }
            } else {
                j += 1;
            }
        }
        if let Some(end) = close_end {
            if run_start > text_start {
                out.push(Piece::Text(&t[text_start..run_start]));
            }
            out.push(Piece::Protected(&t[run_start..end]));
            text_start = end;
            i = end;
        }
        // No closer: the run is literal text; keep scanning after it.
    }
    if text_start < t.len() {
        out.push(Piece::Text(&t[text_start..]));
    }
}

/// Apply the whitespace rules to one editable text run, in order: trailing-trim
/// (CJK lines), newline-run collapse (CJK on both sides), then per-line interior
/// space collapse + boundary-space removal (CJK lines).
fn compact_text(s: &str) -> String {
    let trimmed = trim_trailing_on_cjk_lines(s);
    let collapsed = collapse_newline_runs(&trimmed);
    collapsed.split_inclusive('\n').map(compact_line).collect()
}

fn trim_trailing_on_cjk_lines(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for line in s.split_inclusive('\n') {
        match line.strip_suffix('\n') {
            Some(content) if has_cjk(content) => {
                out.push_str(content.trim_end_matches([' ', '\t', '\r']));
                out.push('\n');
            }
            // Last line of the run (no newline): the true line may continue in
            // an adjacent protected span, so leave it untouched.
            _ => out.push_str(line),
        }
    }
    out
}

fn collapse_newline_runs(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = String::with_capacity(s.len());
    let mut seg_start = 0usize;
    let mut i = 0usize;
    while i < bytes.len() {
        if bytes[i] != b'\n' {
            i += 1;
            continue;
        }
        let run_start = i;
        while i < bytes.len() && bytes[i] == b'\n' {
            i += 1;
        }
        let line_before_start = s[..run_start].rfind('\n').map_or(0, |k| k + 1);
        let line_after_end = s[i..].find('\n').map_or(s.len(), |k| i + k);
        if i - run_start >= 3
            && has_cjk(&s[line_before_start..run_start])
            && has_cjk(&s[i..line_after_end])
        {
            out.push_str(&s[seg_start..run_start]);
            out.push_str("\n\n");
            seg_start = i;
        }
    }
    out.push_str(&s[seg_start..]);
    out
}

fn compact_line(line: &str) -> String {
    let (content, nl) = match line.strip_suffix('\n') {
        Some(c) => (c, "\n"),
        None => (line, ""),
    };
    if !has_cjk(content) {
        return line.to_string();
    }
    let chars: Vec<char> = content.chars().collect();
    let indent = chars
        .iter()
        .take_while(|c| matches!(**c, ' ' | '\t'))
        .count();

    // Interior runs of 2+ spaces collapse to one; indentation is untouched.
    let mut collapsed: Vec<char> = chars[..indent].to_vec();
    let mut i = indent;
    while i < chars.len() {
        if chars[i] == ' ' {
            while i < chars.len() && chars[i] == ' ' {
                i += 1;
            }
            collapsed.push(' ');
        } else {
            collapsed.push(chars[i]);
            i += 1;
        }
    }

    // Delete a single space exactly at a CJK<->ASCII-alnum boundary, unless an
    // adjacent token looks like a path or URL.
    let mut out: Vec<char> = collapsed[..indent].to_vec();
    let mut i = indent;
    while i < collapsed.len() {
        let c = collapsed[i];
        if c == ' ' && i > indent && i + 1 < collapsed.len() {
            let prev = collapsed[i - 1];
            let next = collapsed[i + 1];
            let boundary = (is_cjk_ideograph(prev) && next.is_ascii_alphanumeric())
                || (prev.is_ascii_alphanumeric() && is_cjk_ideograph(next));
            if boundary && !adjacent_token_has_path_sep(&collapsed, i) {
                i += 1;
                continue;
            }
        }
        out.push(c);
        i += 1;
    }
    let mut result: String = out.into_iter().collect();
    result.push_str(nl);
    result
}

/// Whether either whitespace-delimited token adjacent to the space at `sp`
/// contains a path separator — those boundaries keep their space so URLs and
/// file paths stay visually delimited from surrounding prose.
fn adjacent_token_has_path_sep(chars: &[char], sp: usize) -> bool {
    let mut l = sp;
    while l > 0 && !chars[l - 1].is_whitespace() {
        l -= 1;
    }
    let mut r = sp + 1;
    while r < chars.len() && !chars[r].is_whitespace() {
        r += 1;
    }
    chars[l..sp]
        .iter()
        .chain(chars[sp + 1..r].iter())
        .any(|c| matches!(*c, '/' | '\\'))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::benchmark::load_default_coding_corpus;
    use crate::tokenizer::{Tokenizer, TokenizerProxy};

    /// Realistic fixtures: live qwen2.5:7b fluent-mode outputs (they contain the
    /// CJK-Latin spacing the compactor exists to reclaim) plus adversarial cases.
    fn golden_corpus() -> Vec<&'static str> {
        vec![
            "请在 src/config.rs 中重构 parse_config 函数，使其返回一个 Result 而不是直接 panic，但不要更改其公共签名，并确保所有现有的 12 个测试仍然通过。",
            "我有一个在负载较重时偶尔返回500错误的Web服务器。这个服务器用Python和Flask编写。",
            "在 Rust 中使用 tokio 实现并发处理 HTTP 请求。",
            "说明  文字。\n```rust\nlet x  =  1;  \n// 注释  空格\nfn main() {}\n```\n结尾  文字。",
            "- 安装依赖\n1. 第一步\n# 标题\n  - 嵌套 项目",
            "git commit -m \"初始 提交\"",
            "见 https://example.com 上的文档。",
            "运行 `cargo  test` 即可  完成。",
            "第一段。\n\n\n\n第二段。",
            "名字\t年龄\n张三\t30\n",
            "全角：Ａ１２３ 测试 ＡＢＣ。",
        ]
    }

    #[test]
    fn removes_single_space_at_cjk_latin_boundary_both_orders() {
        assert_eq!(
            compact_zh("在 Rust 中使用 tokio 实现并发处理 HTTP 请求。"),
            "在Rust中使用tokio实现并发处理HTTP请求。"
        );
    }

    #[test]
    fn keeps_space_between_latin_words() {
        assert_eq!(compact_zh("使用 cargo build 编译"), "使用cargo build编译");
    }

    #[test]
    fn collapses_interior_space_runs_on_cjk_lines() {
        assert_eq!(compact_zh("结果  如下：状态  正常"), "结果 如下：状态 正常");
        assert_eq!(compact_zh("用  Python  写脚本"), "用Python写脚本");
    }

    #[test]
    fn never_touches_line_leading_indentation() {
        assert_eq!(compact_zh("    缩进的 ZH 行"), "    缩进的ZH行");
        assert_eq!(compact_zh("  - 嵌套 项目"), "  - 嵌套 项目");
    }

    #[test]
    fn trims_trailing_whitespace_on_cjk_lines_only() {
        assert_eq!(
            compact_zh("第一行。  \n第二行。\t\nplain english line   \n"),
            "第一行。\n第二行。\nplain english line   \n"
        );
    }

    #[test]
    fn trims_carriage_return_as_trailing_whitespace() {
        assert_eq!(compact_zh("你好。\r\n世界。\r\n"), "你好。\n世界。\n");
    }

    #[test]
    fn collapses_three_plus_newlines_between_cjk_paragraphs() {
        assert_eq!(
            compact_zh("第一段。\n\n\n\n第二段。"),
            "第一段。\n\n第二段。"
        );
    }

    #[test]
    fn keeps_newline_runs_adjacent_to_non_cjk_lines() {
        // PEP8 double-blank between defs (3 newlines) in unfenced code must survive.
        assert_eq!(
            compact_zh("english para\n\n\n\n第二段。"),
            "english para\n\n\n\n第二段。"
        );
        assert_eq!(
            compact_zh("from typing import List\n\n\ndef f(x):\n    return x"),
            "from typing import List\n\n\ndef f(x):\n    return x"
        );
    }

    #[test]
    fn input_without_cjk_is_byte_identical() {
        let messy = "english  with   runs\t\nand trailing   \n\n\n\nmore  text";
        assert_eq!(compact_zh(messy), messy);
        assert_eq!(compact_zh(""), "");
    }

    #[test]
    fn bundled_humaneval_prompts_are_byte_identical() {
        // The ground-truth coding corpus is unfenced indented Python with
        // significant interior spaces in string literals and doctests. The
        // compactor must never alter it.
        for task in load_default_coding_corpus().iter().take(10) {
            assert_eq!(
                compact_zh(&task.prompt),
                task.prompt,
                "corpus prompt altered: {}",
                task.task_id
            );
        }
    }

    #[test]
    fn markdown_markers_survive() {
        let s = "- 安装依赖\n1. 第一步\n# 标题\n> 引用 文字";
        assert_eq!(compact_zh(s), s);
    }

    #[test]
    fn quoted_cjk_internal_space_kept() {
        let s = "git commit -m \"初始 提交\"";
        assert_eq!(compact_zh(s), s);
        let p = "路径 /home/用户/我的 文档/a.txt 存在吗？";
        // CJK–CJK space inside the path survives; CJK–Latin boundaries around
        // the path also survive because the token contains '/'.
        assert_eq!(compact_zh(p), p);
    }

    #[test]
    fn url_and_path_boundaries_keep_space() {
        let url = "见 https://example.com 上的文档。";
        assert_eq!(compact_zh(url), url);
        let path = "保存到 /tmp/output 目录。";
        assert_eq!(compact_zh(path), path);
        let win = "参考 C:\\Users\\file 路径。";
        assert_eq!(compact_zh(win), win);
        // ...but ordinary identifiers still compact.
        assert_eq!(compact_zh("在 Rust 中"), "在Rust中");
    }

    #[test]
    fn fenced_code_interior_byte_identical() {
        assert_eq!(
            compact_zh("说明  文字。\n```rust\nlet x  =  1;  \n// 注释  空格\nfn main() {}\n```\n结尾  文字。"),
            "说明 文字。\n```rust\nlet x  =  1;  \n// 注释  空格\nfn main() {}\n```\n结尾 文字。"
        );
    }

    #[test]
    fn four_backtick_fence_protects_inner_triple() {
        let s = "````\n```\ncode  here\n```\n````\n";
        assert_eq!(compact_zh(s), s);
    }

    #[test]
    fn indented_fence_recognized() {
        let s = " ```\ncode   x  与  空格\n ```\n";
        assert_eq!(compact_zh(s), s);
    }

    #[test]
    fn unclosed_fence_passes_through_to_end() {
        assert_eq!(
            compact_zh("正文  开始。\n```\nraw   stuff\n\n\n\nmore  "),
            "正文 开始。\n```\nraw   stuff\n\n\n\nmore  "
        );
    }

    #[test]
    fn inline_code_span_byte_identical() {
        assert_eq!(
            compact_zh("运行 `cargo  test` 即可  完成。"),
            "运行 `cargo  test` 即可 完成。"
        );
    }

    #[test]
    fn double_backtick_span_protected() {
        assert_eq!(
            compact_zh("代码 ``a ` b`` 之后  继续。"),
            "代码 ``a ` b`` 之后 继续。"
        );
    }

    #[test]
    fn unmatched_backtick_is_literal_text() {
        assert_eq!(compact_zh("价格是 `100 元整。"), "价格是 `100元整。");
    }

    #[test]
    fn fullwidth_characters_untouched() {
        let s = "全角：Ａ１２３ 测试 ＡＢＣ。";
        assert_eq!(compact_zh(s), s);
    }

    #[test]
    fn tabs_and_tsv_columns_survive() {
        let s = "名字\t年龄\n张三\t30\n";
        assert_eq!(compact_zh(s), s);
    }

    #[test]
    fn markdown_hard_line_break_removed_intentionally() {
        // Documented limitation: two trailing spaces (hard break) on a CJK line
        // are trimmed like any trailing whitespace.
        assert_eq!(compact_zh("第一行  \n第二行"), "第一行\n第二行");
    }

    #[test]
    fn idempotent_over_golden_corpus() {
        for s in golden_corpus() {
            let once = compact_zh(s);
            assert_eq!(compact_zh(&once), once, "not idempotent on: {s:?}");
        }
    }

    #[test]
    fn only_whitespace_is_ever_deleted() {
        for s in golden_corpus() {
            let out = compact_zh(s);
            let strip = |t: &str| t.chars().filter(|c| !c.is_whitespace()).collect::<String>();
            assert_eq!(strip(s), strip(&out), "non-whitespace changed on: {s:?}");
            assert!(out.len() <= s.len(), "output grew on: {s:?}");
        }
    }

    #[test]
    fn proxy_token_count_never_increases_on_golden_corpus() {
        // Regression corpus, not a BPE theorem: the orchestrator additionally
        // guards with a live token comparison before sending.
        let t = TokenizerProxy::new().unwrap();
        for s in golden_corpus() {
            let before = t.count_tokens(s).unwrap();
            let after = t.count_tokens(&compact_zh(s)).unwrap();
            assert!(after <= before, "tokens grew {before}->{after} on: {s:?}");
        }
    }
}
