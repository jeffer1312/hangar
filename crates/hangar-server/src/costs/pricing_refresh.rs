//! Tarifas do models.dev: o download diário para o cache que `Pricing` relê pelo mtime, e o snapshot
//! versionado (`hangar-server pricing-snapshot`). Rede nunca no caminho de um pedido: só na subida.

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, SystemTime};

use serde_json::{Map, Value, json};

pub const URL: &str = "https://models.dev/api.json";
/// Só provedores de primeira mão: uma revenda com preço zero casaria o mesmo id e zeraria a sessão.
pub const PROVIDERS: [&str; 8] = ["anthropic", "openai", "moonshotai", "zhipuai", "deepseek", "google", "xai", "mistral"];
const TTL: Duration = Duration::from_secs(24 * 3600);
const TIMEOUT: Duration = Duration::from_secs(30);
const CACHE: &str = "models.dev.json";
/// O models.dev responde 403 a quem não diz quem é.
const USER_AGENT: &str = "hangar/pricing";
/// O catálogo inteiro tem uns 3 MB; acima disto a resposta não é ele.
pub const MAX_BODY: usize = 32 << 20;

/// Catálogo cru do models.dev para o mapa enxuto modelo -> tarifa, como o `slim()` do Python. `Err`:
/// formato que o Python também recusava (lista no lugar de objeto, número que não é número).
pub fn slim(raw: &Value) -> Result<Map<String, Value>, String> {
    let raw = raw.as_object().ok_or("catálogo não é um objeto")?;
    let mut out = Map::new();
    for provider in PROVIDERS {
        let Some(Value::Object(entry)) = raw.get(provider) else { continue };
        let models = match entry.get("models") {
            Some(Value::Object(models)) => models,
            Some(value) if truthy(value) => return Err(format!("models de {provider} não é um objeto")),
            _ => continue,
        };
        for (model, data) in models {
            let Some(Value::Object(cost)) = data.as_object().and_then(|data| data.get("cost")) else { continue };
            // Sem input ou output a entrada não serve: cache e "equivalente cobrado" partem dos dois.
            let (Some(input), Some(output)) = (cost.get("input").filter(|v| truthy(v)), cost.get("output").filter(|v| truthy(v))) else { continue };
            let optional = |key: &str| match cost.get(key) {
                None | Some(Value::Null) => Ok(Value::Null),
                Some(value) => number(value),
            };
            let rate = json!({"provider": provider, "input": number(input)?, "output": number(output)?,
                "cache_read": optional("cache_read")?, "cache_write": optional("cache_write")?});
            out.entry(model.clone()).or_insert_with(|| rate.clone());
            out.entry(format!("{provider}/{model}")).or_insert(rate);
        }
    }
    Ok(out)
}

/// A veracidade do Python (`not x`).
fn truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64() != Some(0.0),
        Value::String(s) => !s.is_empty(),
        Value::Array(a) => !a.is_empty(),
        Value::Object(o) => !o.is_empty(),
    }
}

/// `float(x)` do Python; o que ele recusaria (ou um não finito, que o JSON não grava) é formato
/// desconhecido.
fn number(value: &Value) -> Result<Value, String> {
    let parsed = match value {
        Value::Number(n) => n.as_f64(),
        Value::Bool(b) => Some(f64::from(u8::from(*b))),
        Value::String(s) => super::py::normalized_number(s).and_then(|s| s.parse().ok()),
        _ => None,
    };
    parsed.and_then(serde_json::Number::from_f64).map(Value::Number).ok_or_else(|| format!("preço inválido: {value}"))
}

/// Uma tentativa por processo a cada 24 h, também quando falha: máquina sem rede não paga o prazo a
/// cada chamada.
pub struct PricingRefresh {
    dir: PathBuf,
    url: String,
    attempted: Mutex<Option<SystemTime>>,
    max_body: usize,
}

