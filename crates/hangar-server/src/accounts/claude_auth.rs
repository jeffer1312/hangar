//! Identidade nativa e cache invalidado pela credencial, sem renovar autenticação.
use super::{
    AccountKey, GuardMode, Provider,
    catalog::{Account, AccountError, AccountService},
};
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    fs,
    path::Path,
    process::Stdio,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::io::AsyncReadExt;

type CacheEntry = (tokio::time::Instant, Option<Vec<u8>>, u64, Value);
#[derive(Clone, Default)]
pub struct AuthCache {
    entries: Arc<Mutex<HashMap<AccountKey, CacheEntry>>>,
    generations: Arc<Mutex<HashMap<AccountKey, u64>>>,
}
impl AuthCache {
    pub fn invalidate(&self, key: &AccountKey) {
        let mut generations = self.generations.lock().unwrap();
        *generations.entry(key.clone()).or_default() += 1;
        self.entries.lock().unwrap().remove(key);
    }
}
pub fn unavailable(reason: &str) -> Value {
    json!({"estado":"indisponivel","loggedIn":null,"email":null,"plano":null,"motivo":reason,"refreshExpiresAt":null})
}
pub fn auth_public(raw: Option<Value>) -> Value {
    let Some(raw) = raw else {
        return unavailable("cli-indisponivel");
    };
    let Some(logged) = raw["loggedIn"].as_bool() else {
        return unavailable("formato-desconhecido");
    };
    let text = |key: &str| {
        raw[key]
            .as_str()
            .filter(|s| !s.contains('\u{fffd}'))
            .map(str::to_owned)
    };
    json!({"estado":"ok","loggedIn":logged,"email":text("email"),"plano":text("subscriptionType"),"motivo":null,"refreshExpiresAt":null})
}
pub fn token_signature(path: &Path, strict: bool) -> Result<Option<Vec<u8>>, AccountError> {
    let bytes = match fs::read(path.join(".credentials.json")) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) if !strict => return Ok(None),
        Err(_) => return Err(credential_error()),
    };
    let raw: Value = match serde_json::from_slice(&bytes) {
        Ok(Value::Object(object)) => Value::Object(object),
        _ if !strict => return Ok(None),
        _ => return Err(credential_error()),
    };
    let oauth = &raw["claudeAiOauth"];
    if strict && !oauth.is_null() && !oauth.is_object() {
        return Err(credential_error());
    }
    Ok(oauth["accessToken"]
        .as_str()
        .filter(|s| !s.is_empty())
        .map(|token| {
            ring::digest::digest(&ring::digest::SHA256, token.as_bytes())
                .as_ref()
                .to_vec()
        }))
}
pub fn credential_error() -> AccountError {
    AccountError::new(
        409,
        "erro_login_credencial_ilegivel",
        "Não foi possível ler a credencial da conta. Tente novamente.",
        json!({}),
    )
}
impl AccountService {
    pub fn claude_by_label(&self, label: &str) -> Result<Account, AccountError> {
        self.claude_catalog()?
            .as_array()
            .and_then(|rows| rows.iter().find(|row| row["label"] == label))
            .and_then(|row| {
                row["path"].as_str().map(|path| Account {
                    id: label.into(),
                    home: path.into(),
                    is_default: row["active"] == true,
                })
            })
            .ok_or_else(|| {
                AccountError::new(
                    404,
                    "erro_conta_inexistente",
                    format!("conta {label} não existe"),
                    json!({"nome":label}),
                )
            })
    }
    pub fn validate_claude(&self, account: &Account, key: &AccountKey) -> Result<(), AccountError> {
        let current = self.claude_by_label(&account.id)?;
        if AccountKey::new(Provider::Claude, &current.home)
            .ok()
            .as_ref()
            != Some(key)
        {
            return Err(AccountError::io());
        }
        Ok(())
    }
    fn claude_command(&self) -> Option<tokio::process::Command> {
        let path = &self
            .env
            .base
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case("PATH"))?
            .1;
        for dir in std::env::split_paths(path) {
            if cfg!(windows) {
                let exe = dir.join("claude.exe");
                if exe.is_file() {
                    return Some(tokio::process::Command::new(exe));
                }
                let script = dir.join("node_modules/@anthropic-ai/claude-code/cli.js");
                if script.is_file() {
                    let node = std::env::split_paths(path)
                        .map(|dir| dir.join("node.exe"))
                        .find(|p| p.is_file())?;
                    let mut command = tokio::process::Command::new(node);
                    command.arg(script);
                    return Some(command);
                }
            } else if dir.join("claude").is_file() {
                return Some(tokio::process::Command::new(dir.join("claude")));
            }
        }
        None
    }
    async fn claude_cli(
        &self,
        account: &Account,
        args: &[&str],
    ) -> Result<(bool, Vec<u8>), &'static str> {
        let Some(mut command) = self.claude_command() else {
            return Err("cli_missing");
        };
        let mut environment = self.env.base.clone();
        environment.retain(|key, _| {
            !matches!(
                key.to_ascii_uppercase().as_str(),
                "CP_AUTH_TOKEN"
                    | "HANGAR_INTERNAL_SECRET"
                    | "HANGAR_RUNTIME_INSTANCE"
                    | "HANGAR_SERVER_LISTEN"
                    | "HANGAR_SERVER_UPSTREAM"
            ) && !key.to_ascii_uppercase().starts_with("ANTHROPIC_")
                && !matches!(
                    key.to_ascii_uppercase().as_str(),
                    "CLAUDE_CODE_OAUTH_TOKEN" | "CLAUDE_CODE_OAUTH_TOKEN_FILE"
                )
        });
        environment.insert(
            "CLAUDE_CONFIG_DIR".into(),
            account.home.to_string_lossy().into(),
        );
        command
            .args(args)
            .env_clear()
            .envs(environment)
            .current_dir(&self.env.home)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true);
        let mut tree = crate::terminal_process::CommandTree::configure(&mut command)
            .map_err(|_| "cli_failed")?;
        let mut child = command.spawn().map_err(|_| "cli_failed")?;
        if tree.attach(&child).is_err() {
            crate::terminal_process::finish(&mut tree).await;
            let _ = child.wait().await;
            return Err("cli_failed");
        }
        let mut stdout = child.stdout.take().unwrap().take(65537);
        let result = tokio::time::timeout(Duration::from_secs(10), async {
            let mut bytes = vec![];
            stdout
                .read_to_end(&mut bytes)
                .await
                .map_err(|_| "cli_failed")?;
            let status = child.wait().await.map_err(|_| "cli_failed")?;
            if bytes.len() > 65536 {
                return Err("cli_invalid");
            }
            Ok((status.success(), bytes))
        })
        .await
        .unwrap_or(Err("cli_timeout"));
        crate::terminal_process::finish(&mut tree).await;
        let _ = child.wait().await;
        result
    }
    // O chamador conserva a guarda da tentativa; nunca readquirir uma shared aqui.
    pub async fn claude_auth_guarded(&self, account: &Account) -> Value {
        let raw = self
            .claude_cli(account, &["auth", "status", "--json"])
            .await
            .ok()
            .and_then(|(_, bytes)| {
                serde_json::from_str::<Value>(&String::from_utf8_lossy(&bytes)).ok()
            })
            .filter(Value::is_object);
        let mut state = auth_public(raw);
        if state["loggedIn"] == true {
            state["refreshExpiresAt"] = fs::read(account.home.join(".credentials.json"))
                .ok()
                .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
                .and_then(|raw| raw["claudeAiOauth"]["refreshTokenExpiresAt"].as_f64())
                .map(|ms| json!(ms / 1000.0))
                .unwrap_or(Value::Null);
        }
        state
    }
    pub async fn read_claude_auth(&self, account: &Account) -> Value {
        let service = self.clone();
        let account = account.clone();
        tokio::spawn(async move {
            let Ok(key) = AccountKey::new(Provider::Claude, &account.home) else {
                return unavailable("cli-indisponivel");
            };
            let Ok(_guard) = service.locks.try_acquire(&key, GuardMode::Shared) else {
                return unavailable("cli-indisponivel");
            };
            if service.validate_claude(&account, &key).is_err() {
                return unavailable("cli-indisponivel");
            }
            let signature = fs::read(account.home.join(".credentials.json"))
                .ok()
                .map(|bytes| {
                    ring::digest::digest(&ring::digest::SHA256, &bytes)
                        .as_ref()
                        .to_vec()
                });
            let generation = *service
                .claude_auth
                .generations
                .lock()
                .unwrap()
                .get(&key)
                .unwrap_or(&0);
            if let Some((at, old, epoch, value)) =
                service.claude_auth.entries.lock().unwrap().get(&key)
                && old == &signature
                && *epoch == generation
                && at.elapsed() < Duration::from_secs(if signature.is_some() { 120 } else { 30 })
            {
                return value.clone();
            }
            let state = service.claude_auth_guarded(&account).await;
            let generations = service.claude_auth.generations.lock().unwrap();
            if *generations.get(&key).unwrap_or(&0) == generation {
                service.claude_auth.entries.lock().unwrap().insert(
                    key,
                    (
                        tokio::time::Instant::now(),
                        signature,
                        generation,
                        state.clone(),
                    ),
                );
            }
            state
        })
        .await
        .unwrap_or_else(|_| unavailable("cli-indisponivel"))
    }
    pub async fn claude_logout_guarded(&self, account: &Account) -> Result<(), AccountError> {
        let result = self.claude_cli(account, &["auth", "logout"]).await;
        let key =
            AccountKey::new(Provider::Claude, &account.home).map_err(|_| AccountError::io())?;
        self.claude_auth.invalidate(&key);
        if !matches!(result, Ok((true, _)))
            || self.claude_auth_guarded(account).await["loggedIn"] != false
        {
            return Err(AccountError::new(
                502,
                "erro_logout_nao_confirmado",
                "a conta não apareceu deslogada depois do logout",
                json!({}),
            ));
        }
        Ok(())
    }
    pub fn complete_claude_onboarding(&self, account: &Account) {
        let mut paths = vec![account.home.join(".claude.json")];
        if AccountKey::new(Provider::Claude, &account.home)
            .ok()
            .zip(AccountKey::new(Provider::Claude, &self.env.home.join(".claude")).ok())
            .is_some_and(|(account, default)| account == default)
        {
            paths.push(self.env.home.join(".claude.json"));
        }
        for path in paths {
            let result = (|| -> std::io::Result<()> {
                let mut data = match fs::read(&path) {
                    Ok(bytes) => {
                        serde_json::from_slice::<Value>(&bytes).map_err(std::io::Error::other)?
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => json!({}),
                    Err(e) => return Err(e),
                };
                if !data.is_object() {
                    return Err(std::io::Error::other("configuração ilegível"));
                }
                if data["hasCompletedOnboarding"] == true {
                    return Ok(());
                }
                data["hasCompletedOnboarding"] = json!(true);
                let tmp = path.with_extension(format!(
                    "hangar-onboarding-{}",
                    super::claude_login::nonce()?
                ));
                fs::write(
                    &tmp,
                    serde_json::to_vec_pretty(&data).map_err(std::io::Error::other)?,
                )?;
                let result = fs::rename(&tmp, &path);
                if result.is_err() {
                    let _ = fs::remove_file(tmp);
                }
                result
            })();
            if result.is_err() {
                tracing::warn!(
                    code = "claude_onboarding_failed",
                    "não foi possível marcar as boas-vindas"
                );
            }
        }
    }
    pub async fn claude_states(&self) -> Result<Value, AccountError> {
        let mut output = vec![];
        for row in self
            .claude_catalog()?
            .as_array()
            .ok_or_else(AccountError::io)?
        {
            let account =
                self.claude_by_label(row["label"].as_str().ok_or_else(AccountError::io)?)?;
            if !account.is_default
                && !super::storage::real_file(&account.home.join(super::storage::CLAUDE_MARKER))
            {
                continue;
            }
            let mut row = row.clone();
            row["login"] = self.read_claude_auth(&account).await;
            row["limite"] = limit(&account.home);
            output.push(row);
        }
        Ok(json!(output))
    }
}
fn limit(path: &Path) -> Value {
    let mut best: Option<(f64, String)> = None;
    if let Ok(files) = fs::read_dir(path.join(".hangar-status")) {
        for entry in files
            .flatten()
            .filter(|entry| entry.path().extension().is_some_and(|ext| ext == "json"))
        {
            let Some(raw) = fs::read(entry.path())
                .ok()
                .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
            else {
                continue;
            };
            let (Some(ts), Some(line)) = (
                raw["ts"].as_f64(),
                raw["line"].as_str().filter(|s| !s.trim().is_empty()),
            ) else {
                continue;
            };
            if best.as_ref().is_none_or(|(old, _)| ts > *old) {
                best = Some((ts, line.into()));
            }
        }
    }
    match best {
        Some((ts, line)) => {
            json!({"estado":"lido","linha":line,"ts":ts,"idade_s":std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_secs_f64()-ts})
        }
        None => json!({"estado":"sem_leitura","linha":null,"ts":null,"idade_s":null}),
    }
}
