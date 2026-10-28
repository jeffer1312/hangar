use super::{Engine,Rpc,error};
use super::super::protocol::{Disposition,Effect,OperationKind,RuntimeError};
use hangar_codex::proto::{self as wire,ClientRequest};
use serde::{Deserialize,Serialize};
use serde_json::{Value,json};
use std::sync::{Arc,Mutex,Weak,atomic::{AtomicU64,Ordering}};

pub(super) const UNAVAILABLE:&str = "erro_subagente_mensagem_codex_indisponivel";
const STALE:&str = "erro_subagente_mensagem_vinculo_desatualizado";

#[derive(Default)]
pub(super) struct FamilyEpoch {
    current:AtomicU64,
    waiting:Mutex<Vec<Weak<futures_util::task::AtomicWaker>>>,
}

impl FamilyEpoch {
    fn invalidate(&self) {
        self.current.fetch_add(1,Ordering::AcqRel);
        for waiter in self.waiting.lock().unwrap_or_else(|error|error.into_inner()).drain(..).filter_map(|waiter|waiter.upgrade()) {
            waiter.wake();
        }
    }

    fn guard(self:&Arc<Self>) -> hangar_codex::client::RawWriteGuard {
        let expected=self.current.load(Ordering::Acquire);
        let waiter=Arc::new(futures_util::task::AtomicWaker::new());
        let mut waiting=self.waiting.lock().unwrap_or_else(|error|error.into_inner());
        waiting.retain(|waiter|waiter.strong_count()>0);
        waiting.push(Arc::downgrade(&waiter));
        let epoch=self.clone();
        Arc::new(move |waker| {
            waiter.register(waker);
            if epoch.current.load(Ordering::Acquire)==expected {Ok(())} else {Err(STALE.into())}
        })
    }
}

#[derive(Clone,Default,Deserialize,Serialize)]
pub(super) struct Agent {
    parent:String,
    path:Option<String>,
    state:Option<String>,
    end_time:Option<i64>,
    turn_id:Option<String>,
    direct_input:Option<bool>,
    hydrated:bool,
    revision:u64,
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::super::protocol::{ClockSample,EngineInput,RuntimeCommand};

    fn clock() -> ClockSample { ClockSample {monotonic_s:1.0,epoch_s:1000.0} }
    fn engine() -> Engine {
        Engine::new(json!({"name":"pai","thread_id":"root","cwd":"/work","headless":false,
            "connected":true,"subscribed":true,"initialized":true}),1,clock())
    }
    fn notify(engine:&mut Engine,method:&str,params:Value) -> Vec<Effect> {
        engine.apply(EngineInput::Line(json!({"method":method,"params":params})),clock()).unwrap()
    }
    fn child(engine:&mut Engine,direct:bool) {
        notify(engine,"thread/started",json!({"thread":{"id":"child","parentThreadId":"root",
            "canAcceptDirectInput":direct,"status":{"type":"active"}}}));
    }
    fn command(engine:&mut Engine,kind:OperationKind) -> Vec<Effect> {
        engine.command(RuntimeCommand {operation_id:"message".into(),kind,
            payload:json!({"parent_thread_id":"root","agent_id":"child","text":"continue"})},clock()).unwrap()
    }
    fn frame(effects:&[Effect]) -> &Value {
        effects.iter().find_map(|effect|match effect {Effect::Write {frame,..}=>Some(frame),_=>None}).unwrap()
    }
    fn answer(engine:&mut Engine,effects:&[Effect],result:Value) -> Vec<Effect> {
        engine.apply(EngineInput::Line(json!({"id":frame(effects)["id"],"result":result})),clock()).unwrap()
    }

