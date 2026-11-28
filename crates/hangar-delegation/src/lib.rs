//! Sessão filha ligada: quando delegar a uma e o que a filha precisa saber de quem a abriu.
//!
//! Não abre sessão nem fala com o servidor: guarda o texto, as regras e a marca da filha sem pai que o
//! `hangar-server` liga.
#![deny(unsafe_code)]

pub mod answer;
pub mod cascade;
pub mod guidance;
pub mod orphans;
