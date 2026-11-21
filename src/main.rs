//! Trayward: your AI coding agents' usage limits and sessions in the taskbar. Made
//! first for Claude Code on Windows 11; Codex CLI and GitHub Copilot work too, and
//! more providers and other systems can follow (AGENTS.md, "Providers and platforms").
//!
//! Started without arguments it is the taskbar app. Claude Code runs the same
//! exe with `trayward-hook` for each hook event (see `hook.rs`). With
//! `--demo` it shows made-up sessions and usage instead (see `demo.rs`).
//! Trayward starts itself with `trayward-terminal` for the moment it looks at a
//! session's terminal (see `terminal.rs`).

#![windows_subsystem = "windows"]

// The logic, from the core crate: `crate::sessions`, `crate::usage` and so on
// work in this crate as if they were its own modules.
use trayward_core::{
    alerts, codex, copilot, demo, extensions, forecast, forge, gpg, hook, keep, mailbox, mqtt, push, registry,
    sessions, setup, stats, store, timefmt, tracking, transcripts, update, usage,
};

mod app;
mod bubble;
mod collector;
mod edit;
mod flyout;
mod gfx;
mod help;
mod hits;
mod i18n;
mod logo;
mod menu;
mod plan;
mod settings;
mod settings_window;
mod sound;
mod stats_window;
mod sysmon;
mod taskbars;
mod terminal;
mod theme;
mod tip;
mod uia;
mod vscode;
mod vscode_state;
mod widget;
mod win;

fn main() {
    if std::env::args().nth(1).is_some_and(|a| a == hook::ARG) {
        std::process::exit(hook::run(win::send_hook));
    }
    if std::env::args().nth(1).is_some_and(|a| a == terminal::ARG) {
        let args: Vec<String> = std::env::args().skip(2).collect();
        std::process::exit(terminal::helper(&args));
    }
    let demo = std::env::args().nth(1).as_deref() == Some("--demo");
    // Before the demo makes its files: a second demo would replace those of the running one.
    if !app::single_instance(demo) {
        return;
    }
    if demo {
        demo::start();
    }
    app::run();
    // The state is on disk (run flushed it). Ending the process the usual way
    // lets every loaded system library clean up first, which took about 4
    // seconds; there's nothing left that needs it.
    // SAFETY: ends this process; nothing of ours runs after this.
    unsafe {
        let _ = windows::Win32::System::Threading::TerminateProcess(
            windows::Win32::System::Threading::GetCurrentProcess(),
            0,
        );
    }
}
