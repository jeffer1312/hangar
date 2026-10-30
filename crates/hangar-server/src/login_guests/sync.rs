//! Cofre cifrado e sessões do hub: senha e chave de decifração nunca chegam ao servidor.
use super::{
    store::{self, Failure, Result},
    validation as v,
};
use base64::{Engine, engine::general_purpose::STANDARD};
use ring::{hmac, pbkdf2};
use serde_json::{Value, json};
use crate::orchestration::truthy;
use std::{collections::HashMap, num::NonZeroU32, path::PathBuf, sync::Mutex};
use subtle::ConstantTimeEq;

pub struct Sync {
    pub(super) diary: super::diary::Diary,
    pub path: PathBuf,
    pub config_path: PathBuf,
    pub secret: Vec<u8>,
    pub bootstrap: String,
    pub enabled: bool,
    pub secure: bool,
    pub rate_max: num_bigint::BigInt,
    pub rate_window: num_bigint::BigInt,
    lock: Mutex<()>,
    fails: Mutex<HashMap<String, Vec<f64>>>,
}
impl Sync {
    pub fn new(path: PathBuf, config_path: PathBuf, secret: Vec<u8>) -> Self {
        Self {
            diary: Default::default(),
            path,
            config_path,
            secret,
            bootstrap: String::new(),
            enabled: false,
            secure: false,
            rate_max: 10.into(),
            rate_window: 300.into(),
            lock: Mutex::new(()),
            fails: Mutex::new(HashMap::new()),
        }
    }
    pub fn enabled(&self) -> bool {
        crate::config_sync::runtime::sync_enabled(&self.config_path, self.enabled)
    }
    fn load(&self) -> Result<Option<crate::orchestration::codec::ValueGuard>> {
        let unreadable = || Failure::code(500, "sync_vault_unreadable", "cofre ilegível");
        match crate::runtime::queue::read_replaced(&self.path) {
            Ok(bytes) => std::str::from_utf8(&bytes).ok().and_then(crate::orchestration::codec::loads)
                .map(|value| if value.is_null() { None } else { Some(crate::orchestration::codec::ValueGuard(value)) }).ok_or_else(unreadable),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(unreadable().with_io(&error)),
        }
    }
    fn save(&self, value: &Value) -> Result<()> {
        store::write_bytes(&self.path, crate::orchestration::codec::dumps(value, false).as_bytes()).map_err(|error| {
            Failure::code(
                500,
                "sync_vault_write_failed",
                "não foi possível gravar o cofre",
            ).with_io(&error)
        })
    }
    fn config(&self, on: bool) -> Result<()> {
        crate::config_sync::runtime::set_sync(&self.config_path, on).map_err(|error| {
            let (code, message) = if error.operation == Some(crate::config_sync::runtime_error::StorageOperation::Read) {
                ("sync_config_unreadable", "configuração ilegível")
            } else {
                ("sync_config_write_failed", "não foi possível gravar a configuração")
            };
            let mut failure = Failure::code(500, code, message);
            failure.os_error = error.os_error;
            failure
        })
    }
    pub fn cookie(&self, user: &str, generation: Option<&str>, now: u64) -> String {
        let msg = match generation {
            Some(g) => format!("{user}.{}:{g}", now + 2592000),
            None => format!("{user}.{}", now + 2592000),
        };
        format!("{msg}.{}", sign(&self.secret, msg.as_bytes()))
    }
    pub fn verify(&self, cookie: &str, now: u64) -> Option<(String, Option<String>)> {
        let (msg, sig) = cookie.rsplit_once('.')?;
        if !bool::from(
            sign(&self.secret, msg.as_bytes())
                .as_bytes()
                .ct_eq(sig.as_bytes()),
        ) {
            return None;
        }
        let (user, exp) = msg.rsplit_once('.')?;
        let (exp, generation) = exp
            .split_once(':')
            .map_or((exp, None), |(e, g)| (e, (!g.is_empty()).then_some(g)));
        if exp.is_empty() || !exp.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        // O Python aceita expiração inteira sem teto; compare os dígitos sem truncar.
        let expiration = exp.trim_start_matches('0');
        let expiration = if expiration.is_empty() { "0" } else { expiration };
        let now = now.to_string();
        if expiration.len() < now.len()
            || (expiration.len() == now.len() && expiration < now.as_str())
        {
            return None;
        }
        Some((user.to_owned(), generation.map(str::to_owned)))
    }
    pub fn authenticate(&self, cookie: &str, now: u64) -> Result<(String, Option<String>)> {
        let (user, generation) = self.verify(cookie, now).ok_or_else(unauthorized)?;
        if let Some(vault) = self.load()?.filter(|value|truthy(value)) {
            let acc = account(&vault, &user)?.ok_or_else(unauthorized)?;
            let invalid = match generation.as_deref() {
                Some(generation) => generation_of(acc)? != generation,
                None => vault["user"] != user,
            };
            if invalid { return Err(unauthorized()); }
        }
        if user.is_empty() { return Err(unauthorized()); }
        Ok((user, generation))
    }
    pub fn handle(
        &self,
        method: &str,
        path: &str,
        body: &Value,
        user: Option<&str>,
        now: f64,
        ip: &str,
        entropy: fn(usize) -> Vec<u8>,
    ) -> Result<Value> {
        match (method, path) {
            ("GET", "status") => Ok(json!({"enabled":true,"registered":self.load()?.is_some()})),
            ("GET", "setup") => self.setup_status(),
            ("POST", "setup") => self.setup(body, entropy),
            ("POST", "setup/disable") => {
                let _g = self.lock.lock().unwrap_or_else(|e| e.into_inner());
                let status = self.setup_status()?;
                self.config(false)?;
                self.diary.event("sync.desativado");
                Ok(json!({"enabled":false,"registered":status["registered"],"user":status["user"]}))
            }
            ("POST", "register") => self.diary.trace("sync.cadastrar", || self.register(body, entropy)),
            ("GET", "prelogin") => self.prelogin(user.unwrap_or_default()),
            ("POST", "login") => self.login(body, now, ip).map(|(value, _)| value),
            ("POST", "logout") => { self.diary.event("sync.logout"); Ok(json!({"ok":true})) },
            ("GET", "vault") => {
                let Some(vault) = self.load()?.filter(|value|truthy(value)) else {
                    return Ok(json!({"enc_blob":null,"rev":0}));
                };
                let acc = account(&vault, user.unwrap_or_default())?.ok_or_else(unauthorized)?;
                let acc = acc.as_object().ok_or_else(unreadable)?;
                Ok(super::object([("enc_blob",acc.get("enc_blob").map(crate::orchestration::codec::clone_value).unwrap_or(Value::Null)),("rev",acc.get("rev").map(crate::orchestration::codec::clone_value).unwrap_or(json!(0)))]))
            }
            ("PUT", "vault") => self.diary.trace("sync.gravar", || self.put_vault(body, user.unwrap_or_default())),
            ("GET", "guests") => {
                let vault = self.owner(user)?;
                let Some(guests)=vault.get("guests").filter(|value|truthy(value)) else {return Ok(json!([]));};
                let guests=guests.as_object().ok_or_else(unreadable)?;
                let rows=guests.iter().map(|(user,account)| {
                    let account=account.as_object().ok_or_else(unreadable)?;
                    Ok(super::object([("user",json!(user)),("admin_blob",account.get("admin_blob").map(crate::orchestration::codec::clone_value).unwrap_or(Value::Null))]))
                }).collect::<Result<Vec<_>>>()?;
                Ok(Value::Array(rows))
            }
            ("POST", "guests") => self.diary.trace("sync.convidado_gravar", || self.put_guest(body, user, entropy)),
            ("POST", p) if p.starts_with("guests/") && p.ends_with("/delete") => {
                let _g = self.lock.lock().unwrap_or_else(|e| e.into_inner());
                let mut vault = self.owner(user)?;
                let target = &p[7..p.len() - 7];
                let guests=vault.get_mut("guests").filter(|value|truthy(value));
                let removed=match guests {
                    Some(guests)=>guests.as_object_mut().ok_or_else(unreadable)?.remove(target).map(crate::orchestration::codec::ValueGuard),
                    None=>None,
                };
                if removed.as_ref().is_none_or(|value|value.is_null()) {
                    return Err(Failure::code(404,"erro_convidado_inexistente","convidado não existe"));
                }
                self.save(&vault)?;
                Ok(json!({"ok":true}))
            }
            _ => Err(Failure {
                os_error: None,
                status: 404,
                body: json!({"detail":"Not Found"}),
            }),
        }
    }
    pub(super) fn owner(&self, user: Option<&str>) -> Result<crate::orchestration::codec::ValueGuard> {
        let vault = self
            .load()?.filter(|value|truthy(value))
            .ok_or_else(|| Failure::code(403, "erro_so_dono", "só o dono"))?;
        if !vault.is_object() { return Err(unreadable()); }
        if vault["user"].as_str() != user {
            return Err(Failure::code(403, "erro_so_dono", "só o dono"));
        }
        Ok(vault)
    }
    fn setup_status(&self) -> Result<Value> {
        let vault = self.load()?;
        if vault.is_none() && self.path.exists() { return Err(unreadable()); }
        if let Some(v) = &vault {
            if !v.is_object()
                || ["user", "salt", "verifier_salt", "auth_verifier"]
                    .iter()
                    .any(|k| v[k].as_str().is_none_or(str::is_empty))
                || !v["rev"].is_number()
                || !v["rev"].to_string().bytes().all(|b| b.is_ascii_digit())
                || v.get("enc_blob").is_none()
            {
                return Err(Failure::code(
                    500,
                    "sync_vault_unreadable",
                    "cofre ilegível",
                ));
            }
            if !v["enc_blob"].is_null()
                && super::schema::validate("SetupBlob", &v["enc_blob"]).is_err()
            {
                return Err(Failure::code(
                    500,
                    "sync_vault_unreadable",
                    "cofre ilegível",
                ));
            }
            for (k, size) in [("salt", 16), ("verifier_salt", 16), ("auth_verifier", 32)] {
                if super::schema::base64_bytes(v[k].as_str().unwrap_or_default())
                    .ok()
                    .is_none_or(|b| b.len() != size)
                {
                    return Err(Failure::code(
                        500,
                        "sync_vault_unreadable",
                        "cofre ilegível",
                    ));
                }
            }
        }
        Ok(
            json!({"enabled":self.enabled(),"registered":vault.is_some(),"user":vault.as_ref().map(|v|&v["user"])}),
        )
    }
    fn register(&self, b: &Value, entropy: fn(usize) -> Vec<u8>) -> Result<Value> {
        let user = v::text(b, "user", 0, usize::MAX)?;
        let salt = v::text(b, "salt", 0, usize::MAX)?;
        let auth = v::text(b, "auth_hash", 0, usize::MAX)?;
        let bootstrap = v::text(b, "bootstrap", 0, usize::MAX)?;
        if self.bootstrap.is_empty()
            || !bool::from(self.bootstrap.as_bytes().ct_eq(bootstrap.as_bytes()))
        {
            self.diary.refused("sync.cadastro_recusado", "bootstrap_invalido", "403");
            return Err(Failure::code(
                403,
                "erro_bootstrap_invalido",
                "bad bootstrap",
            ));
        }
        let _g = self.lock.lock().unwrap_or_else(|e| e.into_inner());
        if self.load()?.is_some() {
            self.diary.refused("sync.cadastro_recusado", "ja_cadastrado", "403");
            return Err(Failure::code(
                403,
                "erro_ja_registrado",
                "already registered",
            ));
        }
        let salt_bytes = entropy(16);
        self.save(&json!({"user":user,"salt":salt,"verifier_salt":STANDARD.encode(&salt_bytes),"auth_verifier":verifier(&auth,&salt_bytes),"enc_blob":null,"rev":0}))?;
        self.diary.event("sync.cadastrado");
        Ok(json!({"ok":true}))
    }
    fn setup(&self, b: &Value, entropy: fn(usize) -> Vec<u8>) -> Result<Value> {
        v::forbid_extra(b, &["user", "salt", "auth_hash", "enc_blob"])?;
        let populated = b.as_object().is_some_and(|b| !b.is_empty());
        if populated {
            let user = v::text(b, "user", 1, 100)?;
            if user.trim().is_empty() {
                return Err(v::value_error(
                    "user",
                    b["user"].clone(),
                    "usuário obrigatório",
                ));
            }
            validate_key(b, "salt", 16, 24)?;
            validate_key(b, "auth_hash", 32, 44)?;
            if !b["enc_blob"].is_null() {
                validate_blob(&b["enc_blob"])?;
            }
        }
        let _g = self.lock.lock().unwrap_or_else(|e| e.into_inner());
        self.setup_status()?;
        let vault = self.load()?;
        if vault.is_some() && populated {
            return Err(Failure::code(
                409,
                "erro_ja_registrado",
                "already registered",
            ));
        }
        if vault.is_none() {
            if !populated {
                return Err(Failure {
                    os_error: None,
                    status: 422,
                    body: json!({"detail":"Informe os dados da conta"}),
                });
            }
            let salt = entropy(16);
            self.save(&json!({"user":b["user"],"salt":b["salt"],"verifier_salt":STANDARD.encode(&salt),"auth_verifier":verifier(b["auth_hash"].as_str().unwrap_or_default(),&salt),"enc_blob":b["enc_blob"],"rev":if b["enc_blob"].is_null(){0}else{1}}))?;
        }
        self.config(true)?;
        self.diary.event("sync.ativado");
        self.setup_status()
    }
    fn prelogin(&self, user: &str) -> Result<Value> {
        let vault = self.load()?;
        let acc = match &vault { Some(vault) => account(vault, user)?, None => None };
        let salt = if let Some(acc) = acc.filter(|v| truthy(v)) {
            acc.as_object().and_then(|v| v.get("salt")).ok_or_else(unreadable)?.clone()
        } else if let Some(vault) = vault.filter(|value|truthy(value)) {
            let key = stored_base64(&vault["verifier_salt"])?;
            let key = hmac::Key::new(hmac::HMAC_SHA256, &key);
            json!(STANDARD.encode(&hmac::sign(&key, user.as_bytes()).as_ref()[..16]))
        } else { json!(STANDARD.encode(b"unregistered----")) };
        Ok(json!({"salt":salt,"iterations":600000}))
    }
    pub(super) fn login(&self, b: &Value, now: f64, ip: &str) -> Result<(Value, String)> {
        self.diary.trace("sync.login", || self.login_snapshot(b, now, ip))
    }
    fn login_snapshot(&self, b: &Value, now: f64, ip: &str) -> Result<(Value, String)> {
        let user = v::text(b, "user", 0, usize::MAX)?;
        let auth = v::text(b, "auth_hash", 0, usize::MAX)?;
        {
            let mut all = self.fails.lock().unwrap_or_else(|e| e.into_inner());
            let hits = all.entry(ip.into()).or_default();
            hits.retain(|t| {
                let elapsed=now-*t;
                if elapsed==f64::NEG_INFINITY { return true; }
                if !elapsed.is_finite() { return false; }
                // Para limite inteiro, x < limite equivale a floor(x) < limite, sem estreitar o limite.
                format!("{:.0}",elapsed.floor()).parse::<num_bigint::BigInt>()
                    .is_ok_and(|elapsed|elapsed < self.rate_window)
            });
            if num_bigint::BigInt::from(hits.len()) >= self.rate_max {
                self.diary.access("sync.login", ip, "senha", Some("bloqueio_temporario"));
                return Err(Failure::code(
                    429,
                    "erro_muitas_tentativas",
                    "too many attempts",
                ));
            }
        }
        // A geração do cookie precisa pertencer à credencial que acabou de ser validada.
        let _guard = self.lock.lock().unwrap_or_else(|e| e.into_inner());
        let vault = self.load()?;
        let acc = match &vault { Some(vault) => account(vault, &user)?, None => None };
        let valid = if let Some(acc) = acc.filter(|v| truthy(v)) {
            let salt = stored_base64(&acc["verifier_salt"])?;
            let expected = acc.get("auth_verifier").and_then(Value::as_str).filter(|s| s.is_ascii()).ok_or_else(unreadable)?;
            bool::from(verifier(&auth, &salt).as_bytes().ct_eq(expected.as_bytes()))
        } else {
            let _ = verifier(&auth, &[0; 16]);
            false
        };
        if !valid {
            self.diary.access("sync.login", ip, "senha", Some("credenciais_invalidas"));
            self.fails
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .entry(ip.into())
                .or_default()
                .push(now);
            return Err(unauthorized());
        }
        let generation = generation_of(acc.ok_or_else(unreadable)?)?;
        self.diary.access("sync.login", ip, "senha", None);
        self.diary.event("sync.login_concluido");
        Ok((json!({"ok":true}), generation))
    }
    fn put_vault(&self, b: &Value, user: &str) -> Result<Value> {
        let _g = self.lock.lock().unwrap_or_else(|e| e.into_inner());
        let Some(mut vault) = self.load()?.filter(|value|truthy(value)) else {
            self.diary.refused("sync.gravacao_recusada", "cadastro_ausente", "409");
            return Err(Failure {
                os_error: None,
                status: 409,
                body: json!({"detail":{"enc_blob":null,"rev":0}}),
            });
        };
        let acc = account_mut(&mut vault, user)?.ok_or_else(unauthorized)?;
        let acc = acc.as_object_mut().ok_or_else(unreadable)?;
        let revision = acc.get("rev").ok_or_else(unreadable)?.clone();
        if b["base_rev"] != revision {
            self.diary.refused("sync.gravacao_recusada", "revisao_desatualizada", "409");
            return Err(Failure {
                os_error: None,
                status: 409,
                body: super::object([("detail",super::object([("enc_blob",crate::orchestration::codec::clone_value(acc.get("enc_blob").ok_or_else(unreadable)?)),("rev",revision)]))]),
            });
        }
        if b.get("enc_blob").is_none() {
            return Err(v::invalid(
                "enc_blob",
                "missing",
                "Field required",
                b.clone(),
                None,
            ));
        }
        if let Some(old)=acc.insert("enc_blob".into(),crate::orchestration::codec::clone_value(&b["enc_blob"])) {crate::orchestration::codec::discard(old);}
        let next = increment_revision(&revision)?;
        acc.insert("rev".into(), next.clone());
        self.save(&vault)?;
        self.diary.event("sync.gravado");
        Ok(json!({"rev":next}))
    }
    fn put_guest(
        &self,
        b: &Value,
        user: Option<&str>,
        entropy: fn(usize) -> Vec<u8>,
    ) -> Result<Value> {
        let _g = self.lock.lock().unwrap_or_else(|e| e.into_inner());
        let mut vault = self.owner(user)?;
        let name = v::text(b, "user", 1, 100)?;
        let name = name.trim();
        if name.is_empty() || name.contains('/') {
            return Err(v::value_error(
                "user",
                b["user"].clone(),
                "usuário inválido",
            ));
        }
        validate_key(b, "salt", 16, 24)?;
        validate_key(b, "auth_hash", 32, 44)?;
        validate_blob(&b["enc_blob"])?;
        validate_blob(&b["admin_blob"])?;
        v::forbid_extra(b, &["user", "salt", "auth_hash", "enc_blob", "admin_blob"])?;
        if vault["user"] == name {
            return Err(Failure::code(
                409,
                "erro_usuario_em_uso",
                "usuário já existe",
            ));
        }
        if vault.get("guests").is_none() {
            vault["guests"] = json!({});
        }
        let guests=vault.get("guests").and_then(Value::as_object).ok_or_else(unreadable)?;
        let old=guests.get(name).filter(|value|truthy(value));
        let rev=if let Some(old)=old {
            let previous=old.as_object().and_then(|value|value.get("rev")).ok_or_else(unreadable)?;
            if let Some(value)=previous.as_bool(){json!(u8::from(value)+1)}
            else if previous.is_number() && previous.to_string().contains(['.','e','E']) {
                serde_json::Number::from_f64(previous.as_f64().ok_or_else(unreadable)?+1.0).map(Value::Number).ok_or_else(unreadable)?
            } else {increment_revision(previous).map_err(|_|unreadable())?}
        }else{json!(1)};
        let salt = entropy(16);
        vault["guests"][name] = json!({"salt":b["salt"],"verifier_salt":STANDARD.encode(&salt),"auth_verifier":verifier(b["auth_hash"].as_str().unwrap_or_default(),&salt),"enc_blob":b["enc_blob"],"admin_blob":b["admin_blob"],"rev":rev});
        self.save(&vault)?;
        Ok(json!({"ok":true}))
    }
}
fn unreadable() -> Failure { Failure::code(500, "sync_vault_unreadable", "cofre ilegível") }
fn account<'a>(v: &'a Value, user: &str) -> Result<Option<&'a Value>> {
    if !truthy(v) { return Ok(None); }
    let root = v.as_object().ok_or_else(unreadable)?;
    if root.get("user").and_then(Value::as_str) == Some(user) { return Ok(Some(v)); }
    let Some(guests) = root.get("guests").filter(|v| truthy(v)) else { return Ok(None); };
    Ok(guests.as_object().ok_or_else(unreadable)?.get(user).filter(|v| !v.is_null()))
}
fn account_mut<'a>(v: &'a mut Value, user: &str) -> Result<Option<&'a mut Value>> {
    if !truthy(v) { return Ok(None); }
    if !v.is_object() { return Err(unreadable()); }
    if v.get("user").and_then(Value::as_str) == Some(user) { return Ok(Some(v)); }
    let Some(guests) = v.get_mut("guests").filter(|v| truthy(v)) else { return Ok(None); };
    Ok(guests.as_object_mut().ok_or_else(unreadable)?.get_mut(user).filter(|v| !v.is_null()))
}
// O leitor histórico ignora lixo ASCII e encerra no primeiro padding completo.
fn stored_base64(value: &Value) -> Result<Vec<u8>> {
    let invalid = || Failure::code(500, "sync_vault_unreadable", "cofre ilegível");
    let text = value.as_str().filter(|s| s.is_ascii()).ok_or_else(invalid)?;
    let mut output = Vec::new();
    let mut position = 0;
    let mut remainder = 0u8;
    let mut padding = 0;
    for byte in text.bytes() {
        if byte == b'=' {
            padding += 1;
            if position >= 2 && position + padding >= 4 { return Ok(output); }
            continue;
        }
        let digit = match byte {
            b'A'..=b'Z' => byte - b'A',
            b'a'..=b'z' => byte - b'a' + 26,
            b'0'..=b'9' => byte - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            _ => continue,
        };
        padding = 0;
        match position {
            0 => remainder = digit,
            1 => { output.push((remainder << 2) | (digit >> 4)); remainder = digit & 15; }
            2 => { output.push((remainder << 4) | (digit >> 2)); remainder = digit & 3; }
            _ => { output.push((remainder << 6) | digit); remainder = 0; }
        }
        position = (position + 1) % 4;
    }
    if position == 0 { Ok(output) } else { Err(invalid()) }
}
fn generation_of(v: &Value) -> Result<String> {
    let salt = stored_base64(&v["verifier_salt"])?;
    Ok(salt.iter().take(4).map(|b| format!("{b:02x}")).collect())
}
fn sign(secret: &[u8], bytes: &[u8]) -> String {
    hmac::sign(&hmac::Key::new(hmac::HMAC_SHA256, secret), bytes)
        .as_ref()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}
