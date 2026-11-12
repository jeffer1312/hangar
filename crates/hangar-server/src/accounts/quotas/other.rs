//! Cota das fontes que não são Claude nem Codex: provedores do Kimi Code (`config.toml`), motores
//! do `engines.json` e o painel do OpenCode Go. Os leitores nunca dizem `expirada`: 401/403/404
//! aqui quase sempre é "esta URL não tem a rota", não chave vencida.
use super::reading;
use crate::accounts::environment::AccountEnvironment;
use reqwest::header::HeaderValue;
use serde_json::{Map, Value, json};
use std::path::{Path, PathBuf};

pub const COMMANDCODE_URL: &str = "https://api.commandcode.ai/alpha/billing/credits";
pub const OPENCODE_URL: &str = "https://opencode.ai/workspace/{}/go";
// Sem User-Agent de navegador o Cloudflare do CommandCode responde 403 (1010) e o painel do
// OpenCode recusa a página.
const COMMANDCODE_UA: &str = "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/126.0.0.0 Safari/537.36";
const OPENCODE_UA: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) Gecko/20100101 Firefox/148.0";

/// URLs fixas dos leitores; só os testes apontam para outro lugar.
#[derive(Clone, Debug)]
pub struct Endpoints {
    pub commandcode: String,
    pub opencode: String,
}

impl Default for Endpoints {
    fn default() -> Self {
        Self { commandcode: COMMANDCODE_URL.into(), opencode: OPENCODE_URL.into() }
    }
}

#[derive(Clone, PartialEq)]
pub enum Reader {
    Kimi { key: String, base: String },
    CommandCode { key: String },
    OpenCode { workspace: String, cookie: String },
}

// Chave e cookie nunca saem num `{:?}` de log.
impl std::fmt::Debug for Reader {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Reader::Kimi { base, .. } => write!(f, "Kimi {{ base: {base:?}, key: \"…\" }}"),
            Reader::CommandCode { .. } => write!(f, "CommandCode {{ key: \"…\" }}"),
            Reader::OpenCode { workspace, .. } => write!(f, "OpenCode {{ workspace: {workspace:?}, cookie: \"…\" }}"),
        }
    }
}

impl Reader {
    pub fn kind(&self) -> &'static str {
        match self {
            Reader::Kimi { .. } => "kimi",
            Reader::CommandCode { .. } => "commandcode",
            Reader::OpenCode { .. } => "opencode",
        }
    }

    /// Só para o teste de contrato comparar com o Python; devolve segredos.
    pub fn args(&self) -> Vec<&str> {
        match self {
            Reader::Kimi { key, base } => vec![key, base],
            Reader::CommandCode { key } => vec![key],
            Reader::OpenCode { workspace, cookie } => vec![workspace, cookie],
        }
    }

    /// Endereço e credencial da leitura: trocar qualquer um descarta a cota guardada e a espera.
    /// Sai do processo só como hash (`QuotaCache::set_endpoint`).
    pub fn fingerprint(&self, endpoints: &Endpoints) -> String {
        let secret = match self {
            Reader::Kimi { key, .. } | Reader::CommandCode { key } => key,
            Reader::OpenCode { cookie, .. } => cookie,
        };
        format!("{}\0{secret}", self.url(endpoints))
    }

    pub fn url(&self, endpoints: &Endpoints) -> String {
        match self {
            Reader::Kimi { base, .. } => format!("{}/usages", base.trim_end_matches('/')),
            Reader::CommandCode { .. } => endpoints.commandcode.clone(),
            Reader::OpenCode { workspace, .. } => endpoints.opencode.replacen(
                "{}",
                &percent_encoding::utf8_percent_encode(workspace, QUOTE).to_string(),
                1,
            ),
        }
    }
}

/// `urllib.parse.quote(safe="")`: só letras, dígitos e `_.-~` passam.
const QUOTE: &percent_encoding::AsciiSet = &percent_encoding::NON_ALPHANUMERIC
    .remove(b'_')
    .remove(b'.')
    .remove(b'-')
    .remove(b'~');

pub struct Source {
    pub id: String,
    /// Rótulo cru: `label` de motor editado à mão pode não ser texto, e assim mesmo sai na lista.
    pub label: Value,
    pub provider: &'static str,
    pub reader: Reader,
}

impl Source {
    pub fn row(&self) -> Value {
        json!({"id":self.id,"label":self.label,"provedor":self.provider,"ativa":false})
    }
}

fn shared(env: &AccountEnvironment) -> PathBuf {
    env.home.join(".claude")
}

/// O nome novo; o antigo `.claude-pocket-*` só quando o novo não existe (no Windows link de
/// arquivo exige privilégio, e o antigo pode ser o único).
pub(crate) use hangar_core::fs::legacy_sidecar_path as read_path;

fn env_value<'a>(env: &'a AccountEnvironment, name: &str) -> Option<&'a str> {
    env.base.get(name).map(String::as_str).filter(|v| !v.is_empty())
}

/// Ausente não é problema (nenhum conteúdo, nenhum código); ilegível dá o código para o diário.
fn read_file(path: &Path, unreadable: &'static str, problems: &mut Vec<&'static str>) -> Option<Vec<u8>> {
    match std::fs::read(path) {
        Ok(bytes) => Some(bytes),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(_) => {
            problems.push(unreadable);
            None
        }
    }
}

/// Objeto JSON de um arquivo de config; fora do formato = vazio, com o código.
fn json_object(bytes: Option<Vec<u8>>, invalid: &'static str, problems: &mut Vec<&'static str>) -> Map<String, Value> {
    let Some(bytes) = bytes else { return Map::new() };
    match serde_json::from_slice::<Value>(&bytes) {
        Ok(Value::Object(map)) => map,
        _ => {
            problems.push(invalid);
            Map::new()
        }
    }
}

