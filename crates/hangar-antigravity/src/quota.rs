//! A cota da conta pela statusline do `agy`: a cada mudança de estado o binário oficial passa o campo `quota` ao
//! comando do `statusLine`, e o Hangar guarda a última leitura num arquivo da conta. Nada roda o `agy` só para ler
//! a cota: o Google não suporta `agy -p` embutido em app de terceiro nos planos de consumidor.
use std::io;
use std::path::Path;

use serde_json::{Map, Value, json};

pub const FILE: &str = "hangar-quota.json";
/// A statusline roda a cada poucos segundos; com a cota igual, só regrava para o "visto há" andar.
const SEEN_REFRESH_S: f64 = 60.0;
/// O grupo que a sessão Gemini usa: vira as janelas gerais da conta.
const MAIN_GROUP: &str = "gemini";

/// Guarda a `quota` da statusline em `path`; `false` quando ela não veio ou nada mudou.
pub fn record(payload: &Value, path: &Path, now: f64) -> io::Result<bool> {
    let Some(quota) = payload["quota"].as_object().filter(|quota| !quota.is_empty()) else { return Ok(false) };
    // `reset_in_seconds` muda a cada chamada e não diz nada que `reset_time` não diga.
    let buckets: Map<String, Value> = quota.iter()
        .map(|(id, bucket)| (id.clone(), json!({"remaining_fraction": bucket["remaining_fraction"],
            "reset_time": bucket["reset_time"], "disabled": bucket["disabled"]})))
        .collect();
    let buckets = Value::Object(buckets);
    if let Some(saved) = read(path)
        && saved["buckets"] == buckets
        && saved["seen"].as_f64().is_some_and(|seen| seen <= now && now - seen < SEEN_REFRESH_S)
    {
        return Ok(false);
    }
    let mut bytes = serde_json::to_vec(&json!({"seen": now, "buckets": buckets})).map_err(io::Error::other)?;
    bytes.push(b'\n');
    hangar_core::fs::atomic_write(path, &bytes)?;
    Ok(true)
}

/// A última cota guardada; `None` sem arquivo ou com arquivo fora do formato.
pub fn read(path: &Path) -> Option<Value> {
    let value: Value = serde_json::from_slice(&std::fs::read(path).ok()?).ok()?;
    (value["seen"].as_f64().is_some_and(f64::is_finite) && value["buckets"].is_object()).then_some(value)
}

/// As janelas no formato da cota do Hangar: o grupo do Gemini como as gerais (`5h`, `7d`) e os outros como janelas
/// por modelo, com o nome do grupo. Janela cujo `reset_time` já passou renovou: volta zerada, sem horário.
pub fn windows(saved: &Value, now: f64) -> Vec<Value> {
    let mut rows: Vec<(bool, String, u8, Value)> = saved["buckets"].as_object().into_iter().flatten()
        .filter(|(_, bucket)| bucket["disabled"] != true)
        .filter_map(|(id, bucket)| {
            let (group, window) = id.rsplit_once('-')?;
            let remaining = bucket["remaining_fraction"].as_f64().filter(|r| r.is_finite())?;
            let window = if window == "weekly" { "7d" } else { window };
            // Janela ainda sem uso volta com a data zero do Unix.
            let reset_ts = bucket["reset_time"].as_str().and_then(unix_seconds).filter(|ts| *ts > 0.0);
            let renewed = reset_ts.is_some_and(|ts| ts <= now);
            let pct = if renewed { 0.0 } else { ((1.0 - remaining) * 100.0).clamp(0.0, 100.0) };
            let main = group == MAIN_GROUP;
            let label = if main { window.to_owned() } else { format!("{} {window}", group_label(group)) };
            let order = match window { "5h" => 0, "7d" => 1, _ => 2 };
            Some((!main, group.to_owned(), order,
                json!({"rotulo": label, "pct": pct, "reset_ts": reset_ts.filter(|_| !renewed), "por_modelo": !main})))
        })
        .collect();
    rows.sort_by(|a, b| (a.0, &a.1, a.2).cmp(&(b.0, &b.1, b.2)));
    rows.into_iter().map(|(_, _, _, window)| window).collect()
}

/// `3p` é o grupo "Claude and GPT models" do `/usage`; o rótulo cabe na barra.
fn group_label(group: &str) -> &str {
    if group == "3p" { "Claude/GPT" } else { group }
}