    #[test]
    fn spawn_acknowledgement_keeps_child_running_and_child_turn_does_not_replace_parent() {
        let mut engine=engine();
        notify(&mut engine,"item/completed",json!({"threadId":"root","item":{"type":"collabAgentToolCall",
            "tool":"spawnAgent","status":"completed","senderThreadId":"root","receiverThreadIds":["child"],
            "agentsStates":{"child":{"status":"running"}}}}));
        assert_eq!(engine.subagent_views()["child"]["state"],"running");
        notify(&mut engine,"turn/started",json!({"threadId":"child","turn":{"id":"child-turn","status":"inProgress"}}));
        let effects=notify(&mut engine,"item/agentMessage/delta",json!({"threadId":"child","turnId":"child-turn","delta":"texto da filha"}));
        assert!(!effects.iter().any(|effect|matches!(effect,Effect::Publish {channel,..} if channel=="preview")));
        assert_eq!(engine.control_view()["thread_id"],"root");
        assert_eq!(engine.control_view()["turn_id"],Value::Null);
        notify(&mut engine,"turn/completed",json!({"threadId":"child","turn":{"id":"child-turn","status":"completed"}}));
        assert_eq!(engine.subagent_views()["child"]["state"],"completed");
        assert_eq!(engine.shown_state().codex_subagents.unwrap()["child"].state.as_deref(), Some("finished"));
    }

    #[test]
    fn published_agents_are_normalized_and_empty_is_an_explicit_snapshot() {
        let mut engine = engine();
        assert_eq!(engine.shown_state().codex_subagents.unwrap().len(), 0);
        child(&mut engine, false);
        let agent = engine.subagents.get_mut("child").unwrap();
        agent.path = Some("/root/review".into());
        agent.state = Some("stopped".into());
        agent.end_time = Some(1000500);
        let view = engine.published_subagents();
        assert_eq!(view["child"].state.as_deref(), Some("failed"));
        assert_eq!(view["child"].ended_at, Some(1000.5));
        assert_eq!(view["child"].agent_path.as_deref(), Some("/root/review"));
        engine.alive = false;
        assert_eq!(engine.published_subagents()["child"].state, None);
    }

    #[test]
    fn unrelated_thread_and_previous_parent_cannot_supply_agent_state() {
        let mut engine=engine();
        notify(&mut engine,"thread/started",json!({"thread":{"id":"stranger","parentThreadId":"other","status":{"type":"active"}}}));
        assert!(engine.subagent_views().as_object().unwrap().is_empty());
        child(&mut engine,true);
        notify(&mut engine,"thread/started",json!({"thread":{"id":"new-root","source":"cli","cwd":"/work","path":"/work/new.jsonl"}}));
        assert_eq!(engine.control_view()["thread_id"],"new-root");
        assert!(engine.subagent_views().as_object().unwrap().is_empty());
    }

    #[test]
    fn direct_message_rechecks_child_and_waits_for_rpc_acceptance() {
        let mut engine=engine();child(&mut engine,true);
        let read=command(&mut engine,OperationKind::SubagentMessage);
        assert_eq!(frame(&read)["method"],"thread/read");
        let write=answer(&mut engine,&read,json!({"thread":{"id":"child","parentThreadId":"root",
            "canAcceptDirectInput":true,"status":{"type":"active"},"turns":[{"id":"child-turn","status":"inProgress"}]}}));
        assert_eq!(frame(&write)["method"],"turn/steer");
        assert_eq!(frame(&write)["params"]["threadId"],"child");
        assert_eq!(frame(&write)["params"]["expectedTurnId"],"child-turn");
        assert!(!write.iter().any(|effect|matches!(effect,Effect::Reply {payload,..} if payload["delivered"]==true)));
        let done=answer(&mut engine,&write,json!({"turnId":"child-turn"}));
        assert!(done.iter().any(|effect|matches!(effect,Effect::Reply {operation_id,disposition:Disposition::Accepted,payload} if operation_id=="message" && payload["delivered"]==true)));
        assert_eq!(engine.control_view()["turn_id"],Value::Null);
        assert_eq!(engine.control_view()["in_progress"],false);
    }

    #[test]
    fn native_direct_input_refusal_is_exposed_without_writing() {
        let mut engine=engine();child(&mut engine,false);
        assert_eq!(engine.subagent_views()["child"]["can_message"],false);
        let error=engine.command(RuntimeCommand {operation_id:"message".into(),kind:OperationKind::SubagentMessage,
            payload:json!({"parent_thread_id":"root","agent_id":"child","text":"continue"})},clock()).unwrap_err();
        assert_eq!(error.code,UNAVAILABLE);
    }