/// `str.strip()` do Python: também tira os separadores U+001C–U+001F, que o `trim` não tira.
pub fn py_strip(text: &str) -> &str {
    text.trim_matches(|c: char| c.is_whitespace() || ('\u{1c}'..='\u{1f}').contains(&c))
}

pub(crate) fn kimi_config(env: &AccountEnvironment) -> PathBuf {
    env_value(env, "KIMI_CODE_HOME").map(PathBuf::from).unwrap_or_else(|| env.home.join(".kimi-code")).join("config.toml")
}

/// Os arquivos das fontes lidos uma vez por pedido (leitura síncrona: quem chama roda em
/// `spawn_blocking`). Arquivo torto vira código em `problems`, nunca a lista caída.
#[derive(Default)]
pub struct Config {
    pub sources: Vec<Source>,
    pub problems: Vec<&'static str>,
    /// Apelidos da tela de contas: só texto não vazio, com as pontas aparadas.
    pub aliases: Map<String, Value>,
    /// `engines.json` cru: a lista de credenciais mostra também motor sem chave.
    pub engines: Map<String, Value>,
    /// Cookies colados na tela, só os completos (`workspace_id` e `auth_cookie` texto não vazio).
    pub opencode: Map<String, Value>,
    /// Texto do `config.toml` do Kimi, para saber quais provedores o app gravou.
    pub kimi_text: Option<String>,
}

fn kimi_providers(text: Option<&str>, problems: &mut Vec<&'static str>) -> Vec<(String, String, String)> {
    let Some(text) = text else { return vec![] };
    // O `tomllib` recusa o arquivo com BOM.
    let table = match text.parse::<toml::Table>() {
        Ok(table) if !text.starts_with('\u{feff}') => table,
        _ => {
            problems.push("kimi_config_invalid");
            return vec![];
        }
    };
    let Some(providers) = table.get("providers").and_then(toml::Value::as_table) else { return vec![] };
    let filled = |value: Option<&toml::Value>| value.and_then(toml::Value::as_str).filter(|s| !s.is_empty()).map(str::to_owned);
    providers
        .iter()
        .filter_map(|(name, provider)| {
            let provider = provider.as_table()?;
            if provider.get("type").and_then(toml::Value::as_str) != Some("kimi") {
                return None;
            }
            // Provider por OAuth não tem chave aqui: adivinhar o armazenamento dele seria inventar.
            Some((name.clone(), filled(provider.get("api_key"))?, filled(provider.get("base_url"))?))
        })
        .collect()
}

/// `^[a-z0-9][a-z0-9_-]{0,31}$` do Python, cujo `$` também casa antes de uma quebra final.
pub(crate) fn engine_name_ok(name: &str) -> bool {
    static NAME: std::sync::LazyLock<regex::Regex> =
        std::sync::LazyLock::new(|| regex::Regex::new(r"\A[a-z0-9][a-z0-9_-]{0,31}\n?\z").unwrap());
    NAME.is_match(name)
}

fn engines(env: &AccountEnvironment, problems: &mut Vec<&'static str>) -> Map<String, Value> {
    let path = env_value(env, "CP_ENGINES_FILE").map(PathBuf::from).unwrap_or_else(|| shared(env).join("engines.json"));
    let Some(bytes) = read_file(&path, "engines_unreadable", problems) else { return Map::new() };
    if std::str::from_utf8(&bytes).is_err() {
        problems.push("engines_not_utf8");
        return Map::new();
    }
    match parse_json(&bytes) {
        Some(Value::Object(map)) => map,
        _ => {
            problems.push("engines_invalid");
            Map::new()
        }
    }
}

fn opencode_env(env: &AccountEnvironment) -> Option<(String, String)> {
    // Reserva do ambiente (as variáveis do pi-quotas): o que a pessoa colou na tela vence.
    let workspace = py_strip(env.base.get("OPENCODE_GO_WORKSPACE_ID").map_or("", String::as_str));
    let cookie = py_strip(env.base.get("OPENCODE_GO_AUTH_COOKIE").map_or("", String::as_str));
    (!workspace.is_empty() && !cookie.is_empty()).then(|| (workspace.to_owned(), cookie.to_owned()))
}

