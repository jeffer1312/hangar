//! A fila da vida adotada conserva a projeção anterior somente na primeira abertura.
use super::*;
use super::super::{queue::{Store,State,Lease,acquire_lease_within}};

pub async fn open_queue(target:&OtherTarget)->Result<QueueActor,RuntimeError> {
    let location=Location::from(target);
    let (store,lease)=open(location).await?;
    Ok(QueueActor::start(store,lease))
}
#[derive(Clone)]
pub(super) struct Location {
    pub key:String,pub generation:u64,pub name:String,pub created:f64,
    pub state:PathBuf,pub lease:PathBuf,pub projection:PathBuf,
}
impl From<&OtherTarget> for Location {
    fn from(target:&OtherTarget)->Self {Self {key:target.key.clone(),generation:target.generation,name:target.name.clone(),created:target.created,
        state:target.state_path.clone(),lease:target.lease_path.clone(),projection:target.projection_dir.clone()}}
}
fn legacy(location:&Location)->Result<Vec<Value>,RuntimeError> {
    let path=super::super::queue::projection_path(&location.projection,&location.name);
    let bytes=match super::super::queue::read_replaced(&path) {
        Ok(bytes)=>bytes,Err(error) if error.kind()==std::io::ErrorKind::NotFound=>return Ok(vec![]),Err(_)=>return Err(failed("queue_legacy_unreadable")),
    };
    let text=std::str::from_utf8(&bytes).map_err(|_|failed("queue_legacy_invalid"))?;
    let mut rows=Vec::new();
    for line in text.lines().filter(|line|!line.trim().is_empty()) {
        let row:Value=serde_json::from_str(line).map_err(|_|failed("queue_legacy_invalid"))?;
        if !row.is_object() || row["id"].as_str().is_none_or(str::is_empty) || !row["text"].is_string() {return Err(failed("queue_legacy_invalid"));}
        rows.push(row);
    }
    Ok(rows)
}
pub(super) async fn open(location:Location)->Result<(Store,Arc<Lease>),RuntimeError> {
    tokio::task::spawn_blocking(move || {
        if !super::super::codex_terminal::valid_key(&location.key) || location.generation==0 || !location.created.is_finite() {return Err(failed("runtime_binding"));}
        let lease=acquire_lease_within(&location.lease,Duration::from_secs(3)).map_err(|_|failed("runtime_lease"))?;
        let rows=match super::super::queue::read_replaced(&location.state) {
            Ok(bytes)=>{
                let (state,_)=State::load(&bytes).map_err(|_|failed("queue_io"))?;
                if state.owner_key!=location.key || state.generation!=location.generation {return Err(failed("runtime_generation"));}
                Vec::new()
            },
            Err(error) if error.kind()==std::io::ErrorKind::NotFound=>legacy(&location)?,
            Err(_)=>return Err(failed("queue_io")),
        };
        let mut store=Store::open(&location.state,&location.projection,State::new(&location.key,location.generation,&location.name,rows)).map_err(|_|failed("queue_io"))?;
        let clock=clock();let scope=format!("other-adopt:{}:{}",std::process::id(),clock.epoch_s);
        store.exec(location.generation,&format!("{scope}:recover"),clock,Action::Recover).map_err(|_|failed("queue_io"))?;
        store.exec(location.generation,&format!("{scope}:prune"),clock,Action::Prune {min_ts:location.created}).map_err(|_|failed("queue_io"))?;
        store.ensure_projection().map_err(|_|failed("queue_io"))?;
        Ok((store,lease))
    }).await.map_err(|_|failed("queue_worker"))?
}