impl PricingRefresh {
    pub fn new(dir: PathBuf, url: impl Into<String>) -> Self {
        Self { dir, url: url.into(), attempted: Mutex::new(None), max_body: MAX_BODY }
    }

    /// Teto da resposta, para provar o corte sem servir 32 MB.
    #[doc(hidden)]
    pub fn with_max_body(mut self, max_body: usize) -> Self {
        self.max_body = max_body;
        self
    }

    /// A tarefa única da subida; `offline` não baixa nada.
    pub fn start(dir: PathBuf, url: &str, offline: bool) -> Option<tokio::task::JoinHandle<()>> {
        if offline { return None; }
        let refresh = Self::new(dir, url);
        Some(tokio::spawn(async move { refresh.run().await }))
    }

    /// Só `CP_PRICING_OFFLINE=1` exato desliga: máquina sem saída para a internet fica no snapshot.
    pub fn offline() -> bool {
        Self::offline_value(std::env::var_os("CP_PRICING_OFFLINE").as_deref())
    }

    pub fn offline_value(value: Option<&std::ffi::OsStr>) -> bool {
        value.is_some_and(|value| value == "1")
    }

    pub async fn run(&self) {
        let target = self.dir.join(CACHE);
        let now = SystemTime::now();
        let modified = tokio::fs::metadata(&target).await.and_then(|m| m.modified()).ok();
        {
            let mut attempted = self.attempted.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            if attempted.is_some_and(|at| now.duration_since(at).unwrap_or_default() < TTL) {
                return;
            }
            if modified.is_some_and(|at| now.duration_since(at).unwrap_or_default() < TTL) {
                return;
            }
            *attempted = Some(now);
        }
        let raw = match self.download(modified).await {
            Download::Fresh(raw) => raw,
            Download::NotModified => {
                let path = target.clone();
                if let Err(error) = blocking(move || touch(&path)).await { tracing::warn!("tarifas: não deu para marcar o cache como conferido: {error}"); }
                return;
            }
            Download::Refused(status) => return tracing::warn!("tarifas: models.dev respondeu {status}"),
            Download::Failed(error) => return tracing::warn!("tarifas: falha ao baixar models.dev: {error}"),
        };
        let models = match slim(&raw) {
            Ok(models) => models,
            // O snapshot usa o mesmo leitor: o que salva aqui é deixar o cache anterior de pé.
            Err(error) => return tracing::warn!("tarifas: formato do models.dev não reconhecido: {error}"),
        };
        // Sem nenhum modelo conhecido, gravar deixaria os custos sem preço até o dia seguinte (D13).
        if models.is_empty() {
            return tracing::warn!("tarifas: models.dev sem modelo conhecido; tabela anterior mantida");
        }
        let count = models.len();
        let text = hangar_core::pycompat::pyjson::dumps_unicode(&json!({"modelos": models}), false);
        if let Err(error) = blocking(move || write_atomic(&target, text.as_bytes())).await {
            return tracing::warn!("tarifas: não deu para gravar o cache: {error}");
        }
        tracing::info!("tarifas: {count} modelos atualizados do models.dev");
    }

    async fn download(&self, modified: Option<SystemTime>) -> Download {
        let client = match hangar_core::net::http_client().timeout(TIMEOUT).build() {
            Ok(client) => client,
            Err(error) => return Download::Failed(error.to_string()),
        };
        let mut request = client.get(&self.url).header(reqwest::header::USER_AGENT, USER_AGENT);
        // Com cache, o caso normal vira 304 em vez de 3 MB.
        if let Some(modified) = modified {
            let since = chrono::DateTime::<chrono::Utc>::from(modified).format("%a, %d %b %Y %H:%M:%S GMT").to_string();
            request = request.header(reqwest::header::IF_MODIFIED_SINCE, since);
        }
        let response = match request.send().await {
            Ok(response) => response,
            Err(error) => return Download::Failed(error.to_string()),
        };
        match response.status().as_u16() {
            304 => return Download::NotModified,
            status if !(200..300).contains(&status) => return Download::Refused(status),
            _ => {}
        }
        match read_capped(response, self.max_body).await {
            Ok(body) => serde_json::from_slice(&body).map_or_else(|error| Download::Failed(error.to_string()), Download::Fresh),
            Err(error) => Download::Failed(error),
        }
    }
}

