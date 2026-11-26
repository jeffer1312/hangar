//! WhatsApp do Hangar: instala e mantém de pé o binário `hangar-whatsapp`, guarda o aceite do risco e serve
//! as rotas `/api/whatsapp*` do dono, que falam com o binário por HMAC no loopback. Lê o arquivo e cria
//! rascunhos de saída; a mensagem só sai quando o dono aprova o rascunho.
pub mod client;
pub mod control;
pub mod http;
pub mod install;
pub mod outbox;
pub mod release;

pub use http::{WhatsApp, api_path, router};
