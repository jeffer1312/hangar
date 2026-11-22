//! Release do `hangar-whatsapp` no GitHub (contrato C3), baixada e conferida pelo `hangar-managed-binary`.
use std::path::Path;
use std::time::Duration;

use hangar_managed_binary::ErrorCodes;
use hangar_managed_binary::release::{ArchNames, Binary};
use serde_json::Value;

pub use hangar_managed_binary::release::{Github, Platform, Release};

use crate::install::Failure;

pub const REPO: &str = "jeffer1312/hangar-whatsapp";

pub const BINARY: Binary = Binary {
    repo: REPO,
    archive_prefix: "hangar-whatsapp",
    program: "hangar-whatsapp",
    arch: ArchNames { x86_64: "amd64", aarch64: "arm64" },
};

pub const CODES: ErrorCodes = hangar_managed_binary::error_codes!("whatsapp");

/// Só a mesma versão maior: uma `1.x` pode ter mudado o contrato C2.
pub const MAJOR: u64 = 0;
/// Binário com API menor que esta não fala o C2 que o Hangar usa.
pub const MIN_API: u32 = 1;

const LIST_TIMEOUT: Duration = Duration::from_secs(10);
const VERSION_TIMEOUT: Duration = Duration::from_secs(10);

/// A release publicada mais nova da versão maior [`MAJOR`].
///
/// # Errors
///
/// `whatsapp_download_failed` sem resposta do GitHub; `whatsapp_no_release` sem release da versão maior.
pub async fn latest(github: &Github) -> Result<Release, Failure> {
    let url = format!("{}/repos/{REPO}/releases?per_page=100", github.api);
    let failed = |e: String| Failure::new(CODES.download_failed, format!("{url}: {e}"));
    let client = hangar_core::net::http_client().timeout(LIST_TIMEOUT).build().map_err(|e| failed(e.to_string()))?;
    let body: Value = client.get(&url)
        .header("accept", "application/vnd.github+json")
        .header("user-agent", "hangar")
        .send()
        .await
        .and_then(reqwest::Response::error_for_status)
        .map_err(|e| failed(e.to_string()))?
        .json()
        .await
        .map_err(|e| failed(e.to_string()))?;
    pick(&body).ok_or_else(|| Failure::new("whatsapp_no_release", format!("nenhuma release {MAJOR}.x publicada em {REPO}")))
}

/// A maior tag semver da versão maior, sem rascunho nem pré-release.
pub fn pick(releases: &Value) -> Option<Release> {
    releases.as_array()?
        .iter()
        .filter(|r| r["draft"] != true && r["prerelease"] != true)
        .filter_map(|r| r["tag_name"].as_str())
        .filter_map(|tag| semver_of(tag).map(|v| (v, tag)))
        .filter(|(v, _)| v.major == MAJOR && v.pre.is_empty())
        .max_by(|a, b| a.0.cmp(&b.0))
        .map(|(_, tag)| Release { tag: tag.to_owned() })
}

pub fn newer(candidate: &str, installed: &str) -> bool {
    matches!((semver_of(candidate), semver_of(installed)), (Some(c), Some(i)) if c > i)
}

/// `hangar-whatsapp <versão> (api <n>)`, a saída do `--version`.
pub fn parse_version(output: &str) -> Option<(String, u32)> {
    let rest = output.trim().strip_prefix("hangar-whatsapp ")?;
    let (version, api) = rest.split_once(" (api ")?;
    let api = api.strip_suffix(')')?.parse().ok()?;
    (!version.is_empty()).then(|| (version.to_owned(), api))
}

