//! Release do CLIProxyAPI no GitHub, baixada e conferida pelo `hangar-managed-binary`.
use std::path::{Path, PathBuf};

use hangar_managed_binary::ErrorCodes;
use hangar_managed_binary::release::{ArchNames, Binary};
pub use hangar_managed_binary::release::{Error, Github, Platform, Release};

pub const REPO: &str = "router-for-me/CLIProxyAPI";

/// O pacote com plugins; o `_no-plugin` só existe para algumas plataformas. O projeto escreve `aarch64`.
pub const BINARY: Binary = Binary {
    repo: REPO,
    archive_prefix: "CLIProxyAPI",
    program: "cli-proxy-api",
    arch: ArchNames { x86_64: "amd64", aarch64: "aarch64" },
};

pub const CODES: ErrorCodes = hangar_managed_binary::error_codes!("cliproxy");

pub fn code(error: &Error) -> &'static str {
    error.code(&CODES)
}

pub async fn latest(github: &Github) -> Result<Release, Error> {
    BINARY.latest(github).await
}

/// Baixa uma vez por versão; devolve o caminho do programa em `<dir>/bin/<versão>/`.
pub async fn fetch(github: &Github, release: &Release, platform: Platform, dir: &Path) -> Result<PathBuf, Error> {
    BINARY.fetch(github, release, platform, dir).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn asset_names_follow_the_published_release() {
        let p = |os, arch| Platform { os, arch };
        assert_eq!(BINARY.asset(p("linux", "x86_64"), "8.0.79").unwrap(), "CLIProxyAPI_8.0.79_linux_amd64.tar.gz");
        assert_eq!(BINARY.asset(p("macos", "aarch64"), "8.0.79").unwrap(), "CLIProxyAPI_8.0.79_darwin_aarch64.tar.gz");
        let windows = p("windows", "x86_64");
        assert_eq!((BINARY.asset(windows, "8.0.79").unwrap().as_str(), BINARY.program_file(windows).as_str()),
            ("CLIProxyAPI_8.0.79_windows_amd64.zip", "cli-proxy-api.exe"));
        assert_eq!(BINARY.program_file(p("linux", "x86_64")), "cli-proxy-api");
        assert_eq!(BINARY.asset(p("freebsd", "x86_64"), "1"), Err(Error::UnsupportedPlatform));
        assert_eq!(Release { tag: "v8.0.79".into() }.version(), "8.0.79");
    }

    #[test]
    fn error_codes_stay_the_ones_the_screen_knows() {
        let codes = [Error::Network(String::new()), Error::UnsupportedPlatform, Error::NoChecksum, Error::Hash,
            Error::BinaryMissing, Error::BinaryTooLarge, Error::Write(String::new())].map(|e| code(&e));
        assert_eq!(codes, ["cliproxy_download_failed", "cliproxy_sem_asset", "cliproxy_sem_checksum", "cliproxy_hash",
            "cliproxy_binary_missing", "cliproxy_binary_too_large", "cliproxy_write_failed"]);
        assert_eq!(CODES.service_failed, "cliproxy_servico_falhou");
    }
}
