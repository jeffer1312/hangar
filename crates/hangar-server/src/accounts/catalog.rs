use super::{
    AccountKey, AccountLocks, GuardMode, Provider,
    environment::{AccountEnvironment, expand},
    storage,
};
use serde_json::{Value, json};
use std::{
    fs,
    path::{Path, PathBuf},
};

#[derive(Clone, Debug)]
pub struct Account {
    pub id: String,
    pub home: PathBuf,
    pub is_default: bool,
}

#[derive(Debug)]
pub struct AccountError {
    pub status: u16,
    pub code: &'static str,
    pub message: String,
    pub params: Value,
}
impl AccountError {
    pub fn new(status: u16, code: &'static str, message: impl Into<String>, params: Value) -> Self {
        Self {
            status,
            code,
            message: message.into(),
            params,
        }
    }
    pub fn codex(status: u16, code: &'static str, id: Option<&str>) -> Self {
        let message = match code {
            "codex_account_not_found" => "conta Codex não encontrada",
            "codex_account_exists" => "conta Codex já existe",
            "codex_account_default_protected" => "a conta padrão do Codex não pode ser apagada",
            "codex_account_invalid_marker" => "conta Codex inválida",
            "codex_account_in_use" => "conta Codex está em uso",
            "codex_account_prepare_required" => "prepare a conta Codex antes do login",
            "codex_account_auth_storage_invalid" => "a conta Codex precisa usar armazenamento em arquivo",
            "codex_login_attempt_mismatch" => "a tentativa de login já mudou",
            "codex_account_delete_failed" => "não foi possível apagar a conta Codex",
            _ => "operação de conta Codex recusada",
        };
        Self::new(
            status,
            code,
            message,
            id.map(|id| json!({"account_id":id}))
                .unwrap_or_else(|| json!({})),
        )
    }
    pub fn io() -> Self {
        Self::new(
            503,
            "account_storage_unavailable",
            "não foi possível consultar as contas",
            json!({}),
        )
    }
}

