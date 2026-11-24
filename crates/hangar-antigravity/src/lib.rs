//! O Antigravity CLI (`agy`) do Google como agente do Hangar: caminhos e ambiente de cada conta, versão mínima,
//! gancho de estado no `hooks.json`, cliente do Agent Client Protocol (`agy acp`) e o login por código colado.
//! Nada aqui lê o token do `agy` nem fala com o Google por conta própria: quem autentica é sempre o binário oficial.

pub mod acp;
pub mod env;
pub mod hooks;
pub mod login;
pub mod paths;
pub mod probe;
pub mod settings;
pub mod usage;
pub mod version;

pub use paths::Account;