/// Lê config do Kimi, `engines.json`, cookies do OpenCode e apelidos uma vez e monta as fontes:
/// provedores do Kimi Code primeiro, depois os motores, cada grupo na ordem do arquivo. Sem
/// deduplicação: a mesma chave nos dois lugares são duas fontes. Motor torto sai sozinho.
pub fn load(env: &AccountEnvironment) -> Config {
    let mut problems = vec![];
    let kimi_text = read_file(&kimi_config(env), "kimi_config_unreadable", &mut problems)
        .and_then(|bytes| String::from_utf8(bytes).map_err(|_| problems.push("kimi_config_invalid")).ok());
    let aliases_file = read_file(&read_path(shared(env).join(".hangar-apelidos.json")), "aliases_unreadable", &mut problems);
    let aliases = json_object(aliases_file, "aliases_invalid", &mut problems)
        .into_iter()
        .filter_map(|(id, value)| {
            let alias = py_strip(value.as_str()?);
            (!alias.is_empty()).then(|| (id, json!(alias)))
        })
        .collect();
    let opencode = hangar_opencode_cookie::read(&env.home).unwrap_or_else(|error| {
        problems.push(if error.kind() == std::io::ErrorKind::InvalidData { "opencode_cookies_invalid" } else { "opencode_cookies_unreadable" });
        Map::new()
    });
    let engines = engines(env, &mut problems);

    let mut sources = vec![];
    for (name, key, base) in kimi_providers(kimi_text.as_deref(), &mut problems) {
        let reader = if base.contains("commandcode.ai") { Reader::CommandCode { key } } else { Reader::Kimi { key, base } };
        sources.push(Source { id: format!("kimi:{name}"), label: json!(name), provider: reader.kind(), reader });
    }
    for (name, data) in &engines {
        if !engine_name_ok(name) {
            continue;
        }
        let Some(data) = data.as_object() else {
            problems.push("engine_not_object");
            continue;
        };
        // Motor com `label` ou `base_url` fora do tipo sai da cota como sai da lista de credenciais.
        let label = match data.get("label") {
            Some(label) if !falsy(label) && !label.is_string() => {
                problems.push("engine_invalid_label");
                continue;
            }
            Some(label) if !falsy(label) => label.clone(),
            _ => json!(name),
        };
        if data.get("base_url").is_some_and(|base| !base.is_null() && !base.is_string()) {
            problems.push("engine_invalid_base_url");
            continue;
        }
        let text = |field: &str| data.get(field).and_then(Value::as_str).filter(|s| !s.is_empty()).map(str::to_owned);
        let (Some(key), Some(base)) = (text("api_key"), text("base_url")) else { continue };
        let id = format!("chave:{name}");
        let saved = |entry: &Value| {
            let text = |field: &str| entry[field].as_str().unwrap_or_default().to_owned();
            (text("workspace_id"), text("auth_cookie"))
        };
        let cookie = if base.contains("opencode.ai") { opencode.get(&id).map(saved).or_else(|| opencode_env(env)) } else { None };
        let (provider, reader) = if let Some((workspace, cookie)) = cookie {
            ("opencode", Reader::OpenCode { workspace, cookie })
        } else if base.contains("commandcode.ai") {
            ("commandcode", Reader::CommandCode { key })
        } else {
            ("kimi", Reader::Kimi { key, base: kimi_usages_base(&base) })
        };
        sources.push(Source { id, label, provider, reader });
    }
    Config { sources, problems, aliases, engines, opencode, kimi_text }
}

pub(crate) fn falsy(value: &Value) -> bool {
    match value {
        Value::Null => true,
        Value::Bool(b) => !b,
        Value::Number(n) => n.as_f64() == Some(0.0),
        Value::String(s) => s.is_empty(),
        Value::Array(a) => a.is_empty(),
        Value::Object(o) => o.is_empty(),
    }
}

fn kimi_usages_base(base: &str) -> String {
    // O motor usa `/coding` (formato Anthropic); o `/usages` só existe sob `/v1`.
    if base.contains("api.kimi.com") && !base.trim_end_matches('/').ends_with("/v1") {
        format!("{}/v1", base.trim_end_matches('/'))
    } else {
        base.to_owned()
    }
}

// ------------------------------------------------------------------------------- semântica Python

/// `float(texto)` do Python: pontas aparadas, `_` só entre dígitos, `inf`/`nan` sem caixa.
pub fn py_float(text: &str) -> Option<f64> {
    let text = py_strip(text);
    let chars: Vec<char> = text.chars().collect();
    for (i, c) in chars.iter().enumerate() {
        if *c == '_'
            && !(i > 0 && chars[i - 1].is_ascii_digit() && chars.get(i + 1).is_some_and(char::is_ascii_digit))
        {
            return None;
        }
    }
    let clean: String = chars.into_iter().filter(|c| *c != '_').collect();
    let unsigned = clean.strip_prefix(['+', '-']).unwrap_or(&clean).to_ascii_lowercase();
    if !matches!(unsigned.as_str(), "inf" | "infinity" | "nan")
        && !unsigned.chars().all(|c| c.is_ascii_digit() || matches!(c, '.' | 'e' | '+' | '-'))
    {
        return None;
    }
    clean.parse::<f64>().ok()
}

fn num(value: &Value) -> Option<f64> {
    match value {
        Value::Number(n) => n.as_f64(),
        Value::String(s) => py_float(s),
        _ => None,
    }
}

/// Como `max(0.0, min(100.0, x))`: `-0.0` vira `0.0`, que o `clamp` preservaria.
fn clamp_pct(x: f64) -> f64 {
    if x <= 0.0 { 0.0 } else { x.min(100.0) }
}

/// JSON como o `json.loads`: aceita `NaN`/`Infinity` fora de texto. Eles viram texto, que o
/// `float()` dos leitores lê igual ao número.
fn parse_json(bytes: &[u8]) -> Option<Value> {
    if let Ok(value) = serde_json::from_slice(bytes) {
        return Some(value);
    }
    let text = std::str::from_utf8(bytes).ok()?;
    let mut out = String::with_capacity(text.len() + 16);
    let (mut in_string, mut escaped) = (false, false);
    let mut rest = text;
    while let Some(c) = rest.chars().next() {
        if in_string {
            out.push(c);
            if escaped {
                escaped = false;
            } else if c == '\\' {
                escaped = true;
            } else if c == '"' {
                in_string = false;
            }
            rest = &rest[c.len_utf8()..];
            continue;
        }
        if let Some(token) = ["-Infinity", "Infinity", "NaN"].into_iter().find(|t| rest.starts_with(t)) {
            out.push('"');
            out.push_str(token);
            out.push('"');
            rest = &rest[token.len()..];
            continue;
        }
        in_string = c == '"';
        out.push(c);
        rest = &rest[c.len_utf8()..];
    }
    serde_json::from_str(&out).ok()
}

