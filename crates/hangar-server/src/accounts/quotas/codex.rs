use super::{reading, timestamp};
use crate::accounts::{
    catalog::{Account, AccountService},
    native::NativeProcess,
};
use base64::Engine;
use serde_json::{Value, json};
use std::time::Duration;

fn label(minutes: Option<f64>) -> String {
    let Some(n) = minutes.filter(|n| n.is_finite() && *n > 0.0) else {
        return "janela".into();
    };
    if n >= 1440.0 && n % 1440.0 == 0.0 {
        format!("{}d", (n / 1440.0) as i64)
    } else if n >= 60.0 && n % 60.0 == 0.0 {
        format!("{}h", (n / 60.0) as i64)
    } else {
        format!("{}min", n as i64)
    }
}

pub fn parse(body: &Value) -> Value {
    let limits = &body["rateLimits"];
    let windows: Vec<Value> = ["primary", "secondary"]
        .into_iter()
        .filter_map(|key| {
            let value = &limits[key];
            let pct = value["usedPercent"].as_f64()?.clamp(0.0, 100.0);
            Some(
                json!({"rotulo":label(value["windowDurationMins"].as_f64()), "pct":pct,
            "reset_ts":value["resetsAt"].as_f64().filter(|ts| *ts != 0.0),"por_modelo":false}),
            )
        })
        .collect();
    let mut result = if windows.is_empty() {
        reading("indisponivel", windows, Some("formato-desconhecido"))
    } else {
        reading("lida", windows, None)
    };
    let credits = &body["rateLimitResetCredits"];
    if let Some(count) = credits["availableCount"].as_u64() {
        let rows = credits["credits"].as_array().map(|items| items.iter().filter_map(|item| {
            let id = item["id"].as_str()?;
            let status = item["status"].as_str().filter(|s| matches!(*s, "available"|"redeeming"|"redeemed"|"unknown")).unwrap_or("unknown");
            Some(json!({"id":id,"expires_at":item["expiresAt"].as_i64(),
                "title":item["title"].as_str(),"description":item["description"].as_str(),"status":status}))
        }).collect::<Vec<_>>());
        result["reset_credits"] = json!({"available_count":count,"credits":rows});
    }
    result
}

pub(super) fn http_limits(usage: &Value) -> Value {
    let mut limits = json!({"rateLimits":{}});
    for (source, target) in [("primary_window", "primary"), ("secondary_window", "secondary")] {
        let row = &usage["rate_limit"][source];
        limits["rateLimits"][target] = json!({"usedPercent":row["used_percent"],
            "resetsAt":row["reset_at"],"windowDurationMins":row["limit_window_seconds"].as_f64().map(|n|(n/60.0).floor())});
    }
    limits
}

async fn http(client: &reqwest::Client, account: &Account, now: f64) -> Option<Value> {
    let raw: Value =
        serde_json::from_slice(&std::fs::read(account.home.join("auth.json")).ok()?).ok()?;
    let tokens = &raw["tokens"];
    let token = tokens["access_token"].as_str().filter(|s| !s.is_empty())?;
    let account_id = tokens["account_id"].as_str().filter(|s| !s.is_empty())?;
    let payload = token.split('.').nth(1)?;
    let decoded = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(payload.trim_end_matches('='))
        .ok()?;
    let jwt: Value = serde_json::from_slice(&decoded).ok()?;
    if jwt["exp"].as_f64()? - 60.0 <= now {
        return None;
    }
    let get = |suffix: &str| {
        client
            .get(format!("https://chatgpt.com/backend-api{suffix}"))
            .bearer_auth(token)
            .header("chatgpt-account-id", account_id)
            .header("user-agent", "codex-cli")
    };
    let response = get("/wham/usage").send().await.ok()?;
    if response.status().as_u16() == 429 {
        return Some(reading("indisponivel", vec![], Some("http-429")));
    }
    if response.status().as_u16() != 200 {
        return None;
    }
    let usage: Value = serde_json::from_slice(&response.bytes().await.ok()?).ok()?;
    let mut limits = http_limits(&usage);
    if parse(&limits)["estado"] != "lida" {
        return None;
    }
    if let Some(count) = usage["rate_limit_reset_credits"]["available_count"].as_u64() {
        limits["rateLimitResetCredits"] = json!({"availableCount":count});
        if count > 0 {
            let response = get("/wham/rate-limit-reset-credits").send().await.ok()?;
            if response.status().as_u16() == 429 {
                return Some(reading("indisponivel", vec![], Some("http-429")));
            }
            if response.status().as_u16() != 200 {
                return None;
            }
            let data: Value = serde_json::from_slice(&response.bytes().await.ok()?).ok()?;
            let credits = data["credits"]
                .as_array()?
                .iter()
                .filter(|v| v.is_object())
                .map(|v| {
                    let mut v = v.clone();
                    v["expiresAt"] = timestamp(&v["expires_at"])
                        .map(|n| json!(n as i64))
                        .unwrap_or(Value::Null);
                    v
                })
                .collect::<Vec<_>>();
            limits["rateLimitResetCredits"] = json!({"availableCount":data["available_count"].as_u64().unwrap_or(count),"credits":credits});
        }
    }
    Some(parse(&limits))
}

