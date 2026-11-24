//! Chave de cliente usada na configuração do Jev, independente do endereço do proxy.
use crate::accounts::environment::AccountEnvironment;
use hangar_cliproxy_setup::config::{ReadError, read};

pub fn client_key(env: &AccountEnvironment) -> Option<String> {
    match read(&env.home) {
        Ok(config) => config?.client_keys.into_iter().next(),
        Err(ReadError::Io(error)) => {
            tracing::warn!(kind=?error.kind(), "Configuração de chave CLIProxy ilegível");
            None
        }
        Err(ReadError::InvalidYaml) => {
            tracing::warn!("Configuração de chave CLIProxy com YAML inválido");
            None
        }
    }
}