/// `AAAA-MM-DDTHH:MM:SSZ` em segundos Unix, sem depender de crate de datas.
fn unix_seconds(text: &str) -> Option<f64> {
    let text = text.strip_suffix('Z')?;
    let (date, time) = text.split_once('T')?;
    let mut d = date.split('-').map(|p| p.parse::<i64>().ok());
    let (y, m, day) = (d.next()??, d.next()??, d.next()??);
    let mut t = time.split(':');
    let (hh, mm) = (t.next()?.parse::<i64>().ok()?, t.next()?.parse::<i64>().ok()?);
    let ss: f64 = t.next()?.parse().ok()?;
    // Dias desde 1970 (algoritmo de Howard Hinnant).
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146097 + doe - 719468;
    Some((days * 86400 + hh * 3600 + mm * 60) as f64 + ss)
}

#[cfg(test)]
mod tests {
    use super::*;

    const PAYLOAD: &str = include_str!("../tests/fixtures/agy-statusline.json");
    /// 2026-11-28T17:46:00Z, antes de toda renovação da fixture.
    const NOW: f64 = 1_795_887_960.0;

    fn payload() -> Value { serde_json::from_str(PAYLOAD).unwrap() }

    #[test]
    fn the_statusline_quota_becomes_the_gemini_windows_and_the_claude_gpt_ones() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(FILE);
        assert!(record(&payload(), &path, NOW).unwrap());
        let saved = read(&path).unwrap();
        assert_eq!(saved["seen"], NOW);
        let windows = windows(&saved, NOW);
        let labels: Vec<&str> = windows.iter().map(|w| w["rotulo"].as_str().unwrap()).collect();
        assert_eq!(labels, ["5h", "7d", "Claude/GPT 5h", "Claude/GPT 7d"]);
        // gemini-weekly: remaining_fraction 0.5533508, renova em 2026-12-03T09:44:55Z.
        assert!((windows[1]["pct"].as_f64().unwrap() - 44.66492).abs() < 1e-3);
        assert_eq!(windows[1]["reset_ts"], 1_796_291_095.0);
        assert_eq!(windows[1]["por_modelo"], false);
        assert_eq!(windows[2]["por_modelo"], true);
        assert_eq!(windows[2]["pct"], 0.0);
        assert_eq!(windows[2]["reset_ts"], Value::Null, "janela sem uso não tem horário de renovação");
    }

    #[test]
    fn a_window_whose_reset_passed_comes_back_empty() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(FILE);
        record(&payload(), &path, NOW).unwrap();
        // Depois de 2026-11-28T22:19:46Z a janela de 5h do Gemini já renovou.
        let later = 1_795_905_000.0;
        let windows = windows(&read(&path).unwrap(), later);
        assert_eq!(windows[0]["rotulo"], "5h");
        assert_eq!(windows[0]["pct"], 0.0);
        assert_eq!(windows[0]["reset_ts"], Value::Null);
        assert!(windows[1]["pct"].as_f64().unwrap() > 40.0, "a semanal ainda não renovou");
    }

    #[test]
    fn the_same_quota_is_rewritten_only_after_a_minute() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(FILE);
        assert!(record(&payload(), &path, NOW).unwrap());
        assert!(!record(&payload(), &path, NOW + 30.0).unwrap(), "a mesma cota em menos de um minuto não regrava");
        assert!(record(&payload(), &path, NOW + 61.0).unwrap(), "depois de um minuto regrava para o visto há andar");
        let mut changed = payload();
        changed["quota"]["gemini-5h"]["remaining_fraction"] = json!(0.5);
        assert!(record(&changed, &path, NOW + 62.0).unwrap(), "cota nova grava na hora");
        assert_eq!(windows(&read(&path).unwrap(), NOW + 62.0)[0]["pct"], 50.0);
    }

    #[test]
    fn a_payload_without_quota_leaves_the_last_reading() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(FILE);
        record(&payload(), &path, NOW).unwrap();
        assert!(!record(&json!({"quota": {}}), &path, NOW + 120.0).unwrap());
        assert!(!record(&json!({"cwd": "/x"}), &path, NOW + 120.0).unwrap());
        assert_eq!(read(&path).unwrap()["seen"], NOW);
    }

    #[test]
    fn a_missing_or_broken_file_is_no_reading() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(FILE);
        assert!(read(&path).is_none());
        std::fs::write(&path, "{ quebrado").unwrap();
        assert!(read(&path).is_none());
        std::fs::write(&path, r#"{"seen":"ontem","buckets":{}}"#).unwrap();
        assert!(read(&path).is_none());
    }

    #[test]
    fn a_disabled_bucket_or_one_without_fraction_is_left_out() {
        let saved = json!({"seen": NOW, "buckets": {
            "gemini-5h": {"remaining_fraction": 0.9, "reset_time": "1970-01-01T00:00:00Z"},
            "gemini-weekly": {"remaining_fraction": 0.9, "disabled": true},
            "3p-5h": {"remaining_amount": 12}}});
        let labels: Vec<Value> = windows(&saved, NOW).into_iter().map(|w| w["rotulo"].clone()).collect();
        assert_eq!(labels, [json!("5h")]);
    }
}