    #[test]
    fn restored_child_rpcs_cannot_change_the_parent() {
        for (method,result) in [
            ("thread/read",json!({"thread":{"id":"child","status":{"type":"active"},"turns":[{"id":"child-turn","status":"inProgress"}]}})),
            ("thread/resume",json!({"thread":{"id":"child","status":{"type":"active"},"path":"/work/child.jsonl"}})),
            ("turn/start",json!({"turn":{"id":"child-turn","status":"inProgress"}})),
        ] {
            let mut engine=engine();
            let frame=json!({"id":"restored","method":method,"params":{"threadId":"child","includeTurns":true}});
            engine.restore_rpc("recovered".into(),&frame,0,0);
            assert!(!engine.write_is_current("recovered"));
            let effects=engine.apply(EngineInput::Line(json!({"id":"restored","result":result})),clock()).unwrap();
            assert!(effects.is_empty());
            assert_eq!(engine.control_view()["thread_id"],"root");
            assert_eq!(engine.control_view()["turn_id"],Value::Null);
            assert_eq!(engine.control_view()["in_progress"],false);
        }
    }

    #[test]
    fn clear_invalidates_queued_child_write_and_its_transport_guard() {
        let mut engine=engine();child(&mut engine,true);
        let read=command(&mut engine,OperationKind::SubagentMessage);
        let write=answer(&mut engine,&read,json!({"thread":{"id":"child","parentThreadId":"root",
            "canAcceptDirectInput":true,"status":{"type":"active"},"turns":[{"id":"child-turn","status":"inProgress"}]}}));
        let guard=engine.subagent_write_guard("message").unwrap();
        let waker=futures_util::task::noop_waker();
        assert!(engine.write_is_current("message"));
        assert!(guard(&waker).is_ok());
        notify(&mut engine,"thread/started",json!({"thread":{"id":"new-root","source":"cli","cwd":"/work","path":"/work/new.jsonl"}}));
        assert!(!engine.write_is_current("message"));
        assert_eq!(guard(&waker),Err(STALE.into()));
        let effects=answer(&mut engine,&write,json!({"turnId":"child-turn"}));
        assert!(!effects.iter().any(|effect|matches!(effect,Effect::Reply {payload,..} if payload["delivered"]==true)));
        assert_eq!(engine.control_view()["thread_id"],"new-root");
    }

    #[test]
    fn steer_without_matching_turn_confirmation_never_claims_delivery() {
        for result in [json!({}),json!({"turnId":"other-turn"})] {
            let mut engine=engine();child(&mut engine,true);
            let read=command(&mut engine,OperationKind::SubagentMessage);
            let write=answer(&mut engine,&read,json!({"thread":{"id":"child","parentThreadId":"root",
                "canAcceptDirectInput":true,"status":{"type":"active"},"turns":[{"id":"child-turn","status":"inProgress"}]}}));
            let done=answer(&mut engine,&write,result);
            assert!(!done.iter().any(|effect|matches!(effect,Effect::Reply {payload,..} if payload["delivered"]==true)));
            assert!(done.iter().any(|effect|matches!(effect,Effect::Reply {operation_id,disposition:Disposition::Rejected,..} if operation_id=="message")));
        }
    }

    fn snapshot(status:&str) -> Value {
        json!({"turns":[{"items":[{"type":"collabAgentToolCall","tool":"spawnAgent","status":"completed",
            "senderThreadId":"root","receiverThreadIds":["child"],"agentsStates":{"child":{"status":status}}}]}]})
    }

    #[test]
    fn resumed_history_completion_replaces_running_state_from_before_disconnect() {
        let mut engine=engine();child(&mut engine,true);
        assert_eq!(engine.subagents["child"].state.as_deref(),Some("running"));
        engine.transport_lost("queda de ensaio",&std::collections::BTreeSet::new());
        let requested_revision=engine.subagent_revision;
        notify(&mut engine,"thread/status/changed",json!({"threadId":"child","status":{"type":"idle"}}));
        engine.hydrate_agents(&snapshot("completed"),requested_revision);
        assert_eq!(engine.subagents["child"].state.as_deref(),Some("completed"));
    }

    #[test]
    fn native_completion_received_during_resume_wins_over_its_older_snapshot() {
        let mut engine=engine();child(&mut engine,true);
        engine.transport_lost("queda de ensaio",&std::collections::BTreeSet::new());
        let requested_revision=engine.subagent_revision;
        notify(&mut engine,"turn/completed",json!({"threadId":"child","turn":{"id":"child-turn","status":"completed"}}));
        engine.hydrate_agents(&snapshot("running"),requested_revision);
        assert_eq!(engine.subagents["child"].state.as_deref(),Some("completed"));
    }

