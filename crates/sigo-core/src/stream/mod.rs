//! Streaming response segmentation for the ZH→EN translation pipeline.
//!
//! [`SentenceBuffer`] converts a streaming Claude ZH response into ordered
//! [`Segment`] items — either [`Segment::Text`] (sentences to be translated)
//! or [`Segment::Passthrough`] (code fences that must pass through unchanged).

pub mod sentence_buffer;

pub use sentence_buffer::{Segment, SentenceBuffer};
