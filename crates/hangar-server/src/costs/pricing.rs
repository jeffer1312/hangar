//! Catálogo e ajustes locais; consultas repetidas compartilham a mesma geração de tarifas.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::SystemTime;

use indexmap::IndexMap;
use serde_json::{Map, Value};

const SNAPSHOT: &str = include_str!("../../data/pricing_data.json");
/// Onde o snapshot embutido mora, relativo à raiz do repositório (`pricing-snapshot` o regrava).
pub const SNAPSHOT_PATH: &str = "crates/hangar-server/data/pricing_data.json";
const PREFIXES: [&str; 13] = [
    "anthropic/", "openai/", "moonshot/", "moonshotai/", "deepseek/", "cline-pass/",
    "clinepass/", "openrouter/", "zhipuai/", "google/", "cx/", "apikey/", "kimi-code/",
];
pub const IGNORADOS: [&str; 4] = ["<synthetic>", "unknown", "mock-engine-1", ""];

#[derive(Clone, Debug, PartialEq)]
pub struct Rate {
    pub input: f64,
    pub output: f64,
    pub cache_read: f64,
    pub cache_write: f64,
    pub provider: String,
    pub origin: String,
    pub cache_estimado: bool,
}

#[derive(Default)]
struct Memos {
    canonical: HashMap<String, String>,
    rates: HashMap<String, Option<Rate>>,
}

pub struct Pricing {
    dir: PathBuf,
    catalog: IndexMap<String, Rate>,
    overrides: Map<String, Value>,
    lowercase: HashMap<String, String>,
    mtimes: [Option<SystemTime>; 2],
    generation: u64,
    memos: Mutex<Memos>,
}

impl Pricing {
    pub fn load(dir: &Path) -> Self {
        let mut pricing = Self {
            dir: dir.to_owned(), catalog: IndexMap::new(), overrides: Map::new(),
            lowercase: HashMap::new(), mtimes: [None; 2], generation: 0,
            memos: Mutex::new(Memos::default()),
        };
        pricing.reload();
        pricing
    }

    pub fn generation(&self) -> u64 {
        self.generation
    }

    pub fn reload_if_changed(&mut self) -> bool {
        if self.mtimes == self.file_mtimes() {
            return false;
        }
        self.reload();
        true
    }

    fn file_mtimes(&self) -> [Option<SystemTime>; 2] {
        ["models.dev.json", "overrides.json"].map(|name| {
            std::fs::metadata(self.dir.join(name)).ok().and_then(|m| m.modified().ok())
        })
    }

    fn reload(&mut self) {
        self.mtimes = self.file_mtimes();
        let cached = read_object(&self.dir.join("models.dev.json"));
        let cached_models = cached.as_ref().and_then(|v| v.get("modelos")).and_then(Value::as_object);
        let snapshot: Value = serde_json::from_str(SNAPSHOT).expect("snapshot de tarifas válido");
        let (models, origin) = match cached_models {
            Some(models) => (models, "models.dev"),
            None => (snapshot["modelos"].as_object().expect("modelos do snapshot"), "snapshot"),
        };
        self.catalog = models.iter().filter_map(|(model, value)| {
            parse_rate(value.as_object()?, origin).map(|rate| (model.clone(), rate))
        }).collect();
        self.lowercase.clear();
        for model in self.catalog.keys() {
            self.lowercase.entry(model.to_lowercase()).or_insert_with(|| model.clone());
        }
        self.overrides = read_object(&self.dir.join("overrides.json")).unwrap_or_default();
        *self.memos.get_mut().unwrap() = Memos::default();
        self.generation += 1;
    }

    pub fn canonizar(&self, model: &str) -> String {
        if let Some(canonical) = self.memos.lock().unwrap().canonical.get(model).cloned() {
            return canonical;
        }
        let raw = crate::transcript::py::strip(model);
        let mut base = raw;
        while let Some(prefix) = PREFIXES.iter().find(|p| base.starts_with(**p)) {
            base = &base[prefix.len()..];
        }
        let canonical = if self.catalog.contains_key(base) {
            base.to_owned()
        } else if self.catalog.contains_key(raw) {
            raw.to_owned()
        } else if let Some(alias) = model_alias(base) {
            alias.to_owned()
        } else {
            let lower = base.to_lowercase();
            self.lowercase.get(&lower).cloned()
                .or_else(|| model_alias(&lower).map(str::to_owned)).unwrap_or_else(|| base.to_owned())
        };
        self.memos.lock().unwrap().canonical.insert(model.to_owned(), canonical.clone());
        canonical
    }

    pub fn rate_for(&self, model: &str) -> Option<Rate> {
        if let Some(rate) = self.memos.lock().unwrap().rates.get(model).cloned() {
            return rate;
        }
        let rate = if IGNORADOS.contains(&crate::transcript::py::strip(model)) {
            None
        } else {
            let canonical = self.canonizar(model);
            self.overrides.get(&canonical).and_then(Value::as_object)
                .and_then(|v| parse_rate(v, "override"))
                .or_else(|| self.catalog.get(&canonical).cloned())
        };
        self.memos.lock().unwrap().rates.insert(model.to_owned(), rate.clone());
        rate
    }

    pub fn provider_for(&self, model: &str) -> Option<String> {
        self.rate_for(model).map(|r| r.provider)
    }