/// `datetime.fromisoformat` (com `Z` trocado) até epoch; data sem fuso é hora local.
pub fn py_iso(value: &Value) -> Option<f64> {
    let text = value.as_str().filter(|s| !s.is_empty())?.replace('Z', "+00:00");
    let chars: Vec<char> = text.chars().collect();
    let digits = |from: usize, len: usize| -> Option<u32> {
        let part: String = chars.get(from..from + len)?.iter().collect();
        part.chars().all(|c| c.is_ascii_digit()).then(|| part.parse().ok())?
    };
    let (year, month, day, date_len) = if chars.get(4) == Some(&'-') {
        if chars.get(7) != Some(&'-') {
            return None;
        }
        (digits(0, 4)?, digits(5, 2)?, digits(8, 2)?, 10)
    } else {
        (digits(0, 4)?, digits(4, 2)?, digits(6, 2)?, 8)
    };
    let date = chrono::NaiveDate::from_ymd_opt(year as i32, month, day)?;
    let mut micros_of_day: i64 = 0;
    let mut offset: Option<i64> = None;
    if chars.len() > date_len {
        let time: Vec<char> = chars[date_len + 1..].to_vec();
        let tz = time.iter().position(|c| *c == '+' || *c == '-');
        let clock: String = time[..tz.unwrap_or(time.len())].iter().collect();
        let clock = clock.trim_end();
        let (h, m, s, us) = hh_mm_ss_ff(clock)?;
        // `24:00` é a meia-noite seguinte, só com o resto zerado.
        if h > 24 || (h == 24 && (m, s, us) != (0, 0, 0)) {
            return None;
        }
        micros_of_day = ((h * 3600 + m * 60 + s) as i64) * 1_000_000 + us as i64;
        if let Some(at) = tz {
            let zone: String = time[at + 1..].iter().collect();
            if matches!(zone.chars().count(), 0 | 1 | 3) {
                return None;
            }
            let (zh, zm, zs, zus) = hh_mm_ss_ff(&zone)?;
            let total = ((zh * 3600 + zm * 60 + zs) as i64) * 1_000_000 + zus as i64;
            if total >= 86_400_000_000 {
                return None;
            }
            offset = Some(if time[at] == '-' { -total } else { total });
        }
    }
    let midnight = date.and_hms_opt(0, 0, 0)?;
    let micros = match offset {
        Some(offset) => midnight.and_utc().timestamp_micros() + micros_of_day - offset,
        None => {
            use chrono::TimeZone;
            // Hora sem fuso na troca do horário de verão: a que não existe sai sem reset e a que
            // se repete fica com a primeira. O Kimi manda `Z`, então isso fica como limite.
            let local = midnight + chrono::Duration::microseconds(micros_of_day);
            chrono::Local.from_local_datetime(&local).earliest()?.timestamp_micros()
        }
    };
    Some(micros as f64 / 1_000_000.0)
}

/// `HH[:?MM[:?SS[{.,}f…]]]`: o separador `:` é do primeiro campo em diante ou de nenhum; fração
/// além de 6 dígitos é cortada.
fn hh_mm_ss_ff(text: &str) -> Option<(u32, u32, u32, u32)> {
    let chars: Vec<char> = text.chars().collect();
    let mut parts = [0u32; 3];
    let mut pos = 0;
    let mut separated = false;
    for (index, part) in parts.iter_mut().enumerate() {
        let field: String = chars.get(pos..pos + 2)?.iter().collect();
        if !field.chars().all(|c| c.is_ascii_digit()) {
            return None;
        }
        *part = field.parse().ok()?;
        pos += 2;
        let next = chars.get(pos);
        if index == 0 {
            separated = next == Some(&':');
        }
        if next.is_none() || index == 2 {
            break;
        }
        if separated {
            if next != Some(&':') {
                break;
            }
            pos += 1;
        } else if !next.is_some_and(char::is_ascii_digit) {
            break;
        }
    }
    let mut micros = 0;
    if pos < chars.len() {
        if !matches!(chars[pos], '.' | ',') {
            return None;
        }
        let fraction: String = chars[pos + 1..].iter().collect();
        if !fraction.chars().all(|c| c.is_ascii_digit()) {
            return None;
        }
        let kept: String = fraction.chars().take(6).collect();
        micros = format!("{kept:0<6}").parse().ok()?;
    }
    let [h, m, s] = parts;
    (m < 60 && s < 60).then_some((h, m, s, micros))
}

/// Rótulo pela duração em minutos; `inf`/`nan` levantavam no `int()` do Python (leitor quebrado).
fn window_label(minutes: &Value) -> Result<String, ()> {
    let Some(n) = num(minutes).filter(|n| !(*n <= 0.0)) else {
        return Ok("janela".into());
    };
    if n >= 1440.0 && n % 1440.0 == 0.0 {
        return Ok(format!("{:.0}d", (n / 1440.0).floor()));
    }
    if n >= 60.0 && n % 60.0 == 0.0 {
        return Ok(format!("{:.0}h", (n / 60.0).floor()));
    }
    if !n.is_finite() {
        return Err(());
    }
    Ok(format!("{:.0}min", n.trunc()))
}

/// Conta não finita (NaN/inf na resposta) não é janela: mostraria a fonte esgotada por lixo.
fn window(label: &str, pct: f64, reset: Option<f64>) -> Option<Value> {
    pct.is_finite()
        .then(|| json!({"rotulo":label,"pct":clamp_pct(pct),"reset_ts":reset,"por_modelo":false}))
}

fn kimi_window(detail: &Value, label: &str) -> Option<Value> {
    let detail = detail.as_object()?;
    let (limit, remaining) = (num(detail.get("limit")?)?, num(detail.get("remaining")?)?);
    if limit <= 0.0 {
        return None;
    }
    window(label, (limit - remaining) / limit * 100.0, detail.get("resetTime").and_then(py_iso))
}

