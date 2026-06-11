//! Sigo CLI: binary entrypoint, clap argument parsing, rustyline REPL, and
//! subcommands (chat, doctor, bench, config-show).
//!
//! # Structure
//!
//! - [`cli`] — clap CLI argument definitions ([`Cli`](cli::Cli), [`Command`](cli::Command), [`BenchCommand`](cli::BenchCommand))
//! - [`commands`] — subcommand implementations
//! - [`repl`] — interactive REPL with slash-commands
//! - [`display`](crate::display) — turn-footer formatting (module is `pub(crate)`)

#![warn(missing_docs)]

pub mod cli;
pub mod commands;
pub(crate) mod display;
/// Terminal multiple-choice picker for AskUserQuestion passthrough.
pub mod picker;
/// Bridges backend question requests to the picker via the translator (SOP).
pub mod question_bridge;
/// Interactive REPL with slash-commands.
pub mod repl;
