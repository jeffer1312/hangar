//! `GET /api/credenciais`: contas Claude, chaves do `engines.json`, contas Codex e as linhas que só
//! a cota conhece, numa lista só. Tudo em processo: nenhuma pergunta ao Python.
use super::{
    AccountKey, AccountService, Provider,
    catalog::Account,
    http::{Json, bridge},
    quotas::other,
};
use axum::{
    extract::Request,
    response::{IntoResponse, Response},
};
use serde_json::{Map, Value, json};
use std::{sync::Arc, time::Duration};

pub(super) const CODEX_AUTH_DEADLINE: Duration = Duration::from_secs(3);

fn unavailable_auth() -> Value {
    json!({"method":"unknown","status":"unavailable","email":null,"plan":null})
}

/// `sk-kimi••••4f2a`: nunca a chave inteira. Conta caracteres, não bytes.
fn mask(key: &str) -> String {
    let chars: Vec<char> = key.chars().collect();
    if chars.len() <= 10 {
        return "•".repeat(chars.len());
    }
    let head: String = chars[..7].iter().collect();
    let tail: String = chars[chars.len() - 4..].iter().collect();
    format!("{head}••••{tail}")
}

fn float(value: &Value) -> Value {
    value.as_f64().map_or(Value::Null, |n| json!(n))
}

/// A linha leva 7 campos da cota, com os números como o pydantic os validava (`13` vira `13.0`).
fn summary(row: &Value) -> Value {
    let windows: Vec<Value> = row["janelas"].as_array().into_iter().flatten().map(|w| json!({
        "rotulo": w["rotulo"], "pct": float(&w["pct"]), "reset_ts": float(&w["reset_ts"]),
        "por_modelo": w["por_modelo"].as_bool().unwrap_or(false),
    })).collect();
    let credits = match &row["reset_credits"] {
        Value::Object(credits) => json!({
            "available_count": credits.get("available_count").cloned().unwrap_or(Value::Null),
            "credits": credits.get("credits").and_then(Value::as_array).map(|items| items.iter().map(|c| json!({
                "id": c["id"], "expires_at": c["expires_at"], "title": c["title"],
                "description": c["description"],
                "status": c.get("status").filter(|s| !s.is_null()).cloned().unwrap_or(json!("unknown")),
            })).collect::<Vec<_>>()),
        }),
        _ => Value::Null,
    };
    json!({"estado": row["estado"], "janelas": windows, "ts": float(&row["ts"]),
        "idade_s": float(&row["idade_s"]), "motivo": row["motivo"], "label": row["label"],
        "reset_credits": credits})
}

fn line(id: &str, kind: &str, auth: &str, natural: &Value, aliases: &Map<String, Value>, quota: &Map<String, Value>) -> Value {
    let alias = aliases.get(id).cloned().unwrap_or(Value::Null);
    let name = if alias.is_null() { natural.clone() } else { alias.clone() };
    json!({"id": id, "tipo": kind, "auth_method": auth, "codex_account": null, "codex_sync": null,
        "nome": name, "nome_natural": natural, "apelido": alias, "ativa": false, "path": null,
        "login": null, "base_url": null, "chave_mascarada": null, "usos": [],
        "cota": quota.get(id).map_or(Value::Null, summary), "aceita_cookie": false,
        "cookie_definido": false, "gerenciada": true})
}

/// Nome natural do motor: o `label` texto, senão o nome; `label` de outro tipo tira o motor da lista.
fn engine_natural(name: &str, data: &Map<String, Value>) -> Option<Value> {
    match data.get("label") {
        Some(label @ Value::String(_)) if !other::falsy(label) => Some(label.clone()),
        Some(label) if !other::falsy(label) => None,
        _ => Some(json!(name)),
    }
}

/// O que a linha de `/api/credenciais` lê além do id e do nome.
pub(crate) enum Origin { Claude(Value, Account), Engine(Map<String, Value>), Codex(Account), Kimi }

pub(crate) struct Listed { pub id: String, pub natural: Value, pub origin: Origin }

