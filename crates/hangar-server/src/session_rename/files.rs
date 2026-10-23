//! Arquivos guardados pelo nome da sessão. Cada função devolve se moveu algo: quem desfaz chama a
//! mesma função com os nomes trocados.
use std::fs::File;
use std::io::{self, Error, ErrorKind};
use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use crate::list::discover_other::sanitize_session_name;
use crate::runtime::queue::{atomic_write, read_replaced, replace};

fn exists(path: &Path) -> io::Result<bool> { path.try_exists() }

fn lock_file(path: &Path) -> io::Result<File> {
    if let Some(dir) = path.parent() { std::fs::create_dir_all(dir)?; }
    let file = std::fs::OpenOptions::new().read(true).append(true).create(true).open(path)?;
    file.lock()?;
    Ok(file)
}

/// As travas `<nome>.lock` dos dois nomes, na ordem do `sorted({Path})` do Python (sem caixa no
/// Windows): o mesmo par travado pelos dois lados nunca se cruza.
fn lock_both(dir: &Path, old: &str, new: &str) -> io::Result<Vec<File>> {
    let mut paths: Vec<PathBuf> = [old, new].iter().map(|n| dir.join(format!("{}.lock", sanitize_session_name(n)))).collect();
    let order = |p: &PathBuf| if cfg!(windows) { p.to_string_lossy().to_lowercase() } else { p.to_string_lossy().into_owned() };
    paths.sort_by_key(order);
    paths.dedup_by_key(|p| order(p));
    paths.iter().map(|p| lock_file(p)).collect()
}

/// Sidecar `<velho>.json` → `<novo>.json` com o campo `name` novo, sob as duas travas que o Rust e o
/// Python pegam ao gravar sidecar. Destino que existe recusa (`AlreadyExists`), sem tocar em nada.
pub fn move_sidecar(dir: &Path, old: &str, new: &str) -> io::Result<bool> {
    let _locks = lock_both(dir, old, new)?;
    let (src, dst) = (crate::runtime::codex_sidecar::path(dir, old), crate::runtime::codex_sidecar::path(dir, new));
    if !exists(&src)? { return Ok(false); }
    if exists(&dst)? { return Err(Error::new(ErrorKind::AlreadyExists, "sidecar do nome novo já existe")); }
    let mut meta: Value = serde_json::from_slice(&read_replaced(&src)?).ok().filter(Value::is_object)
        .ok_or_else(|| Error::new(ErrorKind::InvalidData, "sidecar ilegível"))?;
    meta["name"] = json!(new);
    replace(&src, &dst)?;
    if let Err(error) = atomic_write(&dst, &serde_json::to_vec(&meta)?) {
        // O nome de dentro não foi trocado: o arquivo volta, para a sessão seguir inteira no velho.
        let _ = replace(&dst, &src);
        return Err(error);
    }
    Ok(true)
}

/// Arquivo simples guardado pelo nome (fila sem dono no runtime, vínculo `then`, dossiê do bastão).
/// O `.tmp` meio escrito do velho não migra. Destino que existe recusa.
pub fn move_file(src: &Path, dst: &Path) -> io::Result<bool> {
    if src == dst || !exists(src)? { return Ok(false); }
    if exists(dst)? { return Err(Error::new(ErrorKind::AlreadyExists, "o arquivo do nome novo já existe")); }
    if let Some(dir) = dst.parent() { std::fs::create_dir_all(dir)?; }
    replace(src, dst)?;
    let tmp = src.with_extension(format!("{}.tmp", src.extension().and_then(|e| e.to_str()).unwrap_or_default()));
    match std::fs::remove_file(tmp) {
        Err(error) if error.kind() != ErrorKind::NotFound => Err(error),
        _ => Ok(true),
    }
}

