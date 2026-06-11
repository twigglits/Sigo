//! Load prompt corpora from JSONL or plain-text files.
#![allow(missing_docs)]
use serde::{Deserialize, Serialize};
use std::path::Path;
use thiserror::Error;

const DEFAULT_CORPUS_BYTES: &[u8] = include_bytes!("../../assets/default_corpus.jsonl");

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CorpusEntry {
    pub category: String,
    pub prompt: String,
}

#[derive(Debug, Error)]
pub enum CorpusLoadError {
    #[error("read {path}: {source}")]
    Io {
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error("{path}:{line}: malformed json: {source}")]
    Json {
        path: String,
        line: usize,
        #[source]
        source: serde_json::Error,
    },
    #[error("{path}: empty corpus (no non-blank entries)")]
    Empty { path: String },
}

pub fn load_default_corpus() -> Vec<CorpusEntry> {
    parse_jsonl("<bundled>", DEFAULT_CORPUS_BYTES)
        .expect("bundled corpus must always parse — caught by unit tests")
}

pub fn load_corpus(path: Option<&Path>) -> Result<Vec<CorpusEntry>, CorpusLoadError> {
    let Some(p) = path else {
        return Ok(load_default_corpus());
    };
    let raw = std::fs::read(p).map_err(|source| CorpusLoadError::Io {
        path: p.display().to_string(),
        source,
    })?;
    let path_str = p.display().to_string();
    if looks_like_jsonl(&raw) {
        parse_jsonl(&path_str, &raw)
    } else {
        parse_plain_text(&path_str, &raw)
    }
}

fn looks_like_jsonl(raw: &[u8]) -> bool {
    raw.iter()
        .find(|b| !b.is_ascii_whitespace())
        .map(|b| *b == b'{')
        .unwrap_or(false)
}

fn parse_jsonl(path: &str, raw: &[u8]) -> Result<Vec<CorpusEntry>, CorpusLoadError> {
    let s = std::str::from_utf8(raw).map_err(|e| CorpusLoadError::Io {
        path: path.to_string(),
        source: std::io::Error::new(std::io::ErrorKind::InvalidData, e),
    })?;
    let mut out = Vec::new();
    for (i, line) in s.lines().enumerate() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let entry: CorpusEntry =
            serde_json::from_str(trimmed).map_err(|source| CorpusLoadError::Json {
                path: path.to_string(),
                line: i + 1,
                source,
            })?;
        out.push(entry);
    }
    if out.is_empty() {
        Err(CorpusLoadError::Empty {
            path: path.to_string(),
        })
    } else {
        Ok(out)
    }
}

fn parse_plain_text(path: &str, raw: &[u8]) -> Result<Vec<CorpusEntry>, CorpusLoadError> {
    let s = std::str::from_utf8(raw).map_err(|e| CorpusLoadError::Io {
        path: path.to_string(),
        source: std::io::Error::new(std::io::ErrorKind::InvalidData, e),
    })?;
    let mut out = Vec::new();
    for line in s.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        out.push(CorpusEntry {
            category: "general".into(),
            prompt: trimmed.to_string(),
        });
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
    use std::io::Write;

    #[test]
    fn bundled_default_corpus_parses_and_has_expected_size() {
        let entries = load_default_corpus();
        assert_eq!(
            entries.len(),
            30,
            "default corpus should ship exactly 30 entries"
        );
        let categories: std::collections::BTreeSet<&str> =
            entries.iter().map(|e| e.category.as_str()).collect();
        assert!(categories.contains("coding-short"));
        assert!(categories.contains("prose"));
    }

    #[test]
    fn jsonl_path_parses_correctly() {
        let mut f = tempfile::NamedTempFile::new().unwrap();
        writeln!(f, r#"{{"category":"a","prompt":"hello"}}"#).unwrap();
        writeln!(f).unwrap();
        writeln!(f, r#"# a comment"#).unwrap();
        writeln!(f, r#"{{"category":"b","prompt":"world"}}"#).unwrap();
        let v = load_corpus(Some(f.path())).unwrap();
        assert_eq!(
            v,
            vec![
                CorpusEntry {
                    category: "a".into(),
                    prompt: "hello".into()
                },
                CorpusEntry {
                    category: "b".into(),
                    prompt: "world".into()
                },
            ]
        );
    }

    #[test]
    fn plain_text_path_falls_back_to_general_category() {
        let mut f = tempfile::NamedTempFile::new().unwrap();
        writeln!(f, "first prompt").unwrap();
        writeln!(f, "# skip this comment").unwrap();
        writeln!(f).unwrap();
        writeln!(f, "second prompt").unwrap();
        let v = load_corpus(Some(f.path())).unwrap();
        assert_eq!(v.len(), 2);
        assert_eq!(v[0].category, "general");
        assert_eq!(v[0].prompt, "first prompt");
        assert_eq!(v[1].prompt, "second prompt");
    }

    #[test]
    fn malformed_jsonl_line_reports_line_number() {
        let mut f = tempfile::NamedTempFile::new().unwrap();
        writeln!(f, r#"{{"category":"a","prompt":"ok"}}"#).unwrap();
        writeln!(f, r#"{{"category":"b","#).unwrap();
        let err = load_corpus(Some(f.path())).unwrap_err();
        match err {
            CorpusLoadError::Json { line, .. } => assert_eq!(line, 2),
            other => panic!("expected Json error on line 2, got {other:?}"),
        }
    }

    #[test]
    fn empty_corpus_is_an_error() {
        let mut f = tempfile::NamedTempFile::new().unwrap();
        writeln!(f, "# comment only").unwrap();
        writeln!(f).unwrap();
        let err = load_corpus(Some(f.path())).unwrap_err();
        assert!(matches!(err, CorpusLoadError::Empty { .. }));
    }

    #[test]
    fn missing_file_returns_io_error() {
        let err =
            load_corpus(Some(std::path::Path::new("/nonexistent/path/xyzzy.jsonl"))).unwrap_err();
        assert!(matches!(err, CorpusLoadError::Io { .. }));
    }

    #[test]
    fn no_path_returns_bundled_corpus() {
        let v = load_corpus(None).unwrap();
        assert_eq!(v.len(), 30);
    }
}
