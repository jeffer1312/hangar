//! Um arquivo para os peers e a lista dos apps; a trava cobre leitura e publicação.
use super::Error;
use crate::orchestration::codec;
use serde_json::{Map, Value, json};
use std::{
    fs::{File, OpenOptions},
    io,
    path::{Path, PathBuf},
};

#[derive(Default)]
pub struct Book(Map<String, Value>);
impl Book {pub fn into_value(mut self)->Value{Value::Object(std::mem::take(&mut self.0))}}
impl std::ops::Deref for Book {type Target=Map<String,Value>;fn deref(&self)->&Self::Target{&self.0}}
impl std::ops::DerefMut for Book {fn deref_mut(&mut self)->&mut Self::Target{&mut self.0}}
impl Drop for Book {fn drop(&mut self){codec::discard(Value::Object(std::mem::take(&mut self.0)));}}
impl<'a> IntoIterator for &'a Book {
    type Item=(&'a String,&'a Value);
    type IntoIter=serde_json::map::Iter<'a>;
    fn into_iter(self)->Self::IntoIter{self.0.iter()}
}

#[derive(Clone)]
pub struct Store {
    pub path: PathBuf,
}
impl Store {
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }
    pub fn read(&self) -> Result<Book, Error> {
        let bytes = match crate::runtime::queue::read_replaced(&self.path) {
            Ok(b) => b,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Book::default()),
            Err(_) => {
                return Err(Error::invalid(
                    "peers.json ilegível — corrija o arquivo antes de gravar",
                ));
            }
        };
        let value = std::str::from_utf8(&bytes).ok().and_then(codec::loads).ok_or_else(|| {
            let diagnostic=serde_json::from_slice::<Value>(&bytes).err().map(|e|e.to_string()).unwrap_or_else(||"JSON inválido".into());
            Error::invalid(format!("peers.json ilegível ({diagnostic}) — corrija o arquivo antes de gravar"))
        })?;
        match value {
            Value::Object(data)=>Ok(Book(data)),
            other=>{codec::discard(other);Err(Error::invalid("peers.json não é um objeto JSON — corrija o arquivo antes de gravar"))}
        }
    }

    pub fn tolerant(&self) -> Book {
        self.read().unwrap_or_else(|_| {
            tracing::warn!(code = "peers_unreadable", "peers.json ilegível");
            Book::default()
        })
    }

    pub fn change(
        &self,
        apply: impl FnOnce(&mut Map<String, Value>) -> Result<Value, Error>,
    ) -> Result<Value, Error> {
        let _lock = lock(&self.path)?;
        let mut data = self.read()?;
        let mut result = super::RequestValue(apply(&mut data)?);
        clean_temporary(&self.path)?;
        let document=Value::Object(std::mem::take(&mut data.0));
        let serialized=codec::dumps_indented_utf8(&document,2);
        codec::discard(document);
        let mut bytes=serialized.map_err(|_|Error::invalid("texto contém Unicode inválido"))?.into_bytes();
        bytes.push(b'\n');
        crate::runtime::queue::atomic_write(&self.path, &bytes)
            .map_err(|_| Error::unavailable())?;
        Ok(std::mem::take(&mut result.0))
    }

    pub fn list(&self) -> Value {
        list(&self.tolerant())
    }
    pub fn put(&self, body: &Value) -> Result<Value, Error> {
        let empty=Value::String(String::new());
        let id = body.get("id").filter(|v|truthy(v)).unwrap_or(&empty);
        valid_id(&id)?;
        let id = id.as_str().unwrap();
        let base = body.get("base_url").filter(|v|truthy(v)).unwrap_or(&empty);
        let token = body.get("token").filter(|v|truthy(v)).unwrap_or(&empty);
        let (Some(base), Some(token)) = (text(base), text(token)) else {
            return Err(Error::invalid("endereço e token precisam ser texto"));
        };
        let web = body.get("web_url").filter(|v| !v.is_null());
        if web.is_some_and(|v| text(v).is_none()) {
            return Err(Error::invalid("web_url precisa ser texto"));
        }
        let (base, token) = (base.trim(), token.trim());
        if !(base.starts_with("http://") || base.starts_with("https://")) || token.is_empty() {
            return Err(Error::invalid(
                "endereço precisa ser http(s):// e o token não pode ficar vazio",
            ));
        }
        if ["base_url","token","web_url"].iter().any(|key|body.get(key).is_some_and(crate::orchestration::codec::contains_surrogate)){
            return Err(Error::invalid("texto contém Unicode inválido"));
        }
        self.change(|data| {
            let mut entry = match data.get(id).map(codec::clone_value){
                Some(Value::Object(entry))=>entry,
                Some(other)=>{codec::discard(other);Map::new()},
                None=>Map::new(),
            };
            entry.insert("base_url".into(), json!(base.trim_end_matches('/')));
            entry.insert("token".into(), json!(token));
            if let Some(web) = web
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|s| !s.is_empty())
            {
                entry.insert("web_url".into(), json!(web));
            }
            if let Some(previous)=data.insert(id.into(), Value::Object(entry)){codec::discard(previous);}
            Ok(list(data))
        })
    }
    pub fn remove(&self, id: &str) -> Result<Value, Error> {
        valid_id(&json!(id)).map_err(|e| Error::missing(e.msg))?;
        self.change(|data| {
            let entry = data
                .get(id)
                .filter(|v| is_peer(v))
                .ok_or_else(|| unknown(id))?;
            if let Some(app) = entry.get("app").filter(|v| v.is_object()) {
                data.insert(id.into(), json!({"app":app}));
            } else {
                data.remove(id);
            }
            Ok(list(data))
        }).map_err(|e| if e.code == "peers_registro_invalido" { Error::missing(e.msg) } else { e })
    }
    pub fn enabled(&self, id: &str, value: &Value) -> Result<Value, Error> {
        let enabled = value
            .as_bool()
            .ok_or_else(|| Error::invalid("enabled precisa ser true ou false"))?;
        valid_id(&json!(id)).map_err(|e| Error::missing(e.msg))?;
        self.change(|data| {
            let entry = data
                .get_mut(id)
                .filter(|v| is_peer(v))
                .ok_or_else(|| unknown(id))?;
            entry["enabled"] = json!(enabled);
            Ok(list(data))
        }).map_err(|e| if e.code == "peers_registro_invalido" { Error::missing(e.msg) } else { e })
    }
    pub fn token(&self, id: &str) -> Result<Value, Error> {
        let data = self.tolerant();
        let entry = data
            .get(id)
            .filter(|v| v.is_object() && truthy(&v["base_url"]) && truthy(&v["token"]))
            .ok_or_else(|| Error::missing(format!("peer '{id}' desconhecido")))?;
        Ok(json!({"token":entry["token"]}))
    }
    pub fn app_list(&self) -> Result<Value, Error> {
        let data = self.read().map_err(|mut e| {
            e.status = 503;
            e.code = "peers_arquivo_ilegivel";
            e
        })?;
        Ok(app_response(&data))
    }
    pub fn app_write(&self, body: &Value) -> Result<Value, Error> {
        let rev = text(&body["rev"])
            .ok_or_else(|| Error::invalid("rev obrigatório"))?;
        let machines = body["servers"]
            .as_array()
            .ok_or_else(|| Error::invalid("servers precisa ser uma lista"))?;
        let machines: Vec<Value> = machines.iter().map(app_entry).collect::<Result<_, _>>()?;
        let mut ids = std::collections::HashSet::new();
        if machines
            .iter()
            .any(|v| !ids.insert(v["id"].as_str().unwrap()))
        {
            return Err(Error::invalid("id de máquina repetido"));
        }
        self.change(|data| {
            if app_response(data)["rev"] != rev {
                return Err(Error {
                    status: 409,
                    code: "peers_lista_mudou",
                    msg: "a lista mudou em outro app".into(),
                });
            }
            if machines.iter().any(crate::orchestration::codec::contains_surrogate) {
                return Err(Error::invalid("texto contém Unicode inválido"));
            }
            let mut owners = std::collections::HashMap::new();
            data.retain(|key, cfg| {
                if let Some(obj) = cfg.as_object_mut() {
                    if let Some(app) = obj.get("app").filter(|v| v.is_object()) {
                        owners.insert(app["id"].to_string(), key.clone());
                        obj.remove("app");
                        return !obj.is_empty();
                    }
                }
                true
            });
            let mut used = std::collections::HashSet::new();
            for (pos, mut machine) in machines.into_iter().enumerate() {
                let mut key = owners
                    .get(&machine["id"].to_string())
                    .filter(|k| !used.contains(*k))
                    .cloned();
                let address = host(machine["address"].as_str().unwrap());
                if key.is_none() && machine["invite"] == false && !loopback(&address) {
                    key = data
                        .iter()
                        .find(|(k, cfg)| {
                            !used.contains(*k)
                                && cfg.is_object()
                                && cfg.get("app").is_none()
                                && ["base_url", "web_url"].iter().any(|field| {
                                    host(&crate::transcript::py::py_str(&cfg[*field])) == address
                                })
                        })
                        .map(|(k, _)| k.clone());
                }
                let key = key.unwrap_or_else(|| free_key(machine["id"].as_str().unwrap(), data));
                machine["pos"] = json!(pos);
                data.entry(key.clone()).or_insert_with(|| json!({}))["app"] = machine;
                used.insert(key);
            }
            Ok(app_response(data))
        })
    }
}
fn unknown(id: &str) -> Error {
    Error::missing(format!("servidor '{id}' não está no peers.json"))
}
fn is_peer(v: &Value) -> bool {
    v.is_object() && (v.get("base_url").is_some() || v.get("app").is_none())
}
fn text(value:&Value)->Option<&str>{value.as_str().filter(|_|!crate::orchestration::codec::contains_nonfinite(value))}
pub fn truthy(v: &Value) -> bool {
    match v {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::String(s) => !s.is_empty(),
        Value::Array(a) => !a.is_empty(),
        Value::Object(o) => !o.is_empty(),
        Value::Number(n) => n.as_f64() != Some(0.),
    }
}
pub fn truthy_field(body: &Value, key: &str) -> Value {
    body.get(key)
        .filter(|v| truthy(v))
        .cloned()
        .unwrap_or(json!(""))
}
pub fn valid_id(v: &Value) -> Result<(), Error> {
    let good = v.as_str().is_some_and(|s| {
        !s.is_empty()
            && s.len() <= 32
            && s.as_bytes()[0].is_ascii_alphanumeric()
            && s.bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_' || b == b'-')
    });
    if good {
        Ok(())
    } else {
        Err(Error::invalid(
            "identificador: use minúsculas, números, '-' ou '_' (até 32 caracteres)",
        ))
    }
}
pub fn mask(value: &str) -> String {
    let chars: Vec<_> = value.chars().collect();
    if chars.len() <= 8 {
        "•".repeat(chars.len())
    } else {
        format!(
            "{}{}{}",
            chars[..4].iter().collect::<String>(),
            "•".repeat(8),
            chars[chars.len() - 4..].iter().collect::<String>()
        )
    }
}
pub fn list(data: &Map<String, Value>) -> Value {
    let mut rows: Vec<_> = data
        .iter()
        .filter(|(_, v)| v.is_object() && truthy(&v["base_url"]))
        .collect();
    rows.sort_by(|a, b| a.0.cmp(b.0));
    json!(rows.into_iter().map(|(id,cfg)|{let token=truthy_field(cfg,"token");let mut row=json!({"id":id,"base_url":cfg["base_url"],"token":mask(&crate::transcript::py::py_str(&token))});if truthy(&cfg["web_url"]){row["web_url"]=cfg["web_url"].clone();}if cfg["enabled"].is_boolean(){row["enabled"]=cfg["enabled"].clone();}row}).collect::<Vec<_>>())
}
fn app_entry(raw: &Value) -> Result<Value, Error> {
    if !raw.is_object() {
        return Err(Error::invalid("máquina precisa ser um objeto"));
    }
    let mut out = json!({});
    for field in ["id", "label", "address", "token"] {
        let v = raw.get(field).unwrap_or(&Value::Null);
        let s = if raw.get(field).is_none() {
            ""
        } else {
            text(v)
                .ok_or_else(|| Error::invalid(format!("{field} precisa ser texto")))?
        };
        out[field] = json!(s);
    }
    let id = out["id"].as_str().unwrap();
    if id.is_empty() || id.chars().count() > 64 {
        return Err(Error::invalid("id da máquina vazio ou longo demais"));
    }
    let address = out["address"].as_str().unwrap();
    if !address.starts_with("http://") && !address.starts_with("https://") {
        return Err(Error::invalid("endereço precisa ser http(s)://"));
    }
    for field in ["disabled", "invite"] {
        let v = raw
            .get(field)
            .unwrap_or(&Value::Bool(false))
            .as_bool()
            .ok_or_else(|| Error::invalid(format!("{field} precisa ser true ou false")))?;
        out[field] = json!(v);
    }
    if let Some(lan) = raw.get("lan").filter(|v| !v.is_null()) {
        if !lan.is_object() || text(&lan["url"]).is_none() || text(&lan["id"]).is_none() {
            return Err(Error::invalid("lan precisa ser {url, id}"));
        }
        out["lan"] = json!({"url":lan["url"],"id":lan["id"]});
    }
    Ok(out)
}
fn app_response(data: &Map<String, Value>) -> Value {
    let mut apps: Vec<_> = data
        .values()
        .filter_map(|v| v.get("app").and_then(Value::as_object))
        .cloned()
        .collect();
    apps.sort_by_key(|a| {
        a.get("pos")
            .and_then(|v| v.as_i64().or_else(|| v.as_bool().map(i64::from)))
            .unwrap_or(0)
    });
    for app in &mut apps {
        app.remove("pos");
    }
    let list = json!(apps);
    let canonical = sort_keys(&list);
    let digest = ring::digest::digest(
        &ring::digest::SHA256,
        serde_json::to_string(&canonical).unwrap().as_bytes(),
    );
    let rev = digest.as_ref()[..8]
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect::<String>();
    json!({"rev":rev,"servers":list})
}
fn sort_keys(v: &Value) -> Value {
    match v {
        Value::Object(m) => {
            let mut keys: Vec<_> = m.keys().collect();
            keys.sort();
            let mut out = Map::new();
            for k in keys {
                out.insert(k.clone(), sort_keys(&m[k]));
            }
            Value::Object(out)
        }
        Value::Array(a) => Value::Array(a.iter().map(sort_keys).collect()),
        _ => v.clone(),
    }
}
fn host(s: &str) -> String {
    reqwest::Url::parse(s)
        .ok()
        .and_then(|u| u.host_str().map(str::to_lowercase))
        .unwrap_or_default()
}
fn loopback(h: &str) -> bool {
    h == "localhost" || h == "::1" || h == "[::1]" || h.starts_with("127.")
}
fn free_key(id: &str, data: &Map<String, Value>) -> String {
    let clean = id
        .to_lowercase()
        .chars()
        .map(|c| {
            if c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-' {
                c
            } else {
                '-'
            }
        })
        .collect::<String>();
    let base = clean
        .trim_matches(['-', '_'])
        .chars()
        .take(28)
        .collect::<String>();
    let base = if base.is_empty() {
        "maquina".into()
    } else {
        base
    };
    let mut key = base.clone();
    let mut n = 2;
    while data.contains_key(&key) {
        key = format!("{base}-{n}");
        n += 1;
    }
    key
}
pub fn lock(path: &Path) -> Result<File, Error> {
    let parent = path.parent().ok_or_else(Error::unavailable)?;
    std::fs::create_dir_all(parent).map_err(|_| Error::unavailable())?;
    let lock = OpenOptions::new()
        .read(true)
        .append(true)
        .create(true)
        .open(path.with_file_name(format!(
            "{}.lock",
            path.file_name().unwrap_or_default().to_string_lossy()
        )))
        .map_err(|_| Error::unavailable())?;
    lock.lock().map_err(|_| Error::unavailable())?;
    Ok(lock)
}
pub fn clean_temporary(path: &Path) -> Result<(), Error> {
    let prefix = format!(
        "{}.",
        path.file_name().unwrap_or_default().to_string_lossy()
    );
    for entry in std::fs::read_dir(path.parent().ok_or_else(Error::unavailable)?)
        .map_err(|_| Error::unavailable())?
    {
        let entry = entry.map_err(|_| Error::unavailable())?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with(&prefix) && name.ends_with(".tmp") {
            std::fs::remove_file(entry.path()).map_err(|_| Error::unavailable())?;
        }
    }
    Ok(())
}