/// As contas de `/api/credenciais`, na ordem dela, sem login nem cota. As etiquetas usam esta mesma
/// lista: o id é o elo entre as duas. Catálogo ilegível tira só a parte dele e volta com o código.
pub(crate) fn listed_ids(service: &AccountService, config: &other::Config) -> (Vec<Listed>, Vec<(Provider, &'static str)>) {
    let mut out = vec![];
    let mut problems = vec![];
    match service.claude_listed() {
        Ok(claude) => out.extend(claude.into_iter().map(|(row, account)| Listed {
            id: format!("claude:{}", row["path"].as_str().unwrap_or_default()),
            natural: row["label"].clone(),
            origin: Origin::Claude(row, account),
        })),
        Err(err) => problems.push((Provider::Claude, err.code)),
    }
    // Motor torto sai da lista; o código dele já foi ao diário pela cota (`config.problems`).
    for (name, data) in config.engines.iter().filter(|(name, _)| other::engine_name_ok(name)) {
        let Some(data) = data.as_object() else { continue };
        let Some(natural) = engine_natural(name, data) else { continue };
        if data.get("base_url").is_some_and(|base| !base.is_null() && !base.is_string()) {
            continue;
        }
        out.push(Listed { id: format!("chave:{name}"), natural, origin: Origin::Engine(data.clone()) });
    }
    match service.visible_codex_accounts() {
        Ok(codex) => out.extend(codex.into_iter().map(|account| Listed {
            id: format!("codex:{}", super::catalog::resolved(&account.home).to_string_lossy()),
            natural: json!(account.id),
            origin: Origin::Codex(account),
        })),
        Err(err) => problems.push((Provider::Codex, err.code)),
    }
    // Mesma fonte e mesmo nome das linhas Kimi da cota.
    for source in config.sources.iter().filter(|s| s.id.starts_with("kimi:")) {
        let suffix = source.id.split_once(':').map_or("", |(_, tail)| tail);
        let natural = if other::falsy(&source.label) { json!(suffix) } else { source.label.clone() };
        out.push(Listed { id: source.id.clone(), natural, origin: Origin::Kimi });
    }
    (out, problems)
}

/// O `config.toml` do Kimi tem o bloco que o app gravou para esse provedor?
fn managed_block(text: Option<&str>, name: &str) -> bool {
    let Some(text) = text else { return false };
    let start = format!("# >>> hangar: provedor {name} (app/agentes_sync) — gerado, não editar à mão");
    let end = format!("# <<< hangar: fim do provedor {name}");
    text.find(&start).is_some_and(|i| text[i..].contains(&end))
}

/// Login da linha Codex. Indisponível diz a causa: `cli-ausente`, `prazo` (3 s) ou `leitura-falhou`;
/// preparo em curso não é falha e fica sem motivo. Status fora do conhecido não vira "desconectado".
fn codex_login(auth: &Value, preparing: bool) -> Value {
    let (state, logged, reason) = match auth["status"].as_str() {
        Some("connected") => ("ok", json!(true), Value::Null),
        Some("disconnected" | "none") => ("ok", json!(false), Value::Null),
        Some("unavailable") => {
            let reason = match auth["reason"].as_str() {
                Some("cli_missing") => json!("cli-ausente"),
                Some("deadline") => json!("prazo"),
                Some("busy") => Value::Null,
                _ if preparing => Value::Null,
                _ => json!("leitura-falhou"),
            };
            ("indisponivel", Value::Null, reason)
        }
        _ => ("indisponivel", Value::Null, json!("formato-desconhecido")),
    };
    json!({"estado": state, "loggedIn": logged, "email": auth["email"], "plano": auth["plan"],
        "motivo": reason, "refreshExpiresAt": null})
}

async fn codex_snapshot(state: &crate::routes::AppState, account: &super::catalog::Account, force: bool, bridge: Option<&super::bridge::AccountsBridge>) -> (Value, Value) {
    let service = &state.accounts;
    // Sem a ponte o preparo é desconhecido: `null`, nunca um `idle` inventado.
    let sync = match bridge {
        Some(bridge) => service.preparation_status(account, bridge).await,
        None => json!({"status": null}),
    };
    if sync["status"] == "running" {
        return (unavailable_auth(), sync);
    }
    let read = async {
        if force {
            if let Ok(key) = AccountKey::new(Provider::Codex, &account.home) {
                service.codex_auth.invalidate(&key);
            }
            service.read_codex_auth(account).await
        } else {
            service.read_codex_auth_fast(account).await
        }
    };
    // A leitura segue em segundo plano e guarda o resultado; a lista não espera por ela.
    let auth = match tokio::time::timeout(service.codex_auth_deadline, read).await {
        Ok(auth) => auth,
        Err(_) => {
            tracing::warn!(code = "codex_auth_deadline", account = %account.id, "leitura de autenticação Codex excedeu o prazo");
            let mut auth = unavailable_auth();
            auth["reason"] = json!("deadline");
            auth
        }
    };
    (auth, sync)
}

pub async fn public(state: Arc<crate::routes::AppState>, request: Request) -> Response {
    let query: std::collections::HashMap<String, String> =
        form_urlencoded::parse(request.uri().query().unwrap_or("").as_bytes()).into_owned().collect();
    let force = match crate::query::bool_param(&query, "forcar") {
        Ok(force) => force,
        Err(response) => return *response,
    };
    // Parte com problema sai sozinha (linha indisponível, cota nula, motor fora) e vai ao diário;
    // a lista das outras credenciais nunca cai junto.
    let report = |code: &str, reason: &'static str| state.diag.report("rust.credentials_partial", "credenciais", code, reason);
    let service = state.accounts.clone();
    let bridge = bridge(&state).map_err(|err| report(err.code, "ponte de contas ausente; sem preparo Codex nesta lista")).ok();
    // Arquivos das fontes lidos uma vez, fora do runtime; a cota usa a mesma leitura e leva os
    // problemas deles ao diário.
    let env = service.env.clone();
    let config = match tokio::task::spawn_blocking(move || other::load(&env)).await {
        Ok(config) => Arc::new(config),
        Err(error) => {
            tracing::error!(code = "credentials_config_failed", panic = error.is_panic(), "leitura das configs das fontes falhou");
            report("credentials_config_failed", "leitura das configs das fontes falhou; a lista segue sem chaves");
            Arc::new(other::Config::default())
        }
    };
    let (listed, problems) = listed_ids(&service, &config);
    for (provider, code) in problems {
        report(code, match provider {
            Provider::Claude => "contas Claude ilegíveis; a lista segue sem elas",
            _ => "contas Codex ilegíveis; a lista segue sem elas",
        });
    }
    let claude: Vec<(Value, Account)> = listed.iter().filter_map(|l| match &l.origin {
        Origin::Claude(row, account) => Some((row.clone(), account.clone())),
        _ => None,
    }).collect();
    let codex = listed.iter().filter_map(|l| match &l.origin { Origin::Codex(account) => Some(account), _ => None });
    let snapshots = futures_util::future::join_all(codex.map(|account| codex_snapshot(&state, account, force, bridge.as_ref())));
    let quotas = async {
        let (result, late) = service.quotas_within(bridge.clone(), force, super::quotas::CLIENT_DEADLINE, Some(config.clone())).await;
        if late {
            report("quota_deadline", "cotas passaram do prazo; a lista sai com o cache e a leitura segue");
        }
        result.map_err(|err| report(err.code, "cotas indisponíveis; as linhas saem sem cota")).ok()
    };
    let claude = service.claude_states_of(claude);
    let (snapshots, quotas, claude) = tokio::join!(snapshots, quotas, claude);
    let aliases = &config.aliases;
    let quotas = quotas.unwrap_or_else(|| json!([]));
    // Id repetido na cota: vale o último valor, na posição do primeiro.
    let mut quota = Map::new();
    for row in quotas.as_array().into_iter().flatten() {
        if let Some(id) = row["id"].as_str() {
            quota.insert(id.to_owned(), row.clone());
        }
    }
    let (mut claude, mut snapshots) = (claude.into_iter(), snapshots.into_iter());
    let cookies = &config.opencode;
    let mut out = vec![];
    for Listed { id, natural, origin } in &listed {
        match origin {
            Origin::Claude(..) => {
                let Some(row) = claude.next() else { continue };
                let path = row["path"].as_str().unwrap_or_default();
                let mut item = line(id, "claude", "oauth", natural, aliases, &quota);
                item["ativa"] = json!(row["active"] == true);
                item["path"] = json!(path);
                let login = &row["login"];
                item["login"] = json!({"estado": login["estado"], "loggedIn": login["loggedIn"], "email": login["email"],
                    "plano": login["plano"], "motivo": login["motivo"], "refreshExpiresAt": float(&login["refreshExpiresAt"])});
                out.push(item);
            }
            Origin::Engine(data) => {
                let base = data.get("base_url").filter(|base| base.is_string()).cloned().unwrap_or(Value::Null);
                let accepts = base.as_str().is_some_and(|b| b.contains("opencode.ai"));
                let mut item = line(id, "chave", "api_key", natural, aliases, &quota);
                item["base_url"] = base;
                item["chave_mascarada"] = data.get("api_key").and_then(Value::as_str).filter(|k| !k.is_empty()).map_or(Value::Null, |k| json!(mask(k)));
                item["usos"] = json!(["claude_code"]);
                item["aceita_cookie"] = json!(accepts);
                item["cookie_definido"] = json!(accepts && cookies.contains_key(id));
                out.push(item);
            }
            Origin::Codex(account) => {
                let Some((auth, sync)) = snapshots.next() else { continue };
                let mut method = auth["method"].as_str().unwrap_or("unknown");
                if !matches!(method, "oauth" | "api_key" | "none" | "unknown") {
                    report("codex_auth_method_unknown", "método de autenticação Codex desconhecido; sai unknown");
                    method = "unknown";
                }
                let sync_running = sync["status"] == "running";
                let mut sync = if account.is_default { Value::Null } else { sync["status"].clone() };
                if !(sync.is_null() || matches!(sync.as_str(), Some("idle" | "running" | "ready" | "partial" | "error"))) {
                    report("codex_sync_unknown", "estado de preparo Codex desconhecido; sai null");
                    sync = Value::Null;
                }
                let mut item = line(id, "codex", method, natural, aliases, &quota);
                item["codex_account"] = json!(account.id);
                item["codex_sync"] = sync;
                item["ativa"] = json!(account.is_default);
                item["path"] = json!(account.home.to_string_lossy());
                item["usos"] = json!(["codex_cli"]);
                item["login"] = codex_login(&auth, sync_running);
                // Com o preparo em curso não há leitura (o snapshot nem pergunta): nada a esconder ali.
                match item["login"]["motivo"].as_str() {
                    Some("formato-desconhecido") => report("codex_auth_status_unknown", "status de autenticação Codex desconhecido; sai indisponível"),
                    Some("leitura-falhou") => report("codex_auth_read_failed", "leitura de autenticação Codex falhou; sai indisponível"),
                    Some("prazo") => report("codex_auth_deadline", "leitura de autenticação Codex passou do prazo; segue em segundo plano"),
                    _ => {}
                }
                out.push(item);
            }
            Origin::Kimi => {}
        }
    }
    // Cota sem cadastro continua visível, mas não comprova autenticação do Codex.
    for (id, row) in quota.iter().filter(|(id, _)| id.starts_with("codex:")) {
        if out.iter().any(|item| item["id"] == id.as_str()) {
            continue;
        }
        let suffix = id.split_once(':').map_or("", |(_, tail)| tail);
        let natural = if other::falsy(&row["label"]) { json!(suffix) } else { row["label"].clone() };
        let mut item = line(id, "codex", "unknown", &natural, aliases, &quota);
        item["usos"] = json!(["codex_cli"]);
        out.push(item);
    }
    // Kimi entra depois da cota lida, como sempre entrou.
    let kimi = config.kimi_text.as_deref();
    for Listed { id, .. } in listed.iter().filter(|l| matches!(l.origin, Origin::Kimi) && quota.contains_key(&l.id)) {
        let suffix = id.split_once(':').map_or("", |(_, tail)| tail);
        // O nome é o da linha da cota, como no Python: a fonte só sabe o rótulo cru.
        let row = &quota[id.as_str()];
        let natural = if other::falsy(&row["label"]) { json!(suffix) } else { row["label"].clone() };
        let mut item = line(id, "chave", "api_key", &natural, aliases, &quota);
        item["usos"] = json!(["kimi_cli"]);
        item["gerenciada"] = json!(managed_block(kimi, suffix));
        out.push(item);
    }
    Json(json!(out)).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mask_counts_characters_and_never_returns_the_key() {
        assert_eq!(mask("1234567890"), "••••••••••");
        assert_eq!(mask("12345678901"), "1234567••••8901");
        assert_eq!(mask("çççççççççççç"), "ççççççç••••çççç");
    }

    /// O id é o elo da etiqueta com a linha de Contas: todo id do evento `tags` sai em `/api/credenciais`.
    #[tokio::test]
    async fn every_tagged_account_is_a_credentials_row() {
        use crate::{accounts::{environment::AccountEnvironment, storage}, auth::TrustedHosts, config::Config, routes::AppState};
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().canonicalize().unwrap();
        let write = |path: std::path::PathBuf, text: &str| {
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, text).unwrap();
        };
        write(home.join(".claude/settings.json"), "{}");
        write(home.join(".claude-x").join(storage::CLAUDE_MARKER), "");
        write(home.join(".codex-y").join(storage::CODEX_MARKER), &json!({"version": 1, "id": "y"}).to_string());
        // `torto` tem `base_url` que não é texto: fora das duas listas.
        write(home.join(".claude/engines.json"), &json!({"motor": {"label": "Motor"}, "torto": {"base_url": 7}}).to_string());
        let mut state = AppState::new(Config { listen: "127.0.0.1:0".parse().unwrap(), upstream: "127.0.0.1:1".parse().unwrap(),
            internal_secret: "s".into(), auth_token: "owner".into(), log_path: None, trusted: TrustedHosts::parse("127.0.0.1") });
        state.accounts = AccountService::new(AccountEnvironment::from_map([
            ("HOME".into(), home.to_string_lossy().into_owned()),
            ("USERPROFILE".into(), home.to_string_lossy().into_owned())].into()));
        let tags = crate::tags::event(&state.accounts, "").unwrap();
        let response = public(Arc::new(state), Request::new(axum::body::Body::empty())).await;
        let body = axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let rows: Value = serde_json::from_slice(&body).unwrap();
        let ids: Vec<&str> = rows.as_array().unwrap().iter().filter_map(|row| row["id"].as_str()).collect();
        let tagged: Vec<&String> = tags["accounts"].as_object().unwrap().keys().collect();
        for (prefix, tail) in [("claude:", ".claude"), ("claude:", ".claude-x"), ("chave:", "motor"), ("codex:", ".codex-y")] {
            assert!(tagged.iter().any(|id| id.starts_with(prefix) && id.ends_with(tail)), "{prefix}…{tail} fora das etiquetas {tagged:?}");
        }
        assert!(!tagged.iter().any(|id| *id == "chave:torto"), "motor torto nas etiquetas");
        for id in tagged {
            assert!(ids.contains(&id.as_str()), "{id} nas etiquetas e fora de /api/credenciais {ids:?}");
        }
    }

    #[test]
    fn managed_block_needs_both_marks_in_order() {
        let start = "# >>> hangar: provedor x (app/agentes_sync) — gerado, não editar à mão";
        let end = "# <<< hangar: fim do provedor x";
        assert!(managed_block(Some(&format!("{start}\n{end}")), "x"));
        assert!(!managed_block(Some(&format!("{end}\n{start}")), "x"));
        assert!(!managed_block(None, "x"));
        assert!(managed_block(Some(&format!("{start}\n{end}yz")), "x"));
    }

    #[test]
    fn codex_login_names_the_cause_and_never_guesses_disconnected() {
        let reason = |auth: Value, preparing| codex_login(&auth, preparing)["motivo"].clone();
        let mut deadline = unavailable_auth();
        deadline["reason"] = json!("deadline");
        assert_eq!(reason(deadline, false), json!("prazo"));
        assert_eq!(reason(unavailable_auth(), false), json!("leitura-falhou"));
        assert_eq!(reason(unavailable_auth(), true), Value::Null);
        assert_eq!(reason(json!({"status":"unavailable","reason":"cli_missing"}), false), json!("cli-ausente"));
        // Conta ocupada (login em curso) ou `auth.json` mudando: não é falha, igual ao Python.
        assert_eq!(reason(json!({"status":"unavailable","reason":"busy"}), false), Value::Null);
        let odd = codex_login(&json!({"status":"expired"}), false);
        assert_eq!((&odd["estado"], &odd["loggedIn"]), (&json!("indisponivel"), &Value::Null));
        let off = codex_login(&json!({"status":"disconnected"}), false);
        assert_eq!((&off["estado"], &off["loggedIn"], &off["motivo"]), (&json!("ok"), &json!(false), &Value::Null));
    }
}
