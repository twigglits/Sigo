//! HumanEval-format coding task corpus loading.
#![allow(missing_docs)]
use serde::{Deserialize, Serialize};
use std::path::Path;

use super::corpus::CorpusLoadError;

const BUNDLED: &[u8] = include_bytes!("../../assets/humaneval_sample.jsonl");

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CodingTask {
    pub task_id: String,
    #[serde(default = "default_category")]
    pub category: String,
    pub prompt: String,
    pub entry_point: String,
    pub test: String,
}

fn default_category() -> String {
    "coding-verifiable".into()
}

pub fn load_default_coding_corpus() -> Vec<CodingTask> {
    parse_jsonl("<bundled>", BUNDLED).expect("bundled coding corpus must parse")
}

pub fn load_coding_corpus(path: Option<&Path>) -> Result<Vec<CodingTask>, CorpusLoadError> {
    let Some(p) = path else {
        return Ok(load_default_coding_corpus());
    };
    let raw = std::fs::read(p).map_err(|source| CorpusLoadError::Io {
        path: p.display().to_string(),
        source,
    })?;
    parse_jsonl(&p.display().to_string(), &raw)
}

fn parse_jsonl(path: &str, raw: &[u8]) -> Result<Vec<CodingTask>, CorpusLoadError> {
    let s = std::str::from_utf8(raw).map_err(|e| CorpusLoadError::Io {
        path: path.to_string(),
        source: std::io::Error::new(std::io::ErrorKind::InvalidData, e),
    })?;
    let mut out = Vec::new();
    for (i, line) in s.lines().enumerate() {
        let t = line.trim();
        if t.is_empty() || t.starts_with('#') {
            continue;
        }
        let task: CodingTask = serde_json::from_str(t).map_err(|source| CorpusLoadError::Json {
            path: path.to_string(),
            line: i + 1,
            source,
        })?;
        out.push(task);
    }
    if out.is_empty() {
        Err(CorpusLoadError::Empty {
            path: path.to_string(),
        })
    } else {
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundled_corpus_has_100_complete_tasks() {
        let tasks = load_default_coding_corpus();
        assert_eq!(tasks.len(), 100);
        for t in &tasks {
            assert!(!t.task_id.is_empty());
            assert!(!t.entry_point.is_empty());
            assert!(
                t.test.contains("def check"),
                "task {} missing check()",
                t.task_id
            );
        }
    }

    #[test]
    fn malformed_line_reports_line_number() {
        let raw = b"{\"task_id\":\"x\",\"prompt\":\"p\",\"entry_point\":\"f\",\"test\":\"def check(c): pass\"}\n{bad\n";
        let err = parse_jsonl("t", raw).unwrap_err();
        match err {
            CorpusLoadError::Json { line, .. } => assert_eq!(line, 2),
            o => panic!("{o:?}"),
        }
    }
}