pub fn verifier(auth: &str, salt: &[u8]) -> String {
    let mut out = [0; 32];
    pbkdf2::derive(
        pbkdf2::PBKDF2_HMAC_SHA256,
        NonZeroU32::new(200000).unwrap(),
        salt,
        auth.as_bytes(),
        &mut out,
    );
    STANDARD.encode(out)
}
pub fn unauthorized() -> Failure {
    Failure::code(401, "erro_nao_autorizado", "unauthorized")
}
fn validate_key(b: &Value, key: &str, size: usize, max: usize) -> Result<()> {
    let s = v::text(b, key, 0, max)?;
    if super::schema::base64_bytes(&s).ok().is_none_or(|v| v.len() != size) {
        return Err(v::value_error(
            key,
            b[key].clone(),
            "tamanho inválido da credencial",
        ));
    }
    Ok(())
}
fn validate_blob(b: &Value) -> Result<()> {
    v::forbid_extra(b, &["iv", "data"])?;
    for (key, min, max, size) in [("iv", 16, 16, 12), ("data", 24, 104_857_600, 16)] {
        let s = v::text(b, key, min, max)?;
        if super::schema::base64_bytes(&s).ok().is_none_or(|v| {
            if key == "iv" {
                v.len() != size
            } else {
                v.len() < size
            }
        }) {
            return Err(v::value_error(
                key,
                b[key].clone(),
                "tamanho inválido do conteúdo cifrado",
            ));
        }
    }
    Ok(())
}

