use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    User,
    Assistant,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub role: Role,
    pub content: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Conversation {
    pub system: Option<String>,
    pub messages: Vec<Message>,
}

impl Conversation {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_system(system: impl Into<String>) -> Self {
        Self {
            system: Some(system.into()),
            messages: vec![],
        }
    }

    pub fn push_user(&mut self, content: impl Into<String>) {
        self.messages.push(Message {
            role: Role::User,
            content: content.into(),
        });
    }

    pub fn push_assistant(&mut self, content: impl Into<String>) {
        self.messages.push(Message {
            role: Role::Assistant,
            content: content.into(),
        });
    }

    pub fn last_user(&self) -> Option<&str> {
        self.messages
            .iter()
            .rev()
            .find(|m| m.role == Role::User)
            .map(|m| m.content.as_str())
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum BackendKind {
    Api,
    ClaudeCode,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum Direction {
    EnToZh,
    ZhToEn,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Usage {
    pub input_tokens: u32,
    pub output_tokens: u32,
    pub cache_read: Option<u32>,
    pub cache_write: Option<u32>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn push_and_last_user() {
        let mut c = Conversation::new();
        c.push_user("hello");
        c.push_assistant("hi");
        c.push_user("again");
        assert_eq!(c.last_user(), Some("again"));
        assert_eq!(c.messages.len(), 3);
    }

    #[test]
    fn with_system_sets_system() {
        let c = Conversation::with_system("you are a translator");
        assert_eq!(c.system.as_deref(), Some("you are a translator"));
        assert!(c.messages.is_empty());
    }
}
