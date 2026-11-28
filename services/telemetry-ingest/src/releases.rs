//! Versões publicadas do Hangar por canal e sistema, lidas dos releases públicos do GitHub, para o painel
//! dizer quantas versões cada máquina está atrás. Só em memória: leitura que falha mantém a última boa.
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::{Deserialize, Serialize};

/// Dois pedidos a cada 10 min cabem no teto da API sem token (60 por hora por IP).
const EVERY: Duration = Duration::from_secs(10 * 60);
const TIMEOUT: Duration = Duration::from_secs(20);
/// O canal gravado no pacote da máquina e a tag do release que o publica.
const CHANNELS: [(&str, &str); 2] = [("latest", "package-latest"), ("beta", "package-beta")];
const API: &str = "https://api.github.com/repos/jeffer1312/hangar/releases/tags/";

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Release {
    pub version: String,
    pub os: String,
    /// `updated_at` do arquivo no release, em UTC (ISO 8601).
    pub at: String,
}

#[derive(Default, Serialize)]
struct Published {
    /// Só os canais já lidos alguma vez; em cada um, por sistema, da versão mais nova para a mais velha.
    channels: BTreeMap<&'static str, Vec<Release>>,
    /// A falha da última leitura, por canal; `None` quando todos foram lidos.
    error: Option<String>,
}

#[derive(Default)]
pub struct Releases(Mutex<Published>);

impl Releases {
    pub fn snapshot(&self) -> serde_json::Value { serde_json::to_value(&*self.0.lock().unwrap()).unwrap_or_default() }

    fn apply(&self, results: Vec<(&'static str, Result<Vec<Release>, String>)>) {
        let mut published = self.0.lock().unwrap();
        let mut errors = Vec::new();
        for (channel, result) in results {
            match result {
                Ok(releases) => { published.channels.insert(channel, releases); }
                Err(error) => errors.push(format!("{channel}: {error}")),
            }
        }
        published.error = (!errors.is_empty()).then(|| errors.join("; "));
    }
}

#[derive(Deserialize)]
struct GhRelease {
    assets: Vec<GhAsset>,
}

#[derive(Deserialize)]
struct GhAsset {
    name: String,
    updated_at: String,
}

/// Versão e sistema de `hangar-<versão>-<os>-<arch>.tar.gz`; os outros arquivos do release ficam de fora.
fn parse_asset(name: &str) -> Option<(&str, &str)> {
    let rest = name.strip_prefix("hangar-")?.strip_suffix(".tar.gz")?;
    let mut parts = rest.rsplitn(3, '-');
    let (_arch, os, version) = (parts.next()?, parts.next()?, parts.next()?);
    let numeric = !version.is_empty() && version.split('.').all(|s| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()));
    (numeric && !os.is_empty()).then_some((version, os))
}

/// Por segmento numérico: `0.1.0.9` é mais velha que `0.1.0.10`.
fn version_key(version: &str) -> Vec<u64> { version.split('.').map(|s| s.parse().unwrap_or(0)).collect() }

/// Uma entrada por sistema e versão (as arquiteturas viram uma), com a hora mais recente entre elas.
fn releases_of(body: &str) -> Result<Vec<Release>, serde_json::Error> {
    let release: GhRelease = serde_json::from_str(body)?;
    let mut seen: BTreeMap<(String, String), String> = BTreeMap::new();
    for asset in release.assets {
        if let Some((version, os)) = parse_asset(&asset.name) {
            let at = seen.entry((os.to_owned(), version.to_owned())).or_default();
            if asset.updated_at > *at {
                *at = asset.updated_at;
            }
        }
    }
    let mut releases: Vec<Release> = seen.into_iter().map(|((os, version), at)| Release { version, os, at }).collect();
    releases.sort_by(|a, b| a.os.cmp(&b.os).then_with(|| version_key(&b.version).cmp(&version_key(&a.version))));
    Ok(releases)
}

fn fetch(agent: &ureq::Agent, tag: &str) -> Result<Vec<Release>, String> {
    let mut response = agent
        .get(format!("{API}{tag}"))
        .header("accept", "application/vnd.github+json")
        .call()
        .map_err(|e| e.to_string())?;
    let body = response.body_mut().read_to_string().map_err(|e| e.to_string())?;
    releases_of(&body).map_err(|e| e.to_string())
}

