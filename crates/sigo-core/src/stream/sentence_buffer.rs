#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Segment {
    /// Natural-language text that should be translated.
    Text(String),
    /// Code blocks or other raw passthrough content that must NOT be translated.
    Passthrough(String),
}

#[derive(Debug, PartialEq, Eq)]
enum State {
    Text,
    Code,
}

pub struct SentenceBuffer {
    state: State,
    text_buf: String,
    code_buf: String,
}

impl SentenceBuffer {
    pub fn new() -> Self {
        Self { state: State::Text, text_buf: String::new(), code_buf: String::new() }
    }

    /// Push streamed input and return any complete segments ready for downstream processing.
    /// Segments are returned in arrival order.
    pub fn push(&mut self, chunk: &str) -> Vec<Segment> {
        let mut out = Vec::new();
        for ch in chunk.chars() {
            match self.state {
                State::Text => self.feed_text(ch, &mut out),
                State::Code => self.feed_code(ch, &mut out),
            }
        }
        out
    }

    /// Flush any remaining buffered content at the end of a stream.
    /// An unclosed code fence at stream end is emitted as Passthrough.
    pub fn flush(&mut self) -> Vec<Segment> {
        let mut out = Vec::new();
        match self.state {
            State::Text => {
                if !self.text_buf.is_empty() {
                    out.push(Segment::Text(std::mem::take(&mut self.text_buf)));
                }
            }
            State::Code => {
                if !self.code_buf.is_empty() {
                    out.push(Segment::Passthrough(std::mem::take(&mut self.code_buf)));
                }
                self.state = State::Text;
            }
        }
        out
    }

    fn feed_text(&mut self, ch: char, out: &mut Vec<Segment>) {
        self.text_buf.push(ch);

        // Code fence opening: text_buf ends with "```" AND that "```" is at the start of a line.
        // We accept the fence if text_buf is exactly "```" (start of input) OR text_buf ends with "\n```".
        // Strip only the 3 backticks — any leading "\n" stays in text_buf and is emitted as Text.
        if self.text_buf.ends_with("```") {
            let is_fence_start = self.text_buf == "```" || self.text_buf.ends_with("\n```");
            if is_fence_start {
                let strip_at = self.text_buf.len() - 3;
                self.text_buf.truncate(strip_at);
                if !self.text_buf.is_empty() {
                    out.push(Segment::Text(std::mem::take(&mut self.text_buf)));
                }
                self.code_buf = "```".to_string();
                self.state = State::Code;
                return;
            }
        }

        // Sentence boundary: CJK terminators emit immediately.
        if matches!(ch, '。' | '！' | '？') {
            out.push(Segment::Text(std::mem::take(&mut self.text_buf)));
            return;
        }

        // ASCII !/? followed by whitespace — emit on the whitespace, retaining the whitespace.
        if matches!(ch, ' ' | '\n' | '\t') {
            // The whitespace is the most recent char in text_buf; look at the one before it.
            let mut iter = self.text_buf.chars().rev();
            let _ws = iter.next();
            if let Some(prev) = iter.next() {
                if prev == '!' || prev == '?' {
                    out.push(Segment::Text(std::mem::take(&mut self.text_buf)));
                    return;
                }
            }
        }

        // Paragraph boundary: two consecutive newlines.
        if self.text_buf.ends_with("\n\n") {
            out.push(Segment::Text(std::mem::take(&mut self.text_buf)));
        }
    }

    fn feed_code(&mut self, ch: char, out: &mut Vec<Segment>) {
        self.code_buf.push(ch);
        // Closing fence: "\n```\n" tail.
        if self.code_buf.ends_with("\n```\n") {
            out.push(Segment::Passthrough(std::mem::take(&mut self.code_buf)));
            self.state = State::Text;
        }
    }
}

impl Default for SentenceBuffer {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn collect_all(input: &str) -> Vec<Segment> {
        let mut buf = SentenceBuffer::new();
        let mut all = buf.push(input);
        all.extend(buf.flush());
        all
    }

    #[test]
    fn single_chinese_sentence_emits_on_period() {
        assert_eq!(collect_all("你好。"), vec![Segment::Text("你好。".to_string())]);
    }

    #[test]
    fn two_chinese_sentences() {
        assert_eq!(
            collect_all("你好。再见。"),
            vec![
                Segment::Text("你好。".to_string()),
                Segment::Text("再见。".to_string()),
            ]
        );
    }

    #[test]
    fn english_unfinished_sentence_held_in_buffer() {
        let mut buf = SentenceBuffer::new();
        let immediate = buf.push("Hello world");
        assert!(immediate.is_empty());
        let flushed = buf.flush();
        assert_eq!(flushed, vec![Segment::Text("Hello world".to_string())]);
    }

    #[test]
    fn english_exclamation_with_space_emits() {
        let segs = collect_all("Wow! Cool.");
        assert_eq!(segs, vec![
            Segment::Text("Wow! ".to_string()),
            Segment::Text("Cool.".to_string()),
        ]);
    }

    #[test]
    fn paragraph_break_emits() {
        let segs = collect_all("First paragraph\n\nSecond paragraph");
        assert_eq!(segs, vec![
            Segment::Text("First paragraph\n\n".to_string()),
            Segment::Text("Second paragraph".to_string()),
        ]);
    }

    #[test]
    fn code_fence_at_start() {
        let segs = collect_all("```\nfn main() {}\n```\n");
        assert_eq!(segs, vec![Segment::Passthrough("```\nfn main() {}\n```\n".to_string())]);
    }

    #[test]
    fn text_then_code_fence_then_text() {
        let segs = collect_all("看这段代码。\n```rust\nfn main() {}\n```\n好吗？");
        assert_eq!(segs, vec![
            Segment::Text("看这段代码。".to_string()),
            Segment::Text("\n".to_string()),
            Segment::Passthrough("```rust\nfn main() {}\n```\n".to_string()),
            Segment::Text("好吗？".to_string()),
        ]);
    }

    #[test]
    fn streaming_chunks_produce_same_result_as_one_shot() {
        let one_shot = collect_all("你好。世界！");
        let mut buf = SentenceBuffer::new();
        let mut chunks = vec![];
        for ch in "你好。世界！".chars() {
            chunks.extend(buf.push(&ch.to_string()));
        }
        chunks.extend(buf.flush());
        assert_eq!(one_shot, chunks);
    }

    #[test]
    fn unclosed_code_fence_emits_on_flush() {
        let segs = collect_all("```\nfn x() {\n  // no close");
        assert_eq!(segs, vec![Segment::Passthrough("```\nfn x() {\n  // no close".to_string())]);
    }

    #[test]
    fn multibyte_safe() {
        let mut buf = SentenceBuffer::new();
        let segs1 = buf.push("测");
        let segs2 = buf.push("试");
        let segs3 = buf.push("。");
        let flushed = buf.flush();
        assert!(segs1.is_empty());
        assert!(segs2.is_empty());
        assert_eq!(segs3, vec![Segment::Text("测试。".to_string())]);
        assert!(flushed.is_empty());
    }
}
