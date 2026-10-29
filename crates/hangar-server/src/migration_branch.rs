//! Branch do processo, com nova tentativa limitada quando o checkout não pode ser lido.
use std::{collections::BTreeMap,path::Path,sync::Mutex,time::Duration};

#[derive(Default)]
pub struct Branch {
    value: Option<String>,
    failed_at: Option<Duration>,
}
impl Branch {
    pub fn resolve_with(&mut self, now: Duration, manifest: Option<&str>, mut git: impl FnMut(&[&str], Duration) -> Result<(i32, String), String>) -> Option<String> {
        if self.value.is_some() || self.failed_at.is_some_and(|at|now.saturating_sub(at)<Duration::from_secs(60)) {
            return self.value.clone();
        }
        if let Some(channel)=manifest {
            self.value=Some(channel.to_owned());
            return self.value.clone();
        }
        match git(&["rev-parse","--abbrev-ref","HEAD"],Duration::from_secs(5)) {
            Ok((0,stdout))=>{
                let branch=crate::transcript::py::strip(&stdout);
                self.value=(!branch.is_empty()).then(||branch.to_owned());
            }
            _=>{self.failed_at=Some(now);tracing::warn!(code="migration_branch_unavailable","não foi possível ler a branch instalada");}
        }
        self.value.clone()
    }
}

#[derive(Default)]
pub struct Service { branch:Mutex<Branch> }
impl Service {
    pub fn get(&self,root:&Path,environment:&BTreeMap<String,String>,now:Duration)->Option<String> {
        let manifest=std::fs::read(root.join("manifest.json")).ok()
            .and_then(|bytes|serde_json::from_slice::<serde_json::Value>(&bytes).ok())
            .filter(|value|["version","commit","channel"].iter().all(|key|value.get(key).and_then(serde_json::Value::as_str).is_some_and(|text|!text.is_empty())));
        self.branch.lock().unwrap_or_else(std::sync::PoisonError::into_inner).resolve_with(now,manifest.as_ref().and_then(|value|value["channel"].as_str()),|args,timeout|{
            let mut command=vec!["git".to_owned()];command.extend(args.iter().map(|value|(*value).to_owned()));
            crate::update::process::run_capture_with_env(&command,root,timeout,environment)
                .map(|output|(output.code,output.stdout)).map_err(|error|error.to_string())
        })
    }
}