    #[test]
    fn expired_or_foreign_child_read_cannot_trigger_a_message() {
        let mut engine=engine();
        let read=command(&mut engine,OperationKind::SubagentMessage);
        let effects=answer(&mut engine,&read,json!({"thread":{"id":"child","parentThreadId":"other",
            "canAcceptDirectInput":true,"status":{"type":"idle"}}}));
        assert!(!effects.iter().any(|effect|matches!(effect,Effect::Write {..})));
        assert!(effects.iter().any(|effect|matches!(effect,Effect::Reply {disposition:Disposition::Rejected,payload,..} if payload["code"]==STALE)));
        let read=command(&mut engine,OperationKind::SubagentMessage);
        engine.apply(EngineInput::Tick,ClockSample {monotonic_s:32.0,epoch_s:1031.0}).unwrap();
        let effects=answer(&mut engine,&read,json!({"thread":{"id":"child","parentThreadId":"root",
            "canAcceptDirectInput":true,"status":{"type":"idle"}}}));
        assert!(!effects.iter().any(|effect|matches!(effect,Effect::Write {..})));
    }
}

impl Agent {
    fn status(&mut self,status:&str,now:i64,revision:Option<u64>) {
        let state = match status {
            "pendingInit" | "running" | "active" => "running",
            "completed" => "completed",
            "errored" | "failed" | "systemError" => "failed",
            "shutdown" | "interrupted" | "stopped" => "stopped",
            _ => return,
        };
        if let Some(revision)=revision {self.revision=revision;}
        if state == "running" { self.end_time = None; }
        else if self.state.as_deref() != Some(state) || self.end_time.is_none() { self.end_time = Some(now); }
        self.state = Some(state.into());
    }

    fn view(&self,available:bool) -> Value {
        let can_message = available && self.direct_input == Some(true);
        json!({"state":if available { self.state.as_deref() } else { None },"end_time":self.end_time,
            "can_message":can_message,"message_blocked":(!can_message).then_some(UNAVAILABLE)})
    }
}

impl Engine {
    pub fn subagent_write_guard(&self,id:&str) -> Option<hangar_codex::client::RawWriteGuard> {
        let rpc=self.wires.get(id)?.request_id.as_ref().and_then(|id|self.rpc.get(id))?;
        rpc.continuation.as_ref().filter(|next|next["kind"].as_str().is_some_and(|kind|kind.starts_with("subagent_")))?;
        Some(self.subagent_epoch.guard())
    }

    pub(super) fn cancel_subagent_calls(&mut self,effects:&mut Vec<Effect>) {
        self.subagent_epoch.invalidate();
        let ids:Vec<_>=self.rpc.iter().filter(|(_,rpc)|rpc.continuation.as_ref()
            .is_some_and(|next|next["kind"].as_str().is_some_and(|kind|kind.starts_with("subagent_"))))
            .map(|(id,_)|id.clone()).collect();
        for id in ids {
            let rpc=self.rpc.remove(&id).unwrap();
            if let Some(wire)=self.wires.get_mut(&rpc.operation_id) {wire.final_result=true;}
            let next=rpc.continuation.unwrap();
            let disposition=if next["kind"]=="subagent_message_write" {Disposition::Unknown} else {Disposition::Rejected};
            effects.push(Effect::Reply {operation_id:next["parent"].as_str().unwrap_or(&rpc.operation_id).into(),disposition,
                payload:json!({"code":STALE,"error":"a conversa principal mudou durante a operação do subagente"})});
        }
    }

    fn agent_now(&self) -> i64 { (self.clock.epoch_s * 1000.0) as i64 }

    fn family(&self,id:&str) -> bool { id == self.thread_id || self.subagents.contains_key(id) }

    fn register_agent(&mut self,id:&str,parent:&str,path:Option<&str>) -> bool {
        if id.is_empty() || id == self.thread_id || !self.family(parent) { return false; }
        let agent = self.subagents.entry(id.into()).or_default();
        if !agent.parent.is_empty() && agent.parent != parent { return false; }
        agent.parent = parent.into();
        if let Some(path) = path { agent.path = Some(path.into()); }
        true
    }

