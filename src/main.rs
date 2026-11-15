//! Trayward: Claude Code usage limits and session status in the Windows 11 taskbar.
//!
//! Started without arguments it is the taskbar app. Claude Code runs the same
//! exe with `trayward-hook` for each hook event (see `hook.rs`). With
//! `--demo` it shows made-up sessions and usage instead (see `demo.rs`).

#![windows_subsystem = "windows"]

mod alerts;
mod app;
mod bubble;
mod codex;
mod collector;
mod copilot;
mod demo;
mod flyout;
mod forecast;
mod forge;
mod gfx;
mod gpg;
mod hits;
mod hook;
mod http;
mod i18n;
mod jsonl;
mod keep;
mod logo;
mod mailbox;
mod menu;
mod mqtt;
mod plan;
mod push;
mod registry;
mod sessions;
mod settings;
mod settings_window;
mod setup;
mod sound;
mod stats;
mod stats_window;
mod store;
mod sysmon;
mod taskbars;
mod terminal;
mod theme;
mod timefmt;
mod tip;
mod tls;
mod tracking;
mod transcripts;
mod uia;
mod update;
mod usage;
mod vscode;
mod vscode_state;
mod widget;
mod win;

fn main() {
    if std::env::args().nth(1).is_some_and(|a| a == hook::ARG || a == hook::LEGACY_ARG) {
        std::process::exit(hook::run());
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
