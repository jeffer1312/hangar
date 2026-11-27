//! `~/.hangar/telemetry.json`: a escolha da pessoa e as identidades. Só este crate grava nele.
use std::io;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use hangar_core::ids::random_hex;

use crate::wire::{is_day, is_id};

/// Escopo 2 é o aviso do retrato de preferências, não o da opção B.
pub const CONSENT_SCOPE: u32 = 3;

/// Período contínuo de coleta, vinculado ao diário e à fila sem ir ao receptor.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Collection {
    pub id: String,
    pub since: DateTime<Utc>,
    #[serde(default)]
    pub(crate) sealed_through: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct State {
    // Um binário antigo só enxerga este campo; o aceite de B nunca o liga.
    #[serde(rename = "consent")]
    pub(crate) legacy: Option<bool>,
    pub(crate) machine_id: String,
    pub(crate) owner_id: String,
    pub(crate) owner_created: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) option_b: Option<Acceptance>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) collection: Option<Collection>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Acceptance {
    pub(crate) scope: u32,
    pub(crate) at: DateTime<Utc>,
}

impl State {
    pub(crate) fn consent(&self) -> Option<bool> {
        if self.option_b.is_some() { Some(true) } else { self.legacy.filter(|consent| !consent) }
    }
}

#[derive(Default, Deserialize)]
struct Stored {
    consent: Option<bool>,
    machine_id: Option<String>,
    owner_id: Option<String>,
    owner_created: Option<i64>,
    // Campos inválidos não devem transformar um 'não' em ausência de resposta.
    option_b: Option<serde_json::Value>,
    collection: Option<serde_json::Value>,
}

fn path(home: &Path) -> PathBuf { home.join(".hangar").join("telemetry.json") }

pub(crate) fn load(home: &Path, now: DateTime<Utc>) -> io::Result<(State, Option<io::Error>)> {
    let (stored, recovered) = match hangar_core::fs::read_replaced(&path(home)) {
        Ok(bytes) => match serde_json::from_slice::<Stored>(&bytes) {
            Ok(stored) => (stored, None),
            Err(e) => (Stored::default(), Some(io::Error::new(io::ErrorKind::InvalidData, e))),
        },
        Err(e) if e.kind() == io::ErrorKind::NotFound => (Stored::default(), None),
        Err(e) => return Err(e),
    };
    let machine_id = stored.machine_id.filter(|id| is_id(id));
    let owner = stored.owner_id.filter(|id| is_id(id)).zip(stored.owner_created);
    let complete = machine_id.is_some() && owner.is_some();
    let (owner_id, owner_created) = owner.unwrap_or_else(|| (random_hex(16), now.timestamp()));
    let option_b: Option<Acceptance> = stored.option_b.and_then(|raw| serde_json::from_value(raw).ok())
        .filter(|choice: &Acceptance| complete && stored.consent == Some(false) && choice.scope == CONSENT_SCOPE && choice.at.timestamp() > 0 && choice.at <= now);
    let collection = stored.collection.and_then(|raw| serde_json::from_value(raw).ok()).filter(|period: &Collection| {
        option_b.as_ref().is_some_and(|choice| choice.at <= period.since) && period.since <= now && is_id(&period.id)
            && period.sealed_through.as_deref().is_none_or(is_day)
    });
    let state = State {
        legacy: stored.consent, machine_id: machine_id.unwrap_or_else(|| random_hex(16)), owner_id, owner_created, option_b, collection,
    };
    if !complete { store(home, &state)?; }
    Ok((state, recovered))
}

pub(crate) fn store(home: &Path, state: &State) -> io::Result<()> {
    let path = path(home);
    std::fs::create_dir_all(path.parent().expect("pasta do arquivo"))?;
    hangar_core::fs::atomic_write(&path, &serde_json::to_vec_pretty(state)?)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn telemetry_option_b_needs_the_exact_scope_and_a_valid_acceptance_time() {
        let home = tempfile::tempdir().unwrap();
        let now = "2026-11-27T20:00:00Z".parse::<DateTime<Utc>>().unwrap();
        let (saved, _) = load(home.path(), now).unwrap();
        let mut input = serde_json::to_value(saved).unwrap();
        for (legacy, b, expected) in [
            (true, json!(null), None),
            (false, json!(null), Some(false)),
            (false, json!({"scope": 2, "at": now}), Some(false)),
            (false, json!({"scope": 3}), Some(false)),
            (false, json!({"scope": 3, "at": "2030-01-01T00:00:00Z"}), Some(false)),
            (false, json!({"scope": 3, "at": now}), Some(true)),
        ] {
            input["consent"] = json!(legacy);
            input["option_b"] = b;
            std::fs::write(path(home.path()), input.to_string()).unwrap();
            assert_eq!(load(home.path(), now).unwrap().0.consent(), expected);
        }
    }
}
