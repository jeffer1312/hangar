//! Arquivos de grupo em `.hangar-pair`. O sidecar é o formato de hoje (Python, hook e lista leem);
//! `fed` só existe em grupo entre máquinas e é ignorado por quem não o conhece.
use crate::list::discover_other::truthy;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use std::collections::BTreeMap;

pub const PROTOCOL: u32 = 1;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Fed { pub owner: String, pub local: bool, pub version: u64 }

#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct Sidecar {
    pub peers: Vec<String>,
    /// `None` = `null` no arquivo; a lista mostra `pair_task: null` nesse caso, como o Python.
    pub task: Option<String>,
    pub gid: String,
    pub harness: BTreeMap<String, String>,
    pub orq: bool,
    pub fed: Option<Fed>,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Member { pub server: String, pub name: String, #[serde(default)] pub provider: String }

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GroupRecord {
    pub protocol: u32,
    pub gid: String,
    pub owner: String,
    pub version: u64,
    #[serde(default)]
    pub task: String,
    pub members: Vec<Member>,
}

impl Sidecar {
    /// `PairLink.get` (pair.py:75-94), regra movida de `links.rs::pair_of` sem mudança.
    pub fn parse(name: &str, data: &Map<String, Value>) -> Option<Sidecar> {
        let raw = match data.get("peers") {
            Some(p) => p.clone(),
            None => data.get("peer").filter(|p| truthy(p)).map(|p| Value::Array(vec![p.clone()])).unwrap_or(Value::Null),
        };
        let peers: Vec<String> = raw.as_array().map(|a| a.iter().filter_map(Value::as_str).filter(|p| !p.is_empty()).map(str::to_owned).collect()).unwrap_or_default();
        let orq = data.get("orq") == Some(&Value::Bool(true));
        if peers.is_empty() && !orq {
            return None;
        }
        let task = match data.get("task") { None => Some(String::new()), Some(t) => t.as_str().map(str::to_owned) };
        let gid = data.get("gid").and_then(Value::as_str).filter(|g| !g.is_empty()).map(str::to_owned).unwrap_or_else(|| legacy_gid(name, &peers));
        let harness = data.get("harness").and_then(Value::as_object)
            .map(|h| h.iter().filter_map(|(k, v)| v.as_str().map(|p| (k.clone(), p.to_owned()))).collect())
            .unwrap_or_default();
        // `fed` torto = ausente; quem lê pelo `PairDir` registra no diário.
        let fed = data.get("fed").and_then(|f| serde_json::from_value::<Fed>(f.clone()).ok());
        Some(Sidecar { peers, task, gid, harness, orq, fed })
    }

    /// Mesmo corpo que o `PairLink.set` grava (pair.py:96-108).
    pub fn to_json(&self, name: &str) -> Value {
        let inside: Vec<&str> = std::iter::once(name).chain(self.peers.iter().map(String::as_str)).collect();
        let harness: Map<String, Value> = self.harness.iter()
            .filter(|(k, _)| inside.contains(&k.as_str()))
            .map(|(k, p)| (k.clone(), Value::String(p.clone()))).collect();
        let mut body = json!({"peers": self.peers, "task": self.task, "gid": self.gid, "harness": harness});
        if self.orq { body["orq"] = json!(true); }
        if let Some(fed) = &self.fed { body["fed"] = serde_json::to_value(fed).expect("Fed serializa"); }
        body
    }
}

/// `_gid_legado` do pair.py:55, movido de `links.rs`.
pub fn legacy_gid(name: &str, peers: &[String]) -> String {
    let mut all: Vec<&str> = std::iter::once(name).chain(peers.iter().map(String::as_str)).collect();
    all.sort_unstable();
    sha1_smol::Sha1::from(all.join("\n")).digest().to_string()[..8].to_owned()
}
