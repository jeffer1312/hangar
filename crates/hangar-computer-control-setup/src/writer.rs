//! O único escritor da entrada `mcpServers.hangar-computer-control`: ligar, desligar e o repasse do Jev.
//! Trava todo arquivo envolvido (a trava do Claude Code), lê e valida tudo antes da primeira gravação e
//! relê cada arquivo antes de trocá-lo, preservando o modo e o link que ele tinha.
use std::io::Write;
use std::path::{Path, PathBuf};

use serde_json::{Map, Value, json};

use hangar_core::config_lock::{ConfigLock, lock_config};
use hangar_core::orq_codec::codec::python_string;
use hangar_core::pycompat::{pyjson, truthy};

use crate::files::{entry_of, known_from, object_of, read_entries, read_entry, unreadable};
use crate::{Error, NAME, Paths};

/// As variáveis da página Jev, na ordem em que o Python as gravava.
const JEV_VARS: [&str; 7] =
    ["TYPESAFE_API_KEY", "JEV_ENDPOINT", "JEV_MODEL", "LLM_PROXY_URL", "LLM_MODEL", "LLM_EFFORT", "LLM_PROXY_KEY"];
const LLM_VARS: [&str; 4] = ["LLM_PROXY_URL", "LLM_MODEL", "LLM_EFFORT", "LLM_PROXY_KEY"];

/// O que ligar grava: o programa do MCP e o alvo padrão. O env do Jev é lido depois da trava.
#[derive(Clone, Debug)]
pub struct Entry {
    pub command: String,
    pub agent_config: String,
}

/// Liga: a entrada vai ao principal e a cada conta com `.claude.json` não vazio. Alvo padrão, pasta dos
/// alvos e Jev são trocados; o resto do env da entrada conhecida é de quem montou o MCP e fica (o
/// `VIRTUAL_ENV` vazio de propósito impede herdar o venv), menos `PYTHONPATH`. Gerenciada vazia = ausente.
/// `jev` roda já sob a trava: um repasse do Jev no meio nunca é desfeito pelo env lido antes.
/// Diz se gravou: entrada já igual em todo arquivo não grava nada.
pub fn write_entry(
    paths: &Paths,
    entry: &Entry,
    jev: impl FnOnce() -> Result<Map<String, Value>, Error>,
) -> Result<bool, Error> {
    let _locks = lock_all(paths)?;
    let files = read_entries(paths)?;
    for file in &files {
        checked_servers(&file.path, &file.data)?;
    }
    let active = files.first().and_then(|file| file.entry.clone());
    let enabled = active.is_some();
    let known = known_from(paths, active)?;
    let previous = crate::state::env_of(known.as_ref(), paths, enabled)?;
    let jev = jev()?;
    let mut managed = Map::new();
    managed.insert("HCC_AGENT_CONFIG".into(), json!(entry.agent_config));
    managed.insert("HCC_AGENTS_DIR".into(), json!(paths.targets_dir().to_string_lossy()));
    for key in JEV_VARS {
        managed.insert(key.into(), jev.get(key).cloned().unwrap_or_else(|| json!("")));
    }
    let mut env: Map<String, Value> =
        previous.into_iter().filter(|(key, _)| !managed.contains_key(key) && key != "PYTHONPATH").collect();
    env.extend(managed.into_iter().filter(|(_, value)| truthy(value)));
    let wanted = json!({"command": entry.command, "args": [], "env": env});
    let main = paths.main_file();
    let mut written = Written::default();
    for file in files {
        let account = file.path != main;
        // Conta nunca aberta (vazia ou `{}`) ganha a entrada pelo espelho na primeira reconciliação.
        if account && file.data.is_empty() {
            continue;
        }
        let wrote = update(&file.path, false, |bytes| {
            let mut data = parsed(&file.path, bytes, account)?;
            checked_servers(&file.path, &data)?;
            if (account && data.is_empty()) || data.get("mcpServers").and_then(|s| s.get(NAME)) == Some(&wanted) {
                return Ok(None);
            }
            let servers = data.entry("mcpServers").or_insert_with(|| json!({}));
            if !servers.is_object() {
                *servers = json!({});
            }
            servers[NAME] = wanted.clone();
            Ok(Some(data))
        });
        written.note(&file.path, wrote)?;
    }
    Ok(!written.0.is_empty())
}

