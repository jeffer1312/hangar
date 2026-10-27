//! Leitura e escrita da pasta `.hangar-pair` e do arquivo de contratos. Sem lock aqui: quem
//! escreve é o `GroupService`, um por processo, sob o lock dele.
use super::model::{GroupRecord, Sidecar};
use serde_json::Value;
use std::path::{Path, PathBuf};

#[derive(Debug)]
pub enum StoreError { Io(std::io::Error), Json(serde_json::Error) }
impl std::fmt::Display for StoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self { StoreError::Io(e) => write!(f, "io: {e}"), StoreError::Json(e) => write!(f, "json: {e}") }
    }
}
impl std::error::Error for StoreError {}
impl From<std::io::Error> for StoreError { fn from(e: std::io::Error) -> Self { StoreError::Io(e) } }
impl From<serde_json::Error> for StoreError { fn from(e: serde_json::Error) -> Self { StoreError::Json(e) } }

/// `pqueue._sanitize`: fora de `[A-Za-z0-9_.-]` vira `-`.
pub fn file_stem(name: &str) -> String {
    name.chars().map(|c| if c.is_ascii_alphanumeric() || "_.-".contains(c) { c } else { '-' }).collect()
}

/// Contratos de grupo: `grupo-` é o registro do árbitro, `regras-` o que o time lê.
const CONTRACT_PREFIXES: [&str; 2] = ["grupo", "regras"];

pub struct PairDir { root: PathBuf, archive: PathBuf }

impl PairDir {
    pub fn new(root: PathBuf, archive: PathBuf) -> Self { Self { root, archive } }
    pub fn root(&self) -> &Path { &self.root }

    fn sidecar_path(&self, name: &str) -> PathBuf { self.root.join(format!("{}.json", file_stem(name))) }

    /// Ausente ou de outro tipo que objeto = sem grupo (como o Python). JSON torto ou arquivo
    /// ilegível volta como erro: o chamador o trata como sem grupo e o registra no diário.
    pub fn sidecar(&self, name: &str) -> Result<Option<Sidecar>, StoreError> {
        read_sidecar(&self.sidecar_path(name), name)
    }

    /// Última escrita do sidecar: a janela de lançamento do grupo `orq` conta daqui.
    pub fn sidecar_modified(&self, name: &str) -> std::io::Result<std::time::SystemTime> {
        std::fs::metadata(self.sidecar_path(name))?.modified()
    }

    pub fn write_sidecar(&self, name: &str, s: &Sidecar) -> Result<(), StoreError> {
        std::fs::create_dir_all(&self.root)?;
        let body = serde_json::to_vec(&s.to_json(name))?;
        replace_atomic(&self.sidecar_path(name), &body)?;
        Ok(())
    }

    /// `.json` e o `.json.tmp` que uma escrita interrompida deixa; ausente não é erro.
    pub fn clear_sidecar(&self, name: &str) -> Result<(), StoreError> {
        let path = self.sidecar_path(name);
        let mut tmp = path.clone().into_os_string();
        tmp.push(".tmp");
        for file in [path, PathBuf::from(tmp)] {
            match std::fs::remove_file(file) {
                Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(e.into()),
                _ => {}
            }
        }
        Ok(())
    }

    /// (stem, sidecar) dos `*.json` da raiz, exceto `external_pairs.json`, por ordem de stem. O
    /// stem já é o nome saneado do arquivo, não o da sessão. Arquivo que não vale como sidecar
    /// fica de fora (como no Python), com aviso quando está torto ou ilegível.
    pub fn sidecars(&self) -> Result<Vec<(String, Sidecar)>, StoreError> {
        let entries = match std::fs::read_dir(&self.root) {
            Ok(entries) => entries,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(e.into()),
        };
        let mut found = Vec::new();
        for entry in entries {
            let path = entry?.path();
            let Some(stem) = path.file_name().and_then(|n| n.to_str()).and_then(|n| n.strip_suffix(".json")) else { continue };
            if stem == "external_pairs" || !path.is_file() {
                continue;
            }
            match read_sidecar(&path, stem) {
                Ok(Some(sidecar)) => found.push((stem.to_owned(), sidecar)),
                Ok(None) => {}
                Err(error) => {
                    if crate::warn_limit::allow(Some(stem), "groups_sidecar_unreadable") {
                        tracing::warn!(code = "groups_sidecar_unreadable", stem, %error, "groups: sidecar de grupo ilegível ou torto, tratado como sem grupo");
                    }
                }
            }
        }
        found.sort_by(|a, b| a.0.cmp(&b.0));
        Ok(found)
    }

