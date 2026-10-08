//! Leitor de identidade isolado; compartilha o cliente JSON-RPC com as sessões.
use super::{
    AccountKey, GuardMode, Provider,
    catalog::{Account, AccountService, codex_dto},
};
use hangar_codex::{
    client::{Client, Incoming},
    proto::{ClientInfo, ClientRequest, GetAccountParams, InitializeParams},
};
use serde_json::{Value, json};
use std::{path::PathBuf, process::Stdio, time::Duration};

pub fn auth_public(result: &Value) -> Value {
    let Some(account) = result.get("account") else {
        return unavailable(false);
    };
    if account.is_null() {
        return super::catalog::disconnected_auth();
    }
    match account["type"].as_str() {
        Some("chatgpt") => {
            json!({"method":"oauth","status":"connected","email":account["email"],"plan":account["planType"]})
        }
        Some("apiKey") => json!({"method":"api_key","status":"connected","email":null,"plan":null}),
        _ => unavailable(false),
    }
}
fn unavailable(missing: bool) -> Value {
    let mut result = json!({"method":"unknown","status":"unavailable","email":null,"plan":null});
    if missing {
        result["reason"] = json!("cli_missing");
    }
    result
}

impl AccountService {
    pub fn visible_codex_accounts(&self) -> Result<Vec<Account>, super::catalog::AccountError> {
        let mut accounts = self.snapshot(Provider::Codex)?;
        if self.codex_command().is_none() && !self.env.codex_default.exists() {
            accounts.retain(|account| !account.is_default);
        }
        Ok(accounts)
    }
    /// O estado de preparo vem do coordenador; ausência desse serviço não vira idle.
    pub async fn codex_snapshot(&self, account: &Account, sync: Value) -> Value {
        codex_dto(account, self.read_codex_auth(account).await, sync)
    }
    pub async fn read_codex_auth(&self, account: &Account) -> Value {
        // O worker conserva guarda e árvore até o fim, mesmo se a requisição HTTP sumir.
        let service = self.clone();
        let account = account.clone();
        tokio::spawn(async move { service.read_codex_auth_owned(&account).await })
            .await
            .unwrap_or_else(|_| unavailable(false))
    }
    async fn read_codex_auth_owned(&self, account: &Account) -> Value {
        let Ok(key) = AccountKey::new(Provider::Codex, &account.home) else {
            return unavailable(false);
        };
        let Ok(_guard) = self.locks.try_acquire(&key, GuardMode::Shared) else {
            return unavailable(false);
        };
        let Ok(current) = self.resolve(Provider::Codex, &account.id) else {
            return unavailable(false);
        };
        if AccountKey::new(Provider::Codex, &current.home)
            .ok()
            .as_ref()
            != Some(&key)
        {
            return unavailable(false);
        }
        let Some(mut command) = self.codex_command() else {
            return unavailable(true);
        };
        let mut nonce = [0u8; 16];
        if ring::rand::SecureRandom::fill(&ring::rand::SystemRandom::new(), &mut nonce).is_err() {
            return unavailable(false);
        }
        let suffix: String = nonce.iter().map(|byte| format!("{byte:02x}")).collect();
        let admin_path = std::env::temp_dir().join(format!("hangar-codex-admin-{suffix}"));
        let Ok(_admin) = super::storage::NewDirectory::create(&admin_path) else {
            return unavailable(false);
        };
        command.args(["-c", "project_root_markers=[]"]);
        if !account.is_default {
            command.args(["-c", r#"cli_auth_credentials_store="file""#]);
        }
        command
            .args(["app-server"])
            .env_clear()
            .envs(self.env.codex(account))
            .current_dir(&admin_path)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true);
        let Ok(mut tree) = crate::terminal_process::CommandTree::configure(&mut command) else {
            return unavailable(false);
        };
        let Ok(mut child) = command.spawn() else {
            return unavailable(false);
        };
        if tree.attach(&child).is_err() {
            crate::terminal_process::finish(&mut tree).await;
            let _ = child.wait().await;
            return unavailable(false);
        }
        let (client, mut incoming) =
            Client::over_lines(child.stdout.take().unwrap(), child.stdin.take().unwrap());
        let reply = client.clone();
        let drain = tokio::spawn(async move {
            while let Some(event) = incoming.recv().await {
                if let Incoming::Request { id, .. } = event {
                    let _ = reply
                        .respond(
                            id,
                            Err((-32601, "leitor de conta não executa pedidos".into())),
                        )
                        .await;
                }
            }
        });
        let result = tokio::time::timeout(Duration::from_secs(6), async {
            client
                .request::<Value>(
                    ClientRequest::Initialize(InitializeParams {
                        client_info: ClientInfo {
                            name: "hangar_accounts".into(),
                            title: None,
                            version: env!("CARGO_PKG_VERSION").into(),
                        },
                        capabilities: None,
                    }),
                    Duration::from_secs(3),
                )
                .await?;
            client.notify("initialized", json!({})).await?;
            client
                .request::<Value>(
                    ClientRequest::AccountRead(GetAccountParams {
                        refresh_token: false,
                    }),
                    Duration::from_secs(3),
                )
                .await
        })
        .await;
        drain.abort();
        drop(client);
        crate::terminal_process::finish(&mut tree).await;
        let _ = child.wait().await;
        match result {
            Ok(Ok(result)) => auth_public(&result),
            _ => unavailable(false),
        }
    }
    fn codex_command(&self) -> Option<tokio::process::Command> {
        let path = self
            .env
            .base
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case("PATH"))?
            .1;
        for dir in std::env::split_paths(path) {
            if cfg!(windows) {
                let exe = dir.join("codex.exe");
                if exe.is_file() {
                    return Some(tokio::process::Command::new(exe));
                }
                // A instalação npm publica .cmd; executar o JS evita cmd.exe e seu quoting.
                let script = dir.join("node_modules/@openai/codex/bin/codex.js");
                if script.is_file() {
                    let node = std::env::split_paths(path)
                        .map(|dir| dir.join("node.exe"))
                        .find(|p| p.is_file())?;
                    let mut command = tokio::process::Command::new(node);
                    command.arg(script);
                    return Some(command);
                }
            } else {
                let executable: PathBuf = dir.join("codex");
                if executable.is_file() {
                    return Some(tokio::process::Command::new(executable));
                }
            }
        }
        None
    }
}
