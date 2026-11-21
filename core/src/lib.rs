//! Trayward's logic, without the UI: what the AI coding agents' files and APIs
//! say (Claude Code, Codex CLI, GitHub Copilot), session states, limits and
//! their forecasts, alerts, statistics, kept state, MQTT, push, the update
//! check and the user's own extensions. Everything it needs from the system goes through [`os`], so the same
//! logic can serve an app on another system. The Windows app (the root crate)
//! adds the taskbar widget, the windows and the settings.

pub mod alerts;
pub mod codex;
pub mod copilot;
pub mod demo;
pub mod extensions;
pub mod forecast;
pub mod forge;
pub mod gpg;
pub mod hook;
pub mod i18n;
pub mod jsonl;
pub mod keep;
pub mod mailbox;
pub mod mqtt;
pub mod os;
pub mod push;
pub mod registry;
pub mod sessions;
pub mod setup;
pub mod stats;
pub mod store;
pub mod timefmt;
pub mod tracking;
pub mod transcripts;
pub mod update;
pub mod usage;
