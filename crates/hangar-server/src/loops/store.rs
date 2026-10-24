//! Sidecars do loop (`.hangar-loop/<nome>.json`) e do vínculo `then` (`.hangar-chain/<nome>.json`)
//! na pasta de config do Claude do servidor, no formato que o Python grava e lê. O sidecar do loop
//! fica como mapa JSON (ordem preservada): campo desconhecido gravado por outra versão não some.
use serde_json::{json, Map, Value};
use std::io;
use std::path::{Path, PathBuf};

pub type Loop = Map<String, Value>;

/// Loop que ainda pede tique ou resposta do usuário.
pub const ACTIVE: [&str; 3] = ["running", "paused_awaiting", "done_claimed"];
/// Só estas transições avisam no celular: re-prompt e retomada da pausa nunca.
pub const NOTIFY: [&str; 5] = ["done", "done_claimed", "stopped", "exhausted", "failed"];
pub const HISTORY_MAX: usize = 20;

/// Um read-modify-write por vez no sidecar de loop de cada sessão (tique, rotas, vigia); o comando
/// de conferência roda fora dela. É por sessão: a entrega na fila (adoção, drain) acontece sob a
/// trava, e o ator travado de uma sessão não pode segurar o loop das outras.
pub async fn lock(name: &str) -> tokio::sync::OwnedMutexGuard<()> {
    static LOCKS: std::sync::LazyLock<std::sync::Mutex<std::collections::HashMap<String, std::sync::Arc<tokio::sync::Mutex<()>>>>> =
        std::sync::LazyLock::new(Default::default);
    let one = LOCKS.lock().unwrap_or_else(|e| e.into_inner()).entry(file_stem(name)).or_default().clone();
    one.lock_owned().await
}

pub fn status(d: &Loop) -> &str { d.get("status").and_then(Value::as_str).unwrap_or("") }

pub fn is_active(d: &Loop) -> bool { ACTIVE.contains(&status(d)) }

/// Inteiro do sidecar (`iter`, `max_iters`); o float de valor inteiro também vale.
pub fn int(d: &Loop, key: &str) -> i64 {
    d.get(key).and_then(|v| v.as_i64().or_else(|| v.as_f64().filter(|f| f.fract() == 0.0).map(|f| f as i64))).unwrap_or(0)
}

/// `bool()` do Python para os campos que o cliente manda.
pub fn truthy(v: Option<&Value>) -> bool {
    match v {
        None | Some(Value::Null) => false,
        Some(Value::Bool(b)) => *b,
        Some(Value::Number(n)) => n.as_f64() != Some(0.0),
        Some(Value::String(s)) => !s.is_empty(),
        Some(Value::Array(a)) => !a.is_empty(),
        Some(Value::Object(o)) => !o.is_empty(),
    }
}

pub fn new_loop(goal: &str, check_cmd: Option<&str>, max_iters: i64, require_branch: bool, now: f64) -> Loop {
    let Value::Object(d) = json!({
        "goal": goal, "check_cmd": check_cmd, "max_iters": max_iters, "require_branch": require_branch,
        "status": "running", "iter": 0, "goal_entry_id": null, "goal_delivered_ts": null, "history": [],
        "started_ts": now, "ended_ts": null, "ended_reason": null,
    }) else { unreachable!() };
    d
}

/// O `_sanitize` do Python: o mesmo nome de arquivo que ele usa.
pub fn file_stem(name: &str) -> String { crate::groups::store::file_stem(name) }

fn remove(path: &Path) -> io::Result<()> {
    match std::fs::remove_file(path) {
        Err(e) if e.kind() != io::ErrorKind::NotFound => Err(e),
        _ => Ok(()),
    }
}

fn write_atomic(path: &Path, tmp: &Path, body: &[u8]) -> io::Result<()> {
    if let Some(dir) = path.parent() { std::fs::create_dir_all(dir)?; }
    std::fs::write(tmp, body)?;
    std::fs::rename(tmp, path)
}