/// `limits[]` por duração, depois o `usage` do topo como `7d` (a janela do plano).
pub fn kimi(body: &Value) -> Result<Vec<Value>, ()> {
    let mut windows = vec![];
    for item in body["limits"].as_array().into_iter().flatten() {
        if !item.is_object() {
            continue;
        }
        let duration = item["window"].as_object().and_then(|w| w.get("duration")).cloned().unwrap_or(Value::Null);
        let label = window_label(&duration)?;
        windows.extend(kimi_window(&item["detail"], &label));
    }
    windows.extend(kimi_window(&body["usage"], "7d"));
    Ok(windows)
}

fn commandcode_window(value: &Value, label: &str) -> Option<Value> {
    let value = value.as_object()?;
    let (used, cap) = (num(value.get("used")?)?, num(value.get("cap")?)?);
    if cap <= 0.0 {
        return None;
    }
    // `resetAt` em epoch-milissegundos; 0 = janela sem reset marcado.
    let reset = value.get("resetAt").and_then(num).filter(|r| *r != 0.0).map(|r| r / 1000.0);
    window(label, used / cap * 100.0, reset)
}

pub fn commandcode(body: &Value) -> Vec<Value> {
    let limits = body["windowLimits"].as_object().cloned().unwrap_or_default();
    [("fiveHour", "5h"), ("weekly", "7d")]
        .into_iter()
        .filter_map(|(key, label)| commandcode_window(limits.get(key)?, label))
        .collect()
}

/// As três janelas do plano Go no HTML do painel; a ordem dos dois campos muda entre renderizações.
pub fn opencode(html: &str, now: f64) -> Vec<Value> {
    static PATTERNS: std::sync::LazyLock<Vec<(&str, regex::Regex, regex::Regex)>> = std::sync::LazyLock::new(|| {
        const NUM: &str = r"(-?\d+(?:\.\d+)?)";
        [("5h", "rollingUsage"), ("7d", "weeklyUsage"), ("30d", "monthlyUsage")]
            .into_iter()
            .map(|(label, key)| {
                let pct_first = format!(r"{key}:\$R\[\d+\]=\{{[^}}]*usagePercent:{NUM}[^}}]*resetInSec:{NUM}[^}}]*\}}");
                let reset_first = format!(r"{key}:\$R\[\d+\]=\{{[^}}]*resetInSec:{NUM}[^}}]*usagePercent:{NUM}[^}}]*\}}");
                (label, regex::Regex::new(&pct_first).unwrap(), regex::Regex::new(&reset_first).unwrap())
            })
            .collect()
    });
    PATTERNS
        .iter()
        .filter_map(|(label, pct_first, reset_first)| {
            let (pct, wait) = if let Some(c) = pct_first.captures(html) {
                (c[1].parse::<f64>().ok()?, c[2].parse::<f64>().ok()?)
            } else {
                let c = reset_first.captures(html)?;
                (c[2].parse::<f64>().ok()?, c[1].parse::<f64>().ok()?)
            };
            window(label, pct, Some(now + if wait > 0.0 { wait } else { 0.0 }))
        })
        .collect()
}

// ------------------------------------------------------------------------------------------- rede

/// Cliente das fontes: proxy do ambiente, sem seguir redirecionamento (a chave nunca vai a outro
/// endereço) e conexão com prazo curto, para recusa não ocupar o prazo inteiro.
pub fn client() -> Result<reqwest::Client, crate::accounts::catalog::AccountError> {
    crate::http_client()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(std::time::Duration::from_secs(10))
        .connect_timeout(std::time::Duration::from_secs(3))
        .build()
        .map_err(|error| {
            tracing::warn!(code = "quota_client_build_failed", %error, "cliente HTTP das fontes de cota não montou");
            crate::accounts::catalog::AccountError::io()
        })
}

/// Teto do corpo lido: `base_url` é editável e uma resposta sem fim viraria memória.
const MAX_BODY: usize = 1 << 20;

enum Fetch {
    Body(u16, Vec<u8>),
    Status(u16),
    /// Sem resposta HTTP; `refused` = conexão recusada (entra na espera crescente).
    NoResponse { refused: bool, code: &'static str },
    BadHeader,
    BadUrl,
    /// Endereço que não se monta (host com espaço, porta inválida).
    Invalid,
    TooBig,
}

/// O que a tentativa disse sobre a fonte, além da leitura: decide a espera.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Contact {
    /// Houve resposta HTTP: a fonte está no ar, a espera por recusa zera.
    Answered,
    /// Conexão recusada: entra na espera crescente.
    Refused,
    /// Sem resposta por outro motivo (prazo, DNS, sem rota, corpo cortado, credencial que não vira cabeçalho,
    /// endereço que não se monta): não sobe a escada, o código vai ao diário.
    Silent,
}

/// Uma leitura: a cota, o que a tentativa disse da fonte e, quando há o que registrar, o código do
/// diário (sem segredo).
pub struct Outcome {
    pub value: Value,
    pub contact: Contact,
    pub code: Option<&'static str>,
}

/// Só a recusa conta como "ninguém escutando": sem rota, DNS e prazo de conexão também saem por
/// `is_connect()`, e a Wi-Fi ou a VPN que cai não pode deixar as fontes da nuvem 1 h em espera.
fn refused(error: &reqwest::Error) -> bool {
    let mut cause = std::error::Error::source(error);
    while let Some(err) = cause {
        if err.downcast_ref::<std::io::Error>().is_some_and(|io| is_refusal(io.kind())) {
            return true;
        }
        cause = err.source();
    }
    false
}

fn is_refusal(kind: std::io::ErrorKind) -> bool {
    kind == std::io::ErrorKind::ConnectionRefused
}