    /// `_merge_contract` (pair.py:231-258): anexa `grupo-`/`regras-` do perdedor ao sobrevivente,
    /// com o cabeçalho `## Contrato herdado do grupo <gid> (merge)`. Falha não desfaz a fusão:
    /// `false` para quem chama registrar no diário.
    pub fn merge_contract(&self, loser: &str, survivor: &str) -> bool {
        let mut ok = true;
        for prefix in CONTRACT_PREFIXES {
            let from = self.root.join(format!("{prefix}-{loser}.md"));
            // Sem contrato do perdedor é o caso comum de uma fusão, não falha.
            if !from.is_file() {
                continue;
            }
            let merged = (|| -> std::io::Result<()> {
                let content = std::fs::read_to_string(&from)?;
                let content = content.trim();
                if content.is_empty() {
                    return Ok(());
                }
                let to = self.root.join(format!("{prefix}-{survivor}.md"));
                let old = match std::fs::read_to_string(&to) {
                    Ok(old) => old,
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
                    Err(e) => return Err(e),
                };
                std::fs::write(&to, format!("{old}\n\n## Contrato herdado do grupo {loser} (merge)\n\n{content}\n"))?;
                std::fs::remove_file(&from)
            })();
            if let Err(error) = merged {
                tracing::warn!(code = "groups_contract_merge_failed", prefix, loser, survivor, %error, "groups: a fusão não herdou o contrato");
                ok = false;
            }
        }
        ok
    }

    /// `_arquivar_contratos` (pair.py:271-284): `<archive>/<prefixo>-<gid>-<AAAAmmdd-HHMMSS>.md`
    /// em hora local (`chrono::Local`). Falha só ao diário.
    pub fn archive_contracts(&self, gid: &str) {
        let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S");
        for prefix in CONTRACT_PREFIXES {
            let from = self.root.join(format!("{prefix}-{gid}.md"));
            if !from.is_file() {
                continue;
            }
            let moved = (|| -> std::io::Result<()> {
                std::fs::create_dir_all(&self.archive)?;
                let to = self.archive.join(format!("{prefix}-{gid}-{stamp}.md"));
                // Outro volume não renomeia: copia e apaga, como o `shutil.move`.
                std::fs::rename(&from, &to).or_else(|_| std::fs::copy(&from, &to).and_then(|_| std::fs::remove_file(&from)))
            })();
            if let Err(error) = moved {
                tracing::warn!(code = "groups_contract_archive_failed", prefix, gid, %error, "groups: o contrato não foi arquivado");
            }
        }
    }

    pub fn contract_path(&self, gid: &str) -> PathBuf { self.root.join(format!("grupo-{gid}.md")) }

    /// `.hangar-pair/groups/`: registros da dona, a fila de saída e a identidade desta máquina.
    pub fn groups_dir(&self) -> PathBuf { self.root.join("groups") }

    fn record_path(&self, gid: &str) -> PathBuf { self.groups_dir().join(format!("{}.json", file_stem(gid))) }

