//! Aparelhos (celular e PWA) que já abriram a lista do dono: o primeiro acesso de cada um faz o
//! nativo mostrar a dica de usar o Hangar fora do PC. A conexão confere só a memória; o disco muda
//! apenas com aparelho novo, e a lista publica o retrato (`devices`) que sai daqui.
use std::{io, path::{Path, PathBuf}, sync::{Arc, Mutex}};

use chrono::{DateTime, SecondsFormat, Utc};
use serde::{Deserialize, Serialize};
use tokio::sync::Notify;

use crate::diag::DiagClient;

/// Acima disso sai o de primeira vez mais antiga: o arquivo não cresce com aparelho trocado.
const MAX: usize = 20;

/// O cliente que o `ConnectPortGate` do Python (8768, `connect_port.py`) põe no lugar do real:
/// nada que chega por ela é local, nem conta como aparelho do dono.
pub const CONNECT_PEER: &str = "192.0.2.1";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind { Ios, Android, Pwa }

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Device { pub id: String, pub kind: Kind, pub first_seen: String }

fn valid_id(id: &str) -> bool {
    (1..=64).contains(&id.len()) && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
}

fn instant(device: &Device) -> Option<DateTime<chrono::FixedOffset>> { DateTime::parse_from_rfc3339(&device.first_seen).ok() }

/// `device=<tipo>;<id>` da query da lista; qualquer outra forma é ignorada.
pub fn parse(query: Option<&str>) -> Option<(Kind, String)> {
    let (_, value) = form_urlencoded::parse(query?.as_bytes()).find(|(name, _)| name == "device")?;
    let (kind, id) = value.split_once(';')?;
    let kind = match kind { "ios" => Kind::Ios, "android" => Kind::Android, "pwa" => Kind::Pwa, _ => return None };
    valid_id(id).then(|| (kind, id.to_owned()))
}

