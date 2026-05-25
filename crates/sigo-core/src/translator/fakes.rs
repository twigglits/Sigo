use async_trait::async_trait;
use std::collections::HashMap;
use std::sync::Mutex;

use super::Translator;
use crate::conversation::Direction;
use crate::error::Result;

/// Deterministic translator for tests. Maps input strings to output strings per direction.
pub struct FakeTranslator {
    en_to_zh: Mutex<HashMap<String, String>>,
    zh_to_en: Mutex<HashMap<String, String>>,
}

impl FakeTranslator {
    pub fn new() -> Self {
        Self { en_to_zh: Mutex::new(HashMap::new()), zh_to_en: Mutex::new(HashMap::new()) }
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
        Ok(map.get(text).cloned().unwrap_or_else(|| format!("[mock {:?} {text}]", dir)))
    }
}