pub(super) async fn read_via_proxy(env: &crate::accounts::environment::AccountEnvironment, binding: &str) -> Value {
    match proxy_usage(env, binding).await {
        Ok(usage) => parse(&http_limits(&usage)),
        Err(reason) => reading("indisponivel", vec![], Some(&reason)),
    }
}

async fn proxy_usage(env: &crate::accounts::environment::AccountEnvironment, binding: &str) -> Result<Value, String> {
    use crate::engines::proxy;
    use hangar_cliproxy_setup::login;
    let local = proxy::local(env).map_err(|_| "cliproxy-config-invalido")?.ok_or("cliproxy-ausente")?;
    let base = reqwest::Url::parse(&local.base_url).map_err(|_| "cliproxy-config-invalido")?;
    if !matches!(base.host_str(), Some("127.0.0.1" | "[::1]" | "localhost")) || !base.username().is_empty() || base.password().is_some() {
        return Err("cliproxy-gerenciamento-endereco".into());
    }
    let key = proxy::management_key(env).ok_or("cliproxy-sem-gerenciamento")?;
    if login::rejected(&key) { return Err("cliproxy-gerenciamento-recusado".into()); }
    let (name, account_id) = proxy::quota_identity(env, binding).map_err(|_| "cliproxy-credencial-indisponivel")?;
    let port = base.port_or_known_default().ok_or("cliproxy-config-invalido")?;
    let loopback = [std::net::SocketAddr::from(([127, 0, 0, 1], port)), std::net::SocketAddr::from((std::net::Ipv6Addr::LOCALHOST, port))];
    let client = reqwest::Client::builder().no_proxy().redirect(reqwest::redirect::Policy::none())
        .resolve_to_addrs("localhost", &loopback).timeout(Duration::from_secs(5)).build().map_err(|_| "sem-resposta")?;
    let mut credentials = base;
    credentials.set_path("/v8/management/credentials");
    credentials.query_pairs_mut().append_pair("name", &name);
    let response = client.get(credentials).bearer_auth(&key).send().await.map_err(|_| "sem-resposta")?;
    let url = format!("{}/v8/management", local.base_url);
    let files = management_response(response, &key).await?;
    let index = proxy_auth_index(&files, &name, &account_id)?;
    let response = client.post(format!("{url}/requests/api-call")).bearer_auth(&key).json(&json!({
        "auth_index":index,"method":"GET","url":"https://chatgpt.com/backend-api/wham/usage",
        "header":{"Authorization":"Bearer $TOKEN$","ChatGPT-Account-Id":account_id,"User-Agent":"codex-cli"}
    })).send().await.map_err(|_| "sem-resposta")?;
    proxy_usage_body(&management_response(response, &key).await?)
}

fn proxy_auth_index<'a>(files: &'a Value, name: &str, account_id: &str) -> Result<&'a str, String> {
    let mut matches = files["files"].as_array().into_iter().flatten().filter(|row|
        row["name"] == name && row["type"] == "codex" && row["disabled"] == false
            && row["id_token"]["chatgpt_account_id"] == account_id);
    let index = matches.next().and_then(|r| r["auth_index"].as_str()).filter(|i| !i.is_empty()).ok_or("cliproxy-credencial-indisponivel")?;
    if matches.next().is_some() { return Err("cliproxy-credencial-ambigua".into()); }
    Ok(index)
}

