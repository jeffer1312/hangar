//! Versão do código carregado, congelada antes de servir pedidos.
use std::{collections::{BTreeMap,HashMap},path::{Path,PathBuf},sync::{Arc,LazyLock,Mutex,OnceLock},time::Duration};
use serde_json::Value;

#[derive(Clone,Debug,PartialEq,Eq)]
pub struct Snapshot{pub describe:String,pub readable:Option<String>}
static SNAPSHOTS:LazyLock<Mutex<HashMap<PathBuf,Arc<OnceLock<Snapshot>>>>>=LazyLock::new(Default::default);
fn slot(root:&Path)->Arc<OnceLock<Snapshot>>{
    SNAPSHOTS.lock().unwrap_or_else(std::sync::PoisonError::into_inner).entry(root.to_owned()).or_default().clone()
}
pub fn get(root:&Path)->Option<Snapshot>{
    let slot=SNAPSHOTS.lock().unwrap_or_else(std::sync::PoisonError::into_inner).get(root).cloned()?;
    slot.get().cloned()
}
/// A primeira publicação vence; a raiz permite injetar uma versão isolada em provas.
pub fn publish(root:&Path,snapshot:Snapshot)->Snapshot{slot(root).get_or_init(||snapshot).clone()}
pub fn capture(root:&Path,environment:&BTreeMap<String,String>)->Snapshot{
    capture_with(root,|args,timeout|{
        let mut command=vec!["git".to_owned()];command.extend(args.iter().map(|v|(*v).to_owned()));
        crate::update::process::run_with_env(&command,root,timeout,environment,&|_|{}).map(|output|output.stdout).map_err(|_|())
    })
}
/// A fonte injetada conserva os argumentos e prazos do subprocesso, sem exigir git nas provas.
#[doc(hidden)]
pub fn capture_with(root:&Path,mut git:impl FnMut(&[&str],Duration)->Result<String,()>)->Snapshot{
    slot(root).get_or_init(||{
        let manifest=std::fs::read(root.join("manifest.json")).ok().and_then(|b|serde_json::from_slice::<Value>(&b).ok()).filter(|v|["version","commit","channel"].iter().all(|k|v.get(k).and_then(Value::as_str).is_some_and(|s|!s.is_empty())));
        if let Some(manifest)=manifest{return Snapshot{describe:format!("{}-g{}",manifest["version"].as_str().unwrap(),manifest["commit"].as_str().unwrap().chars().take(12).collect::<String>()),readable:Some(manifest["version"].as_str().unwrap().into())};}
        let describe=git(&["describe","--tags","--match","dist-latest","--long","--always","--dirty"],Duration::from_secs(5)).ok().map(|s|crate::transcript::py::strip(&s).to_owned()).filter(|s|!s.is_empty()).unwrap_or_else(||"indisponivel".into());
        let readable=(||{
            let count=git(&["rev-list","--count","HEAD"],Duration::from_secs(5)).ok().map(|s|crate::transcript::py::strip(&s).to_owned())?;
            if count.is_empty()||!count.chars().all(crate::accounts::claude_config::py_isdigit){return None;}
            let mut version=git(&["show","HEAD:VERSION"],Duration::from_secs(5)).ok().map(|s|crate::transcript::py::strip(&s).to_owned())?;
            if version.is_empty(){version=std::fs::read_to_string(root.join("VERSION")).ok().map(|s|crate::transcript::py::strip(&s).to_owned())?;}
            (!version.is_empty()).then(||format!("{version}.{count}"))
        })();
        Snapshot{describe,readable}
    }).clone()
}
