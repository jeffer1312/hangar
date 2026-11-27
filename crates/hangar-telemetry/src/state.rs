//! `~/.hangar/telemetry.json`: a escolha da pessoa e as identidades. Só este crate grava nele.
use std::io;
use std::path::{Path, PathBuf};

use hangar_core::ids::random_hex;
use serde::{Deserialize, Serialize};

use crate::wire::is_id;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct State {
    /// `None`: a pessoa ainda não respondeu, e nada é contado.
    pub(crate) consent: Option<bool>,
    pub(crate) machine_id: String,
    pub(crate) owner_id: String,
    /// Segundos Unix: entre as máquinas do dono, vence o dono criado primeiro.
    pub(crate) owner_created: i64,
}

#[derive(Default, Deserialize)]
struct Stored {
    consent: Option<bool>,
    machine_id: Option<String>,
    owner_id: Option<String>,
    owner_created: Option<i64>,
}

fn path(home: &Path) -> PathBuf { home.join(".hangar").join("telemetry.json") }

/// Lê o estado; id ausente ou fora do formato nasce aqui e é gravado na hora. Arquivo ilegível
/// recomeça sem resposta, que só faz a pergunta voltar; o erro volta junto para o diário.
pub(crate) fn load(home: &Path, now: i64) -> io::Result<(State, Option<io::Error>)> {
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
    let (owner_id, owner_created) = owner.unwrap_or_else(|| (random_hex(16), now));
    let state = State { consent: stored.consent, machine_id: machine_id.unwrap_or_else(|| random_hex(16)), owner_id, owner_created };
    if !complete {
        store(home, &state)?;
    }
    Ok((state, recovered))
}

pub(crate) fn store(home: &Path, state: &State) -> io::Result<()> {
    let path = path(home);
    std::fs::create_dir_all(path.parent().expect("pasta do arquivo"))?;
    hangar_core::fs::atomic_write(&path, &serde_json::to_vec_pretty(state)?)
}
