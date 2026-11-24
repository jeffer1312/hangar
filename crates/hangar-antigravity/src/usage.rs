//! A cota da conta pelo próprio `agy` (`agy -p /usage --output-format json`): o comando local do CLI, sem turno
//! do modelo e sem token gasto. O Hangar nunca lê o login nem chama o Google; só lê o que o binário oficial imprime.
use std::path::Path;
use std::time::Duration;

use serde_json::{Value, json};

use crate::env::ProcessEnv;

pub const ARGS: [&str; 4] = ["-p", "/usage", "--output-format", "json"];

/// Um grupo de modelos que divide os limites (`Gemini Models`, `Claude and GPT models`).
#[derive(Clone, Debug, PartialEq)]
pub struct Group {
    pub name: String,
    pub models: Vec<String>,
    pub buckets: Vec<Bucket>,
}

/// Uma janela do grupo: `5h` ou `weekly`, com o quanto já foi usado (0 a 100) e quando volta.
#[derive(Clone, Debug, PartialEq)]
pub struct Bucket {
    pub window: String,
    pub used_pct: f64,
    pub reset_ts: Option<f64>,
}

/// Roda o `/usage` numa pasta neutra; `Ok(None)` quando passou do prazo.
pub fn read(program: &Path, cwd: &Path, env: &ProcessEnv, limit: Duration) -> std::io::Result<Option<Result<Vec<Group>, String>>> {
    Ok(crate::probe::run_args(program, cwd, env, &ARGS, limit)?.map(|(ok, text)| {
        if !ok { return Err(format!("agy saiu com erro: {}", text.chars().take(200).collect::<String>())) }
        parse(&text)
    }))
}

/// Os grupos da linha JSON da saída (o resto é log do CLI).
pub fn parse(text: &str) -> Result<Vec<Group>, String> {
    let value: Value = text.lines().filter(|line| line.trim_start().starts_with('{'))
        .find_map(|line| serde_json::from_str::<Value>(line).ok().filter(|v| v.get("command").is_some()))
        .ok_or("a saída do /usage não tem o JSON do comando")?;
    let groups = value.pointer("/command/data/groups").and_then(Value::as_array).ok_or("o /usage não trouxe grupos")?;
    Ok(groups.iter().filter_map(|group| {
        let name = group["name"].as_str()?.to_owned();
        let models = group["description"].as_str().and_then(|d| d.split_once(':')).map(|(_, list)| {
            list.split(',').map(|m| m.trim().trim_end_matches('.').to_owned()).filter(|m| !m.is_empty()).collect()
        }).unwrap_or_default();
        let buckets = group["buckets"].as_array().into_iter().flatten().filter_map(|bucket| {
            let remaining = bucket["remaining_fraction"].as_f64()?;
            // Janela ainda sem uso volta com a data zero do Unix.
            let reset_ts = bucket["reset_time"].as_str().and_then(|t| chrono_like(t)).filter(|ts| *ts > 0.0);
            Some(Bucket { window: bucket["window"].as_str()?.to_owned(), used_pct: ((1.0 - remaining) * 100.0).clamp(0.0, 100.0), reset_ts })
        }).collect();
        Some(Group { name, models, buckets })
    }).collect())
}

/// `AAAA-MM-DDTHH:MM:SSZ` em segundos Unix, sem depender de crate de datas.
fn chrono_like(text: &str) -> Option<f64> {
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

/// As janelas no formato da cota do Hangar: o grupo do Gemini (o padrão do `agy`) como as gerais e os outros
/// como janelas por modelo, com o nome do grupo; `5h` e `7d`, como no Claude.
pub fn windows(groups: &[Group]) -> Vec<Value> {
    let main = groups.iter().position(|g| g.name.to_lowercase().contains("gemini")).unwrap_or(0);
    let mut out = Vec::new();
    for (index, group) in groups.iter().enumerate() {
        for bucket in &group.buckets {
            let window = match bucket.window.as_str() { "weekly" => "7d", other => other };
            let (label, scoped) = if index == main { (window.to_owned(), false) } else { (format!("{} {window}", short(&group.name)), true) };
            out.push(json!({"rotulo": label, "pct": bucket.used_pct, "reset_ts": bucket.reset_ts, "por_modelo": scoped}));
        }
    }
    out
}

/// `Claude and GPT models` -> `Claude/GPT`: o rótulo cabe na barra.
fn short(name: &str) -> String {
    let name = name.trim_end_matches(" models").trim_end_matches(" Models");
    name.replace(" and ", "/")
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = include_str!("../tests/fixtures/agy-usage.json");

    #[test]
    fn the_usage_output_becomes_groups_with_used_percent_and_reset() {
        let groups = parse(&format!("log do cli\n{FIXTURE}")).unwrap();
        assert_eq!(groups.len(), 2);
        assert_eq!(groups[0].name, "Gemini Models");
        assert!(groups[0].models.contains(&"Gemini 4 Neon".to_owned()), "{:?}", groups[0].models);
        assert_eq!(groups[0].buckets[0].window, "weekly");
        assert!((groups[0].buckets[0].used_pct - 2.716).abs() < 0.01, "{}", groups[0].buckets[0].used_pct);
        assert_eq!(groups[0].buckets[0].reset_ts, Some(1_796_092_183.0), "2026-12-01T02:29:43Z");
        assert_eq!(groups[1].buckets[1].reset_ts, None, "janela sem uso volta com a data zero");
        assert_eq!(groups[1].buckets[1].used_pct, 0.0);
    }

    #[test]
    fn the_gemini_group_is_the_general_window_and_the_others_are_per_model() {
        let windows = windows(&parse(FIXTURE).unwrap());
        let labels: Vec<(&str, bool)> = windows.iter().map(|w| (w["rotulo"].as_str().unwrap(), w["por_modelo"].as_bool().unwrap())).collect();
        assert_eq!(labels, [("7d", false), ("5h", false), ("Claude/GPT 7d", true), ("Claude/GPT 5h", true)]);
    }

    #[test]
    fn output_without_the_command_json_is_an_error() {
        assert!(parse("Error: Please sign in").is_err());
        assert!(parse("{\"status\":\"SUCCESS\"}").is_err());
    }
}
