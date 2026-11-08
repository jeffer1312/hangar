//! Posse das sessões Pi/omp/Kimi, sem consultar o backend anterior.
use super::*;
use super::super::other_terminal::{OtherOptions,OtherDiscovery,OtherTarget,OtherHandle,OtherOpening,OtherDiscoveryState};

pub(super) struct OtherConfiguration { pub options:OtherOptions, pub discovery:OtherDiscovery, pub opening:Mutex<BTreeMap<String,OtherOpening>> }
impl RuntimeRegistry {
    pub fn bind_other(self:&Arc<Self>,options:OtherOptions,discovery:OtherDiscovery)->Result<(),RuntimeError> {
        self.bind_lifecycle();
        self.other.set(OtherConfiguration {options,discovery,opening:Mutex::new(BTreeMap::new())}).map_err(|_|failure("other_runtime_already_bound"))
    }
    pub async fn open_other_terminal(&self,target:OtherTarget)->Result<Value,RuntimeError> {
        if !matches!(target.provider,hangar_transcript::Provider::Pi|hangar_transcript::Provider::Omp|hangar_transcript::Provider::Kimi)
            || target.generation==0 || target.binding.generation!=target.generation || target.binding.name!=target.name
            || target.binding.conversation.is_empty() || target.binding.pane.is_empty() || target.pane_address.is_empty()
            || target.agent_pid<=0 || target.pane_pid<=0 || !target.agent_birth.is_finite() || target.agent_birth<=0.0 {
            return Err(failure("runtime_binding"));
        }
        let _name=crate::session_open::open::NameClaim::take(&target.dirs.home,&target.name).map_err(|_|failure("other_terminal_adoption_busy"))?;
        let barrier=self.barrier(&target.key).await;let _guard=barrier.lock().await;
        self.open_other_terminal_locked(target).await
    }
    pub(super) async fn open_other_terminal_locked(&self,target:OtherTarget)->Result<Value,RuntimeError> {
        let configuration=self.other.get().ok_or_else(||failure("other_runtime_unavailable"))?;
        let io=configuration.options.io_override.clone().unwrap_or_else(||Arc::new(super::super::other_terminal::ProcessOtherIo::new(&target,&configuration.options)));
        if !io.prove(&target).await? {return Err(failure("other_terminal_life_changed"));}
        let existing={let entries=self.entries.lock().await;
            if entries.iter().any(|(key,entry)|entry.name==target.name && *key!=target.key) {return Err(failure("runtime_binding"));}
            entries.get(&target.key).map(|entry|(entry.generation,entry.handle.clone()))};
        let handle=match existing {
            Some((generation,EntryHandle::OtherTerminal {target:old,handle})) if generation==target.generation && old.same_life(&target)=>handle,
            Some(_)=>return Err(failure("runtime_generation")),
            None=>{
                configuration.options.interactions.bind(&crate::interactions::InteractionBinding {key:target.key.clone(),generation:target.generation,conversation:target.binding.conversation.clone()}).map_err(crate::interactions::runtime_error)?;
                let queue=super::super::other_terminal::open_queue(&target).await?;
                let live=self.live_sender(&target.name);
                let handle=OtherHandle::spawn(target.clone(),queue,configuration.options.clone(),live.clone());
                self.entries.lock().await.insert(target.key.clone(),Entry {generation:target.generation,
                    handle:EntryHandle::OtherTerminal {target:Box::new(target.clone()),handle:handle.clone()},lease_path:target.lease_path.clone(),name:target.name.clone(),mods_life:0,
                    provider:target.provider.as_str().into(),account_key:None,relaunch:None,settings_owner:None,
                    mode_source:crate::terminal_facts::ModeSource {binding:crate::interactions::InteractionBinding {key:target.key.clone(),generation:target.generation,conversation:target.binding.conversation.clone()},transcript:target.transcript.clone(),confirmed:None},
                    live,runtime_target:None,cano_birth:None,auto_respawn:None,respawn_inflight:false});
                handle
            }
        };
        let snapshot=handle.snapshot().await?;
        if let Some(code)=snapshot["error"].as_str().filter(|code|!code.is_empty()) {return Err(failure(code));}
        Ok(json!({"opened":true,"instance":self.instance,"key":target.key,"generation":target.generation,"state":snapshot}))
    }
    pub(super) async fn adopt_other(&self,name:&str)->Option<Result<(),RuntimeError>> {
        let configuration=self.other.get()?;
        let found=(configuration.discovery)(name.into()).await?;
        Some(match found {
            Err(error)=>Err(error),
            Ok(OtherDiscoveryState::Ready(target)) if target.name==name=> {
                let result=self.open_other_terminal(target).await.map(|_|());
                if result.is_ok() {configuration.opening.lock().await.remove(name);}
                result
            },
            Ok(OtherDiscoveryState::Opening(opening)) if opening.spec.name==name && opening.binding.name==name
                && matches!(opening.spec.provider,crate::session_open::AgentProvider::Pi|crate::session_open::AgentProvider::Omp|crate::session_open::AgentProvider::Kimi)
                && opening.spec.surface==crate::session_open::Surface::Terminal && opening.binding.generation>0=> {
                configuration.opening.lock().await.insert(name.into(),opening);
                Err(failure("other_terminal_starting"))
            },
            _=>Err(failure("runtime_binding")),
        })
    }
    pub async fn other_info(&self,name:&str)->Option<crate::transcript::InternalInfo> {
        let target={let entries=self.entries.lock().await;let mut found=entries.values().filter(|entry|entry.name==name);
            match (found.next(),found.next()) {(Some(Entry {handle:EntryHandle::OtherTerminal {target,..},..}),None)=>Some(target.clone()),(None,None)=>None,_=>return None}};
        let Some(target)=target else {
            let opening=self.other.get()?.opening.lock().await.get(name)?.clone();
            return Some(crate::transcript::InternalInfo {provider:opening.spec.provider.name().into(),jsonl:None,session_key:String::new(),history:json!({"queue":super::super::queue::projection_path(&opening.projection_dir,name)}),headless:false,lifecycle_id:Some(opening.lifecycle_id()),cwd:Some(opening.spec.cwd.into())});
        };
        Some(crate::transcript::InternalInfo {provider:target.provider.as_str().into(),jsonl:Some(target.transcript.clone()),session_key:target.binding.conversation.clone(),
            history:json!({"queue":super::super::queue::projection_path(&target.projection_dir,name)}),headless:false,lifecycle_id:Some(target.lifecycle_id()),cwd:Some(target.cwd.clone().into())})
    }
}
