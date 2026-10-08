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
    let mut limits = json!({"rateLimits":{}});
    for (source, target) in [
        ("primary_window", "primary"),
        ("secondary_window", "secondary"),
    ] {
        let row = &usage["rate_limit"][source];
        limits["rateLimits"][target] = json!({"usedPercent":row["used_percent"],
            "resetsAt":row["reset_at"],"windowDurationMins":row["limit_window_seconds"].as_f64().map(|n|(n/60.0).floor())});
    }
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
