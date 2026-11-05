//! Linhas de papéis preservam as opções que o painel ainda não edita.
use super::markdown as md;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Role {
    pub papel: String,
    pub sessao: String,
    pub provider: String,
    pub conta: String,
    pub modelo: String,
    pub esforco: String,
    pub vez: String,
    pub headless: Option<bool>,
    pub permissao: String,
    pub motor: String,
    pub jev: bool,
    pub subagente: String,
    pub perfil: String,
    pub abertura_extra: String,
    pub janela: String,
}
pub fn cli_rows(text: &str) -> Vec<serde_json::Value> {
    let lines: Vec<_> = super::plan::section(text, "Quem é quem")
        .into_iter()
        .filter(|line| line.trim().starts_with('|'))
        .collect();
    if lines.len() < 2 {
        return Vec::new();
    }
    let cells = |line: &str| {
        line.trim()
            .trim_matches('|')
            .split('|')
            .map(|c| c.trim().to_owned())
            .collect::<Vec<_>>()
    };
    let head: Vec<_> = cells(&lines[0])
        .iter()
        .map(|c| super::markdown::normalize(c))
        .collect();
    lines
        .iter()
        .skip(2)
        .filter_map(|line| {
            let mut row = serde_json::Map::new();
            for key in [
                "papel", "vez", "sessao", "provider", "conta", "modelo", "esforco", "janela",
                "abertura",
            ] {
                row.insert(key.into(), serde_json::json!(""));
            }
            for (key, value) in head.iter().zip(cells(line)) {
                row.insert(
                    key.clone(),
                    serde_json::json!(super::markdown::clean(&value)),
                );
            }
            (!super::text(&row["papel"]).is_empty()).then_some(serde_json::Value::Object(row))
        })
        .collect()
}
pub fn headers(text: &str) -> Option<Vec<String>> {
    let mut choices = Vec::new();
    for rotation in [true, false] {
        for window in [true, false] {
            for opening in [true, false] {
                let mut h = vec!["papel"];
                if rotation {
                    h.push("vez")
                }
                h.extend(["sessão", "provider", "conta", "modelo", "esforço"]);
                if window {
                    h.push("janela")
                }
                if opening {
                    h.push("abertura")
                }
                choices.push(h.into_iter().map(str::to_owned).collect::<Vec<_>>())
            }
        }
    }
    choices.sort_by_key(|h| std::cmp::Reverse(h.len()));
    choices.into_iter().find(|h| !md::table(text, h).is_empty())
}
pub fn default_headers() -> Vec<String> {
    vec!["papel", "sessão", "provider", "conta", "modelo", "esforço"]
        .into_iter()
        .map(str::to_owned)
        .collect()
}
pub fn quote(s: &str) -> String {
    if !s.is_empty()
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || "_@%+=:,./-".contains(c))
    {
        s.into()
    } else {
        format!("'{}'", s.replace('\'', "'\"'\"'"))
    }
}
pub fn split(s: &str) -> Result<Vec<String>, String> {
    let mut args = Vec::new();
    let mut word = String::new();
    let mut quote = None;
    let mut escaped = false;
    let mut active = false;
    for c in s.chars() {
        if escaped {
            if quote == Some('"') && !matches!(c, '"' | '\\' | '$' | '`' | '\n') {
                word.push('\\')
            }
            word.push(c);
            escaped = false;
            active = true;
            continue;
        }
        if c == '\\' && quote != Some('\'') {
            escaped = true;
            active = true;
            continue;
        }
        if let Some(q) = quote {
            if c == q {
                quote = None
            } else {
                word.push(c)
            }
            active = true
        } else if matches!(c, '\'' | '"') {
            quote = Some(c);
            active = true
        } else if c.is_whitespace() {
            if active {
                args.push(std::mem::take(&mut word));
                active = false
            }
        } else {
            word.push(c);
            active = true
        }
    }
    if quote.is_some() || escaped {
        return Err("aspas sem par".into());
    }
    if active {
        args.push(word)
    }
    Ok(args)
}
pub fn opening(role: &Role) -> String {
    let mut flags = Vec::new();
    if let Some(headless) = role.headless {
        flags.push(if headless { "--headless" } else { "--terminal" }.to_owned())
    }
    for (flag, value) in [
        ("--permissao", &role.permissao),
        ("--engine", &role.motor),
        ("--subagente", &role.subagente),
        ("--profile", &role.perfil),
    ] {
        if !value.is_empty() {
            flags.push(flag.into());
            flags.push(value.clone())
        }
    }
    if role.jev {
        flags.push("--jev".into())
    }
    let base = flags.iter().map(|s| quote(s)).collect::<Vec<_>>().join(" ");
    format!("{base} {}", role.abertura_extra).trim().to_owned()
}
fn parse_opening(s: &str, role: &mut Role) {
    let args = split(s).unwrap_or_else(|_| s.split_whitespace().map(str::to_owned).collect());
    let mut extra = Vec::new();
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--headless" => role.headless = Some(true),
            "--terminal" => role.headless = Some(false),
            "--jev" => role.jev = true,
            "--permissao" | "--engine" | "--subagente" | "--profile" if i + 1 < args.len() => {
                let target = match args[i].as_str() {
                    "--permissao" => &mut role.permissao,
                    "--engine" => &mut role.motor,
                    "--subagente" => &mut role.subagente,
                    _ => &mut role.perfil,
                };
                *target = args[i + 1].clone();
                i += 1
            }
            _ => extra.push(args[i].clone()),
        }
        i += 1
    }
    role.abertura_extra = extra.iter().map(|s| quote(s)).collect::<Vec<_>>().join(" ")
}
pub fn read(text: &str) -> Vec<Role> {
    let Some(headers) = headers(text) else {
        return Vec::new();
    };
    md::table(text, &headers)
        .into_iter()
        .filter(|r| !super::text(&r["papel"]).is_empty())
        .map(|row| {
            let get = |k: &str| super::text(&row[k]).to_owned();
            let mut role = Role {
                papel: get("papel"),
                sessao: get("sessão"),
                provider: get("provider").to_lowercase(),
                conta: get("conta"),
                modelo: get("modelo"),
                esforco: get("esforço"),
                vez: get("vez"),
                janela: get("janela").trim_end_matches('%').trim().to_owned(),
                ..Default::default()
            };
            parse_opening(&get("abertura"), &mut role);
            role
        })
        .collect()
}
pub fn write(text: &str, role: &Role) -> Result<String, String> {
    let open = opening(role);
    for field in [
        &role.papel,
        &role.sessao,
        &role.provider,
        &role.conta,
        &role.modelo,
        &role.esforco,
        &role.vez,
        &open,
    ] {
        md::validate(field)?;
    }
    if !role.janela.is_empty()
        && role
            .janela
            .parse::<u16>()
            .ok()
            .is_none_or(|n| !(10..=95).contains(&n))
    {
        return Err("janela: porcentagem inteira entre 10 e 95".into());
    }
    let mut h = headers(text).unwrap_or_else(default_headers);
    let mut text = text.to_owned();
    for (name, needed, at) in [
        ("vez", !role.vez.is_empty(), 1usize),
        ("janela", !role.janela.is_empty(), usize::MAX),
        ("abertura", !open.is_empty(), usize::MAX),
    ] {
        if needed && !h.iter().any(|s| s == name) {
            let pos = if name == "janela" {
                h.iter().position(|s| s == "abertura").unwrap_or(h.len())
            } else {
                at.min(h.len())
            };
            text = md::insert_column(&text, &h, name, pos, "-");
            h.insert(pos, name.into());
        }
    }
    let values = json!({"papel":role.papel,"sessão":role.sessao,"provider":role.provider,"conta":role.conta,"modelo":role.modelo,"esforço":role.esforco,"vez":role.vez,"janela":if role.janela.is_empty(){String::new()}else{format!("{}%",role.janela)},"abertura":open});
    let mut key = vec![role.papel.clone()];
    if h.iter().any(|s| s == "vez") {
        key.push(if role.vez.is_empty() {
            "-".into()
        } else {
            role.vez.clone()
        })
    }
    md::replace_row(&text, &h, &key, &values, "Quem é quem")
}
pub fn live(role: &Role, rows: &[Value]) -> Option<String> {
    let name = role.sessao.trim();
    if name.is_empty() {
        return None;
    }
    if let Some(prefix) = name.strip_suffix('*') {
        rows.iter()
            .filter(|r| super::text(&r["name"]).starts_with(prefix))
            .reduce(|a, b| {
                if b["last_activity"].as_f64().unwrap_or(0.)
                    > a["last_activity"].as_f64().unwrap_or(0.)
                {
                    b
                } else {
                    a
                }
            })
            .map(|r| super::text(&r["name"]).to_owned())
    } else {
        rows.iter()
            .find(|r| r["name"] == name)
            .map(|r| super::text(&r["name"]).to_owned())
    }
}
pub fn remove(text: &str, name: &str, rotation: &str) -> Result<String, String> {
    let h = headers(text).unwrap_or_else(default_headers);
    let mut key = vec![name.to_owned()];
    if h.iter().any(|s| s == "vez") {
        key.push(if rotation.is_empty() {
            "-".into()
        } else {
            rotation.into()
        })
    }
    if !read(text).iter().any(|r| {
        md::normalize(&r.papel) == md::normalize(name)
            && md::normalize(&r.vez) == md::normalize(rotation)
    }) {
        return Err("erro_orq_papel_inexistente".into());
    }
    Ok(md::remove_row(text, &h, &key))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn live_glob_keeps_the_first_candidate_when_activity_ties() {
        let role = Role { sessao: "worker-*".into(), ..Role::default() };
        let cases = [
            (json!(1.0), json!(1.0)),
            (Value::Null, Value::Null),
            (json!(-0.0), json!(0.0)),
        ];
        for (first, second) in cases {
            let rows = [
                json!({"name": "worker-a", "last_activity": first}),
                json!({"name": "worker-b", "last_activity": second}),
            ];
            assert_eq!(live(&role, &rows).as_deref(), Some("worker-a"), "{rows:?}");
        }
    }
}