/// Roda `--version` do programa baixado: recusa API menor que [`MIN_API`]. Devolve a versão que ele diz.
///
/// # Errors
///
/// `whatsapp_version_unreadable` quando o programa não roda ou não responde no formato; `whatsapp_api_too_old`.
pub async fn check(binary: &Path) -> Result<String, Failure> {
    let unreadable = |detail: String| Failure::new("whatsapp_version_unreadable", format!("{} --version: {detail}", binary.display()));
    let mut command = tokio::process::Command::new(binary);
    command.arg("--version").stdin(std::process::Stdio::null()).kill_on_drop(true);
    #[cfg(windows)]
    command.creation_flags(0x0800_0000);
    let output = tokio::time::timeout(VERSION_TIMEOUT, command.output()).await
        .map_err(|_| unreadable("não respondeu".into()))?
        .map_err(|e| unreadable(e.to_string()))?;
    let text = String::from_utf8_lossy(&output.stdout);
    let (version, api) = parse_version(&text).ok_or_else(|| unreadable(format!("saída inesperada {:?}", text.trim())))?;
    if api < MIN_API {
        return Err(Failure::new("whatsapp_api_too_old", format!("hangar-whatsapp {version} fala a API {api}; o Hangar precisa da {MIN_API}")));
    }
    Ok(version)
}

fn semver_of(tag: &str) -> Option<semver::Version> {
    semver::Version::parse(tag.trim_start_matches('v')).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn the_newest_release_of_the_same_major_wins() {
        let releases = json!([
            {"tag_name": "v1.0.0", "draft": false, "prerelease": false},
            {"tag_name": "v0.3.0-rc1", "draft": false, "prerelease": true},
            {"tag_name": "v0.2.9", "draft": true, "prerelease": false},
            {"tag_name": "v0.2.1", "draft": false, "prerelease": false},
            {"tag_name": "v0.10.0", "draft": false, "prerelease": false},
            {"tag_name": "nightly", "draft": false, "prerelease": false},
        ]);
        assert_eq!(pick(&releases), Some(Release { tag: "v0.10.0".into() }), "semver, não ordem de texto");
        assert_eq!(pick(&json!([{"tag_name": "v1.2.0"}])), None, "outra versão maior");
        assert_eq!(pick(&json!({"message": "rate limit"})), None);
        assert!(newer("v0.10.0", "v0.9.1"));
        assert!(!newer("v0.1.0", "v0.1.0"));
    }

    #[test]
    fn the_version_line_carries_the_api() {
        assert_eq!(parse_version("hangar-whatsapp 0.1.0 (api 1)\n"), Some(("0.1.0".into(), 1)));
        assert_eq!(parse_version("hangar-whatsapp dev (api 2)"), Some(("dev".into(), 2)));
        assert_eq!(parse_version("hangar-whatsapp 0.1.0"), None, "sem api");
        assert_eq!(parse_version("outro 0.1.0 (api 1)"), None);
    }

    #[test]
    fn asset_names_follow_the_published_release() {
        let p = |os, arch| Platform { os, arch };
        assert_eq!(BINARY.asset(p("linux", "x86_64"), "0.1.0").unwrap(), "hangar-whatsapp_0.1.0_linux_amd64.tar.gz");
        assert_eq!(BINARY.asset(p("macos", "aarch64"), "0.1.0").unwrap(), "hangar-whatsapp_0.1.0_darwin_arm64.tar.gz");
        assert_eq!(BINARY.asset(p("windows", "x86_64"), "0.1.0").unwrap(), "hangar-whatsapp_0.1.0_windows_amd64.zip");
        assert_eq!(CODES.service_failed, "whatsapp_servico_falhou");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn an_old_api_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let program = dir.path().join("hangar-whatsapp");
        for (api, ok) in [(0, false), (1, true)] {
            std::fs::write(&program, format!("#!/bin/sh\necho 'hangar-whatsapp 0.1.0 (api {api})'\n")).unwrap();
            std::fs::set_permissions(&program, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
            let got = check(&program).await;
            if ok {
                assert_eq!(got.unwrap(), "0.1.0");
            } else {
                assert_eq!(got.unwrap_err().code, "whatsapp_api_too_old");
            }
        }
    }
}