/// O corpo inteiro, recusado ao passar de `max` bytes, antes de juntar tudo na memória.
async fn read_capped(mut response: reqwest::Response, max: usize) -> Result<Vec<u8>, String> {
    let too_big = || format!("resposta maior que {max} bytes");
    if response.content_length().is_some_and(|length| length > max as u64) { return Err(too_big()); }
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|error| error.to_string())? {
        if body.len() + chunk.len() > max { return Err(too_big()); }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

/// IO de arquivo fora das tarefas do runtime.
async fn blocking<T: Send + 'static>(work: impl FnOnce() -> std::io::Result<T> + Send + 'static) -> std::io::Result<T> {
    tokio::task::spawn_blocking(work).await.unwrap_or_else(|error| Err(std::io::Error::other(error)))
}

enum Download {
    Fresh(Value),
    NotModified,
    Refused(u16),
    Failed(String),
}

fn touch(path: &Path) -> std::io::Result<()> {
    std::fs::File::options().write(true).open(path)?.set_modified(SystemTime::now())
}

fn write_atomic(path: &Path, data: &[u8]) -> std::io::Result<()> {
    if let Some(parent) = path.parent() { std::fs::create_dir_all(parent)?; }
    let tmp = path.with_extension(format!("{}.tmp", std::process::id()));
    std::fs::write(&tmp, data)?;
    std::fs::rename(&tmp, path).inspect_err(|_| { let _ = std::fs::remove_file(&tmp); })
}

/// Modelos citados pelo nome nos testes: o corte por posição poderia deixá-los de fora.
const ANCHORS: [(&str, &str); 3] = [("anthropic", "claude-opus-5"), ("moonshotai", "kimi-k3"), ("openai", "gpt-5.6-sol")];

/// O recorte versionado do catálogo para os testes: seis modelos de cada provedor (mais a âncora) e a
/// armadilha de preço zero da primeira revenda que a tiver.
pub fn excerpt(raw: &Value) -> Result<Map<String, Value>, String> {
    let raw = raw.as_object().ok_or("catálogo não é um objeto")?;
    let models_of = |entry: &Map<String, Value>| -> Result<Map<String, Value>, String> {
        match entry.get("models") {
            Some(Value::Object(models)) => Ok(models.clone()),
            Some(value) if truthy(value) => Err("models não é um objeto".into()),
            _ => Ok(Map::new()),
        }
    };
    let name_of = |id: &str, entry: &Map<String, Value>| entry.get("name").cloned().unwrap_or_else(|| json!(id));
    let mut out = Map::new();
    for provider in PROVIDERS {
        let Some(Value::Object(entry)) = raw.get(provider) else { continue };
        let all = models_of(entry)?;
        let mut models: Map<String, Value> = all.iter().take(6).map(|(k, v)| (k.clone(), v.clone())).collect();
        if let Some((_, anchor)) = ANCHORS.iter().find(|(p, _)| *p == provider)
            && !models.contains_key(*anchor) && let Some(model) = all.get(*anchor) {
            models.insert((*anchor).to_owned(), model.clone());
        }
        out.insert(provider.to_owned(), json!({"name": name_of(provider, entry), "models": models}));
    }
    for (id, entry) in raw {
        let Value::Object(entry) = entry else { continue };
        if PROVIDERS.contains(&id.as_str()) { continue; }
        let mut trap = Map::new();
        for (model, data) in models_of(entry)? {
            let Value::Object(data) = data else { continue };
            let input = match data.get("cost") {
                Some(Value::Object(cost)) => cost.get("input").is_some_and(truthy),
                Some(value) if truthy(value) => return Err(format!("cost de {id}/{model} não é um objeto")),
                _ => false,
            };
            if !input { trap.insert(model, Value::Object(data)); }
        }
        if !trap.is_empty() {
            let trap: Map<String, Value> = trap.into_iter().take(2).collect();
            out.insert(id.clone(), json!({"name": name_of(id, entry), "models": trap}));
            break;
        }
    }
    Ok(out)
}

