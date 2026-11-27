//! Sessão filha ligada: quando delegar a uma e o que a filha precisa saber de quem a abriu.
//!
//! Não abre sessão nem fala com o servidor: guarda o texto e as regras que o `hangar-server` liga.
#![deny(unsafe_code)]

pub mod cascade;
pub mod guidance;