/// `pqueue._sanitize`: o nome dos arquivos da fila, do `then` e do bastão.
pub fn stem(name: &str) -> String {
    name.chars().map(|c| if c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-') { c } else { '-' }).collect()
}

pub fn queue_file(claude_base: &Path, name: &str) -> PathBuf { crate::locate::queue_path(claude_base, name) }
pub fn then_file(claude_base: &Path, name: &str) -> PathBuf { claude_base.join(".hangar-chain").join(format!("{}.json", stem(name))) }
pub fn bastao_file(claude_base: &Path, name: &str) -> PathBuf { claude_base.join(".hangar-bastao").join(format!("{}.md", stem(name))) }

/// `guest_users.rename_session`: o dono convidado da sessão passa ao nome novo, sob a `guests.lock`
/// que o Python também pega. Sem dono gravado não há o que mudar.
pub fn move_guest(claude_base: &Path, old: &str, new: &str) -> io::Result<bool> {
    let path = claude_base.join("guests.json");
    if !exists(&path)? { return Ok(false); }
    let _lock = lock_file(&claude_base.join("guests.lock"))?;
    let mut state: Value = serde_json::from_slice(&read_replaced(&path)?).ok().filter(Value::is_object)
        .ok_or_else(|| Error::new(ErrorKind::InvalidData, "guests.json ilegível"))?;
    let Some(sessions) = state.get_mut("sessions").and_then(Value::as_object_mut) else { return Ok(false) };
    if sessions.contains_key(new) { return Err(Error::new(ErrorKind::AlreadyExists, "o nome novo já tem dono convidado")); }
    let Some(entry) = sessions.remove(old) else { return Ok(false) };
    sessions.insert(new.into(), entry);
    atomic_write(&path, &serde_json::to_vec_pretty(&state)?)?;
    Ok(true)
}

/// O spec da sessão (`session-specs/<chave>.json`) e a fotografia do restore passam ao nome novo: depois
/// do reboot ela volta com ele. A chave sai da fotografia, que guarda todas as sessões por chave.
pub fn move_spec(home: &Path, old: &str, new: &str) -> io::Result<bool> {
    let snapshot = home.join(".hangar").join("session-restore.json");
    if !exists(&snapshot)? { return Ok(false); }
    let mut snap: Value = serde_json::from_slice(&std::fs::read(&snapshot)?).ok().filter(Value::is_object)
        .ok_or_else(|| Error::new(ErrorKind::InvalidData, "fotografia das sessões ilegível"))?;
    let Some(sessions) = snap.get_mut("sessions").and_then(Value::as_object_mut) else { return Ok(false) };
    let Some((key, entry)) = sessions.iter_mut().find(|(_, e)| e["name"] == old) else { return Ok(false) };
    entry["name"] = json!(new);
    let key = key.clone();
    let spec_path = crate::session_open::record::path(home, &key).ok().filter(|p| p.exists());
    let spec = match &spec_path {
        Some(p) => Some(serde_json::from_slice::<Value>(&read_replaced(p)?).ok().filter(Value::is_object)
            .ok_or_else(|| Error::new(ErrorKind::InvalidData, "spec ilegível"))?),
        None => None,
    };
    atomic_write(&snapshot, &serde_json::to_vec_pretty(&snap)?)?;
    if let (Some(p), Some(mut spec)) = (spec_path, spec) {
        spec["name"] = json!(new);
        if let Err(error) = atomic_write(&p, &serde_json::to_vec_pretty(&spec)?) {
            // O spec não mudou: a fotografia volta, para os dois seguirem no nome antigo.
            if let Some(entry) = snap["sessions"].get_mut(&key) { entry["name"] = json!(old); }
            atomic_write(&snapshot, &serde_json::to_vec_pretty(&snap)?)?;
            return Err(error);
        }
    }
    Ok(true)
}

/// Pasta das trocas de conversa (Claude → Codex), que continuam no Python.
fn transfers(home: &Path) -> PathBuf { home.join(".hangar").join("conversation-transfers") }

fn index_path(home: &Path, name: &str) -> PathBuf {
    let digest: String = ring::digest::digest(&ring::digest::SHA256, name.as_bytes()).as_ref().iter().map(|b| format!("{b:02x}")).collect();
    transfers(home).join("indexes").join(format!("session-{digest}.json"))
}

fn index_ids(home: &Path, name: &str) -> io::Result<Vec<String>> {
    match read_replaced(&index_path(home, name)) {
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(Vec::new()),
        Err(error) => Err(error),
        Ok(raw) => serde_json::from_slice::<Vec<String>>(&raw).map_err(|_| Error::new(ErrorKind::InvalidData, "índice de transferência inválido")),
    }
}

fn record(home: &Path, id: &str) -> io::Result<Option<Value>> {
    if id.len() != 36 || !id.bytes().all(|b| b.is_ascii_hexdigit() || b == b'-') { return Err(Error::new(ErrorKind::InvalidData, "id de transferência inválido")); }
    match read_replaced(&transfers(home).join(format!("{id}.json"))) {
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
        Ok(raw) => serde_json::from_slice(&raw).map(Some).map_err(|_| Error::new(ErrorKind::InvalidData, "registro de transferência ilegível")),
    }
}

const TERMINAL_PHASES: [&str; 3] = ["complete", "rejected", "rolled_back"];

/// `transfer_active` pelo índice do nome: registro com esse nome numa fase não final.
pub fn transfer_busy(home: &Path, name: &str) -> io::Result<bool> {
    for id in index_ids(home, name)? {
        if let Some(r) = record(home, &id)? && r["name"] == name && !TERMINAL_PHASES.contains(&r["phase"].as_str().unwrap_or_default()) {
            return Ok(true);
        }
    }
    Ok(false)
}

/// `_belongs` do Python para registro em fase final: marcado no sidecar da sessão, ou da mesma vida.
fn belongs(record: &Value, life: Option<&str>, meta: Option<&Value>) -> bool {
    let marked = meta.is_some_and(|m| m["transfer_id"] == record["id"]);
    if record["phase"] == "complete" {
        let destination = &record["destination_meta"];
        if !marked && life.is_none_or(|life| destination["lifecycle_id"] != life) { return false; }
        if let Some(meta) = meta && meta["provider"] == "codex" {
            let thread = destination["thread_id"].as_str().or_else(|| record["boundary"]["thread_id"].as_str());
            return thread.is_some() && meta["thread_id"].as_str() == thread;
        }
        return true;
    }
    if marked { return true; }
    let Some(life) = life else { return false };
    if record["source_life"] != life { return false; }
    let origin = record["origin_meta"]["session_id"].as_str();
    !(meta.is_some_and(|m| m["session_id"].as_str().is_some_and(|sid| origin.is_some_and(|o| o != sid))))
}

/// `rename_transfer`: só a associação mais recente que confere acompanha (o índice do nome novo ganha o
/// id e o registro passa a ter o nome novo), e nenhuma se o nome novo já tem uma. Sob a `.lock` da pasta,
/// a mesma do Python. `life`/`meta`: a vida e o sidecar da sessão já no nome novo.
pub fn move_transfer(home: &Path, old: &str, new: &str, life: Option<&str>, meta: Option<&Value>) -> io::Result<bool> {
    if !exists(&transfers(home))? { return Ok(false); }
    let _lock = lock_file(&transfers(home).join(".lock"))?;
    for id in index_ids(home, new)? {
        if let Some(r) = record(home, &id)? && r["name"] == new && belongs(&r, life, meta) { return Ok(false); }
    }
    for id in index_ids(home, old)?.into_iter().rev() {
        let Some(mut r) = record(home, &id)? else { continue };
        if r["name"] != old || !belongs(&r, life, meta) { continue; }
        let before = index_ids(home, new)?;
        let mut ids = before.clone();
        if !ids.contains(&id) { ids.push(id.clone()); atomic_write(&index_path(home, new), &serde_json::to_vec(&ids)?)?; }
        r["name"] = json!(new);
        if let Err(error) = atomic_write(&transfers(home).join(format!("{id}.json")), &serde_json::to_vec(&r)?) {
            // Índice aditivo de volta ao de antes: o registro segue no nome antigo.
            if ids != before { atomic_write(&index_path(home, new), &serde_json::to_vec(&before)?)?; }
            return Err(error);
        }
        return Ok(true);
    }
    Ok(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(path: &Path, value: Value) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, serde_json::to_vec(&value).unwrap()).unwrap();
    }

    #[test]
    fn sidecar_moves_with_new_name_and_keeps_every_other_field() {
        let dir = tempfile::tempdir().unwrap();
        write(&dir.path().join("a.json"), json!({"key": "k1", "name": "a", "thread_id": "t", "extra": {"name": "x"}}));
        assert!(move_sidecar(dir.path(), "a", "b").unwrap());
        assert!(!dir.path().join("a.json").exists());
        let moved: Value = serde_json::from_slice(&std::fs::read(dir.path().join("b.json")).unwrap()).unwrap();
        assert_eq!(moved, json!({"key": "k1", "name": "b", "thread_id": "t", "extra": {"name": "x"}}));
        // `hangar-send` lê a primeira ocorrência de "name": a de topo continua antes da aninhada.
        let raw = String::from_utf8(std::fs::read(dir.path().join("b.json")).unwrap()).unwrap();
        assert!(raw.find("\"name\":\"b\"").unwrap() < raw.find("\"name\":\"x\"").unwrap());
    }

    #[test]
    fn sidecar_never_overwrites_the_new_name() {
        let dir = tempfile::tempdir().unwrap();
        write(&dir.path().join("a.json"), json!({"key": "k1", "name": "a"}));
        write(&dir.path().join("b.json"), json!({"key": "k2", "name": "b"}));
        assert_eq!(move_sidecar(dir.path(), "a", "b").unwrap_err().kind(), ErrorKind::AlreadyExists);
        let other: Value = serde_json::from_slice(&std::fs::read(dir.path().join("b.json")).unwrap()).unwrap();
        assert_eq!(other["key"], "k2");
        assert!(dir.path().join("a.json").exists());
    }

    #[test]
    fn missing_sidecar_is_nothing_to_move() {
        let dir = tempfile::tempdir().unwrap();
        assert!(!move_sidecar(dir.path(), "a", "b").unwrap());
    }

    #[test]
    fn plain_file_moves_drops_the_old_tmp_and_refuses_an_existing_target() {
        let dir = tempfile::tempdir().unwrap();
        let (a, b) = (queue_file(dir.path(), "a"), queue_file(dir.path(), "b"));
        write(&a, json!({"id": 1}));
        std::fs::write(a.with_extension("jsonl.tmp"), b"half").unwrap();
        assert!(move_file(&a, &b).unwrap());
        assert!(!a.exists() && !a.with_extension("jsonl.tmp").exists() && b.exists());
        assert!(!move_file(&a, &b).unwrap(), "sem fila no velho, nada é criado");
        write(&a, json!({"id": 2}));
        assert_eq!(move_file(&a, &b).unwrap_err().kind(), ErrorKind::AlreadyExists);
    }

    #[test]
    fn names_of_queue_then_and_bastao_follow_the_queue_sanitizer() {
        let base = Path::new("/c");
        assert_eq!(then_file(base, "a b.c"), base.join(".hangar-chain").join("a-b.c.json"));
        assert_eq!(bastao_file(base, "a b.c"), base.join(".hangar-bastao").join("a-b.c.md"));
        assert_eq!(queue_file(base, "a b.c"), base.join(".hangar-queue").join("a-b.c.jsonl"));
    }

    #[test]
    fn guest_owner_follows_the_rename() {
        let dir = tempfile::tempdir().unwrap();
        write(&dir.path().join("guests.json"), json!({"guests": [], "sessions": {"a": {"guest": "g1", "life": "k:1"}}}));
        assert!(move_guest(dir.path(), "a", "b").unwrap());
        let state: Value = serde_json::from_slice(&std::fs::read(dir.path().join("guests.json")).unwrap()).unwrap();
        assert_eq!(state["sessions"], json!({"b": {"guest": "g1", "life": "k:1"}}));
        assert!(!move_guest(dir.path(), "zz", "y").unwrap());
    }

    #[test]
    fn spec_and_restore_snapshot_take_the_new_name() {
        let home = tempfile::tempdir().unwrap();
        let key = "0123456789abcdef0123456789abcdef";
        write(&home.path().join(".hangar/session-restore.json"), json!({"sessions": {key: {"name": "a", "provider": "claude", "conversation": null}}}));
        write(&home.path().join(format!(".hangar/session-specs/{key}.json")), json!({"key": key, "name": "a"}));
        assert!(move_spec(home.path(), "a", "b").unwrap());
        let snap: Value = serde_json::from_slice(&std::fs::read(home.path().join(".hangar/session-restore.json")).unwrap()).unwrap();
        assert_eq!(snap["sessions"][key]["name"], "b");
        let spec: Value = serde_json::from_slice(&std::fs::read(home.path().join(format!(".hangar/session-specs/{key}.json"))).unwrap()).unwrap();
        assert_eq!(spec["name"], "b");
    }

    fn transfer(home: &Path, id: &str, name: &str, phase: &str, life: &str) {
        let base = home.join(".hangar/conversation-transfers");
        write(&base.join(format!("{id}.json")), json!({"id": id, "name": name, "source_life": life, "phase": phase, "source": null,
            "origin_meta": {}, "destination_meta": {"lifecycle_id": life}, "boundary": null, "error_code": null}));
        write(&index_path(home, name), json!([id]));
    }

    #[test]
    fn finished_transfer_of_this_life_follows_the_name() {
        let home = tempfile::tempdir().unwrap();
        let id = "11111111-2222-3333-4444-555555555555";
        transfer(home.path(), id, "a", "complete", "k:key1");
        assert!(move_transfer(home.path(), "a", "b", Some("k:key1"), None).unwrap());
        assert_eq!(index_ids(home.path(), "b").unwrap(), vec![id.to_owned()]);
        let moved = record(home.path(), id).unwrap().unwrap();
        assert_eq!(moved["name"], "b");
    }

    #[test]
    fn transfer_of_another_life_stays() {
        let home = tempfile::tempdir().unwrap();
        transfer(home.path(), "11111111-2222-3333-4444-555555555555", "a", "complete", "k:other");
        assert!(!move_transfer(home.path(), "a", "b", Some("k:key1"), None).unwrap());
    }

    #[test]
    fn transfer_in_progress_is_busy() {
        let home = tempfile::tempdir().unwrap();
        transfer(home.path(), "11111111-2222-3333-4444-555555555555", "a", "source_stopped", "k:key1");
        assert!(transfer_busy(home.path(), "a").unwrap());
        assert!(!transfer_busy(home.path(), "b").unwrap());
    }
}
