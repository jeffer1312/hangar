use crate::accounts::{catalog::valid_name, environment::AccountEnvironment};
use serde_json::{Map, Value};
use std::{
    fs,
    net::IpAddr,
    path::PathBuf,
    sync::Mutex,
};

static WRITE: Mutex<()> = Mutex::new(());
pub const FIELDS: &[(&str, char)] = &[
    ("label", 's'),
    ("base_url", 's'),
    ("api_key", 's'),
    ("model", 's'),
    ("subagent_model", 's'),
    ("context_window", 'i'),
    ("vision", 'b'),
    ("auth_via_api_key", 'b'),
    ("tool_search", 'b'),
    ("bundled_skills", 'b'),
    ("experimental_betas", 'b'),
    ("prompt_caching", 'b'),
    ("adaptive_thinking", 'b'),
    ("gateway_model_discovery", 'b'),
    ("fine_grained_tool_streaming", 'b'),
    ("auto_compact_window", 'i'),
    ("max_output_tokens", 'i'),
];

pub fn path(env: &AccountEnvironment) -> PathBuf {
    env.base
        .get("CP_ENGINES_FILE")
        .filter(|s| !s.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| env.home.join(".claude/engines.json"))
}

pub fn state(env: &AccountEnvironment) -> (Map<String, Value>, bool) {
    match fs::read(path(env)) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => (Map::new(), false),
        Ok(bytes) => match serde_json::from_slice::<Value>(&bytes) {
            Ok(Value::Object(mut map)) => {
                map.retain(|name, _| valid_name(name));
                (map, false)
            }
            _ => (Map::new(), true),
        },
        Err(_) => (Map::new(), true),
    }
}

pub fn local_host(host: &str) -> bool {
    match host.trim_matches(['[', ']']).parse::<IpAddr>() {
        Ok(IpAddr::V4(ip)) => {
            let [a, b, c, d] = ip.octets();
            if a == 192 && b == 0 && c == 0 && matches!(d, 9 | 10) {
                return false;
            }
            // A definição Python inclui endereços especiais sem alcance global.
            ip.is_private()
                || ip.is_loopback()
                || ip.is_link_local()
                || a == 0
                || a >= 240
                || (a == 192 && b == 0 && (c == 0 || c == 2))
                || (a == 198 && (b == 18 || b == 19 || (b == 51 && c == 100)))
                || (a == 203 && b == 0 && c == 113)
        }
        Ok(IpAddr::V6(ip)) => ip
            .to_ipv4_mapped()
            .map(|ip| local_host(&ip.to_string()))
            .unwrap_or_else(|| {
                let n = u128::from(ip);
                let inside = |network: &str, prefix: u32| {
                    let base = u128::from(network.parse::<std::net::Ipv6Addr>().unwrap());
                    n >> (128 - prefix) == base >> (128 - prefix)
                };
                if [
                    ("2001:1::1", 128),
                    ("2001:1::2", 128),
                    ("2001:3::", 32),
                    ("2001:4:112::", 48),
                    ("2001:20::", 28),
                    ("2001:30::", 28),
                ]
                .iter()
                .any(|(net, p)| inside(net, *p))
                {
                    return false;
                }
                ip.is_loopback()
                    || ip.is_unspecified()
                    || ip.is_unique_local()
                    || ip.is_unicast_link_local()
                    || [
                        ("64:ff9b:1::", 48),
                        ("100::", 64),
                        ("2001::", 23),
                        ("2001:db8::", 32),
                        ("2002::", 16),
                        ("3fff::", 20),
                    ]
                    .iter()
                    .any(|(net, p)| inside(net, *p))
            }),
        Err(_) => matches!(host, "localhost" | "localhost.localdomain"),
    }
}

pub fn base_url(value: &str) -> Result<String, String> {
    let parsed =
        reqwest::Url::parse(value).map_err(|_| "base_url: use uma URL http(s) completa")?;
    if !matches!(parsed.scheme(), "http" | "https") || parsed.host_str().is_none() {
        return Err("base_url: use uma URL http(s) completa".into());
    }
    if parsed.scheme() == "http" && !local_host(parsed.host_str().unwrap()) {
        return Err("base_url: use https (http só para loopback ou rede privada)".into());
    }
    Ok(normalize_base(value))
}

pub fn normalize_base(value: &str) -> String {
    let value = value.trim_end_matches('/');
    value
        .strip_suffix("/v1")
        .unwrap_or(value)
        .trim_end_matches('/')
        .into()
}

pub fn positive(field: &str, value: &Value) -> Result<Value, String> {
    let invalid = || format!("{field}: esperado número");
    let raw = match value {
        Value::String(s) => super::decimal::integer(s).ok_or_else(invalid)?,
        Value::Number(n) => n.to_string(),
        _ => return Err(invalid()),
    };
    let integer = if let Ok(n) = raw.parse::<num_bigint::BigInt>() {
        n.to_string()
    } else if value.is_number() {
        let n = raw.parse::<f64>().map_err(|_| invalid())?;
        if !n.is_finite() {
            return Err(invalid());
        }
        format!("{:.0}", n.trunc())
    } else {
        return Err(invalid());
    };
    if integer.starts_with('-') || integer == "0" {
        return Err(format!("{field}: deve ser maior que zero"));
    }
    serde_json::from_str(&integer).map_err(|_| invalid())
}