fn read_object(path: &Path) -> Option<Loop> {
    match serde_json::from_slice(&std::fs::read(path).ok()?) {
        Ok(Value::Object(m)) => Some(m),
        _ => None,
    }
}

#[derive(Clone)]
pub struct Store { claude: PathBuf }

impl Store {
    pub fn new(claude: &Path) -> Self { Self { claude: claude.to_owned() } }

    pub fn loop_dir(&self) -> PathBuf { self.claude.join(".hangar-loop") }
    fn chain_dir(&self) -> PathBuf { self.claude.join(".hangar-chain") }
    fn loop_file(&self, name: &str) -> PathBuf { self.loop_dir().join(format!("{}.json", file_stem(name))) }
    fn chain_file(&self, name: &str) -> PathBuf { self.chain_dir().join(format!("{}.json", file_stem(name))) }

    /// Ausente, torto ou que não é objeto: sem loop, como o `LoopLink.get` do Python.
    pub fn get(&self, name: &str) -> Option<Loop> { read_object(&self.loop_file(name)) }

    pub fn set(&self, name: &str, d: &Loop) -> io::Result<()> {
        let path = self.loop_file(name);
        let body = serde_json::to_vec(d).map_err(io::Error::other)?;
        write_atomic(&path, &path.with_extension("tmp"), &body)
    }

    /// `None`: não havia sidecar, nada gravado.
    pub fn update(&self, name: &str, fields: Loop) -> io::Result<Option<Loop>> {
        let Some(mut d) = self.get(name) else { return Ok(None) };
        d.extend(fields);
        self.set(name, &d)?;
        Ok(Some(d))
    }

    /// Sessão que acabou (fechada, ou outra nasce no nome): a nova não herda o loop.
    pub fn forget(&self, name: &str) -> io::Result<()> {
        let path = self.loop_file(name);
        remove(&path.with_extension("tmp"))?;
        remove(&path)
    }

    /// Nomes (já sanitizados) com sidecar de loop.
    pub fn stems(&self) -> Vec<String> {
        let Ok(dir) = std::fs::read_dir(self.loop_dir()) else { return Vec::new() };
        let mut out: Vec<String> = dir.flatten().filter_map(|e| {
            let p = e.path();
            (p.extension().is_some_and(|x| x == "json")).then(|| p.file_stem()?.to_str().map(str::to_owned)).flatten()
        }).collect();
        out.sort();
        out
    }

    pub fn chain(&self, name: &str) -> Option<Loop> { read_object(&self.chain_file(name)) }

    pub fn set_chain(&self, name: &str, target: &str, text: &str) -> io::Result<()> {
        let path = self.chain_file(name);
        let body = serde_json::to_vec(&json!({"target": target, "text": text})).map_err(io::Error::other)?;
        write_atomic(&path, &path.with_extension("json.tmp"), &body)
    }

    pub fn clear_chain(&self, name: &str) -> io::Result<()> {
        let path = self.chain_file(name);
        remove(&path)?;
        remove(&path.with_extension("json.tmp"))
    }

