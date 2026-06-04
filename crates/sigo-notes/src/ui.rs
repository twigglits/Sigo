use anyhow::Result;
use chrono::Local;
use crossterm::{
    event::{DisableMouseCapture, EnableMouseCapture, Event, EventStream, KeyCode, KeyEventKind},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use futures::StreamExt;
use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    prelude::CrosstermBackend,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, ListState, Paragraph, Wrap},
    Terminal,
};
use std::{io::Stdout, time::Duration};
use tokio::time::interval;

use crate::storage::{parse_remind_at, Store};

#[derive(Debug, Clone, PartialEq, Eq)]
enum Mode {
    Browse,
    AddText,
    AddRemind { text: String },
    EditRemind,
    ConfirmDelete,
}

struct App {
    store: Store,
    selected: usize,
    mode: Mode,
    input: String,
    status: String,
    should_quit: bool,
}

impl App {
    fn new() -> Result<Self> {
        let store = Store::load()?;
        Ok(Self {
            store,
            selected: 0,
            mode: Mode::Browse,
            input: String::new(),
            status: "按 a 新建 · r 设/改提醒 · 空格切换完成 · d 删除 · q 退出".into(),
            should_quit: false,
        })
    }

    fn clamp_selection(&mut self) {
        if self.store.notes.is_empty() {
            self.selected = 0;
        } else if self.selected >= self.store.notes.len() {
            self.selected = self.store.notes.len() - 1;
        }
    }

    fn check_reminders(&mut self) {
        let now = Local::now();
        let mut changed = false;
        for n in &mut self.store.notes {
            if n.done || n.notified {
                continue;
            }
            if let Some(t) = n.remind_at {
                if t <= now {
                    let _ = notify_rust::Notification::new()
                        .summary("Sigo 提醒")
                        .body(&n.text)
                        .show();
                    n.notified = true;
                    changed = true;
                    self.status = format!("⏰ 已触发提醒：{}", n.text);
                }
            }
        }
        if changed {
            let _ = self.store.save();
        }
    }
}

pub async fn run() -> Result<()> {
    let mut terminal = setup_terminal()?;
    let res = run_app(&mut terminal).await;
    restore_terminal(&mut terminal)?;
    res
}

fn setup_terminal() -> Result<Terminal<CrosstermBackend<Stdout>>> {
    enable_raw_mode()?;
    let mut out = std::io::stdout();
    execute!(out, EnterAlternateScreen, EnableMouseCapture)?;
    Ok(Terminal::new(CrosstermBackend::new(out))?)
}

fn restore_terminal(t: &mut Terminal<CrosstermBackend<Stdout>>) -> Result<()> {
    disable_raw_mode()?;
    execute!(t.backend_mut(), LeaveAlternateScreen, DisableMouseCapture)?;
    t.show_cursor()?;
    Ok(())
}

async fn run_app(terminal: &mut Terminal<CrosstermBackend<Stdout>>) -> Result<()> {
    let mut app = App::new()?;
    let mut events = EventStream::new();
    let mut tick = interval(Duration::from_secs(1));

    loop {
        app.check_reminders();
        terminal.draw(|f| draw(f, &app))?;
        if app.should_quit {
            break;
        }

        tokio::select! {
            _ = tick.tick() => {}
            maybe_ev = events.next() => {
                match maybe_ev {
                    Some(Ok(Event::Key(key))) if key.kind == KeyEventKind::Press => {
                        handle_key(&mut app, key.code);
                    }
                    Some(Err(_)) | None => break,
                    _ => {}
                }
            }
        }
    }
    let _ = app.store.save();
    Ok(())
}

fn handle_key(app: &mut App, code: KeyCode) {
    match app.mode.clone() {
        Mode::Browse => match code {
            KeyCode::Char('q') | KeyCode::Esc => app.should_quit = true,
            KeyCode::Char('a') => {
                app.input.clear();
                app.mode = Mode::AddText;
            }
            KeyCode::Char('d') => {
                if !app.store.notes.is_empty() {
                    app.mode = Mode::ConfirmDelete;
                }
            }
            KeyCode::Char('r') => {
                if !app.store.notes.is_empty() {
                    app.input.clear();
                    app.mode = Mode::EditRemind;
                }
            }
            KeyCode::Char(' ') => {
                if let Some(n) = app.store.notes.get_mut(app.selected) {
                    n.done = !n.done;
                    if n.done {
                        n.notified = true;
                    }
                    let _ = app.store.save();
                }
            }
            KeyCode::Down | KeyCode::Char('j') => {
                if !app.store.notes.is_empty() && app.selected + 1 < app.store.notes.len() {
                    app.selected += 1;
                }
            }
            KeyCode::Up | KeyCode::Char('k') => {
                if app.selected > 0 {
                    app.selected -= 1;
                }
            }
            _ => {}
        },
        Mode::AddText => match code {
            KeyCode::Esc => {
                app.input.clear();
                app.mode = Mode::Browse;
            }
            KeyCode::Enter => {
                if app.input.trim().is_empty() {
                    app.mode = Mode::Browse;
                } else {
                    let text = app.input.trim().to_string();
                    app.input.clear();
                    app.mode = Mode::AddRemind { text };
                }
            }
            KeyCode::Backspace => {
                app.input.pop();
            }
            KeyCode::Char(c) => app.input.push(c),
            _ => {}
        },
        Mode::AddRemind { text } => match code {
            KeyCode::Esc => {
                app.input.clear();
                app.mode = Mode::Browse;
            }
            KeyCode::Enter => {
                let remind = if app.input.trim().is_empty() {
                    None
                } else {
                    match parse_remind_at(&app.input, Local::now()) {
                        Some(t) => Some(t),
                        None => {
                            app.status = format!("无法识别时间：'{}'（用 10m / 2h / 14:30）", app.input);
                            return;
                        }
                    }
                };
                app.store.add(text, remind);
                let _ = app.store.save();
                app.input.clear();
                app.mode = Mode::Browse;
                app.status = "已添加".into();
            }
            KeyCode::Backspace => {
                app.input.pop();
            }
            KeyCode::Char(c) => app.input.push(c),
            _ => {}
        },
        Mode::EditRemind => match code {
            KeyCode::Esc => {
                app.input.clear();
                app.mode = Mode::Browse;
            }
            KeyCode::Enter => {
                let Some(n) = app.store.notes.get_mut(app.selected) else {
                    app.mode = Mode::Browse;
                    return;
                };
                if app.input.trim().is_empty() {
                    n.remind_at = None;
                    n.notified = false;
                    app.status = "已清除提醒".into();
                } else {
                    match parse_remind_at(&app.input, Local::now()) {
                        Some(t) => {
                            n.remind_at = Some(t);
                            n.notified = false;
                            app.status = "已更新提醒".into();
                        }
                        None => {
                            app.status = format!("无法识别时间：'{}'", app.input);
                            return;
                        }
                    }
                }
                let _ = app.store.save();
                app.input.clear();
                app.mode = Mode::Browse;
            }
            KeyCode::Backspace => {
                app.input.pop();
            }
            KeyCode::Char(c) => app.input.push(c),
            _ => {}
        },
        Mode::ConfirmDelete => match code {
            KeyCode::Char('y') | KeyCode::Enter => {
                app.store.remove(app.selected);
                let _ = app.store.save();
                app.clamp_selection();
                app.mode = Mode::Browse;
                app.status = "已删除".into();
            }
            _ => app.mode = Mode::Browse,
        },
    }
}

