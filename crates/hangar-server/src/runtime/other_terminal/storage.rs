//! A fila e a vida são gravadas sob a mesma lease antes de publicar a projeção.
use super::super::queue::{Lease, State, Store, acquire_lease_within};
use super::*;
pub(in crate::runtime) fn life(target: &OtherTarget) -> Value {
    json!({"pane":target.binding.pane,"created":target.binding.created,"agent_pid":target.agent_pid,"birth":target.agent_birth,"conversation":target.binding.conversation,"provider":target.provider.as_str()})
}
pub(in crate::runtime) fn desired_generation(target: &OtherTarget) -> Result<u64, RuntimeError> {
    let bytes = match super::super::queue::read_replaced(&target.state_path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(target.generation),
        Err(_) => return Err(failed("queue_io")),
    };
    let (state, _) = State::load(&bytes).map_err(|_| failed("queue_io"))?;
    if state.owner_key != target.key {
        return Err(failed("runtime_generation"));
    }
    let old = &state.runtime_state["other_terminal_life"];
    let next = life(target);
    if old.is_null()
        || *old == next
        || old["created"] == next["created"]
            && old["pane"] == next["pane"]
            && old["conversation"].as_str().is_none_or(str::is_empty)
    {
        Ok(state.generation)
    } else {
        state
            .generation
            .checked_add(1)
            .ok_or_else(|| failed("runtime_generation"))
    }
}
pub async fn open_queue(target: &OtherTarget) -> Result<QueueActor, RuntimeError> {
    let (store, lease) = open(Location::from(target)).await?;
    Ok(QueueActor::start(store, lease))
}
#[derive(Clone)]
pub(in crate::runtime) struct Location {
    pub key: String,
    pub generation: u64,
    pub name: String,
    pub created: f64,
    pub state: PathBuf,
    pub lease: PathBuf,
    pub projection: PathBuf,
    pub life: Option<Value>,
}
impl From<&OtherTarget> for Location {
    fn from(t: &OtherTarget) -> Self {
        Self {
            key: t.key.clone(),
            generation: t.generation,
            name: t.name.clone(),
            created: t.created,
            state: t.state_path.clone(),
            lease: t.lease_path.clone(),
            projection: t.projection_dir.clone(),
            life: Some(life(t)),
        }
    }
}
impl From<&OtherOpening> for Location {
    fn from(t: &OtherOpening) -> Self {
        let (state, lease) =
            super::super::codex_terminal::queue_files(&t.projection_dir, &t.spec.key);
        Self {
            key: t.spec.key.clone(),
            generation: t.binding.generation,
            name: t.spec.name.clone(),
            created: t.binding.created as f64,
            state,
            lease,
            projection: t.projection_dir.clone(),
            life: Some(
                json!({"pane":t.binding.pane,"created":t.binding.created,"conversation":"","provider":t.spec.provider.name()}),
            ),
        }
    }
}
fn legacy(location: &Location) -> Result<Vec<Value>, RuntimeError> {
    let bytes = match super::super::queue::read_replaced(&super::super::queue::projection_path(
        &location.projection,
        &location.name,
    )) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(vec![]),
        Err(_) => return Err(failed("queue_legacy_unreadable")),
    };
    let text = std::str::from_utf8(&bytes).map_err(|_| failed("queue_legacy_invalid"))?;
    let mut rows = Vec::new();
    for line in text.lines().filter(|line| !line.trim().is_empty()) {
        let row: Value = serde_json::from_str(line).map_err(|_| failed("queue_legacy_invalid"))?;
        if !row.is_object()
            || row["id"].as_str().is_none_or(str::is_empty)
            || !row["text"].is_string()
        {
            return Err(failed("queue_legacy_invalid"));
        }
        rows.push(row);
    }
    Ok(rows)
}
pub(in crate::runtime) async fn open(
    location: Location,
) -> Result<(Store, Arc<Lease>), RuntimeError> {
    tokio::task::spawn_blocking(move || {
        if !super::super::codex_terminal::valid_key(&location.key)
            || location.generation == 0
            || !location.created.is_finite()
        {
            return Err(failed("runtime_binding"));
        }
        let lease = acquire_lease_within(&location.lease, Duration::from_secs(3))
            .map_err(|_| failed("runtime_lease"))?;
        let rows = match super::super::queue::read_replaced(&location.state) {
            Ok(bytes) => {
                let (state, _) = State::load(&bytes).map_err(|_| failed("queue_io"))?;
                if state.owner_key != location.key
                    || !(state.generation == location.generation
                        || state.generation.checked_add(1) == Some(location.generation))
                {
                    return Err(failed("runtime_generation"));
                }
                Vec::new()
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => legacy(&location)?,
            Err(_) => return Err(failed("queue_io")),
        };
        let mut store = Store::open(
            &location.state,
            &location.projection,
            State::new(&location.key, location.generation, &location.name, rows),
        )
        .map_err(|_| failed("queue_io"))?;
        let now = clock();
        let scope = format!("other-adopt:{}:{}", std::process::id(), now.epoch_s);
        if let Some(life) = location.life {
            let old = store.state().runtime_state["other_terminal_life"].clone();
            let changed = !old.is_null()
                && old["conversation"].as_str().is_some_and(|sid| {
                    !sid.is_empty() && Some(sid) != life["conversation"].as_str()
                });
            if changed {
                store
                    .exec(
                        store.state().generation,
                        &format!("{scope}:clear"),
                        now,
                        Action::Clear,
                    )
                    .map_err(|_| failed("queue_io"))?;
            }
            if old != life
                || store.state().generation != location.generation
                || store.state().name != location.name
            {
                store
                    .exec(
                        store.state().generation,
                        &format!("{scope}:life"),
                        now,
                        Action::AdvanceLifecycle {
                            generation: location.generation,
                            state: json!({"name":location.name,"other_terminal_life":life}),
                        },
                    )
                    .map_err(|_| failed("queue_io"))?;
            }
        }
        let generation = store.state().generation;
        store
            .exec(
                generation,
                &format!("{scope}:recover"),
                now,
                Action::Recover,
            )
            .map_err(|_| failed("queue_io"))?;
        store
            .exec(
                generation,
                &format!("{scope}:prune"),
                now,
                Action::Prune {
                    min_ts: location.created,
                },
            )
            .map_err(|_| failed("queue_io"))?;
        store.ensure_projection().map_err(|_| failed("queue_io"))?;
        Ok((store, lease))
    })
    .await
    .map_err(|_| failed("queue_worker"))?
}