    /// Registro do grupo nesta máquina dona; ausente = `None`, torto = erro.
    pub fn record(&self, gid: &str) -> Result<Option<GroupRecord>, StoreError> {
        match std::fs::read(self.record_path(gid)) {
            Ok(raw) => Ok(Some(serde_json::from_slice(&raw)?)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    pub fn write_record(&self, record: &GroupRecord) -> Result<(), StoreError> {
        std::fs::create_dir_all(self.groups_dir())?;
        replace_durable(&self.record_path(&record.gid), &serde_json::to_vec(record)?)?;
        Ok(())
    }

    /// Guarda a versão do registro antes de apagá-lo: o gid pode voltar a ser entre máquinas.
    pub fn remove_record(&self, gid: &str) -> Result<(), StoreError> {
        match self.record(gid) {
            Ok(Some(record)) => self.raise_issued(gid, record.version)?,
            Ok(None) => {}
            Err(error) => tracing::warn!(code = "groups_record_unreadable", %gid, %error, "groups: registro torto apagado sem guardar a versão"),
        }
        match std::fs::remove_file(self.record_path(gid)) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e.into()),
            _ => Ok(()),
        }
    }

    /// Fora de `.json` para o `records()` não o ler como registro.
    fn issued_path(&self) -> PathBuf { self.groups_dir().join("issued.versions") }

    fn read_issued(&self) -> Result<std::collections::BTreeMap<String, u64>, StoreError> {
        match std::fs::read(self.issued_path()) {
            Ok(raw) => Ok(serde_json::from_slice(&raw)?),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Default::default()),
            Err(e) => Err(e.into()),
        }
    }

    /// Maior versão que esta dona já deu a `gid`, também depois de o registro sair. Quem já foi
    /// participante lembra essa versão e descarta como atrasado o que vier abaixo dela.
    pub fn issued(&self, gid: &str) -> Result<u64, StoreError> { Ok(self.read_issued()?.get(gid).copied().unwrap_or(0)) }

    fn raise_issued(&self, gid: &str, version: u64) -> Result<(), StoreError> {
        let mut issued = self.read_issued()?;
        if issued.get(gid).is_some_and(|v| *v >= version) { return Ok(()) }
        issued.insert(gid.to_owned(), version);
        std::fs::create_dir_all(self.groups_dir())?;
        replace_durable(&self.issued_path(), &serde_json::to_vec(&issued)?)?;
        Ok(())
    }

    /// Registros válidos por ordem de gid; torto fica de fora com aviso, como o sidecar.
    pub fn records(&self) -> Result<Vec<GroupRecord>, StoreError> {
        let entries = match std::fs::read_dir(self.groups_dir()) {
            Ok(entries) => entries,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(e.into()),
        };
        let mut found = Vec::new();
        for entry in entries {
            let path = entry?.path();
            let Some(stem) = path.file_name().and_then(|n| n.to_str()).and_then(|n| n.strip_suffix(".json")) else { continue };
            if [OUTBOX_STEM, SELF_STEM].contains(&stem) { continue; }
            match std::fs::read(&path).map_err(StoreError::from).and_then(|raw| Ok(serde_json::from_slice::<GroupRecord>(&raw)?)) {
                Ok(record) => found.push(record),
                Err(error) => {
                    if crate::warn_limit::allow(Some(stem), "groups_record_unreadable") {
                        tracing::warn!(code = "groups_record_unreadable", stem, %error, "groups: registro de grupo ilegível ou torto, ignorado");
                    }
                }
            }
        }
        found.sort_by(|a, b| a.gid.cmp(&b.gid));
        Ok(found)
    }

    fn self_path(&self) -> PathBuf { self.groups_dir().join(format!("{SELF_STEM}.json")) }

    /// O `server_id` gravado na primeira entrada desta máquina num grupo entre máquinas.
    pub fn self_id(&self) -> Result<Option<String>, StoreError> {
        let raw = match std::fs::read(self.self_path()) {
            Ok(raw) => raw,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(e.into()),
        };
        let saved: SelfFile = serde_json::from_slice(&raw)?;
        Ok(Some(saved.server_id))
    }

    /// Chamar só depois de `identity_check` passar: troca o id gravado pelo atual.
    pub fn remember_self(&self, server_id: &str) -> Result<(), StoreError> {
        std::fs::create_dir_all(self.groups_dir())?;
        replace_durable(&self.self_path(), &serde_json::to_vec(&SelfFile { server_id: server_id.to_owned() })?)?;
        Ok(())
    }