pub fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 32
        && (name.as_bytes()[0].is_ascii_lowercase() || name.as_bytes()[0].is_ascii_digit())
        && name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_' || b == b'-')
}
fn canonical(provider: Provider, path: &Path) -> Result<PathBuf, AccountError> {
    AccountKey::new(provider, path)
        .map(|key| key.canonical_home)
        .map_err(|_| AccountError::io())
}
fn displayed(path: &Path) -> String {
    // O prefixo estendido do Windows é detalhe do canonicalize, ausente nos DTOs Python.
    let value = path.to_string_lossy();
    value
        .strip_prefix(r"\\?\UNC\")
        .map(|rest| format!(r"\\{rest}"))
        .or_else(|| value.strip_prefix(r"\\?\").map(str::to_owned))
        .unwrap_or_else(|| value.into_owned())
}
pub fn resolved(path: &Path) -> PathBuf {
    path.canonicalize()
        .map(|p| PathBuf::from(displayed(&p)))
        .unwrap_or_else(|_| path.to_owned())
}

#[derive(Clone)]
pub struct AccountService {
    pub claude_auth: super::claude_auth::AuthCache,
    pub claude_logins: super::claude_login::ClaudeLogins,
    pub codex_logins: super::codex_login::CodexLogins,
    pub codex_auth: super::native::AuthCache,
    pub codex_readers: super::native::NativeReaders,
    pub env: AccountEnvironment,
    pub locks: AccountLocks,
    pub preparations: super::preparation::Preparations,
    pub preparation_gates: super::preparation::PreparationGates,
}
impl AccountService {
    pub fn new(env: AccountEnvironment) -> Self {
        Self {
            claude_auth: Default::default(),
            claude_logins: Default::default(),
            codex_logins: Default::default(),
            codex_auth: Default::default(),
            codex_readers: Default::default(),
            locks: AccountLocks::new(env.home.join(".hangar/account-locks")),
            preparations: Default::default(),
            preparation_gates: Default::default(),
            env,
        }
    }
    pub fn target(&self, provider: Provider, name: &str) -> PathBuf {
        self.env.home.join(format!(".{}-{name}", provider.as_str()))
    }
    pub fn managed(&self, provider: Provider, path: &Path, name: &str) -> bool {
        if !storage::real_dir(path) || storage::pending(path) {
            return false;
        }
        let marker = path.join(match provider {
            Provider::Claude => storage::CLAUDE_MARKER,
            Provider::Codex => storage::CODEX_MARKER,
        });
        if !storage::real_file(&marker) {
            return false;
        }
        if provider == Provider::Claude {
            return true;
        }
        fs::read(marker)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
            .is_some_and(|data| {
                data["version"].as_u64() == Some(1) && data["id"].as_str() == Some(name)
            })
    }
    pub fn snapshot(&self, provider: Provider) -> Result<Vec<Account>, AccountError> {
        let mut accounts = vec![];
        if provider == Provider::Codex {
            accounts.push(Account {
                id: "default".into(),
                home: resolved(&self.env.codex_default),
                is_default: true,
            });
        }
        let prefix = format!(".{}-", provider.as_str());
        let entries = fs::read_dir(&self.env.home).map_err(|_| AccountError::io())?;
        let mut managed = vec![];
        for entry in entries {
            let entry = entry.map_err(|_| AccountError::io())?;
            let name = entry.file_name();
            let Some(id) = name.to_str().and_then(|name| name.strip_prefix(&prefix)) else {
                continue;
            };
            if valid_name(id)
                && !(provider == Provider::Codex && id == "default")
                && self.managed(provider, &entry.path(), id)
            {
                managed.push(Account {
                    id: id.into(),
                    home: resolved(&entry.path()),
                    is_default: false,
                });
            }
        }
        managed.sort_by(|a, b| a.id.cmp(&b.id));
        accounts.extend(managed);
        Ok(accounts)
    }
    pub fn resolve(&self, provider: Provider, id: &str) -> Result<Account, AccountError> {
        if !valid_name(id) {
            return Err(if provider == Provider::Codex {
                AccountError::codex(400, "codex_account_invalid_name", None)
            } else {
                AccountError::new(
                    400,
                    "erro_conta_nome_invalido",
                    "nome: use minúsculas, números, '-' ou '_' (até 32 caracteres)",
                    json!({}),
                )
            });
        }
        if provider == Provider::Codex && id == "default" {
            return Ok(Account {
                id: id.into(),
                home: resolved(&self.env.codex_default),
                is_default: true,
            });
        }
        let path = self.target(provider, id);
        if !storage::real_dir(&path)
            || (provider == Provider::Claude && !self.managed(provider, &path, id))
        {
            return Err(if provider == Provider::Codex {
                AccountError::codex(404, "codex_account_not_found", Some(id))
            } else {
                AccountError::new(
                    404,
                    "erro_conta_inexistente",
                    format!("{} não é uma conta criada pelo hangar", displayed(&path)),
                    json!({"nome":id}),
                )
            });
        }
        if !self.managed(provider, &path, id) {
            return Err(AccountError::codex(
                409,
                "codex_account_invalid_marker",
                Some(id),
            ));
        }
        Ok(Account {
            id: id.into(),
            home: resolved(&path),
            is_default: false,
        })
    }
    pub fn claude_catalog(&self) -> Result<Value, AccountError> {
        let mut entries = vec![];
        if !self.env.claude_fixed.trim().is_empty() {
            for item in self
                .env
                .claude_fixed
                .split(',')
                .map(str::trim)
                .filter(|v| !v.is_empty())
            {
                let pair = item.split_once(':').filter(|(label, tail)| {
                    !(cfg!(windows)
                        && label.len() == 1
                        && label.as_bytes()[0].is_ascii_alphabetic()
                        && tail.starts_with(['\\', '/']))
                });
                let (label, path) = pair.unwrap_or(("", item));
                let path = resolved(&expand(path, &self.env.home));
                if !storage::pending(&path) {
                    entries.push((label.trim().to_owned(), path));
                }
            }
        } else {
            let mut found = vec![];
            for entry in fs::read_dir(&self.env.home).map_err(|_| AccountError::io())? {
                let entry = entry.map_err(|_| AccountError::io())?;
                let path = entry.path();
                if !entry.file_name().to_string_lossy().starts_with(".claude")
                    || !storage::real_dir(&path)
                    || storage::pending(&path)
                {
                    continue;
                }
                let name = entry.file_name();
                let name = name.to_string_lossy();
                let managed = self.managed(
                    Provider::Claude,
                    &path,
                    name.strip_prefix(".claude-").unwrap_or(""),
                );
                let unmarked = fs::symlink_metadata(path.join(storage::CLAUDE_MARKER)).is_err();
                if managed
                    || (unmarked
                        && storage::real_file(&path.join(".credentials.json"))
                        && storage::real_dir(&path.join("projects")))
                {
                    found.push(resolved(&path));
                }
            }
            found.sort_by_cached_key(|path| {
                std::cmp::Reverse(projects_mtime(&path.join("projects")))
            });
            let base = resolved(&self.env.claude_base);
            if !found.contains(&base) && !storage::pending(&base) {
                found.insert(0, base);
            }
            entries.extend(found.into_iter().map(|path| (String::new(), path)));
        }
        let aliases: Value = fs::read(self.env.home.join(".claude/.hangar-apelidos.json"))
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or(Value::Null);
        let active = canonical(Provider::Claude, &self.env.claude_base)?;
        let mut seen = std::collections::HashSet::new();
        let mut output = vec![];
        for (label, path) in entries {
            let canonical = canonical(Provider::Claude, &path)?;
            if !seen.insert(canonical.clone()) {
                continue;
            }
            let path_text = displayed(&path);
            let name = path.file_name().unwrap_or_default().to_string_lossy();
            let default_label = name.strip_prefix(".claude-").unwrap_or(&name);
            let default_label = default_label
                .strip_prefix(".claude")
                .unwrap_or(default_label);
            let label = aliases
                .get(format!("claude:{path_text}"))
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|v| !v.is_empty())
                .unwrap_or(if label.is_empty() {
                    if default_label.is_empty() {
                        "default"
                    } else {
                        default_label
                    }
                } else {
                    &label
                });
            output.push(json!({"path":path_text,"label":label,"active":canonical == active}));
        }
        Ok(json!(output))
    }
    pub fn protect_delete(
        &self,
        provider: Provider,
        account: &Account,
    ) -> Result<(), AccountError> {
        let key = canonical(provider, &account.home)?;
        if provider == Provider::Codex {
            if account.is_default || key == canonical(provider, &self.env.codex_default)? {
                return Err(AccountError::codex(
                    409,
                    "codex_account_default_protected",
                    Some(&account.id),
                ));
            }
        } else {
            if key == canonical(provider, &self.env.claude_base)?
                || key == canonical(provider, &self.env.home.join(".claude"))?
            {
                return Err(AccountError::new(
                    409,
                    "erro_conta_ativa_backend",
                    "esta conta é a configuração ativa do backend — não dá pra apagar por aqui",
                    json!({}),
                ));
            }
            if !self.env.claude_fixed.trim().is_empty()
                && self
                    .claude_catalog()?
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|row| {
                        row["path"].as_str().is_some_and(|path| {
                            canonical(provider, Path::new(path)).is_ok_and(|path| path == key)
                        })
                    })
            {
                return Err(AccountError::new(
                    409,
                    "erro_conta_lista_fixa",
                    "CP_CLAUDE_CONFIG_DIRS está setado: esta conta está na lista fixa por ambiente. Remova-a da variável antes de apagar.",
                    json!({}),
                ));
            }
        }
        Ok(())
    }
    /// A Task de preparo fornece a semeadura; só o retorno publica o marcador.
    pub fn create(
        &self,
        provider: Provider,
        name: &str,
        seed: impl FnOnce(&Path) -> Result<(), AccountError>,
    ) -> Result<Account, AccountError> {
        if !valid_name(name) || (provider == Provider::Codex && name == "default") {
            return Err(if provider == Provider::Codex {
                AccountError::codex(400, "codex_account_invalid_name", None)
            } else {
                AccountError::new(
                    400,
                    "erro_conta_nome_invalido",
                    "nome: use minúsculas, números, '-' ou '_' (até 32 caracteres)",
                    json!({}),
                )
            });
        }
        if provider == Provider::Claude && !self.env.claude_fixed.trim().is_empty() {
            return Err(AccountError::new(
                409,
                "erro_config_dirs_fixo",
                "CP_CLAUDE_CONFIG_DIRS está setado: a lista de contas é fixa por ambiente. Remova a variável ou acrescente a conta nela.",
                json!({}),
            ));
        }
        let path = self.target(provider, name);
        let key = AccountKey::new(provider, &path).map_err(|_| AccountError::io())?;
        let _guard = self
            .locks
            .try_acquire(&key, GuardMode::Shared)
            .map_err(|_| {
                AccountError::new(409, "account_busy", "a conta está ocupada", json!({}))
            })?;
        if provider == Provider::Codex
            && key.canonical_home == canonical(provider, &self.env.codex_default)?
        {
            return Err(AccountError::codex(
                409,
                "codex_account_conflict",
                Some(name),
            ));
        }
        let directory = storage::NewDirectory::create(&path).map_err(|error| {
            if error.kind() == std::io::ErrorKind::AlreadyExists {
                if provider == Provider::Claude {
                    AccountError::new(
                        409,
                        "account_exists",
                        format!("já existe {}", displayed(&path)),
                        json!({}),
                    )
                } else {
                    AccountError::codex(409, "codex_account_exists", Some(name))
                }
            } else {
                AccountError::codex(500, "codex_account_create_failed", Some(name))
            }
        })?;
        if provider == Provider::Codex {
            fs::write(
                path.join("config.toml"),
                b"cli_auth_credentials_store = \"file\"\n",
            )
            .map_err(|_| AccountError::codex(500, "codex_account_marker_failed", Some(name)))?;
            Self::persist(&self.result_path(&key)?, &idle_sync())?;
        }
        seed(&path)?;
        let marker = if provider == Provider::Codex {
            storage::CODEX_MARKER
        } else {
            storage::CLAUDE_MARKER
        };
        let body = if provider == Provider::Codex {
            format!("{}\n", json!({"version":1,"id":name}))
        } else {
            String::new()
        };
        directory
            .publish(marker, body.as_bytes())
            .map_err(|_| AccountError::io())?;
        Ok(Account {
            id: name.into(),
            home: resolved(&path),
            is_default: false,
        })
    }
    /// O chamador conserva a guarda exclusiva e relê os fatos antes deste ponto.
    pub fn delete(
        &self,
        provider: Provider,
        account: &Account,
        guard: &super::AccountGuard,
        facts: &super::UsageFacts,
    ) -> Result<(), AccountError> {
        let current = self.resolve(provider, &account.id)?;
        let key = AccountKey::new(provider, &current.home).map_err(|_| AccountError::io())?;
        if guard.mode != GuardMode::Exclusive || guard.key != key {
            return Err(AccountError::io());
        }
        self.protect_delete(provider, &current)?;
        facts.ensure_unused().map_err(|code| {
            if code == "account_in_use" {
                if provider == Provider::Codex {
                    return AccountError::codex(409, "codex_account_in_use", Some(&account.id));
                }
                if let Some(name) = facts.sessions.first() {
                    return AccountError::new(
                        409,
                        "erro_sessao_usa_conta",
                        format!("a sessão '{name}' está usando esta conta"),
                        json!({"nome":name}),
                    );
                }
                return AccountError::new(
                    409,
                    "erro_processos_usam_conta",
                    format!("processo(s) {:?} estão usando esta conta", facts.pids),
                    json!({"pids":facts.pids}),
                );
            }
            AccountError::new(
                409,
                code,
                "não foi possível confirmar que a conta está livre",
                json!({}),
            )
        })?;
        storage::remove_tree(&current.home).map_err(|_| {
            AccountError::codex(500, "codex_account_delete_failed", Some(&account.id))
        })?;
        self.preparations.lock().unwrap().remove(&key);
        for path in [
            self.preparation_path(&key)?,
            self.result_path(&key)?,
            self.force_path(&key)?,
        ] {
            match fs::remove_file(path) {
                Err(error) if error.kind() != std::io::ErrorKind::NotFound => {
                    return Err(AccountError::io());
                }
                _ => {}
            }
        }
        Ok(())
    }
}

fn projects_mtime(path: &Path) -> std::time::SystemTime {
    let mut latest = std::time::UNIX_EPOCH;
    if let Ok(entries) = fs::read_dir(path) {
        for entry in entries.flatten() {
            if let Ok(meta) = fs::symlink_metadata(entry.path()) {
                if meta.file_type().is_symlink() {
                    continue;
                }
                let time = if meta.is_dir() {
                    projects_mtime(&entry.path())
                } else {
                    meta.modified().unwrap_or(latest)
                };
                latest = latest.max(time);
            }
        }
    }
    latest
}

pub fn idle_sync() -> Value {
    json!({"status":"idle","trust_pending":false,"issues":[],"etapa":null,"herdado":null})
}
pub fn disconnected_auth() -> Value {
    json!({"method":"none","status":"disconnected","email":null,"plan":null})
}
pub fn codex_dto(account: &Account, auth: Value, sync: Value) -> Value {
    let home = displayed(&resolved(&account.home));
    let settings = account.is_default
        && (account.home.join("config.toml").is_file()
            || ["agents", "skills", "hooks", "plugins"].iter().any(|name| {
                fs::read_dir(account.home.join(name))
                    .is_ok_and(|mut entries| entries.next().is_some())
            }));
    json!({"id":account.id,"credential_id":format!("codex:{home}"),"name":account.id,"home":home,
        "is_default":account.is_default,"auth":auth,"sync":sync,"has_settings":settings})
}