async fn get(client: &reqwest::Client, url: &str, headers: &[(&str, String)]) -> Fetch {
    // `urllib.request.Request` recusa endereço sem esquema antes da rede: era leitor quebrado.
    static SCHEME: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| regex::Regex::new(r"\A[^/:]+:").unwrap());
    if !SCHEME.is_match(url) {
        return Fetch::BadUrl;
    }
    if reqwest::Url::parse(url).is_err() {
        return Fetch::Invalid;
    }
    let mut request = client.get(url);
    for (name, value) in headers {
        // O valor nunca vai a log nem a erro: é chave ou cookie.
        let Ok(value) = HeaderValue::from_str(value) else { return Fetch::BadHeader };
        request = request.header(*name, value);
    }
    let mut response = match request.send().await {
        Ok(response) => response,
        Err(error) if refused(&error) => return Fetch::NoResponse { refused: true, code: "quota_source_unreachable" },
        Err(error) if error.is_timeout() => return Fetch::NoResponse { refused: false, code: "quota_source_timeout" },
        Err(_) => return Fetch::NoResponse { refused: false, code: "quota_source_no_response" },
    };
    let status = response.status().as_u16();
    if !response.status().is_success() {
        return Fetch::Status(status);
    }
    if response.content_length().is_some_and(|length| length > MAX_BODY as u64) {
        return Fetch::TooBig;
    }
    let mut body = Vec::new();
    loop {
        match response.chunk().await {
            Ok(Some(chunk)) if body.len() + chunk.len() > MAX_BODY => return Fetch::TooBig,
            Ok(Some(chunk)) => body.extend_from_slice(&chunk),
            Ok(None) => return Fetch::Body(status, body),
            Err(error) if error.is_timeout() => return Fetch::NoResponse { refused: false, code: "quota_source_timeout" },
            Err(_) => return Fetch::NoResponse { refused: false, code: "quota_source_no_response" },
        }
    }
}