    /// Os outros guardam esta máquina pelo id antigo: com grupo entre máquinas vivo, trocar o id
    /// os deixaria apontando para ninguém. Sem grupo vivo ou sem id gravado, nada a comparar.
    pub fn identity_check(&self, current: &str) -> Result<Option<IdentityChanged>, StoreError> {
        let Some(saved) = self.self_id()? else { return Ok(None) };
        if saved == current || !self.sidecars()?.iter().any(|(_, s)| s.fed.is_some()) {
            return Ok(None);
        }
        Ok(Some(IdentityChanged { saved, current: current.to_owned() }))
    }
}

/// Nomes reservados em `groups/`: não são registro de grupo.
pub(crate) const OUTBOX_STEM: &str = "outbox";
const SELF_STEM: &str = "self";

#[derive(serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct SelfFile { server_id: String }

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IdentityChanged { pub saved: String, pub current: String }

fn read_sidecar(path: &Path, name: &str) -> Result<Option<Sidecar>, StoreError> {
    let raw = match std::fs::read(path) {
        Ok(raw) => raw,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e.into()),
    };
    let Value::Object(data) = serde_json::from_slice(&raw)? else { return Ok(None) };
    Ok(parse_noting_fed(name, &data))
}

/// `Sidecar::parse` que não deixa um `fed` torto sumir calado: vale como ausente e vai ao diário.
fn parse_noting_fed(name: &str, data: &serde_json::Map<String, Value>) -> Option<Sidecar> {
    let sidecar = Sidecar::parse(name, data)?;
    if sidecar.fed.is_none() && data.get("fed").is_some_and(|f| !f.is_null()) && crate::warn_limit::allow(Some(name), "groups_fed_invalid") {
        tracing::warn!(code = "groups_fed_invalid", name, "groups: fed torto no sidecar, tratado como grupo de uma máquina");
    }
    Some(sidecar)
}

/// Troca o arquivo inteiro: temporário na mesma pasta + rename, com as novas tentativas do
/// Windows (acesso negado no rename) copiadas de `hangar-workspace/src/files.rs:160-175`. Sem
/// fsync e sem a conferência de conteúdo do `files::write`, que não servem aqui.
pub(crate) fn replace_atomic(target: &Path, bytes: &[u8]) -> std::io::Result<()> { replace_with(target, bytes, false) }

/// Como `replace_atomic`, com fsync do temporário antes do rename e da pasta depois: registro,
/// identidade e fila entre máquinas não podem voltar vazios nem antigos de uma queda, porque os
/// outros lados confiam neles. Não é o `runtime::queue::atomic_write`: o temporário de nome fixo é
/// o que uma escrita interrompida sobrescreve na próxima, sem sobrar lixo na pasta.
pub(crate) fn replace_durable(target: &Path, bytes: &[u8]) -> std::io::Result<()> {
    replace_with(target, bytes, true)?;
    #[cfg(unix)]
    if let Some(dir) = target.parent() { std::fs::File::open(dir)?.sync_all()?; }
    Ok(())
}

fn replace_with(target: &Path, bytes: &[u8], durable: bool) -> std::io::Result<()> {
    // Mesmo nome que o `PairLink.set` usa (`<stem>.json.tmp`), que o `clear_sidecar` também apaga.
    let mut tmp = target.as_os_str().to_owned();
    tmp.push(".tmp");
    let tmp = PathBuf::from(tmp);
    if durable {
        let written = std::fs::File::create(&tmp).and_then(|mut f| { std::io::Write::write_all(&mut f, bytes)?; f.sync_all() });
        if let Err(e) = written {
            let _ = std::fs::remove_file(&tmp);
            return Err(e);
        }
    } else {
        std::fs::write(&tmp, bytes)?;
    }
    let waits: &[u64] = if cfg!(windows) { &[20, 40, 60, 80, 100, 120] } else { &[] };
    let mut waits = waits.iter();
    loop {
        match std::fs::rename(&tmp, target) {
            Ok(()) => return Ok(()),
            Err(e) => {
                if e.kind() == std::io::ErrorKind::PermissionDenied && let Some(ms) = waits.next() {
                    std::thread::sleep(std::time::Duration::from_millis(*ms));
                    continue;
                }
                let _ = std::fs::remove_file(&tmp);
                return Err(e);
            }
        }
    }
}
