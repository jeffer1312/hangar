use crate::accounts::{
    AccountKey, GuardMode, Provider,
    catalog::{Account, AccountError, AccountService},
    native::NativeProcess,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{sync::Arc, time::Duration};
use tokio::sync::Mutex;

#[derive(Clone, Default)]
pub struct Resets(pub Arc<Mutex<()>>);

/// Mesmo prazo do registro Python: uma retentativa com a mesma chave chega bem antes disso.
const ATTEMPT_TTL: f64 = 86400.0;

#[derive(Serialize, Deserialize, Clone)]
pub struct Attempt {
    pub account_id: String,
    pub credit_id: Option<String>,
    pub idempotency_key: String,
    pub accepted_at: f64,
    #[serde(default)]
    pub outcome: Option<String>,
}

pub fn weekly_used(body: &Value) -> Option<f64> {
    ["primary", "secondary"].into_iter().find_map(|key| {
        let window = &body["rateLimits"][key];
        if window["windowDurationMins"].as_f64()? != 10080.0 {
            return None;
        }
        window["usedPercent"]
            .as_f64()
            .map(|pct| pct.clamp(0.0, 100.0))
    })
}

pub fn check_first(body: &Value) -> Result<bool, AccountError> {
    let weekly = weekly_used(body).ok_or_else(|| {
        failure(
            409,
            "codex_reset_weekly_unavailable",
            "não foi possível confirmar a cota semanal",
        )
    })?;
    if weekly < 100.0 {
        return Err(AccountError::new(
            409,
            "codex_reset_weekly_not_exhausted",
            "a cota semanal ainda não acabou",
            json!({"pct":weekly.round()}),
        ));
    }
    Ok(body["rateLimitResetCredits"]["availableCount"]
        .as_u64()
        .is_some_and(|count| count > 0))
}

fn failure(status: u16, code: &'static str, message: &str) -> AccountError {
    AccountError::new(status, code, message, json!({}))
}

fn reset_failed() -> AccountError {
    failure(
        502,
        "codex_reset_failed",
        "não foi possível redefinir a cota do Codex",
    )
}

pub fn canonical_uuid(raw: &str) -> Option<String> {
    let raw = raw
        .strip_prefix("urn:uuid:")
        .unwrap_or(raw)
        .trim_matches(['{', '}']);
    let hex: String = raw.chars().filter(|ch| *ch != '-').collect();
    if hex.len() != 32 || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let hex = hex.to_ascii_lowercase();
    Some(format!(
        "{}-{}-{}-{}-{}",
        &hex[..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..]
    ))
}

impl AccountService {
    pub async fn consume_reset(
        &self,
        account: Account,
        credit: Option<String>,
        uuid: String,
    ) -> Result<Value, AccountError> {
        let service = self.clone();
        tokio::spawn(async move { service.consume_reset_owned(account, credit, uuid).await })
            .await
            .map_err(|_| AccountError::io())?
    }

    async fn consume_reset_owned(
        &self,
        account: Account,
        credit: Option<String>,
        uuid: String,
    ) -> Result<Value, AccountError> {
        let _serial = self.resets.0.lock().await;
        let key =
            AccountKey::new(Provider::Codex, &account.home).map_err(|_| AccountError::io())?;
        let _guard = self
            .locks
            .try_acquire(&key, GuardMode::Shared)
            .map_err(|_| AccountError::io())?;
        let current = self.resolve(Provider::Codex, &account.id)?;
        if AccountKey::new(Provider::Codex, &current.home)
            .ok()
            .as_ref()
            != Some(&key)
        {
            return Err(AccountError::io());
        }
        let path = self
            .quota_path()
            .with_file_name("codex-reset-attempts.json");
        let raw = match std::fs::read_to_string(&path) {
            Ok(raw) => raw,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
            Err(_) => {
                return Err(failure(
                    503,
                    "codex_reset_failed",
                    "não foi possível conferir a tentativa anterior",
                ));
            }
        };
        // Linha ilegível não trava a redefinição, e só a janela vigente fica: o registro não cresce.
        let at = super::now();
        let mut attempts = vec![];
        let mut unreadable = 0;
        for line in raw.lines().filter(|line| !line.trim().is_empty()) {
            match serde_json::from_str::<Attempt>(line) {
                Ok(attempt)
                    if at - ATTEMPT_TTL <= attempt.accepted_at
                        && attempt.accepted_at <= at + 300.0 =>
                {
                    attempts.push(attempt)
                }
                Ok(_) => {}
                Err(_) => unreadable += 1,
            }
        }
        if unreadable > 0 {
            tracing::warn!(
                code = "codex_reset_ledger_unreadable",
                lines = unreadable,
                "linhas ilegíveis no registro de redefinição"
            );
        }
        let same = attempts.iter().position(|old| {
            old.account_id == account.id && old.credit_id == credit && old.idempotency_key == uuid
        });
        if let Some(index) = same
            && let Some(outcome) = &attempts[index].outcome
        {
            return Ok(json!({"outcome":outcome}));
        }
        let mut process = NativeProcess::open(self, &account)
            .await
            .map_err(|_| reset_failed())?;
        let result = tokio::time::timeout(Duration::from_secs(60), async {
            process.initialize().await.map_err(|_| reset_failed())?;
            let current: Value = process
                .client
                .request(
                    hangar_codex::proto::ClientRequest::AccountRateLimitsRead,
                    Duration::from_secs(10),
                )
                .await
                .map_err(|_| {
                    failure(
                        502,
                        "codex_reset_failed",
                        "não foi possível confirmar a cota semanal",
                    )
                })?;
            let index = if let Some(index) = same {
                index
            } else {
                if !check_first(&current)? {
                    return Ok(json!({"outcome":"noCredit"}));
                }
                attempts.push(Attempt {
                    account_id: account.id.clone(),
                    credit_id: credit.clone(),
                    idempotency_key: uuid.clone(),
                    accepted_at: super::now(),
                    outcome: None,
                });
                save(&path, &attempts)?;
                attempts.len() - 1
            };
            let mut params = json!({"idempotencyKey":uuid});
            if let Some(credit) = credit.as_ref().filter(|s| !s.is_empty()) {
                params["creditId"] = json!(credit);
            }
            let response: Value = process
                .client
                .request_method(
                    "account/rateLimitResetCredit/consume",
                    params,
                    Duration::from_secs(30),
                )
                .await
                .map_err(|_| reset_failed())?;
            let outcome = response["outcome"]
                .as_str()
                .filter(|s| {
                    matches!(
                        *s,
                        "reset" | "nothingToReset" | "noCredit" | "alreadyRedeemed"
                    )
                })
                .ok_or_else(|| {
                    failure(
                        502,
                        "codex_reset_invalid_response",
                        "o Codex devolveu uma resposta inválida ao redefinir a cota",
                    )
                })?;
            attempts[index].outcome = Some(outcome.to_owned());
            save(&path, &attempts)?;
            let latest: Option<Value> = process
                .client
                .request(
                    hangar_codex::proto::ClientRequest::AccountRateLimitsRead,
                    Duration::from_secs(10),
                )
                .await
                .ok();
            let id = format!(
                "codex:{}",
                crate::accounts::catalog::resolved(&account.home).to_string_lossy()
            );
            let mut held = self.quotas.cache.lock().await;
            let cache =
                held.get_or_insert_with(|| super::cache::QuotaCache::load(&self.quota_path()));
            cache.remove(&id);
            if let Some(latest) = latest {
                let mut value = super::codex::parse(&latest);
                value["id"] = json!(id);
                value["provedor"] = json!("codex");
                value["ativa"] = json!(false);
                value["label"] = json!(if account.is_default {
                    "Codex"
                } else {
                    &account.id
                });
                cache.update(&id, value, super::now());
                let signature = std::fs::read(account.home.join("auth.json"))
                    .ok()
                    .map(|bytes| {
                        json!(ring::digest::digest(&ring::digest::SHA256, &bytes).as_ref())
                    })
                    .unwrap_or(Value::Null);
                cache.set_credential(&id, signature);
            }
            if cache.save(&self.quota_path()).is_err() {
                tracing::warn!(
                    code = "quota_cache_write_failed",
                    "cache de cotas não gravado após redefinição"
                );
            }
            Ok(json!({"outcome":outcome}))
        })
        .await
        .unwrap_or_else(|_| Err(reset_failed()));
        process.close().await;
        result
    }
}

fn save(path: &std::path::Path, attempts: &[Attempt]) -> Result<(), AccountError> {
    let mut body = String::new();
    for attempt in attempts {
        body.push_str(&serde_json::to_string(attempt).map_err(|_| AccountError::io())?);
        body.push('\n');
    }
    crate::runtime::queue::atomic_write(path, body.as_bytes()).map_err(|_| {
        failure(
            503,
            "codex_reset_failed",
            "não foi possível guardar a tentativa de redefinição",
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn only_exhausted_weekly_window_can_consume_a_credit() {
        let mut body = json!({"rateLimits":{"primary":{"windowDurationMins":300,"usedPercent":100},
            "secondary":{"windowDurationMins":10080,"usedPercent":99}},"rateLimitResetCredits":{"availableCount":1}});
        assert_eq!(
            check_first(&body).unwrap_err().code,
            "codex_reset_weekly_not_exhausted"
        );
        body["rateLimits"]["secondary"]["usedPercent"] = json!(100);
        assert!(check_first(&body).unwrap());
        body["rateLimitResetCredits"]["availableCount"] = json!(0);
        assert!(!check_first(&body).unwrap());
        body["rateLimits"]["secondary"] = Value::Null;
        assert_eq!(
            check_first(&body).unwrap_err().code,
            "codex_reset_weekly_unavailable"
        );
    }
    #[test]
    fn uuid_is_canonical_and_pending_attempt_survives_serialization() {
        let uuid = canonical_uuid("550E8400-E29B-41D4-A716-446655440000").unwrap();
        assert_eq!(uuid, "550e8400-e29b-41d4-a716-446655440000");
        assert!(canonical_uuid("invalid").is_none());
        let attempt = Attempt {
            account_id: "test".into(),
            credit_id: None,
            idempotency_key: uuid,
            accepted_at: 1000.0,
            outcome: None,
        };
        let restored: Attempt =
            serde_json::from_str(&serde_json::to_string(&attempt).unwrap()).unwrap();
        assert_eq!(restored.idempotency_key, attempt.idempotency_key);
        assert!(restored.outcome.is_none());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn lost_response_reuses_persisted_uuid_and_completed_retry_does_not_consume() {
        use crate::accounts::environment::AccountEnvironment;
        
        let temp = tempfile::tempdir().unwrap();
        let bin = temp.path().join("bin");
        std::fs::create_dir(&bin).unwrap();
        let cli = bin.join("codex");
        crate::write_test_executable(&cli, r#"#!/usr/bin/env node
const fs=require('fs'),path=require('path');
const home=process.env.CODEX_HOME, ledger=path.join(process.env.HOME,'.hangar','codex-reset-attempts.json');
const rows=path.join(home,'consumes.jsonl');
const rl=require('readline').createInterface({input:process.stdin});
rl.on('line',line=>{
 const m=JSON.parse(line); if(!m.id)return;
 let result={};
 if(m.method==='account/rateLimits/read')result={rateLimits:{secondary:{windowDurationMins:10080,usedPercent:100}},rateLimitResetCredits:{availableCount:1}};
 if(m.method==='account/rateLimitResetCredit/consume'){
  const persisted=fs.readFileSync(ledger,'utf8').trim().split('\n').map(JSON.parse);
  if(!persisted.some(row=>row.idempotency_key===m.params.idempotencyKey))process.exit(3);
  const old=fs.existsSync(rows)?fs.readFileSync(rows,'utf8').trim().split('\n'):[];
  fs.appendFileSync(rows,JSON.stringify(m.params)+'\n');
  if(!old.length)process.exit(0);
  result={outcome:'alreadyRedeemed'};
 }
 process.stdout.write(JSON.stringify({id:m.id,result})+'\n');
});
"#);
        let path = std::env::join_paths(
            std::iter::once(bin).chain(std::env::split_paths(&std::env::var_os("PATH").unwrap())),
        )
        .unwrap();
        let env = AccountEnvironment::from_map(
            [
                ("HOME".into(), temp.path().to_string_lossy().into_owned()),
                (
                    "USERPROFILE".into(),
                    temp.path().to_string_lossy().into_owned(),
                ),
                ("PATH".into(), path.to_string_lossy().into_owned()),
            ]
            .into(),
        );
        let service = AccountService::new(env);
        let account = service.create(Provider::Codex, "test", |_| Ok(())).unwrap();
        let uuid = "550e8400-e29b-41d4-a716-446655440000".to_owned();
        assert_eq!(
            service
                .consume_reset(account.clone(), None, uuid.clone())
                .await
                .unwrap_err()
                .code,
            "codex_reset_failed"
        );
        assert_eq!(
            service
                .consume_reset(account.clone(), None, uuid.clone())
                .await
                .unwrap()["outcome"],
            "alreadyRedeemed"
        );
        // Até outra instância usa o resultado definitivo, sem novo consumo.
        let restarted = AccountService::new(service.env.clone());
        assert_eq!(
            restarted
                .consume_reset(account.clone(), None, uuid.clone())
                .await
                .unwrap()["outcome"],
            "alreadyRedeemed"
        );
        let log = std::fs::read_to_string(account.home.join("consumes.jsonl")).unwrap();
        let rows: Vec<Value> = log
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        assert_eq!(rows.len(), 2);
        assert!(rows.iter().all(|row| row["idempotencyKey"] == uuid));
        let key = AccountKey::new(Provider::Codex, &account.home).unwrap();
        assert!(
            service
                .locks
                .try_acquire(&key, GuardMode::Exclusive)
                .is_ok()
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn unreadable_line_and_expired_attempts_do_not_block_a_new_reset() {
        use crate::accounts::environment::AccountEnvironment;
        
        let temp = tempfile::tempdir().unwrap();
        let bin = temp.path().join("bin");
        std::fs::create_dir(&bin).unwrap();
        let cli = bin.join("codex");
        crate::write_test_executable(&cli, r#"#!/usr/bin/env node
const rl=require('readline').createInterface({input:process.stdin});
rl.on('line',line=>{
 const m=JSON.parse(line); if(!m.id)return;
 let result={};
 if(m.method==='account/rateLimits/read')result={rateLimits:{secondary:{windowDurationMins:10080,usedPercent:100}},rateLimitResetCredits:{availableCount:1}};
 if(m.method==='account/rateLimitResetCredit/consume')result={outcome:'reset'};
 process.stdout.write(JSON.stringify({id:m.id,result})+'\n');
});
"#);
        let path = std::env::join_paths(
            std::iter::once(bin).chain(std::env::split_paths(&std::env::var_os("PATH").unwrap())),
        )
        .unwrap();
        let home = temp.path().to_string_lossy().into_owned();
        let service = AccountService::new(AccountEnvironment::from_map(
            [
                ("HOME".into(), home.clone()),
                ("USERPROFILE".into(), home),
                ("PATH".into(), path.to_string_lossy().into_owned()),
            ]
            .into(),
        ));
        let account = service.create(Provider::Codex, "test", |_| Ok(())).unwrap();
        let ledger = service
            .quota_path()
            .with_file_name("codex-reset-attempts.json");
        std::fs::create_dir_all(ledger.parent().unwrap()).unwrap();
        let old = serde_json::to_string(&Attempt {
            account_id: "test".into(),
            credit_id: None,
            idempotency_key: "11111111-1111-1111-1111-111111111111".into(),
            accepted_at: super::super::now() - 2.0 * 86400.0,
            outcome: Some("reset".into()),
        })
        .unwrap();
        std::fs::write(&ledger, format!("{{corrompida\n{old}\n")).unwrap();
        let uuid = "22222222-2222-2222-2222-222222222222".to_owned();
        let result = service
            .consume_reset(account, None, uuid.clone())
            .await
            .expect("uma linha ilegível não pode travar toda redefinição");
        assert_eq!(result["outcome"], "reset");
        let rows: Vec<Attempt> = std::fs::read_to_string(&ledger)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        // Só a tentativa vigente fica: o registro não cresce para sempre.
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].idempotency_key, uuid);
    }
}