/// Uma leitura. Sem resposta = `sem-resposta`; status de erro = `http-<código>` (o 429 é o que faz o
/// cache esperar).
pub async fn read(client: &reqwest::Client, reader: &Reader, endpoints: &Endpoints, now: f64) -> Outcome {
    let url = reader.url(endpoints);
    let headers: Vec<(&str, String)> = match reader {
        Reader::Kimi { key, .. } => vec![
            ("authorization", format!("Bearer {key}")),
            ("accept", "application/json".into()),
            ("user-agent", "hangar".into()),
        ],
        Reader::CommandCode { key } => vec![
            ("authorization", format!("Bearer {key}")),
            ("accept", "application/json".into()),
            ("user-agent", COMMANDCODE_UA.into()),
        ],
        Reader::OpenCode { cookie, .. } => vec![
            ("user-agent", OPENCODE_UA.into()),
            ("accept", "text/html".into()),
            ("cookie", format!("auth={cookie}")),
        ],
    };
    let out = |reason: &str, contact, code| Outcome { value: reading("indisponivel", vec![], Some(reason)), contact, code };
    let (status, body) = match get(client, &url, &headers).await {
        Fetch::Body(status, body) => (status, body),
        Fetch::Status(status) => return out(&format!("http-{status}"), Contact::Answered, None),
        Fetch::NoResponse { refused, code } => {
            return out("sem-resposta", if refused { Contact::Refused } else { Contact::Silent }, Some(code));
        }
        Fetch::BadHeader => return out("credencial-invalida", Contact::Silent, Some("quota_credential_invalid")),
        Fetch::BadUrl => return out("erro-leitor", Contact::Silent, Some("quota_reader_failed")),
        // O Python também dizia `sem-resposta`; o código separado aponta a config, não a rede.
        Fetch::Invalid => return out("sem-resposta", Contact::Silent, Some("quota_source_bad_url")),
        Fetch::TooBig => return out("resposta-grande", Contact::Answered, Some("quota_response_too_large")),
    };
    let windows = match reader {
        Reader::OpenCode { .. } => {
            if body.is_empty() {
                return out(&format!("http-{status}"), Contact::Answered, None);
            }
            let windows = opencode(&String::from_utf8_lossy(&body), now);
            if windows.is_empty() {
                // Página de login com 200 ou painel mudou: "não informa cota", nunca zero.
                return out("painel-sem-numeros", Contact::Answered, None);
            }
            windows
        }
        Reader::Kimi { .. } | Reader::CommandCode { .. } => {
            let Some(value) = parse_json(&body) else { return out("sem-resposta", Contact::Answered, None) };
            if !value.is_object() {
                return out(&format!("http-{status}"), Contact::Answered, None);
            }
            // O braço só recebe Kimi ou CommandCode.
            let windows = if matches!(reader, Reader::CommandCode { .. }) {
                commandcode(&value)
            } else {
                match kimi(&value) {
                    Ok(windows) => windows,
                    Err(()) => return out("erro-leitor", Contact::Answered, Some("quota_reader_failed")),
                }
            };
            if windows.is_empty() {
                return out("formato-desconhecido", Contact::Answered, None);
            }
            windows
        }
    };
    Outcome { value: reading("lida", windows, None), contact: Contact::Answered, code: None }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Entrada externa qualquer não derruba os leitores: bytes soltos, JSON mutado e HTML mutado.
    #[test]
    fn readers_never_panic_on_hostile_input() {
        let mut seed: u64 = 0x9e37_79b9_7f4a_7c15;
        let mut next = move || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed
        };
        let samples = [
            r#"{"limits":[{"window":{"duration":300},"detail":{"limit":"100","remaining":"75","resetTime":"2026-08-18T17:59:46.782017Z"}}],"usage":{"limit":"1e2","remaining":"NaN"}}"#,
            r#"{"windowLimits":{"fiveHour":{"used":1,"cap":"14","resetAt":1787564008022},"weekly":{"used":"inf","cap":35}}}"#,
            "rollingUsage:$R[3]={usagePercent:12.5,resetInSec:3600};weeklyUsage:$R[4]={resetInSec:9,usagePercent:-1}",
        ];
        let alphabet = br#"{}[]":,.-+0123456789eEnaNIfity_ $R=resetTimeZT:limitusage"#;
        for _ in 0..20_000 {
            let base = samples[(next() % 3) as usize].as_bytes().to_vec();
            let mut bytes = base;
            for _ in 0..(next() % 8) {
                let at = (next() as usize) % (bytes.len() + 1);
                match next() % 3 {
                    0 if at < bytes.len() => {
                        bytes.remove(at);
                    }
                    1 => bytes.insert(at, alphabet[(next() as usize) % alphabet.len()]),
                    _ => bytes.insert(at, (next() % 256) as u8),
                }
            }
            if let Some(value) = parse_json(&bytes) {
                let _ = kimi(&value);
                let _ = commandcode(&value);
                for item in value["limits"].as_array().into_iter().flatten() {
                    let _ = py_iso(&item["detail"]["resetTime"]);
                }
            }
            let text = String::from_utf8_lossy(&bytes);
            let _ = opencode(&text, 0.0);
            let _ = py_float(&text);
            let _ = py_iso(&json!(text));
        }
    }

    /// Arquivos de config editados à mão (motores, Kimi, cookies, apelidos) nunca derrubam a leitura.
    #[test]
    fn config_readers_never_panic_on_hostile_files() {
        let mut seed: u64 = 0x2545_f491_4f6c_dd1d;
        let mut next = move || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed
        };
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path();
        let claude = home.join(".claude");
        std::fs::create_dir_all(&claude).unwrap();
        std::fs::create_dir_all(home.join(".kimi-code")).unwrap();
        let env = AccountEnvironment::from_map([("HOME".into(), home.to_string_lossy().into_owned())].into());
        let files = [
            (claude.join("engines.json"), r#"{"a":{"base_url":"https://opencode.ai/x","api_key":"k","label":"L"},"b":"t","c":{"base_url":["x"]}}"#),
            (home.join(".kimi-code/config.toml"), "[providers.apikey]\ntype = \"kimi\"\napi_key = \"k\"\nbase_url = \"https://api.kimi.com/coding\"\n"),
            (claude.join(".hangar-opencode.json"), r#"{"chave:a":{"workspace_id":"w","auth_cookie":"c"}}"#),
            (claude.join(".hangar-apelidos.json"), r#"{"chave:a":" Nome ","x":3}"#),
        ];
        let alphabet = br#"{}[]":,.=\n-+0123456789 abcktypeurl_"#;
        for _ in 0..3_000 {
            for (path, sample) in &files {
                let mut bytes = sample.as_bytes().to_vec();
                for _ in 0..(next() % 6) {
                    let at = (next() as usize) % (bytes.len() + 1);
                    match next() % 3 {
                        0 if at < bytes.len() => {
                            bytes.remove(at);
                        }
                        1 => bytes.insert(at, alphabet[(next() as usize) % alphabet.len()]),
                        _ => bytes.insert(at, (next() % 256) as u8),
                    }
                }
                std::fs::write(path, bytes).unwrap();
            }
            let _ = load(&env);
        }
    }

    #[test]
    fn reader_debug_never_shows_key_or_cookie() {
        let readers = [
            Reader::Kimi { key: "sk-segredo-1".into(), base: "https://api.kimi.com/coding/v1".into() },
            Reader::CommandCode { key: "sk-segredo-2".into() },
            Reader::OpenCode { workspace: "wrk".into(), cookie: "cookie-segredo".into() },
        ];
        for reader in readers {
            let text = format!("{reader:?}");
            assert!(!text.contains("segredo"), "{text}");
        }
    }

    #[tokio::test]
    async fn base_url_that_is_not_an_address_is_a_config_problem_not_a_refusal() {
        let reader = Reader::Kimi { key: "k".into(), base: "http://host com espaço".into() };
        let outcome = read(&client().unwrap(), &reader, &Endpoints::default(), 0.0).await;
        assert_eq!(outcome.value["motivo"], "sem-resposta");
        assert_eq!((outcome.contact, outcome.code), (Contact::Silent, Some("quota_source_bad_url")));
    }

    /// DNS que não resolve não é "ninguém escutando": não pode pôr a fonte na espera crescente.
    #[tokio::test]
    async fn name_that_does_not_resolve_is_silent_not_refused() {
        let reader = Reader::Kimi { key: "k".into(), base: "http://hangar-nao-existe.invalid".into() };
        let outcome = read(&client().unwrap(), &reader, &Endpoints::default(), 0.0).await;
        assert_eq!(outcome.value["motivo"], "sem-resposta");
        assert_eq!(outcome.contact, Contact::Silent);
    }

    #[tokio::test]
    async fn closed_local_port_is_refused() {
        let closed = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap();
        let reader = Reader::Kimi { key: "k".into(), base: format!("http://{closed}") };
        let outcome = read(&client().unwrap(), &reader, &Endpoints::default(), 0.0).await;
        assert_eq!((outcome.contact, outcome.code), (Contact::Refused, Some("quota_source_unreachable")));
    }

    /// Resposta acima do teto não vira memória: `resposta-grande`, a fonte está no ar.
    #[tokio::test]
    async fn oversized_body_is_cut_off() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let big = "x".repeat(MAX_BODY + 1);
        let app = axum::Router::new().route("/usages", axum::routing::get(move || {
            let big = big.clone();
            async move { big }
        }));
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let reader = Reader::Kimi { key: "k".into(), base: format!("http://{address}") };
        let outcome = read(&client().unwrap(), &reader, &Endpoints::default(), 0.0).await;
        assert_eq!(outcome.value["motivo"], "resposta-grande");
        assert_eq!((outcome.contact, outcome.code), (Contact::Answered, Some("quota_response_too_large")));
        server.abort();
    }

    #[test]
    fn fingerprint_changes_with_the_secret_and_the_address() {
        let endpoints = Endpoints::default();
        let a = Reader::Kimi { key: "k1".into(), base: "https://x".into() }.fingerprint(&endpoints);
        assert_ne!(a, Reader::Kimi { key: "k2".into(), base: "https://x".into() }.fingerprint(&endpoints));
        assert_ne!(a, Reader::Kimi { key: "k1".into(), base: "https://y".into() }.fingerprint(&endpoints));
    }

    #[test]
    fn engine_with_label_that_is_not_text_is_left_out_of_the_quotas() {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path();
        std::fs::create_dir_all(home.join(".claude")).unwrap();
        std::fs::write(home.join(".claude/engines.json"), r#"{"a":{"base_url":"https://x","api_key":"k","label":7},"b":{"base_url":"https://x","api_key":"k"}}"#).unwrap();
        let env = AccountEnvironment::from_map([("HOME".into(), home.to_string_lossy().into_owned())].into());
        let config = load(&env);
        let ids: Vec<&str> = config.sources.iter().map(|s| s.id.as_str()).collect();
        assert_eq!(ids, ["chave:b"]);
        assert_eq!(config.problems, ["engine_invalid_label"]);
    }

    #[test]
    fn broken_config_files_are_reported_and_missing_ones_are_not() {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path();
        let env = AccountEnvironment::from_map([("HOME".into(), home.to_string_lossy().into_owned())].into());
        assert!(load(&env).problems.is_empty(), "ausente não é problema");
        std::fs::create_dir_all(home.join(".claude")).unwrap();
        std::fs::create_dir_all(home.join(".kimi-code")).unwrap();
        std::fs::write(home.join(".kimi-code/config.toml"), "\u{feff}[providers]\n").unwrap();
        std::fs::write(home.join(".claude/.hangar-apelidos.json"), "[1]").unwrap();
        std::fs::write(home.join(".claude/.hangar-opencode.json"), b"\xff{").unwrap();
        let mut problems = load(&env).problems;
        problems.sort();
        assert_eq!(problems, ["aliases_invalid", "kimi_config_invalid", "opencode_cookies_invalid"], "cada arquivo uma vez");
        std::fs::write(home.join(".kimi-code/config.toml"), "[providers]\n").unwrap();
        std::fs::write(home.join(".claude/.hangar-apelidos.json"), "{}").unwrap();
        std::fs::write(home.join(".claude/.hangar-opencode.json"), "{}").unwrap();
        assert!(load(&env).problems.is_empty());
    }

    #[test]
    fn only_a_refused_connection_climbs_the_wait() {
        use std::io::ErrorKind::*;
        assert!(is_refusal(ConnectionRefused));
        assert!(!is_refusal(NetworkUnreachable), "Wi-Fi/VPN caída não é motor desligado");
        assert!(!is_refusal(HostUnreachable));
        assert!(!is_refusal(TimedOut));
    }

    #[test]
    fn negative_zero_percent_is_zero() {
        assert!(clamp_pct(-0.0).is_sign_positive());
        assert_eq!(clamp_pct(-5.0), 0.0);
        assert_eq!(clamp_pct(150.0), 100.0);
    }

    #[test]
    fn hour_24_is_next_midnight_like_fromisoformat() {
        for text in ["2026-01-01T24:00:00Z", "2026-01-01T24:00Z", "2026-01-01T24:00:00.000Z", "2026-01-01T24Z", "2026-01-01T2400Z"] {
            assert_eq!(py_iso(&json!(text)), Some(1767312000.0), "{text}");
        }
        assert_eq!(py_iso(&json!("2026-12-31T24:00:00Z")), Some(1798761600.0));
        assert_eq!(py_iso(&json!("2026-01-01T24:00:01Z")), None);
        assert_eq!(py_iso(&json!("2026-01-01T25:00:00Z")), None);
    }

    /// Sem `Content-Length` o teto vale do mesmo jeito, pedaço a pedaço.
    #[tokio::test]
    async fn chunked_body_over_the_cap_is_cut_off() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let app = axum::Router::new().route("/usages", axum::routing::get(|| async {
            let chunks = (0..20).map(|_| Ok::<_, std::io::Error>(bytes::Bytes::from(vec![b'x'; 64 * 1024])));
            axum::body::Body::from_stream(futures_util::stream::iter(chunks))
        }));
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let reader = Reader::Kimi { key: "k".into(), base: format!("http://{address}") };
        let outcome = read(&client().unwrap(), &reader, &Endpoints::default(), 0.0).await;
        assert_eq!(outcome.value["motivo"], "resposta-grande");
        server.abort();
    }

    #[test]
    fn python_float_rules() {
        assert_eq!(py_float(" 1e2 "), Some(100.0));
        assert_eq!(py_float("1_000"), Some(1000.0));
        assert_eq!(py_float("1__0"), None);
        assert_eq!(py_float("_1"), None);
        assert!(py_float("NaN").unwrap().is_nan());
        assert_eq!(py_float("-Infinity"), Some(f64::NEG_INFINITY));
        assert_eq!(py_float("0x10"), None);
        assert_eq!(py_float(""), None);
    }
}
