use base64::Engine;
use ring::{
    digest,
    rand::{SecureRandom, SystemRandom},
};
use serde_json::{Value, json};
use std::{
    io,
    path::{Path, PathBuf},
    sync::{Arc, LazyLock, Mutex},
};
use subtle::ConstantTimeEq;

#[derive(Debug, Clone)]
pub struct Failure {
    pub os_error: Option<i32>,
    pub status: u16,
    pub body: Value,
}
impl Failure {
    pub fn code(status: u16, code: &str, message: &str) -> Self {
        Self {
            os_error: None,
            status,
            body: json!({"detail":{"code":code,"params":{},"msg":message}}),
        }
    }
    pub fn with_io(mut self, error: &io::Error) -> Self {
        self.os_error = error.raw_os_error();
        self
    }
    pub fn io(error: io::Error) -> Self {
        Self::code(
            500,
            "erro_convidados_ilegivel",
            "o arquivo de convidados está ilegível; corrija-o antes de mudar convidados",
        ).with_io(&error)
    }
}
pub type Result<T> = std::result::Result<T, Failure>;
pub fn hash(token: &[u8]) -> String {
    digest::digest(&digest::SHA256, token)
        .as_ref()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}
pub fn random(n: usize) -> Vec<u8> {
    let mut bytes = vec![0; n];
    SystemRandom::new()
        .fill(&mut bytes)
        .expect("gerador aleatório do sistema");
    bytes
}
pub fn token() -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(random(32))
}
pub fn id() -> String {
    random(8).iter().map(|b| format!("{b:02x}")).collect()
}

pub fn read(path: &Path) -> io::Result<Option<Value>> {
    match crate::runtime::queue::read_replaced(path) {
        Ok(bytes) => serde_json::from_slice(&bytes)
            .map(Some)
            .map_err(io::Error::other),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e),
    }
}
/// O nome único e a permissão fechada existem antes de publicar o arquivo.
pub fn write(path: &Path, value: &Value) -> io::Result<()> {
    write_bytes(path, &serde_json::to_vec_pretty(value)?)
}
pub(super) fn write_bytes(path: &Path, bytes: &[u8]) -> io::Result<()> {
    use std::io::Write;
    let parent = path
        .parent()
        .ok_or_else(|| io::Error::other("arquivo sem pasta"))?;
    std::fs::create_dir_all(parent)?;
    let mut file = tempfile::NamedTempFile::new_in(parent)?;
    file.write_all(bytes)?;
    file.as_file().sync_all()?;
    // Como o `persist`: no Windows o arquivo deixa de ser marcado temporário antes da troca.
    let (handle, temporary) = file.keep().map_err(|error| error.error)?;
    drop(handle);
    crate::runtime::queue::replace(&temporary, path).inspect_err(|_| {
        let _ = std::fs::remove_file(&temporary);
    })
}

#[derive(Clone, Debug)]
pub struct Guest {
    pub id: String,
    pub name: String,
    pub root: PathBuf,
    pub sees_owner: bool,
    pub owner_sees: bool,
}
#[derive(Clone, Debug, PartialEq, Eq)]
struct FileStamp {
    modified: std::time::SystemTime,
    size: u64,
    identity: Option<(u64, u64)>,
}
fn file_stamp(path: &Path) -> Option<FileStamp> {
    let metadata = std::fs::metadata(path).ok()?;
    #[cfg(unix)]
    let identity = {
        use std::os::unix::fs::MetadataExt;
        Some((metadata.dev(), metadata.ino()))
    };
    #[cfg(not(unix))]
    let identity = None;
    Some(FileStamp {
        modified: metadata.modified().ok()?,
        size: metadata.len(),
        identity,
    })
}