    /// Renomear leva o loop e o vínculo da sessão, e quem encadeava para o nome velho passa a
    /// apontar para o novo.
    pub fn rename(&self, old: &str, new: &str) -> io::Result<()> {
        for (from, to, tmp) in [(self.loop_file(old), self.loop_file(new), "tmp"), (self.chain_file(old), self.chain_file(new), "json.tmp")] {
            remove(&from.with_extension(tmp))?;
            match std::fs::rename(&from, &to) {
                Err(e) if e.kind() != io::ErrorKind::NotFound => return Err(e),
                _ => {}
            }
        }
        let Ok(dir) = std::fs::read_dir(self.chain_dir()) else { return Ok(()) };
        for entry in dir.flatten() {
            let path = entry.path();
            if path.extension().is_none_or(|x| x != "json") { continue; }
            let Some(mut link) = read_object(&path) else { continue };
            if link.get("target").and_then(Value::as_str) != Some(old) { continue; }
            link.insert("target".into(), json!(new));
            let body = serde_json::to_vec(&link).map_err(io::Error::other)?;
            write_atomic(&path, &path.with_extension("json.tmp"), &body)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> (tempfile::TempDir, Store) {
        let dir = tempfile::tempdir().unwrap();
        let s = Store::new(dir.path());
        (dir, s)
    }

    #[test]
    fn paths_and_format_are_the_python_ones() {
        let (dir, s) = store();
        s.set("a b/ç", &new_loop("g", None, 10, true, 5.0)).unwrap();
        let raw = std::fs::read(dir.path().join(".hangar-loop/a-b--.json")).unwrap();
        let v: Value = serde_json::from_slice(&raw).unwrap();
        let keys: Vec<&str> = v.as_object().unwrap().keys().map(String::as_str).collect();
        assert_eq!(keys, ["goal", "check_cmd", "max_iters", "require_branch", "status", "iter", "goal_entry_id",
            "goal_delivered_ts", "history", "started_ts", "ended_ts", "ended_reason"]);
        s.set_chain("x", "y", "t").unwrap();
        assert_eq!(serde_json::from_slice::<Value>(&std::fs::read(dir.path().join(".hangar-chain/x.json")).unwrap()).unwrap(),
            json!({"target": "y", "text": "t"}));
    }

    #[test]
    fn broken_or_non_object_sidecar_is_no_loop() {
        let (dir, s) = store();
        std::fs::create_dir_all(dir.path().join(".hangar-loop")).unwrap();
        for raw in ["{quebrado", "[1]", "null", ""] {
            std::fs::write(dir.path().join(".hangar-loop/s.json"), raw).unwrap();
            assert!(s.get("s").is_none(), "{raw}");
        }
        assert!(s.update("s", Loop::new()).unwrap().is_none());
    }

    /// B1 e B8: o loop e o vínculo vão com o nome; o vínculo de outra sessão segue o alvo renomeado.
    #[test]
    fn rename_moves_the_loop_and_retargets_chains() {
        let (_dir, s) = store();
        s.set("old", &new_loop("g", None, 3, true, 1.0)).unwrap();
        s.set_chain("old", "z", "segue").unwrap();
        s.set_chain("src", "old", "roda os testes").unwrap();
        s.set_chain("other", "keep", "x").unwrap();
        s.rename("old", "new").unwrap();
        assert!(s.get("old").is_none());
        assert_eq!(s.get("new").unwrap()["max_iters"], 3);
        assert!(s.chain("old").is_none());
        assert_eq!(s.chain("new").unwrap()["target"], "z");
        assert_eq!(s.chain("src").unwrap()["target"], "new");
        assert_eq!(s.chain("src").unwrap()["text"], "roda os testes");
        assert_eq!(s.chain("other").unwrap()["target"], "keep");
    }

    #[test]
    fn forget_removes_only_the_loop() {
        let (_dir, s) = store();
        s.set("a", &new_loop("g", None, 3, true, 1.0)).unwrap();
        s.forget("a").unwrap();
        s.forget("a").unwrap();
        assert!(s.get("a").is_none());
        assert_eq!(s.stems(), Vec::<String>::new());
    }

    proptest::proptest! {
        #[test]
        fn any_sidecar_bytes_never_panic(raw in proptest::collection::vec(proptest::prelude::any::<u8>(), 0..256)) {
            let (dir, s) = store();
            std::fs::create_dir_all(dir.path().join(".hangar-loop")).unwrap();
            std::fs::write(dir.path().join(".hangar-loop/s.json"), &raw).unwrap();
            if let Some(d) = s.get("s") { let _ = (status(&d), int(&d, "iter"), is_active(&d)); }
            std::fs::create_dir_all(dir.path().join(".hangar-chain")).unwrap();
            std::fs::write(dir.path().join(".hangar-chain/c.json"), &raw).unwrap();
            let _ = s.rename("s", "t");
        }
    }
}