fn draw(f: &mut ratatui::Frame, app: &App) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(3),
            Constraint::Length(3),
            Constraint::Length(1),
        ])
        .split(f.area());

    draw_list(f, chunks[0], app);
    draw_input(f, chunks[1], app);
    draw_status(f, chunks[2], app);
}

fn draw_list(f: &mut ratatui::Frame, area: Rect, app: &App) {
    let now = Local::now();
    let items: Vec<ListItem> = app
        .store
        .notes
        .iter()
        .map(|n| {
            let check = if n.done { "[x]" } else { "[ ]" };
            let mut spans = vec![
                Span::styled(format!("{} ", check), Style::default().fg(Color::Cyan)),
                Span::raw(n.text.clone()),
            ];
            if let Some(t) = n.remind_at {
                let label = if n.notified {
                    format!("  ✓ 已提醒 {}", t.format("%m-%d %H:%M"))
                } else {
                    let diff = t - now;
                    let secs = diff.num_seconds();
                    let rel = if secs < 0 {
                        "已过期".to_string()
                    } else if secs < 60 {
                        format!("{}秒后", secs)
                    } else if secs < 3600 {
                        format!("{}分后", secs / 60)
                    } else if secs < 86400 {
                        format!("{}小时{}分后", secs / 3600, (secs % 3600) / 60)
                    } else {
                        format!("{}天后", secs / 86400)
                    };
                    format!("  ⏰ {} ({})", t.format("%m-%d %H:%M"), rel)
                };
                let color = if n.notified {
                    Color::DarkGray
                } else if t <= now {
                    Color::Red
                } else {
                    Color::Yellow
                };
                spans.push(Span::styled(label, Style::default().fg(color)));
            }
            let style = if n.done {
                Style::default()
                    .fg(Color::DarkGray)
                    .add_modifier(Modifier::CROSSED_OUT)
            } else {
                Style::default()
            };
            ListItem::new(Line::from(spans)).style(style)
        })
        .collect();

    let title = format!(" 笔记 ({}) ", app.store.notes.len());
    let list = List::new(items)
        .block(Block::default().borders(Borders::ALL).title(title))
        .highlight_style(
            Style::default()
                .bg(Color::Blue)
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("▶ ");

    let mut state = ListState::default();
    if !app.store.notes.is_empty() {
        state.select(Some(app.selected));
    }
    f.render_stateful_widget(list, area, &mut state);
}

fn draw_input(f: &mut ratatui::Frame, area: Rect, app: &App) {
    let (title, content) = match &app.mode {
        Mode::Browse => (
            " 提示 ",
            "a: 新建 · r: 设/改提醒 · 空格: 完成 · d: 删除 · ↑↓/jk: 移动 · q: 退出".to_string(),
        ),
        Mode::AddText => (" 新建笔记（Enter 继续，Esc 取消）", app.input.clone()),
        Mode::AddRemind { .. } => (
            " 提醒时间（可空；如 10m / 2h / 14:30 / 2026-05-26 14:00） ",
            app.input.clone(),
        ),
        Mode::EditRemind => (
            " 修改提醒（留空可清除；如 10m / 14:30） ",
            app.input.clone(),
        ),
        Mode::ConfirmDelete => (" 确认删除？", "按 y 确认，其他键取消".to_string()),
    };
    let p = Paragraph::new(content)
        .block(Block::default().borders(Borders::ALL).title(title))
        .wrap(Wrap { trim: false });
    f.render_widget(p, area);
}

fn draw_status(f: &mut ratatui::Frame, area: Rect, app: &App) {
    let p = Paragraph::new(app.status.clone()).style(Style::default().fg(Color::Gray));
    f.render_widget(p, area);
}