fn read(path: &Path) -> io::Result<Vec<Device>> {
    match crate::runtime::queue::read_replaced(path) {
        Ok(bytes) => {
            let devices: Vec<Device> = serde_json::from_slice(&bytes).map_err(io::Error::other)?;
            if devices.iter().all(|d| valid_id(&d.id) && instant(d).is_some()) { Ok(devices) } else { Err(io::Error::other("aparelho inválido")) }
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(error) => Err(error),
    }
}

fn write(path: &Path, devices: &[Device]) -> io::Result<()> {
    std::fs::create_dir_all(path.parent().ok_or_else(|| io::Error::other("pasta ausente"))?)?;
    crate::runtime::queue::atomic_write(path, &serde_json::to_vec(devices).map_err(io::Error::other)?)
}

fn snapshot_json(devices: &[Device]) -> Arc<str> {
    serde_json::to_string(devices).unwrap_or_else(|_| "[]".into()).into()
}

fn known(inner: &Inner, id: &str) -> bool { inner.devices.as_ref().is_some_and(|known| known.iter().any(|d| d.id == id)) }

struct Inner {
    /// `None` até uma leitura boa do arquivo: sem ela nada é gravado (ilegível não é sobrescrito).
    devices: Option<Vec<Device>>,
    /// O retrato publicado; com o arquivo ilegível fica o último.
    json: Arc<str>,
    /// O tique já tentou ler: ilegível só é relido por conexão de aparelho, nunca a cada tique.
    tried: bool,
}

pub struct Registry {
    path: PathBuf,
    inner: Mutex<Inner>,
    /// Aparelho novo gravado: a lista publica na hora, sem esperar o tique.
    pub(crate) changed: Notify,
}

impl Registry {
    /// `path` é `~/.hangar/devices.json`; o teste troca por uma pasta temporária.
    pub fn new(path: PathBuf) -> Self {
        Self { path, inner: Mutex::new(Inner { devices: None, json: "[]".into(), tried: false }), changed: Notify::new() }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> { self.inner.lock().unwrap_or_else(|e| e.into_inner()) }

    /// Lê o arquivo para a memória; `true` se o retrato mudou.
    fn load(&self, inner: &mut Inner, diag: &DiagClient) -> bool {
        inner.tried = true;
        match read(&self.path) {
            Ok(devices) => {
                let json = snapshot_json(&devices);
                inner.devices = Some(devices);
                // Mesmo texto mantém o `Arc`: a lista compara pelo ponteiro.
                let changed = json != inner.json;
                if changed { inner.json = json; }
                changed
            }
            Err(error) => {
                tracing::warn!(code = "devices_unreadable", %error, "devices.json ilegível");
                diag.report("rust.devices", "", "devices_unreadable", "devices.json ilegível; não é gravado e vale o último retrato");
                false
            }
        }
    }

    /// O retrato para a lista do dono. Bloqueia na primeira leitura do arquivo.
    pub fn snapshot(&self, diag: &DiagClient) -> Arc<str> {
        let mut inner = self.lock();
        if !inner.tried { self.load(&mut inner, diag); }
        inner.json.clone()
    }

    /// Para o código async: sem esperar a trava, que uma gravação segura durante o disco.
    fn try_inner(&self) -> Option<std::sync::MutexGuard<'_, Inner>> {
        match self.inner.try_lock() {
            Ok(inner) => Some(inner),
            Err(std::sync::TryLockError::Poisoned(e)) => Some(e.into_inner()),
            Err(std::sync::TryLockError::WouldBlock) => None,
        }
    }

    /// O retrato em memória, sem tocar o disco; `None` com uma gravação em curso, que avisa pelo `changed` ao terminar.
    pub fn current(&self) -> Option<Arc<str>> { self.try_inner().map(|inner| inner.json.clone()) }

    /// Já gravado na memória: a conexão não precisa ir ao pool bloqueante. Trava ocupada responde `false`, e o
    /// `record` confere de novo sob a trava.
    pub fn is_known(&self, id: &str) -> bool { self.try_inner().is_some_and(|inner| known(&inner, id)) }

    pub fn record(&self, kind: Kind, id: &str, diag: &DiagClient) { self.record_at(kind, id, Utc::now(), diag) }

    /// Primeiro acesso do aparelho: grava e avisa a lista. Já conhecido na memória, não toca o disco.
    pub fn record_at(&self, kind: Kind, id: &str, now: DateTime<Utc>, diag: &DiagClient) {
        let mut inner = self.lock();
        if known(&inner, id) { return; }
        let mut changed = inner.devices.is_none() && self.load(&mut inner, diag);
        if let Some(known) = inner.devices.as_ref().filter(|known| !known.iter().any(|d| d.id == id)) {
            let mut next = known.clone();
            while next.len() >= MAX {
                let Some(oldest) = next.iter().enumerate().min_by_key(|(_, d)| instant(d)).map(|(i, _)| i) else { break };
                next.remove(oldest);
            }
            next.push(Device { id: id.to_owned(), kind, first_seen: now.to_rfc3339_opts(SecondsFormat::Millis, true) });
            match write(&self.path, &next) {
                Ok(()) => {
                    inner.json = snapshot_json(&next);
                    inner.devices = Some(next);
                    changed = true;
                }
                Err(error) => {
                    tracing::warn!(code = "devices_write_failed", %error, "devices.json sem gravar");
                    diag.report("rust.devices", "", "devices_write_failed", "aparelho novo sem gravar em devices.json; vale o último retrato");
                }
            }
        }
        // Solta antes de avisar: quem acorda lê sem esperar a trava.
        drop(inner);
        if changed { self.changed.notify_one(); }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn diag() -> DiagClient { DiagClient::new("127.0.0.1:1".parse().unwrap(), String::new()) }

    fn at(second: u32) -> DateTime<Utc> { Utc.with_ymd_and_hms(2026, 10, 30, 12, 0, second).unwrap() }

    fn ids(registry: &Registry) -> Vec<String> {
        let json: serde_json::Value = serde_json::from_str(&registry.snapshot(&diag())).unwrap();
        json.as_array().unwrap().iter().map(|d| d["id"].as_str().unwrap().to_owned()).collect()
    }

    #[test]
    fn only_the_three_kinds_and_a_short_plain_id_are_accepted() {
        assert_eq!(parse(Some("token=x&device=ios%3Babc-1")), Some((Kind::Ios, "abc-1".into())));
        assert_eq!(parse(Some("device=android;A-9")), Some((Kind::Android, "A-9".into())));
        assert_eq!(parse(Some("device=pwa%3Bz")), Some((Kind::Pwa, "z".into())));
        assert_eq!(parse(Some(&format!("device=ios%3B{}", "a".repeat(64)))).map(|(_, id)| id.len()), Some(64));
        for bad in ["device=desktop%3Babc", "device=ios%3B", "device=ios", "device=ios%3Ba_b", "device=ios%3Ba%20b",
            "device=IOS%3Babc", "token=x"] {
            assert_eq!(parse(Some(bad)), None, "{bad}");
        }
        assert_eq!(parse(Some(&format!("device=ios%3B{}", "a".repeat(65)))), None, "65 caracteres");
        assert_eq!(parse(None), None);
    }

    #[tokio::test]
    async fn the_21st_device_takes_the_place_of_the_oldest() {
        let dir = tempfile::tempdir().unwrap();
        let registry = Registry::new(dir.path().join(".hangar/devices.json"));
        // Gravados fora de ordem: quem sai é o de primeira vez mais antiga, não o primeiro da lista.
        registry.record_at(Kind::Ios, "late", at(50), &diag());
        for i in 0..19 { registry.record_at(Kind::Android, &format!("d{i}"), at(10 + i), &diag()); }
        assert_eq!(ids(&registry).len(), 20);
        registry.record_at(Kind::Pwa, "new", at(59), &diag());
        let now = ids(&registry);
        assert_eq!(now.len(), 20);
        assert!(!now.contains(&"d0".to_owned()) && now.contains(&"late".to_owned()) && now.contains(&"new".to_owned()));
        let file: Vec<Device> = serde_json::from_slice(&std::fs::read(dir.path().join(".hangar/devices.json")).unwrap()).unwrap();
        assert_eq!(file.len(), 20);
        assert_eq!(file.last().unwrap(), &Device { id: "new".into(), kind: Kind::Pwa, first_seen: "2026-10-30T12:00:59.000Z".into() });
    }

    #[tokio::test]
    async fn a_known_device_never_touches_the_disk() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(".hangar/devices.json");
        let registry = Registry::new(path.clone());
        registry.record_at(Kind::Ios, "abc-1", at(1), &diag());
        std::fs::remove_file(&path).unwrap();
        registry.record_at(Kind::Ios, "abc-1", at(2), &diag());
        assert!(!path.exists(), "conferido na memória");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            registry.record_at(Kind::Ios, "abc-2", at(3), &diag());
            assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
        }
    }

    #[test]
    fn a_busy_lock_answers_without_waiting() {
        let registry = Registry::new(std::env::temp_dir().join("hangar-devices-busy/devices.json"));
        let held = registry.lock();
        assert_eq!(registry.current(), None);
        assert!(!registry.is_known("abc-1"), "ocupada: o registro confere de novo sob a trava");
        drop(held);
        assert_eq!(registry.current().as_deref(), Some("[]"));
    }

    #[tokio::test]
    async fn an_unreadable_file_is_kept_and_the_snapshot_stays() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(".hangar/devices.json");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "{quebrado").unwrap();
        let registry = Registry::new(path.clone());
        assert_eq!(&*registry.snapshot(&diag()), "[]");
        registry.record_at(Kind::Ios, "abc-1", at(1), &diag());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "{quebrado", "ilegível não é sobrescrito");
        assert_eq!(&*registry.snapshot(&diag()), "[]");
    }
}