/// Desliga: guarda a entrada ativa (0600, a marca de "desligado à mão") e tira a chave de todo arquivo que a tem.
pub fn remove_entry(paths: &Paths) -> Result<(), Error> {
    let _locks = lock_all(paths)?;
    let files = read_entries(paths)?;
    for file in &files {
        checked_servers(&file.path, &file.data)?;
    }
    let mut written = Written::default();
    if let Some(active) = files.first().and_then(|file| file.entry.clone()) {
        // A guardada é só a cópia da última entrada ativa: não precisa ser lida.
        let parked = paths.parked_file();
        written.note(&parked, update(&parked, true, |_| Ok(Some(active.clone()))))?;
    }
    let main = paths.main_file();
    for file in files {
        let account = file.path != main;
        let wrote = update(&file.path, false, |bytes| {
            let mut data = parsed(&file.path, bytes, account)?;
            checked_servers(&file.path, &data)?;
            let removed = match data.get_mut("mcpServers") {
                Some(Value::Object(servers)) => servers.shift_remove(NAME).is_some(),
                _ => false,
            };
            Ok(removed.then_some(data))
        });
        written.note(&file.path, wrote)?;
    }
    Ok(())
}

/// Leva o env do Jev às entradas ativas e à guardada, sem ligar nada nem mexer no resto do env. Lê e
/// valida tudo (guardada incluída) antes da primeira gravação; a guardada vai antes das contas.
pub fn sync_jev(paths: &Paths, jev: &Map<String, Value>) -> Result<(), Error> {
    let _locks = lock_all(paths)?;
    let main = paths.main_file();
    let mut pending = Vec::new();
    for path in paths.config_files() {
        let account = path != main;
        let file = read_entry(path, account)?;
        if let Some(entry) = &file.entry {
            let env = env_in(entry, &file.path)?;
            if with_jev(&env, jev) != env {
                pending.push((file.path, env));
            }
        }
    }
    let parked = paths.parked_file();
    if let Some(entry) = crate::files::parked_entry(paths)? {
        env_in(&entry, &parked)?;
    }
    for (path, env) in &pending {
        keep_old_llm(paths, path, env, jev)?;
    }
    let mut written = Written::default();
    written.note(&parked, publish_jev(paths, &parked, false, jev))?;
    for (path, _) in pending {
        let wrote = publish_jev(paths, &path, path != main, jev);
        written.note(&path, wrote)?;
    }
    Ok(())
}

/// Arquivos já gravados na rodada: uma falha no meio diz quais ficaram com a mudança.
#[derive(Default)]
struct Written(Vec<String>);

impl Written {
    fn note(&mut self, path: &Path, result: Result<bool, Error>) -> Result<(), Error> {
        match result {
            Ok(true) => {
                self.0.push(path.to_string_lossy().into_owned());
                Ok(())
            }
            Ok(false) => Ok(()),
            Err(mut error) if !self.0.is_empty() => {
                error.msg = format!("{}; já gravados antes da falha: {}", error.msg, self.0.join(", "));
                if let Some(params) = error.params.as_object_mut() {
                    params.insert("written".into(), json!(self.0));
                }
                Err(error)
            }
            Err(error) => Err(error),
        }
    }
}

/// Só `null` vale como `mcpServers` ausente: qualquer outro valor que não é objeto recusa antes de gravar.
fn checked_servers(path: &Path, data: &Map<String, Value>) -> Result<(), Error> {
    match data.get("mcpServers") {
        None | Some(Value::Null | Value::Object(_)) => Ok(()),
        Some(_) => Err(Error::read(
            path,
            "mcpServers inválido",
            format!("mcpServers em {} não é um objeto JSON", path.display()),
        )),
    }
}