pub async fn watch(releases: Arc<Releases>) {
    let config = ureq::Agent::config_builder().timeout_global(Some(TIMEOUT)).user_agent("hangar-telemetry-ingest").build();
    let agent = ureq::Agent::new_with_config(config);
    let mut every = tokio::time::interval(EVERY);
    loop {
        every.tick().await;
        let agent = agent.clone();
        let read = tokio::task::spawn_blocking(move || CHANNELS.map(|(channel, tag)| (channel, fetch(&agent, tag))).into_iter().collect());
        match read.await {
            Ok(results) => {
                releases.apply(results);
                if let Some(error) = &releases.0.lock().unwrap().error {
                    eprintln!("telemetry-ingest: releases: {error}");
                }
            }
            Err(error) => eprintln!("telemetry-ingest: releases: {error}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_package_archives_count() {
        assert_eq!(parse_asset("hangar-0.1.0.12300-linux-x86_64.tar.gz"), Some(("0.1.0.12300", "linux")));
        assert_eq!(parse_asset("hangar-0.1.0.12251-windows-x86_64.tar.gz"), Some(("0.1.0.12251", "windows")));
        assert_eq!(parse_asset("hangar-0.2.0-macos-aarch64.tar.gz"), Some(("0.2.0", "macos")));
        assert_eq!(parse_asset("package-entry-linux-x86_64.json"), None);
        assert_eq!(parse_asset("package-latest.json"), None);
        assert_eq!(parse_asset("hangar-dev-linux-x86_64.tar.gz"), None);
        assert_eq!(parse_asset("hangar-0.1..2-linux-x86_64.tar.gz"), None);
    }

    #[test]
    fn versions_per_system_newest_first_by_number() {
        let body = serde_json::json!({"assets": [
            {"name": "hangar-0.1.0.9-linux-x86_64.tar.gz", "updated_at": "2026-11-27T10:00:00Z"},
            {"name": "hangar-0.1.0.12300-linux-x86_64.tar.gz", "updated_at": "2026-11-28T19:16:19Z"},
            {"name": "hangar-0.1.0.12300-linux-aarch64.tar.gz", "updated_at": "2026-11-28T19:20:00Z"},
            {"name": "hangar-0.1.0.10-linux-x86_64.tar.gz", "updated_at": "2026-11-27T11:00:00Z"},
            {"name": "hangar-0.1.0.12251-windows-x86_64.tar.gz", "updated_at": "2026-11-28T14:45:44Z"},
            {"name": "package-latest.json", "updated_at": "2026-11-28T19:16:23Z"}
        ]})
        .to_string();
        let got: Vec<(String, String, String)> = releases_of(&body).unwrap().into_iter().map(|r| (r.os, r.version, r.at)).collect();
        let row = |os: &str, v: &str, at: &str| (os.to_owned(), v.to_owned(), at.to_owned());
        assert_eq!(got, vec![
            row("linux", "0.1.0.12300", "2026-11-28T19:20:00Z"),
            row("linux", "0.1.0.10", "2026-11-27T11:00:00Z"),
            row("linux", "0.1.0.9", "2026-11-27T10:00:00Z"),
            row("windows", "0.1.0.12251", "2026-11-28T14:45:44Z"),
        ]);
    }

    #[test]
    fn a_failed_read_keeps_the_last_good_one_and_says_so() {
        let releases = Releases::default();
        let one = vec![Release { version: "0.1.0.1".into(), os: "linux".into(), at: "x".into() }];
        releases.apply(vec![("latest", Ok(one.clone())), ("beta", Err("http status: 403".into()))]);
        releases.apply(vec![("latest", Err("timeout".into())), ("beta", Err("timeout".into()))]);
        let snapshot = releases.snapshot();
        assert_eq!(snapshot["channels"]["latest"][0]["version"], "0.1.0.1");
        assert!(snapshot["channels"].get("beta").is_none());
        assert_eq!(snapshot["error"], "latest: timeout; beta: timeout");
        releases.apply(vec![("latest", Ok(one)), ("beta", Ok(vec![]))]);
        assert!(releases.snapshot()["error"].is_null());
    }
}
