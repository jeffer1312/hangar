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

/// Cria a pasta e a deixa 0700 (só o dono), também quando ela já existia.
pub(super) fn private_dir(dir: &Path) -> io::Result<()> {
    std::fs::create_dir_all(dir)?;
    #[cfg(unix)]
    std::fs::set_permissions(dir, std::os::unix::fs::PermissionsExt::from_mode(0o700))?;
    Ok(())
}

/// Gravação em andamento (temporário, ficha antes do arquivo) não é sobra para a varredura.
const YOUNG: Duration = Duration::from_secs(10);

/// Grava e devolve o caminho. A ficha vai antes do arquivo, inteira de uma vez (tmp + rename): a
/// varredura apaga arquivo sem ficha e nunca lê uma ficha pela metade.
pub fn write(dir: &Path, session_key: &str, session: &str, value: &[u8]) -> io::Result<PathBuf> {
    private_dir(dir)?;
    let id = super::new_id().map_err(|_| io::Error::other("sem aleatório"))?;
    let meta = Meta { session_key: session_key.to_owned(), session: session.to_owned(), created: super::now() };
    crate::runtime::queue::atomic_write(&dir.join(format!("{id}.json")), &serde_json::to_vec(&meta).map_err(io::Error::other)?)?;
    let path = dir.join(&id);
    let written = private_file(&path).and_then(|mut f| { f.write_all(value)?; f.sync_all() });
    if let Err(error) = written {
        remove_logged(dir, &id);
        return Err(error);
    }
    Ok(path)
}

/// Os dois arquivos, mesmo com o primeiro falhando; arquivo que já sumiu não é falha.
pub(super) fn remove(dir: &Path, id: &str) -> io::Result<()> {
    let gone = |path: PathBuf| match std::fs::remove_file(path) {
        Err(error) if error.kind() != io::ErrorKind::NotFound => Err(error),
        _ => Ok(()),
    };
    let (secret, meta) = (gone(dir.join(id)), gone(dir.join(format!("{id}.json"))));
    secret.and(meta)
}

/// Falha aqui é senha em claro que ficou no disco: precisa aparecer.
fn remove_logged(dir: &Path, id: &str) {
    if let Err(error) = remove(dir, id) {
        tracing::warn!(code = "vault_file_remove_failed", id, kind = ?error.kind(), "o arquivo de senha do cofre não saiu do disco");
    }
}

/// Espera a tarefa para a queda dela não passar calada.
async fn blocking(what: &'static str, job: impl FnOnce() + Send + 'static) {
    if let Err(error) = tokio::task::spawn_blocking(job).await {
        tracing::warn!(code = "vault_file_task_failed", what, panicked = error.is_panic(), "a limpeza de arquivos do cofre não terminou");
    }
}

fn young(path: &Path) -> bool {
    std::fs::metadata(path).and_then(|m| m.modified()).ok().and_then(|t| t.elapsed().ok()).is_some_and(|age| age < YOUNG)
}

/// Apaga tudo o que `keep` não guarda; arquivo sem ficha e ficha ilegível também saem. `spare_young`
/// (varredura): o que mudou há menos de 10 s pode ser uma gravação em andamento e fica.
pub fn retain(dir: &Path, keep: impl Fn(&Meta) -> bool, spare_young: bool) {
    let read = match std::fs::read_dir(dir) {
        Ok(read) => read,
        Err(error) => {
            // Pasta que ainda não existe é só nenhum `vault_file` até agora.
            if error.kind() != io::ErrorKind::NotFound {
                tracing::warn!(code = "vault_file_dir_unreadable", kind = ?error.kind(), "a pasta de arquivos do cofre não leu; nada foi apagado");
            }
            return;
        }
    };
    let names: Vec<String> = read.flatten().filter_map(|e| e.file_name().into_string().ok()).collect();
    for name in &names {
        if spare_young && young(&dir.join(name)) { continue; }
        match name.strip_suffix(".json") {
            Some(id) => {
                let meta = std::fs::read(dir.join(name)).ok().and_then(|raw| serde_json::from_slice::<Meta>(&raw).ok());
                if !meta.is_some_and(|m| keep(&m)) { remove_logged(dir, id); }
            }
            None if !names.contains(&format!("{name}.json")) => remove_logged(dir, name),
            None => {}
        }
    }
}

/// Mais velho que 24 h, ou (com a lista em mãos) de sessão que não está nela.
pub fn sweep(dir: &Path, now: f64, alive: Option<&dyn Fn(&Meta) -> bool>) {
    retain(dir, |m| now - m.created < LIFE.as_secs_f64() && alive.is_none_or(|alive| alive(m)), true);
}

/// O fechamento de uma sessão que ainda vai acontecer: a chave do pane é lida antes do kill.
pub struct Closing { dir: PathBuf, name: String, key: Option<String> }

/// `None` sem arquivo nenhum: o fechamento não paga a pergunta ao multiplexador.
pub async fn before_close(st: &AppState, name: &str) -> Option<Closing> {
    let dir = base()?;
    if std::fs::read_dir(&dir).map_or(true, |mut d| d.next().is_none()) { return None; }
    let key = st.list.env().mux.session_env(name, "CP_SESSION_KEY").await;
    Some(Closing { dir, name: name.to_owned(), key })
}

impl Closing {
    /// Só depois do fechamento que deu certo: recusado, a sessão segue viva e o arquivo é dela.
    pub fn done(self) {
        tokio::spawn(blocking("close", move || close(&self.dir, &self.name, self.key.as_deref())));
    }
}

/// Na subida a lista ainda não tem as sessões que voltam pelo spec: só o prazo de 24 h apaga.
pub async fn sweep_at_start() {
    let Some(dir) = base() else { return };
    blocking("sweep_at_start", move || sweep(&dir, super::now(), None)).await;
}

pub fn close(dir: &Path, name: &str, key: Option<&str>) {
    retain(dir, |m| m.session != name && key != Some(m.session_key.as_str()), false);
}

/// A varredura contra a lista de sessões; lista que não responde ou sem fatos só apaga pelo prazo.
// shortcut: sessão renomeada com terminal e sem chave na vida da lista conta como saída; o arquivo some antes das 24 h.
pub async fn sweep_live(st: &AppState) {
    let Some(dir) = base() else { return };
    let rows = st.list.snapshot().await.ok().filter(|p| !p.facts.unknown).map(|p| p.rows);
    blocking("sweep_live", move || {
        let alive = rows.as_ref().map(|rows| move |m: &Meta| rows.iter().any(|r| r.name == m.session
            || r.lifecycle_id.as_deref().and_then(|l| l.strip_prefix("k:")) == Some(m.session_key.as_str())));
        sweep(&dir, super::now(), alive.as_ref().map(|f| f as &dyn Fn(&Meta) -> bool));
    }).await;
}

/// Some em 24 h mesmo sem varredura nesse meio-tempo.
pub fn expire_later(path: PathBuf) {
    tokio::spawn(async move {
        tokio::time::sleep(LIFE).await;
        if let (Some(dir), Some(id)) = (path.parent(), path.file_name().and_then(|n| n.to_str())) { remove_logged(dir, id); }
    });
}
