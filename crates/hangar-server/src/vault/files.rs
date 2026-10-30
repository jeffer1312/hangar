//! Arquivo 0600 com a senha para o agente (`vault_file`), numa pasta 0700 da sessão do usuário que o
//! reinício da máquina limpa. Cada arquivo tem ao lado um `<id>.json` com a sessão dona: some no
//! fechamento dela, em 24 h e na varredura (subida e cada `vault_file`) quando a dona saiu da lista.
use serde::{Deserialize, Serialize};
use std::{io::{self, Write}, path::{Path, PathBuf}, time::Duration};

use crate::routes::AppState;

pub const LIFE: Duration = Duration::from_secs(24 * 60 * 60);

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Meta { pub session_key: String, pub session: String, pub created: f64 }

/// Linux: `$XDG_RUNTIME_DIR/hangar/vault-files`; Windows: `%LOCALAPPDATA%\hangar\vault-files`.
pub fn base() -> Option<PathBuf> {
    let root = if cfg!(windows) { std::env::var_os("LOCALAPPDATA") } else { std::env::var_os("XDG_RUNTIME_DIR") };
    root.filter(|r| !r.is_empty()).map(|r| PathBuf::from(r).join("hangar").join("vault-files"))
}

fn private_file(path: &Path) -> io::Result<std::fs::File> {
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    options.open(path)
}

fn private_dir(dir: &Path) -> io::Result<()> {
    std::fs::create_dir_all(dir)?;
    #[cfg(unix)]
    std::fs::set_permissions(dir, std::os::unix::fs::PermissionsExt::from_mode(0o700))?;
    Ok(())
}

/// Grava e devolve o caminho. A ficha vai antes do arquivo: a varredura apaga arquivo sem ficha.
pub fn write(dir: &Path, session_key: &str, session: &str, value: &[u8]) -> io::Result<PathBuf> {
    private_dir(dir)?;
    let id = super::new_id().map_err(|_| io::Error::other("sem aleatório"))?;
    let meta = Meta { session_key: session_key.to_owned(), session: session.to_owned(), created: super::now() };
    private_file(&dir.join(format!("{id}.json")))?.write_all(&serde_json::to_vec(&meta).map_err(io::Error::other)?)?;
    let path = dir.join(&id);
    let written = private_file(&path).and_then(|mut f| { f.write_all(value)?; f.sync_all() });
    if let Err(error) = written {
        remove(dir, &id);
        return Err(error);
    }
    Ok(path)
}

fn remove(dir: &Path, id: &str) {
    let _ = std::fs::remove_file(dir.join(id));
    let _ = std::fs::remove_file(dir.join(format!("{id}.json")));
}

/// Apaga tudo o que `keep` não guarda; arquivo sem ficha e ficha ilegível também saem.
pub fn retain(dir: &Path, keep: impl Fn(&Meta) -> bool) {
    let Ok(read) = std::fs::read_dir(dir) else { return };
    let names: Vec<String> = read.flatten().filter_map(|e| e.file_name().into_string().ok()).collect();
    for name in &names {
        match name.strip_suffix(".json") {
            Some(id) => {
                let meta = std::fs::read(dir.join(name)).ok().and_then(|raw| serde_json::from_slice::<Meta>(&raw).ok());
                if !meta.is_some_and(|m| keep(&m)) { remove(dir, id); }
            }
            None if !names.contains(&format!("{name}.json")) => remove(dir, name),
            None => {}
        }
    }
}

/// Mais velho que 24 h, ou (com a lista em mãos) de sessão que não está nela.
pub fn sweep(dir: &Path, now: f64, alive: Option<&dyn Fn(&Meta) -> bool>) {
    retain(dir, |m| now - m.created < LIFE.as_secs_f64() && alive.is_none_or(|alive| alive(m)));
}

/// Fechamento da sessão: os arquivos dela, pelo nome e pela chave do pane.
pub async fn on_close(st: &AppState, name: &str) {
    let Some(dir) = base() else { return };
    let empty = std::fs::read_dir(&dir).map_or(true, |mut d| d.next().is_none());
    if empty { return; }
    let key = st.list.env().mux.session_env(name, "CP_SESSION_KEY").await;
    let name = name.to_owned();
    let _ = tokio::task::spawn_blocking(move || close(&dir, &name, key.as_deref())).await;
}

pub fn close(dir: &Path, name: &str, key: Option<&str>) {
    retain(dir, |m| m.session != name && key != Some(m.session_key.as_str()));
}

/// A varredura contra a lista de sessões; lista que não responde ou sem fatos só apaga pelo prazo.
// shortcut: sessão renomeada com terminal e sem chave na vida da lista conta como saída; o arquivo some antes das 24 h.
pub async fn sweep_live(st: &AppState) {
    let Some(dir) = base() else { return };
    let rows = st.list.snapshot().await.ok().filter(|p| !p.facts.unknown).map(|p| p.rows);
    let _ = tokio::task::spawn_blocking(move || {
        let alive = rows.as_ref().map(|rows| move |m: &Meta| rows.iter().any(|r| r.name == m.session
            || r.lifecycle_id.as_deref().and_then(|l| l.strip_prefix("k:")) == Some(m.session_key.as_str())));
        sweep(&dir, super::now(), alive.as_ref().map(|f| f as &dyn Fn(&Meta) -> bool));
    }).await;
}

/// Some em 24 h mesmo sem varredura nesse meio-tempo.
pub fn expire_later(path: PathBuf) {
    tokio::spawn(async move {
        tokio::time::sleep(LIFE).await;
        if let (Some(dir), Some(id)) = (path.parent(), path.file_name().and_then(|n| n.to_str())) { remove(dir, id); }
    });
}