/// `account`: conta de 0 bytes vale `{}`, como no PUT.
fn publish_jev(paths: &Paths, path: &Path, account: bool, jev: &Map<String, Value>) -> Result<bool, Error> {
    let active = path != paths.parked_file().as_path();
    update(path, !active, |bytes| {
        let mut data = parsed(path, bytes, account)?;
        let entry = if active { entry_of(path, &data)? } else { (!data.is_empty()).then(|| data.clone()) };
        let Some(mut entry) = entry else {
            return Ok(None);
        };
        let env = env_in(&entry, path)?;
        let next = with_jev(&env, jev);
        if next == env {
            return Ok(None);
        }
        keep_old_llm(paths, path, &env, jev)?;
        entry.insert("env".into(), Value::Object(next));
        if !active {
            return Ok(Some(entry));
        }
        if let Some(servers) = data.get_mut("mcpServers").and_then(Value::as_object_mut) {
            servers.insert(NAME.into(), Value::Object(entry));
        }
        Ok(Some(data))
    })
}

fn with_jev(env: &Map<String, Value>, jev: &Map<String, Value>) -> Map<String, Value> {
    let mut out: Map<String, Value> =
        env.iter().filter(|(key, _)| !JEV_VARS.contains(&key.as_str())).map(|(k, v)| (k.clone(), v.clone())).collect();
    out.extend(jev.iter().map(|(k, v)| (k.clone(), v.clone())));
    out
}

/// `env` falso vale `{}`; verdadeiro e não objeto é erro com código.
fn env_in(entry: &Map<String, Value>, path: &Path) -> Result<Map<String, Value>, Error> {
    match entry.get("env").filter(|v| truthy(v)) {
        None => Ok(Map::new()),
        Some(Value::Object(env)) => Ok(env.clone()),
        Some(_) => Err(Error {
            status: 500,
            code: "erro_computer_control_io",
            msg: format!("env em {} não é um objeto JSON", path.display()),
            params: json!({"file": path.to_string_lossy(), "error": "env inválido"}),
        }),
    }
}

/// O modelo do Computer Use que o auxiliar do Jev vai trocar fica guardado (0600), um por arquivo de
/// origem: o de antes da primeira troca; os seguintes já são do auxiliar.
fn keep_old_llm(paths: &Paths, source: &Path, old: &Map<String, Value>, new: &Map<String, Value>) -> Result<(), Error> {
    let old: Map<String, Value> = LLM_VARS
        .into_iter()
        .filter_map(|key| old.get(key).filter(|v| truthy(v)).map(|v| (key.to_owned(), json!(python_string(v)))))
        .collect();
    if old.is_empty() || old.iter().all(|(key, value)| new.get(key) == Some(value)) {
        return Ok(());
    }
    let backup = paths.old_llm_file();
    let source = source.to_string_lossy().into_owned();
    update(&backup, true, |bytes| {
        let mut previous = parsed(&backup, bytes, false)?;
        if previous.contains_key(&source) {
            return Ok(None);
        }
        previous.insert(source.clone(), Value::Object(old.clone()));
        Ok(Some(previous))
    })
    .map(drop)
}

/// Trava, em ordem fixa, todo arquivo que um escritor desta entrada toca: duas gravações nunca se cruzam.
fn lock_all(paths: &Paths) -> Result<Vec<ConfigLock>, Error> {
    let mut files = paths.config_files();
    files.extend([paths.parked_file(), paths.old_llm_file()]);
    let mut files: Vec<PathBuf> = files.into_iter().map(|path| path.canonicalize().unwrap_or(path)).collect();
    files.sort();
    files.dedup();
    files
        .iter()
        .map(|path| {
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent).map_err(|e| Error::at(parent, &e))?;
            }
            lock_config(path).map_err(|e| Error::io(format!("{}: {e}", path.display())))
        })
        .collect()
}

/// Relê o arquivo, monta o conteúdo novo e grava se ele não mudou no meio; `None` não grava. Diz se
/// gravou. Detecta a mudança vista durante a montagem; contra quem não usa a trava não é CAS.
fn update(
    path: &Path,
    private: bool,
    mut build: impl FnMut(Option<&[u8]>) -> Result<Option<Map<String, Value>>, Error>,
) -> Result<bool, Error> {
    for _ in 0..3 {
        let link = std::fs::read_link(path).ok();
        let raw = read_raw(path)?;
        let Some(data) = build(raw.as_deref())? else {
            return Ok(false);
        };
        if read_raw(path)? != raw {
            continue;
        }
        return write_json(path, data, private, raw.as_deref(), link.as_deref()).map(|()| true);
    }
    Err(Error::io(format!("Arquivo continua mudando: {}; tentar novamente", path.display())))
}

