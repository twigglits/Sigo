use anyhow::{Context, Result};
use chrono::{DateTime, Local};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Note {
    pub id: u64,
    pub text: String,
    pub created_at: DateTime<Local>,
    pub remind_at: Option<DateTime<Local>>,
    pub notified: bool,
    pub done: bool,
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct Store {
    pub next_id: u64,
    pub notes: Vec<Note>,
}

pub fn data_path() -> Result<PathBuf> {
    let mut p = dirs::data_dir().context("no data dir")?;
    p.push("sigo-notes");
    fs::create_dir_all(&p)?;
    p.push("notes.json");
    Ok(p)
}

impl Store {
    pub fn load() -> Result<Self> {
        let p = data_path()?;
        if !p.exists() {
            return Ok(Self::default());
        }
        let bytes = fs::read(&p)?;
        if bytes.is_empty() {
            return Ok(Self::default());
        }
        Ok(serde_json::from_slice(&bytes)?)
    }

    pub fn save(&self) -> Result<()> {
        let p = data_path()?;
        let tmp = p.with_extension("json.tmp");
        fs::write(&tmp, serde_json::to_vec_pretty(self)?)?;
        fs::rename(tmp, p)?;
        Ok(())
    }

    pub fn add(&mut self, text: String, remind_at: Option<DateTime<Local>>) -> &Note {
        self.next_id += 1;
        let note = Note {
            id: self.next_id,
            text,
            created_at: Local::now(),
            remind_at,
            notified: false,
            done: false,
        };
        self.notes.push(note);
        self.notes.last().unwrap()
    }

    pub fn remove(&mut self, idx: usize) {
        if idx < self.notes.len() {
            self.notes.remove(idx);
        }
    }
}

/// Parse strings like "10m", "2h", "1d30m", "1h15m", or "HH:MM" (today/tomorrow),
/// or "YYYY-MM-DD HH:MM".
pub fn parse_remind_at(input: &str, now: DateTime<Local>) -> Option<DateTime<Local>> {
    let s = input.trim();
    if s.is_empty() {
        return None;
    }

    // Absolute "YYYY-MM-DD HH:MM"
    if let Ok(dt) = chrono::NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M") {
        return dt.and_local_timezone(Local).single();
    }

    // "HH:MM" — today, or tomorrow if already past
    if let Ok(t) = chrono::NaiveTime::parse_from_str(s, "%H:%M") {
        let today = now.date_naive().and_time(t);
        let dt = today.and_local_timezone(Local).single()?;
        if dt <= now {
            return Some(dt + chrono::Duration::days(1));
        }
        return Some(dt);
    }

    // Relative duration: collect (number, unit) pairs
    let mut total_secs: i64 = 0;
    let mut num = String::new();
    let mut any = false;
    for c in s.chars() {
        if c.is_ascii_digit() {
            num.push(c);
        } else if c.is_whitespace() {
            continue;
        } else {
            let n: i64 = num.parse().ok()?;
            num.clear();
            let mult = match c {
                's' => 1,
                'm' => 60,
                'h' => 3600,
                'd' => 86400,
                _ => return None,
            };
            total_secs += n * mult;
            any = true;
        }
    }
    if !num.is_empty() {
        // bare number → assume minutes
        let n: i64 = num.parse().ok()?;
        total_secs += n * 60;
        any = true;
    }
    if !any {
        return None;
    }
    Some(now + chrono::Duration::seconds(total_secs))
}
