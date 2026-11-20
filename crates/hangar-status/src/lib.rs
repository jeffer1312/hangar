//! Leituras da tela Status: processo do servidor, máquina, diário, sessões, restos e lista de
//! atenção. O servidor só liga a rota e o hub (`hangar-server/src/status.rs`).
pub mod attention;
pub mod diary;
pub mod dirs;
pub mod host;
pub mod leftovers;
pub mod process;
pub mod sessions;
