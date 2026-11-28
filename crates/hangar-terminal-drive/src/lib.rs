//! Condução do envio e dos controles dos agentes com terminal.
#![forbid(unsafe_code)]

mod interaction;
mod terminal;

pub mod drive;

pub use hangar_other_terminal_policy::{kimi_approval_open, kimi_picker_open};
pub use interaction::{PendingInteraction, menu_interaction, read_pending_interaction};
pub use terminal::{OtherFrame, OtherIo, OtherTarget, OtherWriteGuard, failed, lifecycle_id};

#[cfg(test)]
mod drive_agy_tests;
#[cfg(test)]
mod drive_tests;
