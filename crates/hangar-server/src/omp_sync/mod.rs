//! Reconciliação conservadora de catálogos, plugins e contexto do omp, sem passar pelo Python.
mod context;
mod directories;
mod io;
mod model;
mod service;
mod sync;

pub use context::{DISABLED_CONTEXT_IDS, configure as configure_claude_context};
pub use directories::agent_directory;
use futures_util::future::BoxFuture;
pub use io::NativeRunner;
use serde_json::{Map, Value, json};
pub use service::{Service, matches, serve, validate_environment};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};
pub use sync::Synchronizer;

pub type Stop<'a> = dyn Fn() -> bool + Send + Sync + 'a;
#[derive(Clone, Debug)]
pub struct Output {
    pub code: i64,
    pub stdout: String,
    pub stderr: String,
}
/// Só a fronteira do processo é substituída na prova diferencial.
pub trait Runner: Send + Sync {
    fn run<'a>(
        &'a self,
        args: Vec<String>,
        cwd: &'a Path,
        env: &'a BTreeMap<String, String>,
        timeout: Duration,
    ) -> BoxFuture<'a, Result<Output, String>>;
}
pub fn parse_record(raw: &str) -> Result<Value, String> {
    io::parse(raw)
}
fn err(s: &str) -> String {
    s.into()
}
fn report(dry: bool) -> Value {
    json!({"complete_inventory":false,"items":[],"errors":[],"mode":if dry{"read_only_local"}else{"native"}})
}
fn push_error(report: &mut Value, error: String) {
    report["errors"].as_array_mut().unwrap().push(json!(error));
}
fn truth(v: Option<&Value>) -> bool {
    match v {
        None | Some(Value::Null) => false,
        Some(Value::Bool(b)) => *b,
        Some(Value::String(s)) => !s.is_empty(),
        Some(Value::Array(v)) => !v.is_empty(),
        Some(Value::Object(v)) => !v.is_empty(),
        Some(Value::Number(v)) => v.as_f64() != Some(0.0),
    }
}
#[cfg(test)]
mod tests;

#[cfg(test)]
mod real_runtime;
