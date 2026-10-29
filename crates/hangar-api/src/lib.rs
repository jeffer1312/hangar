//! Formatos da conversa que o backend manda aos aparelhos (`backend/app/models.py`), lidos e escritos
//! como o pydantic: mesmos nomes e ordem de campos, `null` no que falta, e leitura que ignora campo
//! desconhecido.
pub mod ask;
pub mod chat;
pub mod preview;
pub mod session;
pub mod state;
pub mod workspace;

/// Versão da API que o servidor fala; o app nativo compilado exige essa mesma. Suba ao mudar o que o app consome.
pub const CLIENT_API: u32 = 6;