/// `json.dumps(..., indent=1, sort_keys=True, ensure_ascii=False) + "\n"`, o formato dos dois arquivos
/// versionados.
pub fn versioned_text(value: &Value) -> String {
    fn sorted(value: &Value) -> Value {
        match value {
            Value::Object(map) => {
                let mut keys: Vec<_> = map.keys().collect();
                keys.sort();
                Value::Object(keys.into_iter().map(|key| (key.clone(), sorted(&map[key]))).collect())
            }
            Value::Array(items) => Value::Array(items.iter().map(sorted).collect()),
            other => other.clone(),
        }
    }
    hangar_core::pycompat::pyjson::dumps_indent(&sorted(value), 1, false) + "\n"
}

/// Baixa o catálogo e regrava o snapshot (`backend/app/pricing_data.json`) e o recorte dos testes a
/// partir da raiz do repositório. Falha antes de gravar qualquer um.
pub async fn write_snapshot(root: &Path, url: &str) -> Result<Vec<String>, String> {
    let client = hangar_core::net::http_client().timeout(TIMEOUT).build().map_err(|e| e.to_string())?;
    let response = client.get(url).header(reqwest::header::USER_AGENT, "hangar/fetch_model_prices").send().await.map_err(|e| e.to_string())?;
    let status = response.status();
    if !status.is_success() { return Err(format!("models.dev respondeu {}", status.as_u16())); }
    let body = response.bytes().await.map_err(|e| e.to_string())?;
    let raw: Value = serde_json::from_slice(&body).map_err(|e| format!("JSON inválido: {e}"))?;
    let models = slim(&raw)?;
    let excerpt = excerpt(&raw)?;
    let snapshot = root.join("backend/app/pricing_data.json");
    let fixture = root.join("crates/hangar-server/tests/it/fixtures/pricing/models_dev_recorte.json");
    let generated = chrono::Utc::now().format("%Y-%m-%dT%H:%M:%S+00:00").to_string();
    let count = models.len();
    for (path, text) in [(&snapshot, versioned_text(&json!({"gerado_em": generated, "modelos": models}))),
        (&fixture, versioned_text(&Value::Object(excerpt)))] {
        if let Some(parent) = path.parent() { std::fs::create_dir_all(parent).map_err(|e| e.to_string())?; }
        std::fs::write(path, text).map_err(|e| format!("{}: {e}", path.display()))?;
    }
    Ok(vec![format!("{count} entradas -> {}", snapshot.display()), format!("recorte -> {}", fixture.display())])
}

/// `hangar-server pricing-snapshot [raiz]`: a raiz do repositório, ou a pasta atual.
pub fn cli(args: &[String]) -> std::process::ExitCode {
    let root = args.first().map_or_else(|| PathBuf::from("."), PathBuf::from);
    let runtime = match tokio::runtime::Builder::new_current_thread().enable_all().build() {
        Ok(runtime) => runtime,
        Err(error) => { eprintln!("pricing-snapshot: {error}"); return std::process::ExitCode::FAILURE; }
    };
    match runtime.block_on(write_snapshot(&root, URL)) {
        Ok(lines) => { for line in lines { println!("{line}"); } std::process::ExitCode::SUCCESS }
        Err(error) => { eprintln!("pricing-snapshot: {error}"); std::process::ExitCode::FAILURE }
    }
}