// A revisão do cofre não tem teto: o formato em disco vem do inteiro Python.
fn increment_revision(value: &Value) -> Result<Value> {
    let text = value.to_string();
    let negative = text.starts_with('-');
    let mut digits = text.trim_start_matches('-').as_bytes().to_vec();
    if digits.is_empty() || !digits.iter().all(u8::is_ascii_digit) {
        return Err(Failure::code(
            500,
            "sync_revision_invalid",
            "revisão inválida",
        ));
    }
    if negative {
        for digit in digits.iter_mut().rev() {
            if *digit > b'0' {
                *digit -= 1;
                break;
            }
            *digit = b'9';
        }
        let magnitude = String::from_utf8(digits).expect("dígitos ASCII");
        let magnitude = magnitude.trim_start_matches('0');
        return serde_json::from_str(&if magnitude.is_empty() {
            "0".into()
        } else {
            format!("-{magnitude}")
        })
        .map_err(|_| Failure::code(500, "sync_revision_invalid", "revisão inválida"));
    }
    let mut carry = true;
    for digit in digits.iter_mut().rev() {
        if *digit < b'9' {
            *digit += 1;
            carry = false;
            break;
        }
        *digit = b'0';
    }
    if carry {
        digits.insert(0, b'1');
    }
    serde_json::from_slice(&digits)
        .map_err(|_| Failure::code(500, "sync_revision_invalid", "revisão inválida"))
}