    pub(super) fn subagent_views(&self) -> Value {
        let available = self.alive && self.sendable();
        json!(self.subagents.iter().map(|(id,agent)|(id,agent.view(available))).collect::<std::collections::BTreeMap<_,_>>())
    }

    pub(super) fn published_subagents(&self) -> std::collections::BTreeMap<String, hangar_api::state::CodexSubagent> {
        let available = self.alive && self.sendable();
        self.subagents.iter().filter(|(_, agent)| agent.parent == self.thread_id).map(|(id, agent)| {
            let state = available.then_some(agent.state.as_deref()).flatten().and_then(|state| match state {
                "running" => Some("running"),
                "completed" | "finished" => Some("finished"),
                "failed" | "errored" | "stopped" => Some("failed"),
                _ => None,
            }).map(str::to_owned);
            let can_message = available && agent.direct_input == Some(true);
            (id.clone(), hangar_api::state::CodexSubagent { agent_path: agent.path.clone(), state,
                ended_at: agent.end_time.map(|ms| ms as f64 / 1000.), can_message,
                message_blocked: (!can_message).then(|| UNAVAILABLE.to_owned()) })
        }).collect()
    }

    fn observe_agent_item(&mut self,thread:&str,item:&wire::ThreadItem,revision:Option<u64>) {
        if !self.family(thread) { return; }
        let now = self.agent_now();
        match item {
            wire::ThreadItem::CollabAgentToolCall { tool,status,sender_thread_id,receiver_thread_ids,agents_states,.. }
                if sender_thread_id == thread => {
                for id in receiver_thread_ids {
                    if tool == "spawnAgent" { self.register_agent(id,thread,None); }
                    if let Some(agent) = self.subagents.get_mut(id) {
                        if status == "completed" {
                            match tool.as_str() {
                                "spawnAgent" | "sendInput" | "sendMessage" | "followupTask" | "resumeAgent" => agent.status("running",now,revision),
                                "closeAgent" | "interruptAgent" => agent.status("stopped",now,revision),
                                _ => {},
                            }
                        }
                    }
                }
                for (id,state) in agents_states {
                    if let Some(agent) = self.subagents.get_mut(id) {
                        agent.status(&state.status,now,revision);
                    }
                }
            }
            wire::ThreadItem::SubAgentActivity { kind,agent_thread_id,agent_path,.. } => {
                if self.register_agent(agent_thread_id,thread,Some(agent_path)) {
                    let agent = self.subagents.get_mut(agent_thread_id).unwrap();
                    match kind.as_str() {
                        "started" | "interacted" => agent.status("running",now,revision),
                        "interrupted" => agent.status("stopped",now,revision),
                        "completed" => agent.status("completed",now,revision),
                        _ => {},
                    }
                }
            }
            _ => {},
        }
    }

    pub(super) fn hydrate_agents(&mut self,thread:&Value,requested_revision:u64) {
        let observed=std::mem::take(&mut self.subagents);
        let root = self.thread_id.clone();
        for turn in thread["turns"].as_array().into_iter().flatten() {
            for item in turn["items"].as_array().into_iter().flatten() {
                if let Ok(item) = serde_json::from_value::<wire::ThreadItem>(item.clone()) {
                    self.observe_agent_item(&root,&item,None);
                }
            }
        }
        // Só sinais posteriores ao pedido vencem a leitura; o estado anterior à desconexão não a sobrepõe.
        for agent in self.subagents.values_mut() { agent.end_time=None; }
        self.subagents.extend(observed.into_iter().filter(|(_,agent)|agent.revision>requested_revision));
    }

