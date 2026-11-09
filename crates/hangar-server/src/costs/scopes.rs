//! Descoberta local das fontes de custos, sem consultar o backend Python.
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};

use serde_json::Value;

use super::collect::{
    ClaudeScope, CodexScope, CollectError, KimiScope, PiScope, ScopeSource, Scopes,
};
use crate::accounts::{
    Provider,
    catalog::{Account, AccountService, resolved},
    environment::AccountEnvironment,
};
use crate::list::discover_other::Dirs;
use crate::transcript::py::{py_str, truthy};

pub struct NativeScopes {
    accounts: AccountService,
    dirs: Dirs,
    repo: PathBuf,
}

impl ScopeSource for Option<NativeScopes> {
    fn fetch(&self) -> Result<Scopes, CollectError> {
        self.as_ref().ok_or(CollectError::NoScopes)?.fetch()
    }
}

impl NativeScopes {
    pub fn new(mut env: AccountEnvironment, dirs: Dirs, repo: PathBuf) -> Self {
        env.claude_base = dirs.claude.clone();
        env.codex_default = dirs.codex_home.clone();
        Self {
            accounts: AccountService::new(env),
            dirs,
            repo,
        }
    }
}

fn account_info(
    config: &Path,
    home: &Path,
    fallback: &str,
) -> Result<(String, String), CollectError> {
    for file in [config.join(".claude.json"), home.join(".claude.json")] {
        let Ok(bytes) = fs::read(file) else { continue };
        let Ok(Value::Object(data)) = serde_json::from_slice::<Value>(&bytes) else {
            continue;
        };
        let Some(oauth) = data.get("oauthAccount").filter(|v| truthy(Some(v))) else {
            continue;
        };
        // Estrutura inválida não é ausência de conta: a fonte anterior também recusava essa leitura.
        let oauth = oauth.as_object().ok_or(CollectError::NoScopes)?;
        let Some(uuid) = oauth.get("accountUuid").filter(|v| truthy(Some(v))) else {
            continue;
        };
        let label = match oauth.get("emailAddress").filter(|v| truthy(Some(v))) {
            Some(email) => email.as_str().ok_or(CollectError::NoScopes)?,
            None => fallback,
        };
        return Ok((py_str(uuid), label.to_owned()));
    }
    Ok((fallback.to_owned(), fallback.to_owned()))
}

impl ScopeSource for NativeScopes {
    fn fetch(&self) -> Result<Scopes, CollectError> {
        let catalog = self.accounts.claude_catalog().map_err(|error| {
            tracing::warn!(code = error.code, "custos: catálogo Claude indisponível");
            CollectError::NoScopes
        })?;
        let mut claude = Vec::new();
        for entry in catalog.as_array().ok_or(CollectError::NoScopes)? {
            let path = entry["path"].as_str().ok_or(CollectError::NoScopes)?;
            let label = entry["label"].as_str().ok_or(CollectError::NoScopes)?;
            let (uuid, label) = account_info(Path::new(path), &self.accounts.env.home, label)?;
            claude.push(ClaudeScope {
                root: Path::new(path).join("projects"),
                account: format!("anthropic:{uuid}"),
                label,
            });
        }
        // O rótulo é da identidade da conta, compartilhado pelas pastas que guardam o mesmo UUID.
        let labels: BTreeMap<_, _> = claude
            .iter()
            .map(|scope| (scope.account.clone(), scope.label.clone()))
            .collect();
        for scope in &mut claude {
            if let Some(label) = labels.get(&scope.account) {
                scope.label.clone_from(label);
            }
        }
        let accounts = match self.accounts.snapshot(Provider::Codex) {
            Ok(accounts) => accounts,
            Err(error) if error.code == "account_storage_unavailable" => {
                // A leitura anterior conserva a conta padrão quando não consegue enumerar as adicionais.
                tracing::warn!(
                    code = error.code,
                    "custos: contas Codex indisponíveis; conserva a padrão"
                );
                vec![Account {
                    id: "default".into(),
                    home: self.dirs.codex_home.clone(),
                    is_default: true,
                }]
            }
            Err(error) => {
                tracing::warn!(code = error.code, "custos: catálogo Codex recusado");
                return Err(CollectError::NoScopes);
            }
        };
        let codex = accounts
            .into_iter()
            .map(|account| {
                let home = resolved(&account.home);
                CodexScope {
                    account: format!("codex:{}", home.display()),
                    label: format!("Codex · {}", account.id),
                    home,
                }
            })
            .collect();
        let omp = self.dirs.omp_agent.join("sessions");
        let mut pi = Vec::new();
        for (root, source) in [(&self.dirs.pi_sessions, "pi"), (&omp, "omp")] {
            if source == "omp" && root == &self.dirs.pi_sessions {
                if crate::warn_limit::allow(None, "costs_shared_pi_omp_root") {
                    tracing::warn!(
                        code = "costs_shared_pi_omp_root",
                        "custos: Pi e omp compartilham raiz; contabiliza somente Pi"
                    );
                }
                continue;
            }
            if root.is_dir() {
                pi.push(PiScope {
                    root: root.clone(),
                    source: source.into(),
                });
            }
        }
        let kimi_root = self.dirs.kimi_home.join("sessions");
        let kimi = kimi_root.is_dir().then(|| KimiScope {
            root: kimi_root,
            index: self.dirs.kimi_home.join("session_index.jsonl"),
        });
        Ok(Scopes {
            claude,
            codex,
            pi,
            kimi,
            repo: self.repo.clone(),
        })
    }
}