#[cfg(all(test, unix))]
mod diary_io_tests {
    use super::*;
    fn finish(dir: &std::path::Path) -> Value {
        std::fs::read_dir(dir).unwrap().flat_map(|entry| {
            std::fs::read_to_string(entry.unwrap().path()).unwrap().lines()
                .map(|line| serde_json::from_str::<Value>(line).unwrap()).collect::<Vec<_>>()
        }).find(|event|event["evento"]=="sync.prova.fim").unwrap()
    }
    #[test]
    fn sync_diary_io_preserves_load_and_save_failures() {
        for operation in ["load","save","config"] {
            let tmp=tempfile::tempdir().unwrap();
            let path=tmp.path().join("vault");std::fs::create_dir(&path).unwrap();
            let mut sync=Sync::new(path,tmp.path().join("config.json"),b"fixture".to_vec());
            if operation=="config" { sync.config_path=sync.path.clone(); }
            let diary=tmp.path().join("diary");sync.diary.directory=Some(diary.clone());
            let result=sync.diary.trace("sync.prova",||match operation {"load"=>sync.load().map(|_|()),"save"=>sync.save(&json!({})),_=>sync.config(true)});
            assert_eq!(result.unwrap_err().status,500);
            let event=finish(&diary);
            assert_eq!(event["errno"],21,"{operation}");
            assert_eq!(event["erro_tipo"],match operation {"load"=>"sync_vault_unreadable","save"=>"sync_vault_write_failed",_=>"sync_config_unreadable"});
        }
    }
}