    pub fn rate_fast(&self, rate: &Rate, model: &str) -> Rate {
        if rate.origin == "override" || rate.provider != "anthropic"
            || !matches!(self.canonizar(model).as_str(), "claude-opus-5" | "claude-opus-4-8") {
            return rate.clone();
        }
        scaled_rate(rate, 2.0, 2.0)
    }

    pub fn rate_codex(&self, rate: &Rate, model: &str, long: bool) -> Rate {
        if !long || rate.origin == "override" || rate.provider != "openai"
            || !matches!(self.canonizar(model).as_str(), "gpt-6-astra" | "gpt-6-sol" | "gpt-6-luna"
                | "gpt-5.6-sol" | "gpt-5.6-terra" | "gpt-5.6-luna") {
            return rate.clone();
        }
        scaled_rate(rate, 2.0, 1.5)
    }
}

fn model_alias(model: &str) -> Option<&'static str> {
    match model {
        "k3" | "k3-256k" | "kimi-for-coding" => Some("kimi-k3"),
        "gpt-5.6-sol-high" => Some("gpt-5.6-sol"),
        "claude-sonnet-4" => Some("claude-sonnet-4-5-20250929"),
        "claude-haiku-4.5" => Some("claude-haiku-4-5-20251001"),
        "deepseek-v4-flash-0731" => Some("deepseek-v4-flash"),
        _ => None,
    }
}

fn read_object(path: &Path) -> Option<Map<String, Value>> {
    let raw = std::fs::read(path).ok()?;
    match serde_json::from_slice(&raw).ok()? {
        Value::Object(obj) => Some(obj),
        _ => None,
    }
}

fn float_value(value: &Value) -> Option<f64> {
    match value {
        Value::Number(n) => n.as_f64(),
        Value::String(s) => super::py::normalized_number(s)?.parse().ok(),
        Value::Bool(b) => Some(f64::from(u8::from(*b))),
        _ => None,
    }
}

fn parse_rate(data: &Map<String, Value>, origin: &str) -> Option<Rate> {
    let input = float_value(data.get("input")?)?;
    let output = float_value(data.get("output")?)?;
    let cache_read = data.get("cache_read").filter(|v| !v.is_null());
    let cache_write = data.get("cache_write").filter(|v| !v.is_null());
    let provider = data.get("provider").and_then(Value::as_str).unwrap_or("?");
    let free_write = cache_write.is_none() && provider == "openai";
    Some(Rate {
        input, output,
        cache_read: match cache_read { Some(v) => float_value(v)?, None => input },
        cache_write: match cache_write { Some(v) => float_value(v)?, None if free_write => 0.0, None => input },
        provider: provider.to_owned(), origin: origin.to_owned(),
        cache_estimado: cache_read.is_none() || (cache_write.is_none() && !free_write),
    })
}

fn scaled_rate(rate: &Rate, input_factor: f64, output_factor: f64) -> Rate {
    Rate {
        input: rate.input * input_factor, output: rate.output * output_factor,
        cache_read: rate.cache_read * input_factor, cache_write: rate.cache_write * input_factor,
        ..rate.clone()
    }
}

pub fn custo(rate: &Rate, input: i64, output: i64, cache_write: i64, cache_read: i64) -> [f64; 4] {
    [input as f64 / 1e6 * rate.input, output as f64 / 1e6 * rate.output,
        cache_write as f64 / 1e6 * rate.cache_write, cache_read as f64 / 1e6 * rate.cache_read]
}

pub fn canonizar_provedor(provider: &str) -> String {
    let provider = crate::transcript::py::strip(provider);
    match provider {
        "openai-codex" => "openai",
        "kimi-coding" | "moonshot" => "moonshotai",
        _ => provider,
    }.to_owned()
}

/// Janela de contexto do modelo pelo arquivo de janelas do models.dev, gravado junto das tarifas. O esforço que o
/// Antigravity põe no id (`-low`…) não muda a janela. `None`: sem cache ou modelo fora dele.
pub fn context_window(dir: &Path, model: &str) -> Option<u64> {
    type Windows = (PathBuf, Option<SystemTime>, HashMap<String, u64>);
    static MEMO: Mutex<Option<Windows>> = Mutex::new(None);
    let key = |id: &str| id.rsplit('/').next().unwrap_or(id).to_lowercase().replace('.', "-");
    let file = dir.join(super::pricing_refresh::WINDOWS);
    let mtime = std::fs::metadata(&file).ok().and_then(|meta| meta.modified().ok());
    let mut memo = MEMO.lock().unwrap_or_else(|e| e.into_inner());
    if !memo.as_ref().is_some_and(|(path, seen, _)| *path == file && *seen == mtime) {
        let windows = read_object(&file).map(|models| models.iter()
            .filter_map(|(id, window)| Some((key(id), window.as_u64().filter(|n| *n > 0)?))).collect())
            .unwrap_or_default();
        *memo = Some((file, mtime, windows));
    }
    let windows = &memo.as_ref()?.2;
    let model = key(model);
    let base = ["-low", "-medium", "-high", "-xhigh"].iter().find_map(|suffix| model.strip_suffix(suffix)).unwrap_or(&model);
    windows.get(&model).or_else(|| windows.get(base)).copied()
}

pub fn default_dir() -> PathBuf {
    PathBuf::from(std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")).unwrap_or_default())
        .join(".claude").join(".hangar-pricing")
}