    /// O estado das filhas nunca altera turno, prévia ou pergunta da conversa principal.
    pub(super) fn observe_subagent(&mut self,method:&str,params:&Value) {
        if !matches!(method,"thread/started" | "item/started" | "item/completed" | "turn/started" | "turn/completed" | "thread/status/changed") {return;}
        self.subagent_revision=self.subagent_revision.saturating_add(1);
        let revision=self.subagent_revision;
        let now = self.agent_now();
        if method == "thread/started" {
            let thread = &params["thread"];
            if let (Some(id),Some(parent)) = (thread["id"].as_str(),thread["parentThreadId"].as_str()) {
                if self.register_agent(id,parent,None) {
                    let agent = self.subagents.get_mut(id).unwrap();
                    agent.direct_input = thread["canAcceptDirectInput"].as_bool();
                    if let Some(status) = thread["status"]["type"].as_str() { agent.status(status,now,Some(revision)); }
                }
            }
        }
        let Some(thread) = params["threadId"].as_str().filter(|thread|self.family(thread)) else { return };
        if matches!(method,"item/started" | "item/completed") {
            if let Ok(item) = serde_json::from_value::<wire::ThreadItem>(params["item"].clone()) {
                self.observe_agent_item(thread,&item,Some(revision));
            }
        }
        let Some(agent) = self.subagents.get_mut(thread) else { return };
        match method {
            "turn/started" => {
                agent.turn_id = params["turn"]["id"].as_str().filter(|id|!id.is_empty()).map(Into::into);
                agent.status("running",now,Some(revision));
            }
            "turn/completed" => {
                let id = params["turn"]["id"].as_str();
                if agent.turn_id.as_deref().is_some_and(|current|id != Some(current)) { return; }
                if let Some(status) = params["turn"]["status"].as_str() { agent.status(status,now,Some(revision)); }
                agent.turn_id = None;
            }
            "thread/status/changed" => {
                if let Some(status) = params["status"]["type"].as_str() { agent.status(status,now,Some(revision)); }
            }
            _ => {},
        }
    }

    pub(super) fn subagent_command(&mut self,id:String,kind:OperationKind,payload:&Value,effects:&mut Vec<Effect>) -> Result<(),RuntimeError> {
        let parent = payload["parent_thread_id"].as_str().filter(|parent|*parent == self.thread_id)
            .ok_or_else(||RuntimeError::new(STALE,"a conversa principal mudou"))?;
        let child = payload["agent_id"].as_str().filter(|child|!child.is_empty() && *child != parent)
            .ok_or_else(||error("subagente sem identificação"))?;
        if !self.sendable() { return Err(RuntimeError::new(UNAVAILABLE,"o Codex não está conectado")); }
        let message = kind == OperationKind::SubagentMessage;
        if message && !payload["text"].as_str().is_some_and(|text|!text.trim().is_empty()) { return Err(error("mensagem sem texto")); }
        if let Some(agent) = self.subagents.get(child) {
            if !message && agent.hydrated {
                effects.push(Effect::Reply {operation_id:id,disposition:Disposition::Accepted,payload:agent.view(true)});
                return Ok(());
            }
            if message && agent.direct_input == Some(false) {
                return Err(RuntimeError::new(UNAVAILABLE,"o Codex não permite mensagem direta a este subagente"));
            }
        }
        let next = json!({"kind":if message { "subagent_message_read" } else { "subagent_status" },
            "parent":id,"root":parent,"agent":child,"text":payload["text"]});
        self.send(format!("{id}:read"),ClientRequest::ThreadRead(wire::ThreadReadParams {thread_id:child.into(),include_turns:message}),Some(next),effects);
        Ok(())
    }

