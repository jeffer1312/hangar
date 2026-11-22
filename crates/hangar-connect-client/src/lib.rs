//! Cliente do serviço do Hangar Connect e do login de convidado, sem servidor local: o app nativo entra
//! por aqui, e o servidor registra a própria máquina pelo mesmo código. Espelha `packages/core/src`
//! (`connectAccount.ts` e `syncLogin.ts`): mesma ordem de pedidos, mesmos códigos de erro.
//!
//! Sem `hangar-core`: o nativo depende deste crate. Tudo o que sai do processo passa por [`Transport`].
#![deny(unsafe_code)]

mod account;
mod client;
mod error;
mod redeem;
mod sync_login;
mod transport;

pub mod crypto;

#[cfg(test)]
mod fake;
#[cfg(test)]
mod tests;

pub use account::{
    AccountDevice, AccountSession, ConnectMachine, DeviceKey, DeviceStatus, MachineRegistration, SignInResult, install_code,
    normalize_name, valid_guest_user,
};
#[cfg(test)]
pub(crate) use account::NameLookup;
pub use client::{CONNECT_SERVICE_URL, ConnectClient};
pub use error::{Code, Error};
pub use redeem::MachineEntry;
pub use sync_login::VaultServer;
pub use transport::{Method, ReqwestTransport, Request, Response, Transport};
