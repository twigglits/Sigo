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
/// Interactive REPL with slash-commands.
pub mod repl;