fn read_raw(path: &Path) -> Result<Option<Vec<u8>>, Error> {
    match hangar_core::fs::read_replaced(path) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(unreadable(path, &error)),
    }
}

/// Ausente = `{}`.
fn parsed(path: &Path, bytes: Option<&[u8]>, empty_ok: bool) -> Result<Map<String, Value>, Error> {
    bytes.map_or_else(|| Ok(Map::new()), |bytes| object_of(path, bytes, empty_ok))
}

/// O JSON do Python (`indent=2`, ASCII; surrogate solto lido volta como estava), trocado de uma vez só
/// se o que sai do lugar for `expected`. `private` grava 0600 (tem chave); senão fica o modo que o
/// arquivo tinha, lido através do link.
pub fn write_json(
    path: &Path,
    data: Map<String, Value>,
    private: bool,
    expected: Option<&[u8]>,
    link: Option<&Path>,
) -> Result<(), Error> {
    let bytes = pyjson::dumps_indent(&Value::Object(data), 2, true).into_bytes();
    let mode = if private { 0o600 } else { mode(path).unwrap_or(0o644) };
    let parent = path.parent().ok_or_else(|| Error::io(format!("{}: arquivo sem pasta", path.display())))?;
    let io = |error: std::io::Error| Error::at(path, &error);
    std::fs::create_dir_all(parent).map_err(io)?;
    let mut file = tempfile::Builder::new().prefix(".hangar-config-").tempfile_in(parent).map_err(io)?;
    file.write_all(&bytes).map_err(io)?;
    file.as_file().sync_all().map_err(io)?;
    chmod(file.path(), mode).map_err(io)?;
    let file = file.into_temp_path();
    #[cfg(test)]
    race::fire(path);
    hangar_core::fs_replace::replace(&file, path, expected, link).map_err(Error::io)
}

fn mode(path: &Path) -> Option<u32> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        path.metadata().ok().map(|m| m.permissions().mode() & 0o777)
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        None
    }
}

fn chmod(path: &Path, mode: u32) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))
    }
    #[cfg(not(unix))]
    {
        let _ = (path, mode);
        Ok(())
    }
}

/// Escrita alheia no último instante antes da troca, para provar que ela é retida e não perdida.
#[cfg(test)]
pub(crate) mod race {
    use std::cell::RefCell;
    use std::path::{Path, PathBuf};

    type Hook = (PathBuf, Box<dyn FnOnce()>);
    thread_local! {
        static HOOK: RefCell<Option<Hook>> = const { RefCell::new(None) };
    }

    pub(crate) fn fire(path: &Path) {
        let hook = HOOK.with(|hook| {
            let mut hook = hook.borrow_mut();
            if hook.as_ref().is_some_and(|(target, _)| target == path) { hook.take().map(|(_, run)| run) } else { None }
        });
        if let Some(run) = hook {
            run();
        }
    }

