//! Sonho da memória: relê as transcrições de um projeto e corrige a pasta `memory/` do Claude Code.
//! Nada aqui depende do `hangar-server`: o modelo e a limpeza de segredos entram de fora.

pub mod budget;
pub mod config;
pub mod memory;
pub mod project;
pub mod render;
pub mod run;
pub mod state;
