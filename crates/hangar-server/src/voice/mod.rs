//! Voz ao vivo: o organizador roda aqui; o aparelho dono da chamada só fala, ouve e atende a tela.
pub mod call;
pub mod computer;
pub mod controller;
pub mod hub;
pub mod jev;
pub mod machines;
pub mod observe;
pub mod organizer;
pub mod plan;
pub mod protocol;
pub mod routes;
pub mod rpc;
pub mod rules;
pub mod settings;
#[cfg(test)]
pub(crate) mod test_support;
pub mod usage;

/// Diário da voz. Nunca recebe fala, transcrição, texto de pedido nem argumentos de ferramenta.
pub(crate) fn log(text: impl AsRef<str>) { tracing::info!(target: "voice", "{}", text.as_ref()); }

/// O que o Jev, com certeza alta, diz sobre mandar a fala de um turno para a sessão.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum SendVerdict { Send, Block, Unsure }
