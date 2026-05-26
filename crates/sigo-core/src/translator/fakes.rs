use async_trait::async_trait;
use std::collections::HashMap;
use std::sync::Mutex;

use super::Translator;
use crate::conversation::Direction;
use crate::error::{Result, SigoError};

/// Deterministic translator for tests. Maps input strings to output strings per direction.
///
/// In lenient mode (default via `new()`), unknown inputs return a `[mock ...]` placeholder.
/// In strict mode (via `new_strict()`), unknown inputs return `Err(SigoError::Translator(...))`.
pub struct FakeTranslator {
    en_to_zh: Mutex<HashMap<String, String>>,
    zh_to_en: Mutex<HashMap<String, String>>,
    strict: bool,
}

impl FakeTranslator {
    pub fn new() -> Self {
        Self { en_to_zh: Mutex::new(HashMap::new()), zh_to_en: Mutex::new(HashMap::new()), strict: false }
    }
    /// Strict mode: returns `Err` for any input not registered via `add_en_to_zh` / `add_zh_to_en`.
    pub fn new_strict() -> Self {
        Self { en_to_zh: Mutex::new(HashMap::new()), zh_to_en: Mutex::new(HashMap::new()), strict: true }
    }
    pub fn add_en_to_zh(&self, en: &str, zh: &str) {
        self.en_to_zh.lock().unwrap().insert(en.to_string(), zh.to_string());
    }
    pub fn add_zh_to_en(&self, zh: &str, en: &str) {
        self.zh_to_en.lock().unwrap().insert(zh.to_string(), en.to_string());
    }
}

impl Default for FakeTranslator {
    fn default() -> Self { Self::new() }
}

#[async_trait]
impl Translator for FakeTranslator {
    async fn translate(&self, text: &str, dir: Direction) -> Result<String> {
        let map = match dir {
            Direction::EnToZh => self.en_to_zh.lock().unwrap(),
            Direction::ZhToEn => self.zh_to_en.lock().unwrap(),
        };
        match map.get(text).cloned() {
            Some(v) => Ok(v),
            None if self.strict => Err(SigoError::Translator(format!("no mapping for {:?} {:?}", dir, text))),
            None => Ok(format!("[mock {:?} {text}]", dir)),
        }
    }
}
