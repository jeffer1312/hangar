//! Leitura da captura de terminal para as decisões de envio e controle.
#![forbid(unsafe_code)]

mod composer;
mod controls;
mod proof;
mod screen;

pub use composer::{CapturedPane, Composer, agy_draft, compact, composer, own_residue, unstyle};
pub use controls::{
    KimiPermissionMenu, allowed_key, kimi_permission_menu, menu_cursor, multiple_choice, pi_cursor,
    same_question,
};
pub use proof::{ComposerSnapshot, Proof, stash_held};
pub use screen::{codex_menu_open, kimi_approval_open, kimi_picker_open, ready, screen};

#[cfg(test)]
mod tests;