fn proxy_usage_body(payload: &Value) -> Result<Value, String> {
    match payload["status_code"].as_u64() {
        Some(200) => serde_json::from_str(payload["body"].as_str().ok_or("formato-desconhecido")?).map_err(|_| "formato-desconhecido".into()),
        Some(status) => Err(format!("http-{status}")),
        None => Err("formato-desconhecido".into()),
    }
}

async fn management_response(response: reqwest::Response, key: &str) -> Result<Value, String> {
    match response.status().as_u16() {
        200 => { hangar_cliproxy_setup::login::note_accepted(key); response.json().await.map_err(|_| "formato-desconhecido".into()) }
        401 => { hangar_cliproxy_setup::login::note_rejected(key); Err("cliproxy-gerenciamento-recusado".into()) }
        429 => Err("cliproxy-gerenciamento-http-429".into()),
        status => Err(format!("cliproxy-gerenciamento-http-{status}")),
    }
}

pub async fn read(
    service: &AccountService,
    client: &reqwest::Client,
    account: &Account,
    now: f64,
) -> Value {
    if let Some(value) = http(client, account, now).await {
        return value;
    }
    let mut process = match NativeProcess::open(service, account).await {
        Ok(process) => process,
        Err(code) => {
            return reading(
                "indisponivel",
                vec![],
                Some(if code == "codex_account_cli_missing" {
                    "codex-ausente"
                } else {
                    "sem-resposta"
                }),
            );
        }
    };
    let result = tokio::time::timeout(Duration::from_secs(8), async {
        process.initialize().await?;
        let auth = process.read().await?;
        if auth["status"] == "disconnected" {
            return Ok(reading("sem_credencial", vec![], None));
        }
        let raw: Value = process
            .client
            .request(
                hangar_codex::proto::ClientRequest::AccountRateLimitsRead,
                Duration::from_secs(8),
            )
            .await?;
        Ok::<_, hangar_codex::client::ClientError>(parse(&raw))
    })
    .await;
    process.close().await;
    match result {
        Ok(Ok(value)) => value,
        _ => reading("indisponivel", vec![], Some("sem-resposta")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn quota_requires_the_expected_account_and_reads_the_upstream_status() {
        let row = |name: &str, id: &str, index: &str| json!({"name":name,"type":"codex","disabled":false,"auth_index":index,
            "id_token":{"chatgpt_account_id":id}});
        let files = json!({"files":[row("pro.json", "acct-plus", "wrong"), row("pro.json", "acct-pro", "pro")]});
        assert_eq!(proxy_auth_index(&files, "pro.json", "acct-pro").unwrap(), "pro");
        assert!(proxy_auth_index(&files, "plus.json", "acct-plus").is_err());
        let ambiguous = json!({"files":[row("pro.json", "acct-pro", "a"), row("pro.json", "acct-pro", "b")]});
        assert!(proxy_auth_index(&ambiguous, "pro.json", "acct-pro").is_err());
        assert_eq!(proxy_usage_body(&json!({"status_code":429,"body":"{}"})).unwrap_err(), "http-429");
        let upstream = json!({"rate_limit":{"secondary_window":{"used_percent":74,"reset_at":2026,"limit_window_seconds":604800}}});
        let usage = proxy_usage_body(&json!({"status_code":200,"body":upstream.to_string()})).unwrap();
        assert_eq!(parse(&http_limits(&usage))["janelas"], json!([{"rotulo":"7d","pct":74.0,"reset_ts":2026.0,"por_modelo":false}]));
    }

    #[test]
    fn rate_limits_and_reset_credits_match_public_shape() {
        let result = parse(
            &json!({"rateLimits":{"primary":{"usedPercent":120,"windowDurationMins":300,"resetsAt":42},
            "secondary":{"usedPercent":30,"windowDurationMins":10080}},"rateLimitResetCredits":{"availableCount":1,
            "credits":[{"id":"credit","status":"available","expiresAt":1000}, false]}}),
        );
        assert_eq!(result["estado"], "lida");
        assert_eq!(
            result["janelas"][0],
            json!({"rotulo":"5h","pct":100.0,"reset_ts":42.0,"por_modelo":false})
        );
        assert_eq!(result["janelas"][1]["rotulo"], "7d");
        assert_eq!(
            result["reset_credits"]["credits"].as_array().unwrap().len(),
            1
        );
        assert_eq!(result["reset_credits"]["available_count"], 1);
    }
}