#[derive(Debug)]
pub struct Store {
    pub path: PathBuf,
    lock: Mutex<()>,
    last: Mutex<Option<Value>>,
    loaded: Mutex<Option<(FileStamp, Value)>>,
}
impl Store {
    pub fn shared(path: PathBuf) -> Arc<Self> {
        static STORES: LazyLock<Mutex<std::collections::HashMap<PathBuf, Arc<Store>>>> = LazyLock::new(Mutex::default);
        let mut stores = STORES.lock().unwrap_or_else(|e| e.into_inner());
        stores.entry(path.clone()).or_insert_with(|| Arc::new(Self::new(path))).clone()
    }
    pub fn new(path: PathBuf) -> Self {
        Self {
            path,
            lock: Mutex::new(()),
            last: Mutex::new(None),
            loaded: Mutex::new(None),
        }
    }
    fn load(&self) -> Result<Value> {
        let stamp = file_stamp(&self.path);
        if let Some(stamp) = &stamp {
            let loaded = self.loaded.lock().unwrap_or_else(|e| e.into_inner());
            if let Some((previous, doc)) = loaded.as_ref().filter(|(previous, _)| previous == stamp)
            {
                let _ = previous;
                return Ok(doc.clone());
            }
        }
        // Uma falha transitória precisa tentar a fonte na próxima consulta, sem cache de erro.
        *self.loaded.lock().unwrap_or_else(|e| e.into_inner()) = None;
        let doc = read(&self.path)
            .map_err(Failure::io)?
            .unwrap_or_else(|| json!({"guests":[],"sessions":{}}));
        let object = doc
            .as_object()
            .ok_or_else(|| Failure::io(io::Error::other("arquivo inválido")))?;
        let empty = json!([]);
        let guests = object
            .get("guests")
            .unwrap_or(&empty)
            .as_array()
            .ok_or_else(|| Failure::io(io::Error::other("convidados inválidos")))?;
        let mut normalized: Vec<Value> = Vec::new();
        for g in guests {
            let valid = g.as_object().is_some_and(|g| {
                g.keys().all(|k| {
                    [
                        "id",
                        "name",
                        "root",
                        "token_hash",
                        "sees_owner",
                        "owner_sees",
                    ]
                    .contains(&k.as_str())
                }) && ["id", "name", "root", "token_hash"]
                    .iter()
                    .all(|k| g.get(*k).is_some_and(Value::is_string))
            });
            if !valid {
                return Err(Failure::io(io::Error::other("convidado inválido")));
            }
            let mut g = g.clone();
            let fields = g.as_object_mut().expect("objeto conferido");
            fields.entry("sees_owner").or_insert(json!(false));
            fields.entry("owner_sees").or_insert(json!(true));
            // O dict original mantém a posição inicial e substitui o valor do ID repetido.
            if let Some(index) = normalized.iter().position(|old| old["id"] == g["id"]) {
                normalized[index] = g;
            } else {
                normalized.push(g);
            }
        }
        let sessions = match object.get("sessions") {
            None => json!({}),
            Some(Value::Object(values)) => Value::Object(values.clone()),
            Some(Value::Array(pairs)) => {
                let mut values = serde_json::Map::new();
                for pair in pairs {
                    let pair = pair
                        .as_array()
                        .filter(|p| p.len() == 2)
                        .ok_or_else(|| Failure::io(io::Error::other("sessões inválidas")))?;
                    let key = pair[0]
                        .as_str()
                        .ok_or_else(|| Failure::io(io::Error::other("sessão inválida")))?;
                    values.insert(key.into(), pair[1].clone());
                }
                Value::Object(values)
            }
            _ => return Err(Failure::io(io::Error::other("sessões inválidas"))),
        };
        let doc = json!({"guests":normalized,"sessions":sessions});
        // Publicação e confirmação da identidade são indivisíveis para os escritores locais.
        let mut loaded = self.loaded.lock().unwrap_or_else(|e| e.into_inner());
        if stamp == file_stamp(&self.path) {
            *loaded = stamp.map(|stamp| (stamp, doc.clone()));
            *self.last.lock().unwrap_or_else(|e| e.into_inner()) = Some(doc.clone());
        }
        Ok(doc)
    }
    pub fn snapshot(&self, preserve: bool) -> Option<Value> {
        match self.load() {
            Ok(doc) => Some(doc),
            Err(_) if preserve => {
                if crate::warn_limit::allow(None, "guests_unreadable") {
                    tracing::warn!(code = "guests_unreadable", "guests.json ilegível; fica a última leitura boa");
                }
                self.last.lock().unwrap_or_else(|e| e.into_inner()).clone()
            }
            Err(_) => None,
        }
    }
    pub fn lookup(&self, token: &[u8]) -> Option<Guest> {
        if token.is_empty() {
            return None;
        }
        let hash = hash(token);
        self.load().ok()?["guests"]
            .as_array()?
            .iter()
            .find_map(|g| {
                let stored = g["token_hash"].as_str()?;
                bool::from(stored.as_bytes().ct_eq(hash.as_bytes()))
                    .then(|| guest(g))
                    .flatten()
            })
    }
    pub fn mutate<T>(&self, f: impl FnOnce(&mut Value) -> Result<T>) -> Result<T> {
        self.mutate_optional(|state| f(state).map(Some)).map(|result| result.expect("mutação sempre produz resultado"))
    }
    fn mutate_optional<T>(&self, f: impl FnOnce(&mut Value) -> Result<Option<T>>) -> Result<Option<T>> {
        let _guard = self.lock.lock().unwrap_or_else(|e| e.into_inner());
        let parent = self
            .path
            .parent()
            .ok_or_else(|| Failure::io(io::Error::other("arquivo sem pasta")))?;
        let _file = crate::session_rename::files::lock_file(&parent.join("guests.lock")).map_err(Failure::io)?;
        let mut state = self.load()?;
        let result = f(&mut state)?;
        if result.is_none() { return Ok(None); }
        write(&self.path, &state).map_err(Failure::io)?;
        let mut loaded = self.loaded.lock().unwrap_or_else(|e| e.into_inner());
        *loaded = file_stamp(&self.path).map(|stamp| (stamp, state.clone()));
        *self.last.lock().unwrap_or_else(|e| e.into_inner()) = Some(state);
        Ok(result)
    }
    pub fn claim(&self, name: &str, id: &str, life: Option<&str>) -> Result<()> {
        self.mutate(|state| {
            state["sessions"][name] = json!({"guest":id, "life":life});
            Ok(())
        })
    }
    pub fn set_life(&self, name: &str, life: Option<&str>) -> Result<()> {
        let Some(life) = life else { return Ok(()) };
        self.mutate_optional(|state| {
            let Some(entry) = state["sessions"].get_mut(name).filter(|entry| !entry.is_null()) else { return Ok(None) };
            let object = entry.as_object_mut().ok_or_else(|| Failure::io(io::Error::other("propriedade inválida")))?;
            object.insert("life".into(), json!(life));
            Ok(Some(()))
        }).map(|_| ())
    }
    pub fn create(
        &self,
        name: &str,
        root: &str,
        sees: bool,
        owner: bool,
        id: String,
        token: String,
    ) -> Result<Value> {
        let root = valid_root(root)?;
        self.mutate(|state| {
            state["guests"].as_array_mut().unwrap().push(json!({"id":id,"name":name.trim(),"root":root,"token_hash":hash(token.as_bytes()),"sees_owner":sees,"owner_sees":owner}));
            Ok(json!({"id":id,"token":token}))
        })
    }
    pub fn update(&self, id: &str, root: &str, sees: bool, owner: bool) -> Result<Value> {
        let root = valid_root(root)?;
        self.mutate(|state| {
            let g = state["guests"]
                .as_array_mut()
                .unwrap()
                .iter_mut()
                .find(|g| g["id"] == id)
                .ok_or_else(missing)?;
            g["root"] = json!(root);
            g["sees_owner"] = json!(sees);
            g["owner_sees"] = json!(owner);
            Ok(json!({"id":id}))
        })
    }
    pub fn delete(&self, id: &str) -> Result<Value> {
        self.mutate(|state| {
            let guests = state["guests"].as_array_mut().unwrap();
            let i = guests
                .iter()
                .position(|g| g["id"] == id)
                .ok_or_else(missing)?;
            guests.remove(i);
            if let Some(s) = state["sessions"].as_object_mut() {
                s.retain(|_, v| v["guest"] != id);
            }
            Ok(json!({"ok":true}))
        })
    }
    pub fn owner(&self, name: &str, life: Option<&str>, preserve: bool) -> Option<Guest> {
        let doc = self.snapshot(preserve)?;
        let entry = doc["sessions"].get(name)?;
        let g = doc["guests"]
            .as_array()?
            .iter()
            .find(|g| g["id"] == entry["guest"])
            .and_then(guest)?;
        match (entry["life"].as_str(), life) {
            (Some(a), Some(b)) if a != b => None,
            (None, Some(life)) => {
                let saved = self.mutate(|fresh| {
                    if fresh["sessions"].get(name) == Some(entry) {
                        fresh["sessions"][name]["life"] = json!(life);
                    }
                    Ok(())
                });
                if saved.is_err() {
                    // O nome não pode adotar outra vida só porque o disco recusou a primeira.
                    let mut loaded = self.loaded.lock().unwrap_or_else(|e| e.into_inner());
                    let mut last = self.last.lock().unwrap_or_else(|e| e.into_inner());
                    if let Some(current) = last.as_mut().filter(|current| current["sessions"].get(name) == Some(entry)) {
                        current["sessions"][name]["life"] = json!(life);
                        if let Some((stamp, cached)) = loaded.as_mut().filter(|(stamp, cached)| {
                            Some(stamp.clone()) == file_stamp(&self.path)
                                && cached["sessions"].get(name) == Some(entry)
                        }) {
                            let _ = stamp;
                            cached["sessions"][name]["life"] = json!(life);
                        }
                    } else {
                        return None;
                    }
                }
                Some(g)
            }
            _ => Some(g),
        }
    }
    pub fn visible(&self, viewer: Option<&Guest>, name: &str, life: Option<&str>) -> bool {
        match (viewer, self.owner(name, life, true)) {
            (None, None) => true,
            (None, Some(owner)) => owner.owner_sees,
            (Some(viewer), None) => viewer.sees_owner,
            (Some(viewer), Some(owner)) => viewer.id == owner.id,
        }
    }
}
fn missing() -> Failure {
    Failure::code(404, "erro_convidado_inexistente", "convidado não existe")
}
fn truth(v: Option<&Value>, default: bool) -> bool {
    match v {
        None => default,
        Some(Value::Null) => false,
        Some(Value::Bool(b)) => *b,
        Some(Value::Number(n)) => n.as_f64() != Some(0.0),
        Some(Value::String(s)) => !s.is_empty(),
        Some(Value::Array(a)) => !a.is_empty(),
        Some(Value::Object(o)) => !o.is_empty(),
    }
}
fn guest(v: &Value) -> Option<Guest> {
    Some(Guest {
        id: v["id"].as_str()?.into(),
        name: v["name"].as_str()?.into(),
        root: v["root"].as_str()?.into(),
        sees_owner: truth(v.get("sees_owner"), false),
        owner_sees: truth(v.get("owner_sees"), true),
    })
}
pub fn valid_root(root: &str) -> Result<String> {
    let p = crate::accounts::environment::expand(root, &crate::orchestration::home())
        .canonicalize()
        .ok()
        .filter(|p| p.is_dir())
        .ok_or_else(|| Failure::code(400, "erro_pasta_inexistente", "a pasta não existe"))?;
    Ok(p.to_string_lossy()
        .strip_prefix(r"\\?\")
        .unwrap_or(&p.to_string_lossy())
        .to_string())
}