    pub(super) fn subagent_reply(&mut self,rpc:&Rpc,line:&Value,effects:&mut Vec<Effect>) {
        let next = rpc.continuation.as_ref().unwrap();
        let parent = next["parent"].as_str().unwrap_or("");
        let child = next["agent"].as_str().unwrap_or("");
        let reject = |code:&str,message:&str,effects:&mut Vec<Effect>| {
            effects.push(Effect::Reply {operation_id:parent.into(),disposition:Disposition::Rejected,payload:json!({"code":code,"error":message})});
        };
        if rpc.operation_id != parent {
            effects.push(Effect::Reply {operation_id:rpc.operation_id.clone(),disposition:if line["error"].is_null() {Disposition::Accepted} else {Disposition::Rejected},payload:json!({})});
        }
        // Uma resposta de leitura tardia não pode disparar uma escrita depois do prazo ou do /clear.
        if rpc.timed_out {
            effects.push(Effect::Reply {operation_id:parent.into(),disposition:Disposition::Unknown,
                payload:json!({"code":UNAVAILABLE,"error":"o pedido do subagente expirou sem confirmação"})});return;
        }
        if next["root"] != self.thread_id || !self.alive {
            if next["kind"] == "subagent_message_write" {
                effects.push(Effect::Reply {operation_id:parent.into(),disposition:Disposition::Unknown,
                    payload:json!({"code":STALE,"error":"a conversa mudou antes de confirmar a mensagem"})});
            } else {reject(STALE,"a conversa mudou antes da confirmação",effects);}
            return;
        }
        if !line["error"].is_null() {
            reject(UNAVAILABLE,line["error"]["message"].as_str().unwrap_or("o Codex recusou a operação do subagente"),effects);return;
        }
        let result = &line["result"];
        if next["kind"] == "subagent_message_write" {
            if rpc.method == "turn/start" && !result["turn"]["id"].as_str().is_some_and(|id|!id.is_empty()) {
                reject(UNAVAILABLE,"o Codex não confirmou o turno do subagente",effects);return;
            }
            if rpc.method=="turn/steer" && (!rpc.params["expectedTurnId"].as_str().is_some_and(|id|!id.is_empty())
                || result["turnId"]!=rpc.params["expectedTurnId"]) {
                reject(UNAVAILABLE,"o Codex não confirmou a orientação no turno do subagente",effects);return;
            }
            let now = self.agent_now();
            if let Some(agent) = self.subagents.get_mut(child) {
                if rpc.method == "turn/start" { agent.turn_id = result["turn"]["id"].as_str().map(Into::into); }
                agent.status("running",now,None);
            }
            effects.push(Effect::Reply {operation_id:parent.into(),disposition:Disposition::Accepted,payload:json!({"delivered":true})});
            return;
        }
        let Ok(thread) = serde_json::from_value::<wire::Thread>(result["thread"].clone()) else {
            reject(UNAVAILABLE,"o Codex devolveu uma conversa inválida",effects);return;
        };
        let Some(owner) = thread.parent_thread_id.as_deref().filter(|owner|self.family(owner)) else {
            reject(STALE,"o subagente não pertence a esta conversa",effects);return;
        };
        if thread.id != child || !self.register_agent(child,owner,None) {
            reject(STALE,"a identificação do subagente mudou",effects);return;
        }
        let now = self.agent_now();
        let agent = self.subagents.get_mut(child).unwrap();
        let ended=agent.end_time;
        agent.hydrated = true; agent.direct_input = thread.can_accept_direct_input;
        agent.turn_id = thread.turns.iter().rev().find(|turn|turn.status == "inProgress").map(|turn|turn.id.clone());
        if thread.status == wire::ThreadStatus::Active { agent.status("running",now,None); }
        else if let Some(turn) = thread.turns.last() { agent.status(&turn.status,now,None); }
        if agent.state.as_deref()!=Some("running") { agent.end_time=ended; }
        if next["kind"] == "subagent_status" {
            effects.push(Effect::Reply {operation_id:parent.into(),disposition:Disposition::Accepted,payload:agent.view(true)});return;
        }
        if agent.direct_input != Some(true) {
            reject(UNAVAILABLE,"o Codex não permite mensagem direta a este subagente",effects);return;
        }
        if thread.status == wire::ThreadStatus::NotLoaded && next["kind"] != "subagent_message_resume" {
            let mut after = next.clone();after["kind"] = json!("subagent_message_resume");
            self.send(format!("{parent}:resume"),ClientRequest::ThreadResume(wire::ThreadResumeParams {thread_id:child.into(),..Default::default()}),
                Some(after),effects);
            return;
        }
        let input = vec![json!({"type":"text","text":next["text"]})];
        let request = match thread.status {
            wire::ThreadStatus::Active => {
                let Some(turn) = agent.turn_id.clone().filter(|id|!id.is_empty()) else {
                    reject(UNAVAILABLE,"o turno ativo do subagente não pôde ser identificado",effects);return;
                };
                ClientRequest::TurnSteer(wire::TurnSteerParams {thread_id:child.into(),expected_turn_id:turn,input})
            }
            wire::ThreadStatus::Idle => ClientRequest::TurnStart(wire::TurnStartParams {thread_id:child.into(),input,..Default::default()}),
            _ => {reject(UNAVAILABLE,"o subagente não está disponível para receber mensagem",effects);return;},
        };
        let mut after = next.clone();after["kind"] = json!("subagent_message_write");
        self.send(parent.into(),request,Some(after),effects);
    }
}
