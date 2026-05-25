use sigo_core::{ClaudeTokenizer, Tokenizer};

#[test]
fn tokenizer_token_counts() {
    let tokenizer = ClaudeTokenizer::new().unwrap();
    
    let english = "Hello, world!";
    let en_count = tokenizer.count_tokens(english).unwrap();
    println!("'{}' -> {} tokens", english, en_count);
    
    let chinese = "你好，世界！";
    let zh_count = tokenizer.count_tokens(chinese).unwrap();
    println!("'{}' -> {} tokens", chinese, zh_count);
    
    let longer = "The quick brown fox jumps over the lazy dog.";
    let longer_count = tokenizer.count_tokens(longer).unwrap();
    println!("'{}' -> {} tokens", longer, longer_count);
    
    let short = "Hi.";
    let short_count = tokenizer.count_tokens(short).unwrap();
    println!("'{}' -> {} tokens", short, short_count);
}