    pub(crate) fn with<R>(path: &Path, hook: impl FnOnce() + 'static, action: impl FnOnce() -> R) -> R {
        HOOK.with(|slot| *slot.borrow_mut() = Some((path.to_owned(), Box::new(hook))));
        let result = action();
        HOOK.with(|slot| slot.borrow_mut().take());
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn paths(home: &Path) -> Paths {
        Paths::new(home.to_owned(), Vec::new())
    }

    fn jev() -> Map<String, Value> {
        Map::from_iter([("TYPESAFE_API_KEY".to_owned(), json!("fixture-key"))])
    }

    /// Movido de `config_sync/review_tests.rs`: o repasse do Jev valida a guardada antes de gravar e
    /// retém a escrita alheia que chega no último instante.
    #[test]
    fn jev_validates_before_publication_and_retains_external_replacement() {
        let home = tempfile::tempdir().unwrap();
        let paths = paths(home.path());
        let target = paths.main_file();
        let original = br#"{"mcpServers":{"hangar-computer-control":{"command":"tool","env":{"LOCAL":"kept"}}}}"#;
        std::fs::write(&target, original).unwrap();
        let parked = paths.parked_file();
        std::fs::create_dir_all(parked.parent().unwrap()).unwrap();
        std::fs::write(&parked, br#"{"env":"invalid"}"#).unwrap();
        assert!(sync_jev(&paths, &jev()).is_err());
        assert_eq!(std::fs::read(&target).unwrap(), original);
        std::fs::remove_file(&parked).unwrap();
        let outside = target.clone();
        let error = race::with(
            &target,
            move || std::fs::write(outside, br#"{"external":"preserved"}"#).unwrap(),
            || sync_jev(&paths, &jev()),
        )
        .unwrap_err();
        let saved = std::fs::read_dir(home.path())
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .find(|path| path.file_name().unwrap().to_string_lossy().starts_with(".hangar-recovery-"))
            .expect("versão deslocada retida");
        assert!(error.msg.contains(saved.to_str().unwrap()), "{}", error.msg);
        assert_eq!(std::fs::read(&saved).unwrap(), br#"{"external":"preserved"}"#);
        std::fs::write(&target, b"third writer").unwrap();
        assert_eq!(std::fs::read(&target).unwrap(), b"third writer");
        assert_eq!(std::fs::read(saved).unwrap(), br#"{"external":"preserved"}"#);
    }

    fn with_account(home: &Path) -> Paths {
        std::fs::create_dir_all(home.join("acc")).unwrap();
        Paths::new(home.to_owned(), vec![home.join("acc")])
    }

    fn entry() -> Entry {
        Entry { command: "/bin/hcu".into(), agent_config: String::new() }
    }

    fn active() -> &'static str {
        r#"{"mcpServers":{"hangar-computer-control":{"command":"c","env":{"LOCAL":"1"}}}}"#
    }

    fn recovered(dir: &Path) -> PathBuf {
        std::fs::read_dir(dir)
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .find(|path| path.file_name().unwrap().to_string_lossy().starts_with(".hangar-recovery-"))
            .expect("versão deslocada retida")
    }

    /// D2.7 no repasse do Jev: conta com `.claude.json` de 0 bytes é pulada, não derruba o resto.
    #[test]
    fn jev_skips_an_empty_account_file() {
        let home = tempfile::tempdir().unwrap();
        let paths = with_account(home.path());
        std::fs::write(paths.main_file(), active()).unwrap();
        std::fs::write(home.path().join("acc/.claude.json"), b"").unwrap();
        sync_jev(&paths, &jev()).unwrap();
        let main = std::fs::read_to_string(paths.main_file()).unwrap();
        assert!(main.contains("fixture-key"), "{main}");
        assert_eq!(std::fs::read(home.path().join("acc/.claude.json")).unwrap(), b"");
    }

    /// O modelo antigo só é guardado depois de tudo validado: guardada inválida não deixa backup.
    #[test]
    fn jev_keeps_the_old_model_only_after_validating_everything() {
        let home = tempfile::tempdir().unwrap();
        let paths = paths(home.path());
        std::fs::write(paths.main_file(), r#"{"mcpServers":{"hangar-computer-control":{"command":"c","env":{"LLM_MODEL":"velho"}}}}"#).unwrap();
        std::fs::create_dir_all(paths.parked_file().parent().unwrap()).unwrap();
        std::fs::write(paths.parked_file(), br#"{"env":"invalid"}"#).unwrap();
        let jev = Map::from_iter([("LLM_MODEL".to_owned(), json!("novo"))]);
        assert!(sync_jev(&paths, &jev).is_err());
        assert!(!paths.old_llm_file().exists());
    }

    /// O env do Jev é pedido com a trava já tomada.
    #[test]
    fn jev_env_is_read_under_the_lock() {
        let home = tempfile::tempdir().unwrap();
        let paths = paths(home.path());
        let lock = hangar_core::config_lock::lock_path(&paths.main_file());
        write_entry(&paths, &entry(), || {
            assert!(lock.is_dir(), "trava ausente ao ler o Jev");
            Ok(jev())
        })
        .unwrap();
        assert!(!lock.exists());
        assert!(std::fs::read_to_string(paths.main_file()).unwrap().contains("fixture-key"));
    }

    /// Falha no meio: o erro diz quais arquivos já foram gravados.
    #[test]
    fn write_entry_failure_midway_lists_what_was_written() {
        let home = tempfile::tempdir().unwrap();
        let paths = with_account(home.path());
        let account = home.path().join("acc/.claude.json");
        std::fs::write(paths.main_file(), b"{}").unwrap();
        std::fs::write(&account, br#"{"x":1}"#).unwrap();
        let swapped = account.clone();
        let error = race::with(
            &paths.main_file(),
            move || {
                std::fs::remove_file(&swapped).unwrap();
                std::fs::create_dir(&swapped).unwrap();
            },
            || write_entry(&paths, &entry(), || Ok(Map::new())),
        )
        .unwrap_err();
        assert_eq!(error.params["written"], json!([paths.main_file().to_string_lossy()]), "{}", error.params);
        assert!(std::fs::read_to_string(paths.main_file()).unwrap().contains(NAME));
    }

    #[test]
    fn remove_entry_failure_midway_lists_the_parked_and_written_files() {
        let home = tempfile::tempdir().unwrap();
        let paths = with_account(home.path());
        let account = home.path().join("acc/.claude.json");
        std::fs::write(paths.main_file(), active()).unwrap();
        std::fs::write(&account, active()).unwrap();
        let swapped = account.clone();
        let error = race::with(
            &paths.main_file(),
            move || {
                std::fs::remove_file(&swapped).unwrap();
                std::fs::create_dir(&swapped).unwrap();
            },
            || remove_entry(&paths),
        )
        .unwrap_err();
        let written = json!([paths.parked_file().to_string_lossy(), paths.main_file().to_string_lossy()]);
        assert_eq!(error.params["written"], written, "{}", error.params);
        assert!(!std::fs::read_to_string(paths.main_file()).unwrap().contains(NAME));
    }

    /// Escrita alheia no último instante antes de ligar: retida, nunca sobrescrita calada.
    #[test]
    fn write_entry_retains_an_external_last_moment_write() {
        let home = tempfile::tempdir().unwrap();
        let paths = paths(home.path());
        std::fs::write(paths.main_file(), b"{}").unwrap();
        let outside = paths.main_file();
        let error = race::with(
            &paths.main_file(),
            move || std::fs::write(outside, br#"{"external":true}"#).unwrap(),
            || write_entry(&paths, &entry(), || Ok(Map::new())),
        )
        .unwrap_err();
        let saved = recovered(home.path());
        assert!(error.msg.contains(saved.to_str().unwrap()), "{}", error.msg);
        assert_eq!(std::fs::read(saved).unwrap(), br#"{"external":true}"#);
    }

    /// Idem ao desligar: a guardada já gravada aparece no erro.
    #[test]
    fn remove_entry_retains_an_external_last_moment_write() {
        let home = tempfile::tempdir().unwrap();
        let paths = paths(home.path());
        std::fs::write(paths.main_file(), active()).unwrap();
        let outside = paths.main_file();
        let error = race::with(
            &paths.main_file(),
            move || std::fs::write(outside, br#"{"external":true}"#).unwrap(),
            || remove_entry(&paths),
        )
        .unwrap_err();
        let saved = recovered(home.path());
        assert!(error.msg.contains(saved.to_str().unwrap()), "{}", error.msg);
        assert_eq!(std::fs::read(saved).unwrap(), br#"{"external":true}"#);
        assert_eq!(error.params["written"], json!([paths.parked_file().to_string_lossy()]), "{}", error.params);
    }

    /// O surrogate solto que o Claude Code grava volta igual depois da troca da entrada.
    #[test]
    fn lone_surrogate_is_written_back() {
        let home = tempfile::tempdir().unwrap();
        let paths = paths(home.path());
        std::fs::write(paths.main_file(), r#"{"a": "x\ud800y"}"#).unwrap();
        let entry = Entry { command: "/bin/hcu".into(), agent_config: String::new() };
        write_entry(&paths, &entry, || Ok(Map::new())).unwrap();
        let text = std::fs::read_to_string(paths.main_file()).unwrap();
        assert!(text.contains(r#""a": "x\ud800y""#), "{text}");
        assert!(text.contains(NAME), "{text}");
    }
}