pub fn normalize(name: &str, data: &Map<String, Value>) -> Result<Map<String, Value>, String> {
    if !valid_name(name) {
        return Err("nome: use minúsculas, números, '-' ou '_' (até 32 caracteres)".into());
    }
    let mut out = Map::new();
    for &(field, kind) in FIELDS {
        let Some(value) = data.get(field).filter(|v| !v.is_null()) else {
            continue;
        };
        let normalized = match kind {
            'b' => Value::Bool(
                value
                    .as_bool()
                    .ok_or_else(|| format!("{field}: esperado true/false"))?,
            ),
            'i' => {
                if value.as_str().is_some_and(|s| s.trim().is_empty()) {
                    continue;
                }
                positive(field, value)?
            }
            _ => {
                let text = value
                    .as_str()
                    .ok_or_else(|| format!("{field}: esperado texto"))?
                    .trim();
                if text.contains(['\n', '\r', '\0']) {
                    return Err(format!("{field}: sem quebra de linha nem caractere nulo"));
                }
                if text.is_empty() {
                    continue;
                }
                Value::String(text.into())
            }
        };
        out.insert(field.into(), normalized);
    }
    for field in ["base_url", "api_key", "model"] {
        if !out.contains_key(field) {
            return Err(format!("{field}: obrigatório"));
        }
    }
    out.insert(
        "base_url".into(),
        Value::String(base_url(out["base_url"].as_str().unwrap())?),
    );
    out.entry("label")
        .or_insert_with(|| Value::String(name.into()));
    Ok(out)
}

/// Conta pontos de código como o Python: o surrogate guardado pelo codec vale um só.
pub fn mask(value: &str) -> String {
    use crate::orchestration::codec;
    let length = codec::string_length(value);
    if length <= 8 {
        "•".repeat(length)
    } else {
        let head = codec::truncate_string(value, 4);
        let tail = &value[codec::truncate_string(value, length - 4).len()..];
        format!("{head}{}{tail}", "•".repeat(8))
    }
}

pub fn visible(env: &AccountEnvironment) -> Map<String, Value> {
    state(env)
        .0
        .into_iter()
        .filter_map(|(name, item)| {
            let mut item = item.as_object()?.clone();
            let key = item.get("api_key").and_then(Value::as_str).unwrap_or("");
            let (masked, defined) = (mask(key), !key.is_empty());
            item.insert("api_key".into(), Value::String(masked));
            item.insert("api_key_definida".into(), Value::Bool(defined));
            Some((name, Value::Object(item)))
        })
        .collect()
}

fn change(
    env: &AccountEnvironment,
    apply: impl FnOnce(&mut Map<String, Value>) -> Result<bool, String>,
) -> Result<bool, String> {
    let _guard = WRITE.lock().unwrap_or_else(|e| e.into_inner());
    let target = path(env);
    target.parent().ok_or("arquivo sem diretório")?;
    let _lock = crate::session_rename::files::lock_file(&target.with_extension("lock"))
        .map_err(|e| e.to_string())?;
    let (mut map, corrupt) = state(env);
    if corrupt {
        return Err(format!(
            "engines.json ({}) existe mas está corrompido (JSON inválido); corrija-o à mão ou mova-o antes de salvar/remover — gravar por cima agora apagaria os outros motores e as keys deles.",
            target.display()
        ));
    }
    let changed = apply(&mut map)?;
    if changed {
        crate::runtime::queue::atomic_write(
            &target,
            crate::transcript::pyjson::dumps_indent(&Value::Object(map), 2, false).as_bytes(),
        )
        .map_err(|e| e.to_string())?;
    }
    Ok(changed)
}

pub fn put(
    env: &AccountEnvironment,
    name: &str,
    mut body: Map<String, Value>,
) -> Result<(), String> {
    change(env, |map| {
        if let Some(old) = map.get(name).and_then(Value::as_object) {
            if let Some(key) = old
                .get("api_key")
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty())
            {
                if body
                    .get("api_key")
                    .and_then(Value::as_str)
                    .is_none_or(|s| s.trim().is_empty() || s.trim() == mask(key))
                {
                    body.insert("api_key".into(), Value::String(key.into()));
                }
            }
            for (field, value) in old {
                if body.get(field).is_none_or(Value::is_null) {
                    body.insert(field.clone(), value.clone());
                }
            }
        }
        map.insert(name.into(), Value::Object(normalize(name, &body)?));
        Ok(true)
    })
    .map(|_| super::catalog::invalidate(env, name))
}

pub fn remove(env: &AccountEnvironment, name: &str) -> Result<bool, String> {
    let changed = change(env, |map| Ok(map.remove(name).is_some()))?;
    if changed {
        super::catalog::invalidate(env, name);
    }
    Ok(changed)
}
