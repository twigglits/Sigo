//! One-off measurement harness: o200k proxy token costs of candidate ZH
//! compaction transforms. Run: cargo run -p sigo-core --example token_measure
use sigo_core::tokenizer::{Tokenizer, TokenizerProxy};

fn main() {
    let t = TokenizerProxy::new().unwrap();
    let n = |s: &str| t.count_tokens(s).unwrap();

    // With args: count each arg and exit (lets shell scripts measure live translator output).
    let args: Vec<String> = std::env::args().skip(1).collect();
    if !args.is_empty() {
        for a in &args {
            println!("{:6} tok  {:5} chars   {}", n(a), a.chars().count(), a);
        }
        return;
    }
    let row = |label: &str, s: &str| {
        println!("{:6} tok  {:5} chars   {}", n(s), s.chars().count(), label);
    };

    println!("== A. Full-width vs ASCII punctuation ==");
    row(
        "full-width",
        "请编写一个函数，检查数字是否为质数。它应该返回布尔值！对吗？",
    );
    row(
        "ascii",
        "请编写一个函数,检查数字是否为质数.它应该返回布尔值!对吗?",
    );
    row("fw colon/paren", "注意：这个函数（递归版本）很慢；请优化。");
    row("ascii colon/paren", "注意:这个函数(递归版本)很慢;请优化.");

    println!("\n== B. CJK-Latin spacing ==");
    row("spaced", "在 Rust 中使用 tokio 实现并发处理 HTTP 请求。");
    row("unspaced", "在Rust中使用tokio实现并发处理HTTP请求。");

    println!("\n== C. Fluent vs terse phrasing (same meaning) ==");
    row(
        "EN original",
        "Please write a function that checks whether a number is prime, and explain its time complexity.",
    );
    row(
        "ZH fluent",
        "请编写一个函数来检查一个数字是否为质数，并解释它的时间复杂度。",
    );
    row("ZH terse", "写一个判断质数的函数，并解释其时间复杂度。");
    row("ZH ultra", "写函数判断质数，解释时间复杂度。");

    println!("\n== D. Longer realistic prompt ==");
    let en = "I have a web server that occasionally returns 500 errors under heavy load. \
Can you help me figure out what might be causing this? The server is written in Python \
using Flask, and it talks to a PostgreSQL database. The errors seem to happen more \
often when many users are uploading files at the same time.";
    let zh_fluent = "我有一个网络服务器，在高负载下偶尔会返回500错误。你能帮我找出可能的原因吗？\
这个服务器是用Python编写的，使用Flask框架，并且与PostgreSQL数据库通信。\
当许多用户同时上传文件时，这些错误似乎发生得更频繁。";
    let zh_terse = "我的Flask(Python)服务器高负载时偶发500错误，后端是PostgreSQL。\
多用户同时上传文件时错误更频繁。帮我找原因。";
    row("EN original", en);
    row("ZH fluent", zh_fluent);
    row("ZH terse", zh_terse);

    println!("\n== E. Whitespace normalization ==");
    row("messy", "第一段。  \n\n\n\n第二段。   \n");
    row("clean", "第一段。\n\n第二段。\n");

    println!("\n== F. Function-word elision ==");
    row(
        "with particles",
        "如果这个文件不存在的话，那么就创建一个新的文件。",
    );
    row("elided", "如果文件不存在，则新建文件。");
    row("classical", "文件不存在则新建。");

    println!("\n== G. Per-char cost of common CJK runs ==");
    for s in [
        "质数",
        "时间复杂度",
        "数据库",
        "服务器",
        "并发",
        "函数",
        "复杂",
        "错误",
    ] {
        row(s, s);
    }

    println!("\n== H. Digits & latin inside CJK ==");
    row("fw digits", "返回５００错误");
    row("ascii digits", "返回500错误");
}
