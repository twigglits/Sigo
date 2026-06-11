//! The per-turn record structure used for benchmark logging and analysis.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::conversation::BackendKind;

/// Current schema version for turn records. Incremented on structural changes
/// to [`TurnRecord`] fields.
pub const SCHEMA_VERSION: u32 = 2;

/// A complete record of one turn through the Sigo pipeline.
///
/// Captures the English prompt, the Chinese bridge text, Claude's response,
/// local proxy token counts, Claude's reported usage, timing, and optional
/// English control-arm data.
#[allow(missing_docs)]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TurnRecord {
    pub schema_version: u32,
    pub session_id: Uuid,
    pub turn_index: u32,
    pub timestamp: DateTime<Utc>,
    pub backend: BackendKind,
    pub claude_model: String,
    pub translator_model: String,

    pub english_prompt: String,
    pub chinese_prompt: String,
    pub chinese_response: String,
    pub english_response: String,

    pub english_prompt_tokens_local: u32,
    pub chinese_prompt_tokens_local: u32,
    /// Proxy token count of the EN→ZH translation BEFORE whitespace compaction;
    /// `chinese_prompt`/`chinese_prompt_tokens_local` reflect what was actually
    /// sent. Equal values mean compaction changed nothing (or was skipped by
    /// the never-worse guard).
    #[serde(default)]
    pub chinese_prompt_tokens_precompact_local: u32,
    pub chinese_response_tokens_local: u32,

    pub chinese_prompt_tokens_reported: Option<u32>,
    pub chinese_response_tokens_reported: Option<u32>,
    pub cache_read_tokens_reported: Option<u32>,
    pub cache_write_tokens_reported: Option<u32>,

    pub chinese_cumulative_prompt_tokens_local: u32,
    pub english_cumulative_prompt_tokens_local: u32,

    pub english_control_run: Option<EnglishControlRun>,

    pub incomplete: bool,
    pub turn_errors: Vec<String>,

    pub translation_in_ms: u64,
    pub translation_out_ms_total: u64,
    pub translation_out_calls: u32,
    pub claude_ttft_ms: u64,
    pub claude_total_ms: u64,
    pub turn_total_ms: u64,
}

/// Result of the English control arm (parallel direct-EN run for comparison).
#[allow(missing_docs)]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnglishControlRun {
    pub english_response: String,
    pub prompt_tokens_reported: u32,
    pub response_tokens_reported: u32,
    #[serde(default)]
    pub cache_read_tokens_reported: Option<u32>,
    #[serde(default)]
    pub cache_write_tokens_reported: Option<u32>,
    pub duration_ms: u64,
}
