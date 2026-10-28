//! Acesso ao navegador existente da sessão, sem passar pelo Python.
pub mod access;
pub mod url;
pub mod ws;
pub mod layout;
mod cdp;
mod input;
mod frames;
mod shell;
mod login;
mod shutdown;
