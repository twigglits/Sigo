//! JSONL-based benchmark sink for persistent turn recording.
#![allow(missing_docs)]
use std::fs::OpenOptions;
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use super::{BenchmarkSink, TurnRecord};
use crate::error::{Result, SigoError};

pub struct JsonlSink {
    path: PathBuf,
    writer: Mutex<BufWriter<std::fs::File>>,
}

impl JsonlSink {
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref().to_path_buf();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let file = OpenOptions::new().create(true).append(true).open(&path)?;
        Ok(Self {
            path,
            writer: Mutex::new(BufWriter::new(file)),
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl BenchmarkSink for JsonlSink {
    fn record(&self, turn: &TurnRecord) -> Result<()> {
        let mut w = self
            .writer
            .lock()
            .map_err(|_| SigoError::Sink("lock poisoned".into()))?;
        serde_json::to_writer(&mut *w, turn)?;
        w.write_all(b"\n")?;
        w.flush()?;
        Ok(())
    }

    fn flush(&self) -> Result<()> {
        let mut w = self
            .writer
            .lock()
            .map_err(|_| SigoError::Sink("lock poisoned".into()))?;
        w.flush().map_err(|e| SigoError::Sink(e.to_string()))
    }
}

impl Drop for JsonlSink {
    fn drop(&mut self) {
        if let Ok(mut w) = self.writer.lock() {
            let _ = w.flush();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::conversation::BackendKind;
    use chrono::Utc;
    use uuid::Uuid;

    fn dummy_record() -> TurnRecord {
        TurnRecord {
            schema_version: 1,
            session_id: Uuid::nil(),
            turn_index: 0,
            timestamp: Utc::now(),
            backend: BackendKind::Api,
            claude_model: "claude-sonnet-4-6".into(),
            translator_model: "qwen3:14b".into(),
            english_prompt: "hello".into(),
            chinese_prompt: "你好".into(),
            chinese_response: "你好。".into(),
            english_response: "Hello.".into(),
            english_prompt_tokens_local: 3,
            chinese_prompt_tokens_local: 2,
            chinese_prompt_tokens_precompact_local: 2,
            chinese_response_tokens_local: 3,
            chinese_prompt_tokens_reported: Some(2),
            chinese_response_tokens_reported: Some(3),
            cache_read_tokens_reported: None,
            cache_write_tokens_reported: None,
            chinese_cumulative_prompt_tokens_local: 2,
            english_cumulative_prompt_tokens_local: 3,
            english_control_run: None,
            incomplete: false,
            turn_errors: vec![],
            translation_in_ms: 50,
            translation_out_ms_total: 80,
            translation_out_calls: 1,
            claude_ttft_ms: 200,
            claude_total_ms: 500,
            turn_total_ms: 800,
        }
    }

    fn tempfile_path(prefix: &str) -> PathBuf {
        let mut p = std::env::temp_dir();
        p.push(format!("{prefix}_{}.jsonl", uuid::Uuid::new_v4()));
        p
    }

    #[test]
    fn appends_jsonl_record_and_reads_back() {
        let tmp = tempfile_path("sigo_jsonl_test");
        let sink = JsonlSink::open(&tmp).unwrap();
        sink.record(&dummy_record()).unwrap();
        sink.record(&dummy_record()).unwrap();
        let contents = std::fs::read_to_string(&tmp).unwrap();
        assert_eq!(contents.lines().count(), 2);
        let first: TurnRecord = serde_json::from_str(contents.lines().next().unwrap()).unwrap();
        assert_eq!(first.english_prompt, "hello");
        std::fs::remove_file(&tmp).ok();
    }
}
