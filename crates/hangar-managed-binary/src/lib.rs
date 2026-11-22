//! Programa de fora que o Hangar baixa de uma release do GitHub, guarda por versão em `bin/<versão>/` e
//! mantém de pé como serviço do usuário (systemd, launchd ou tarefa do Windows). Cada consumidor diz o
//! repositório, os nomes publicados, o serviço e o prefixo dos códigos de erro.
pub mod hex;
pub mod jobs;
pub mod record;
pub mod release;
pub mod service;

/// Códigos de erro que a tela traduz, um conjunto por programa. Monte com [`error_codes!`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ErrorCodes {
    pub download_failed: &'static str,
    pub no_asset: &'static str,
    pub no_checksum: &'static str,
    pub hash: &'static str,
    pub binary_missing: &'static str,
    pub write_failed: &'static str,
    pub service_failed: &'static str,
}

/// `error_codes!("cliproxy")` dá `cliproxy_download_failed`, `cliproxy_sem_asset` e os demais.
/// Os sufixos são os que o CLIProxy já publicava: a tela e os textos dependem deles.
#[macro_export]
macro_rules! error_codes {
    ($prefix:literal) => {
        $crate::ErrorCodes {
            download_failed: concat!($prefix, "_download_failed"),
            no_asset: concat!($prefix, "_sem_asset"),
            no_checksum: concat!($prefix, "_sem_checksum"),
            hash: concat!($prefix, "_hash"),
            binary_missing: concat!($prefix, "_binary_missing"),
            write_failed: concat!($prefix, "_write_failed"),
            service_failed: concat!($prefix, "_servico_falhou"),
        }
    };
}

#[cfg(test)]
mod tests {
    #[test]
    fn codes_take_the_prefix() {
        let codes = crate::error_codes!("whatsapp");
        assert_eq!(
            [codes.download_failed, codes.no_asset, codes.no_checksum, codes.hash, codes.binary_missing, codes.write_failed, codes.service_failed],
            ["whatsapp_download_failed", "whatsapp_sem_asset", "whatsapp_sem_checksum", "whatsapp_hash",
                "whatsapp_binary_missing", "whatsapp_write_failed", "whatsapp_servico_falhou"]
        );
    }
}
