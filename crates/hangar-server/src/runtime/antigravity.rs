//! Motor do Antigravity sem terminal: o `agy acp` (Agent Client Protocol, JSON-RPC por linha) atrás do cano.
//! O `acp::Session` do crate só decodifica o que o agente manda (atualizações e pedidos); os pedidos do
//! cliente e as respostas são daqui, com id próprio por geração, para a resposta de uma vida nunca casar com a outra.
//! A resposta do `session/prompt` só chega no fim do turno: a mensagem se resolve pela escrita, e o turno pela resposta.
use super::LiveBuffer;
use super::protocol::*;
use crate::interactions::{ExpectedInteraction,InteractionBinding,InteractionError,InteractionKind,InteractionStore,native_target,runtime_error};
use crate::session_open::PermissionMode;
use hangar_antigravity::acp;
use hangar_api::state::StateEvent;
use serde_json::{Value,json};
use std::collections::{BTreeMap,BTreeSet};
use std::sync::Arc;

/// Pedido do cliente sem resposta neste prazo vira incerto (o prompt não tem prazo: dura o turno).
const RPC_LIMIT:f64 = 30.0;
/// O `session/cancel` que não fecha o turno neste prazo: o processo sobe de novo (como o T3 Code).
const CANCEL_LIMIT:f64 = 15.0;

#[derive(Clone,Debug,PartialEq)]
enum Call { Initialize, Authenticate, Open { resumed:bool }, BootMode, BootModel(String), Prompt, SetModel(String), SetMode(PermissionMode) }

struct Rpc { operation_id:Option<String>, parent:Option<String>, call:Call, deadline:Option<f64>, timed_out:bool }

#[derive(Clone,Copy,PartialEq)]
enum WireKind { Prompt { steer:bool }, Answer, Cancel, Rpc }

struct Wire { kind:WireKind, request_id:Option<String>, server_request:Option<RequestId>, server_epoch:Option<u64>, interaction:Option<ExpectedInteraction>, final_result:bool }

fn error(code:&str,message:&str) -> RuntimeError { RuntimeError::new(code,message) }
fn unsupported() -> RuntimeError { error("antigravity_unsupported","o Antigravity sem terminal não atende esta operação") }
fn text(value:&Value) -> Option<String> { value.as_str().filter(|s|!s.is_empty()).map(str::to_owned) }

/// Sufixos de esforço nos ids do `agy models` (`gemini-4-neon-high`): o app manda modelo e esforço separados.
const EFFORTS:[&str;4] = ["low","medium","high","xhigh"];

/// O id do `session/set_model`: o modelo pedido (ou o atual) sem o esforço que tinha, com o esforço pedido.
fn model_id(model:Option<&str>,effort:Option<&str>,current:Option<&str>) -> Option<String> {
    let base = model.or(current)?;
    let Some(effort) = effort else { return Some(base.to_owned()) };
    let bare = EFFORTS.iter().find_map(|e|base.strip_suffix(&format!("-{e}"))).unwrap_or(base);
    Some(format!("{bare}-{effort}"))
}

pub struct Engine {
    interactions:Option<(String,Arc<InteractionStore>)>,
    generation:u64,
    clock:ClockSample,
    counter:u64,
    alive:bool,
    initialized:bool,
    ready:bool,
    booting:Option<String>,
    in_progress:bool,
    turn_id:Option<String>,
    conversation:String,
    cwd:String,
    brain_dir:Option<String>,
    model:Option<String>,
    wanted_model:Option<String>,
    mode:Option<String>,
    permission_mode:PermissionMode,
    models:Vec<acp::Choice>,
    modes:Vec<acp::Choice>,
    tier:Option<String>,
    token_usage:Value,
    state:StateEvent,
    preview:LiveBuffer,
    thinking:LiveBuffer,
    tool:Option<String>,
    rpc:BTreeMap<String,Rpc>,
    server_requests:Vec<(RequestId,Value)>,
    request_epochs:BTreeMap<RequestId,u64>,
    answering:BTreeSet<RequestId>,
    wires:BTreeMap<String,Wire>,
    policies:BTreeMap<RequestId,String>,
    cancel_due:Option<f64>,
    /// A abertura já pediu o modo uma vez nesta vida: recusado, segue sem ele (com nota).
    boot_mode_tried:bool,
    last_event:f64,
    decoder:acp::Session,
}

impl Engine {
    /// Abertura pelo ator: modo gravado desconhecido recusa antes do motor existir.
    pub fn new_checked(metadata:Value,generation:u64,clock:ClockSample) -> Result<Self,RuntimeError> {
        let mode = super::antigravity_launch::stored_mode(&metadata)?;
        Ok(Self::new(metadata,generation,clock,mode))
    }

    fn new(metadata:Value,generation:u64,clock:ClockSample,permission_mode:PermissionMode) -> Self {
        let conversation = metadata["conversation_id"].as_str().filter(|id|hangar_antigravity::paths::valid_conversation_id(id)).unwrap_or("").to_owned();
        Self { interactions:None,generation,clock,counter:metadata["runtime_counter"].as_u64().unwrap_or(0),
            alive:true,initialized:metadata["initialized"] == true,ready:metadata["ready"] == true,booting:None,in_progress:false,turn_id:None,
            cwd:metadata["cwd"].as_str().unwrap_or("").into(),brain_dir:text(&metadata["brain_dir"]),
            model:text(&metadata["current_model"]),wanted_model:text(&metadata["model"]),mode:text(&metadata["mode"]),permission_mode,
            models:Vec::new(),modes:Vec::new(),tier:None,token_usage:Value::Null,
            state:StateEvent { session:metadata["name"].as_str().unwrap_or("").into(),state:"idle".into(),headless:true,..StateEvent::default() },
            preview:LiveBuffer::default(),thinking:LiveBuffer::default(),tool:None,
            rpc:BTreeMap::new(),server_requests:Vec::new(),request_epochs:BTreeMap::new(),answering:BTreeSet::new(),
            wires:BTreeMap::new(),policies:BTreeMap::new(),cancel_due:None,boot_mode_tried:false,last_event:clock.monotonic_s,
            decoder:acp::Session::new(String::new(),None),conversation }
    }

    pub(super) fn conversation(&self) -> &str { &self.conversation }
    pub fn problem(&self) -> Option<&str> { self.state.problema.as_deref() }

    /// O transcript da conversa: só existe depois que o `agy` a abriu.
    fn transcript(&self) -> Option<String> {
        let dir = self.brain_dir.as_deref()?;
        hangar_antigravity::paths::valid_conversation_id(&self.conversation).then(||
            std::path::Path::new(dir).join(&self.conversation).join(".system_generated").join("logs").join("transcript_full.jsonl")
                .to_string_lossy().into_owned())
    }

    fn prompt_open(&self) -> bool { self.rpc.values().any(|rpc|rpc.call == Call::Prompt) }

    /// Um prompt por vez: o agy trata o segundo como orientação do turno, e a fila só manda turno novo.
    pub(super) fn deliverable(&self) -> bool {
        self.alive && self.ready && self.booting.is_none() && !self.in_progress && !self.prompt_open() && self.server_requests.is_empty()
    }

    pub fn park_idle(&self,now:f64) -> Option<f64> {
        let open = !self.alive || self.in_progress || self.booting.is_some() || !self.server_requests.is_empty() || !self.rpc.is_empty();
        (!open).then(||now - self.last_event)
    }

    pub fn next_deadline(&self) -> Option<f64> {
        self.preview.deadline().into_iter().chain(self.thinking.deadline())
            .chain(self.rpc.values().filter(|rpc|!rpc.timed_out).filter_map(|rpc|rpc.deadline))
            .chain(self.cancel_due.filter(|_|self.in_progress)).min_by(f64::total_cmp)
    }

    pub(super) fn input_frame_template(&self) -> Value {
        json!({"jsonrpc":"2.0","id":format!("agy:{}:{}",self.generation,u64::MAX),"method":"session/prompt",
            "params":{"sessionId":self.conversation,"prompt":[]}})
    }

    pub fn view(&self) -> Value {
        let mut state = self.shown_state();
        self.stamp_shown(&mut state);
        serde_json::to_value(state).unwrap_or(Value::Null)
    }

    pub fn control_view(&self) -> Value {
        json!({"alive":self.alive,"initialized":self.initialized,"ready":self.ready,"in_progress":self.in_progress,"iniciando":self.booting.is_some(),
            "headless":true,"connected":false,"conversation_id":self.conversation,"rollout_path":self.transcript(),"turn_id":self.turn_id,
            "current_model":self.model,"mode":self.mode,"permission_mode":self.permission_mode.name(),"tier":self.tier,
            "models":self.models.iter().map(|m|json!({"id":m.id,"name":m.name})).collect::<Vec<_>>(),
            "token_usage":self.token_usage,"runtime_counter":self.counter,"deliverable":self.deliverable(),
            "pending":self.server_requests.iter().map(|(id,request)|json!({"request_id":id,"request":request})).collect::<Vec<_>>()})
    }

    fn shown_state(&self) -> StateEvent {
        let mut state = self.state.clone();
        let asked = self.server_requests.first();
        state.state = if !self.alive { "dead" } else if asked.is_some() { "awaiting_input" } else if self.in_progress { "working" } else { "idle" }.into();
        state.label = self.booting.as_ref().map(|_|"Abrindo a conversa…".into());
        // A pílula de modo lê o campo do Claude, como no Antigravity com terminal.
        state.claude_permission_mode = Some(self.permission_mode.name().into());
        state.question = None; state.options = None;
        if let Some((_,request)) = asked {
            let params = &request["params"];
            let title = params["toolCall"]["title"].as_str().filter(|t|!t.is_empty())
                .map_or_else(||"O Antigravity pede permissão para agir".to_owned(),|title|format!("Permitir: {title}?"));
            let warning = params["options"].as_array().into_iter().flatten().chain(std::iter::once(params))
                .find_map(|o|o["_meta"]["agy.security.warning"].as_str());
            state.question = Some(match warning { Some(warning) => format!("{title}\n{warning}"), None => title });
            state.options = Some(params["options"].as_array().into_iter().flatten()
                .map(|o|o["name"].as_str().or(o["optionId"].as_str()).unwrap_or("?").to_owned()).collect());
        }
        state
    }

    fn binding(&self,key:&str) -> InteractionBinding { InteractionBinding { key:key.into(),conversation:self.conversation.clone(),generation:self.generation } }

    fn stamp_shown(&self,state:&mut StateEvent) {
        if state.options.is_none() { return; }
        let id = (|| -> Result<String,InteractionError> {
            let (key,store) = self.interactions.as_ref().filter(|_|!self.conversation.is_empty()).ok_or(InteractionError::Unavailable)?;
            let (id,request) = self.server_requests.first().ok_or(InteractionError::Unavailable)?;
            let revision = self.request_epochs.get(id).ok_or(InteractionError::Unavailable)?.to_string();
            store.observe_revision(&self.binding(key),&native_target(InteractionKind::Approval,&self.conversation,&json!(id)),Some(request.clone()),Some(revision))?
                .ok_or(InteractionError::Unavailable)
        })();
        match id {
            Ok(id) => state.interaction_id = Some(id),
            Err(error) => { crate::interactions::mark_unidentified(state,error); state.options = None; state.interaction_id = None; }
        }
    }

    pub fn set_interactions(&mut self,key:String,store:Arc<InteractionStore>) -> Result<(),RuntimeError> {
        self.interactions = Some((key,store));
        self.bind_interactions()
    }

    fn bind_interactions(&self) -> Result<(),RuntimeError> {
        if !self.conversation.is_empty() && let Some((key,store)) = &self.interactions { store.bind(&self.binding(key)).map_err(runtime_error)?; }
        Ok(())
    }

    fn expected(&self,payload:&Value) -> Result<ExpectedInteraction,RuntimeError> {
        let supplied = payload["interaction_id"].as_str().filter(|id|!id.is_empty()).ok_or_else(||runtime_error(InteractionError::Missing))?;
        let (key,store) = self.interactions.as_ref().filter(|_|!self.conversation.is_empty()).ok_or_else(||runtime_error(InteractionError::Unavailable))?;
        store.expected(&self.binding(key),Some(supplied)).map_err(runtime_error)
    }

    fn check_request(&self,expected:&ExpectedInteraction,id:&RequestId,request:&Value) -> Result<(),RuntimeError> {
        if self.answering.contains(id) { return Err(runtime_error(InteractionError::Changed)); }
        let revision = self.request_epochs.get(id).ok_or_else(||runtime_error(InteractionError::Unavailable))?.to_string();
        expected.check(&native_target(InteractionKind::Approval,&self.conversation,&json!(id)),request,Some(&revision)).map_err(runtime_error)
    }

    pub fn write_interaction(&self,id:&str) -> Option<&ExpectedInteraction> { self.wires.get(id)?.interaction.as_ref() }

    pub fn interaction_source(&self,expected:&ExpectedInteraction) -> Option<ExpectedInteraction> {
        let native = expected.native_id(None).ok()?;
        let (id,request) = self.server_requests.iter().find(|(id,_)|json!(id) == native)?;
        let (key,_) = self.interactions.as_ref()?;
        Some(ExpectedInteraction { binding:self.binding(key),target:native_target(InteractionKind::Approval,&self.conversation,&native),
            interaction_id:expected.interaction_id.clone(),semantic:request.clone(),source_revision:Some(self.request_epochs.get(id)?.to_string()) })
    }

    pub fn interaction_write_error(&self,id:&str) -> Option<RuntimeError> {
        let expected = self.wires.get(id)?.interaction.as_ref()?;
        let check = || -> Result<(),RuntimeError> {
            if self.expected(&json!({"interaction_id":expected.interaction_id}))? != *expected { return Err(runtime_error(InteractionError::Changed)); }
            let native = expected.native_id(None).map_err(runtime_error)?;
            let (id,request) = self.server_requests.iter().find(|(id,_)|json!(id) == native).ok_or_else(||runtime_error(InteractionError::Changed))?;
            let revision = self.request_epochs.get(id).ok_or_else(||runtime_error(InteractionError::Changed))?.to_string();
            expected.check(&native_target(InteractionKind::Approval,&self.conversation,&native),request,Some(&revision)).map_err(runtime_error)
        };
        check().err()
    }

    pub fn write_is_current(&self,id:&str) -> bool {
        self.wires.get(id).is_none_or(|wire|!wire.final_result
            && wire.server_request.as_ref().is_none_or(|request|self.request_epochs.get(request).copied() == wire.server_epoch))
    }

    pub fn forget_policy(&mut self,request_id:&RequestId) { self.policies.remove(request_id); }

    pub fn confirm_input(&mut self,operation_id:&str) -> Vec<Effect> {
        if let Some(wire) = self.wires.get_mut(operation_id) { wire.final_result = true; }
        vec![Effect::Reply { operation_id:operation_id.into(),disposition:Disposition::Accepted,payload:json!({"confirmed":true}) }]
    }

    pub(super) fn queue_problem(&mut self,detail:Option<String>) -> Vec<Effect> {
        let mut effects = Vec::new();
        if super::queue::queue_problem(&mut self.state,detail) { effects.push(Effect::StateChanged); }
        effects
    }

    pub fn set_problem(&mut self,code:&str,detail:Option<String>) -> Vec<Effect> {
        self.put_problem(code,detail.map(|d|d.chars().take(300).collect()));
        vec![Effect::StateChanged]
    }

    fn put_problem(&mut self,code:&str,detail:Option<String>) {
        if crate::problem_gate::keeps(self.state.problema.as_deref(),code) { return; }
        self.state.problema = Some(code.into());
        self.state.problema_detalhe = detail;
    }

    pub fn clear_problem(&mut self,code:&str) -> Vec<Effect> {
        if self.state.problema.as_deref() != Some(code) { return Vec::new(); }
        self.state.problema = None; self.state.problema_detalhe = None;
        vec![Effect::StateChanged]
    }

    fn policy(&mut self,kind:&str,payload:Value,effects:&mut Vec<Effect>) {
        self.counter += 1;
        let request_id = RequestId::String(format!("policy:{}:{}",self.generation,self.counter));
        self.policies.insert(request_id.clone(),kind.into());
        effects.push(Effect::Policy { kind:kind.into(),request_id,payload });
    }

    fn note(&mut self,text:String,effects:&mut Vec<Effect>) { self.policy("local_output",json!({"text":text}),effects); }

    fn publish(&self,channel:&str,text:String,effects:&mut Vec<Effect>) {
        effects.push(Effect::Publish { channel:channel.into(),data:json!({"session":self.state.session,"text":text,"md":true,"full":true,"vivo":true}) });
    }

    fn clear_live(&mut self,effects:&mut Vec<Effect>) {
        if let Some(text) = self.preview.clear() { self.publish("preview",text,effects); }
        if let Some(text) = self.thinking.clear() { self.publish("thinking",text,effects); }
        if self.tool.take().is_some() { self.publish("tool",String::new(),effects); }
    }

    /// Pedido do cliente com id desta geração; a resposta chega por `reply`.
    fn send(&mut self,operation_id:Option<String>,parent:Option<String>,call:Call,method:&str,params:Value,effects:&mut Vec<Effect>) -> String {
        self.counter += 1;
        let id = format!("agy:{}:{}",self.generation,self.counter);
        let deadline = (call != Call::Prompt).then_some(self.clock.monotonic_s + RPC_LIMIT);
        if let Some(op) = &operation_id {
            let kind = if call == Call::Prompt { WireKind::Prompt { steer:false } } else { WireKind::Rpc };
            self.wires.insert(op.clone(),Wire { kind,request_id:Some(id.clone()),server_request:None,server_epoch:None,interaction:None,final_result:false });
        }
        self.rpc.insert(id.clone(),Rpc { operation_id:operation_id.clone(),parent,call,deadline,timed_out:false });
        effects.push(Effect::Write { frame:json!({"jsonrpc":"2.0","id":id,"method":method,"params":params}),operation_id });
        id
    }

    /// `initialize` → `authenticate` → `session/resume` (conversa conhecida) ou `session/new` → modo e modelo → pronta.
    pub fn bootstrap(&mut self,operation_id:String) -> Result<Vec<Effect>,RuntimeError> {
        let mut effects = Vec::new();
        self.initialized = false; self.ready = false; self.boot_mode_tried = false;
        self.booting = Some(operation_id.clone());
        let params = json!({"protocolVersion":acp::PROTOCOL_VERSION,
            // Com `readTextFile` ligado o agy pede `fs/read_text_file` ao cliente e o turno espera a resposta.
            "clientCapabilities":{"fs":{"readTextFile":false,"writeTextFile":false},"terminal":false},
            "clientInfo":{"name":"hangar","version":env!("CARGO_PKG_VERSION")}});
        self.send(Some(format!("{operation_id}:initialize")),Some(operation_id),Call::Initialize,"initialize",params,&mut effects);
        effects.push(Effect::StateChanged);
        Ok(effects)
    }

    fn boot_failed(&mut self,code:&str,detail:String,effects:&mut Vec<Effect>) {
        tracing::warn!(session=%self.state.session,code,"antigravity: a conversa não abriu");
        self.put_problem(code,Some(detail.chars().take(300).collect()));
        if let Some(parent) = self.booting.take() {
            effects.push(Effect::Reply { operation_id:parent,disposition:Disposition::Rejected,payload:json!({"error":detail}) });
        }
        effects.push(Effect::StateChanged);
    }

    /// O que falta da abertura depois da conversa aberta: o modo e o modelo pedidos, um de cada vez.
    fn boot_next(&mut self,effects:&mut Vec<Effect>) {
        let Some(parent) = self.booting.clone() else { return };
        let wanted_mode = super::antigravity_launch::acp_mode(self.permission_mode);
        // A retomada pode não listar os modos: o pedido vai assim mesmo, e a recusa só vira nota.
        if let Some(mode) = wanted_mode.filter(|m|self.mode.as_deref() != Some(*m) && (self.modes.is_empty() || self.modes.iter().any(|c|c.id == *m)))
            && !self.boot_mode_tried {
            self.boot_mode_tried = true;
            let params = json!({"sessionId":self.conversation,"modeId":mode});
            self.send(Some(format!("{parent}:mode")),Some(parent),Call::BootMode,"session/set_mode",params,effects);
            return;
        }
        if let Some(model) = self.wanted_model.clone().filter(|m|self.model.as_deref() != Some(m.as_str()))
            && !self.rpc.values().any(|rpc|matches!(rpc.call,Call::BootModel(_))) {
            // Uma tentativa por abertura: a recusa não repete o pedido.
            self.wanted_model = None;
            let params = json!({"sessionId":self.conversation,"modelId":model});
            self.send(Some(format!("{parent}:model")),Some(parent),Call::BootModel(model),"session/set_model",params,effects);
            return;
        }
        self.booting = None;
        self.initialized = true; self.ready = true;
        self.clear_problem_codes();
        effects.push(Effect::Reply { operation_id:parent,disposition:Disposition::Accepted,payload:json!({"ready":true}) });
        effects.push(Effect::WakeQueue);
        effects.push(Effect::StateChanged);
    }

    fn clear_problem_codes(&mut self) {
        if matches!(self.state.problema.as_deref(),Some("antigravity_sem_login" | "antigravity_conversa_nao_abriu" | "headless_caiu")) {
            self.state.problema = None; self.state.problema_detalhe = None;
        }
    }

    pub fn command(&mut self,command:RuntimeCommand,clock:ClockSample) -> Result<Vec<Effect>,RuntimeError> {
        self.clock = clock;
        let mut effects = Vec::new();
        let RuntimeCommand { operation_id,kind,payload } = command;
        let reply = |disposition,payload| Effect::Reply { operation_id:operation_id.clone(),disposition,payload };
        type K = OperationKind;
        match kind {
            K::Input | K::Steer => {
                let steer = kind == K::Steer;
                if !steer && !self.deliverable() { return Ok(vec![reply(Disposition::Deferred,json!({}))]); }
                if steer && !(self.alive && self.ready && self.in_progress) { return Err(error("steer_no_turn","não há turno em andamento para orientar")); }
                let prompt = payload["prompt"].as_array().cloned().filter(|p|!p.is_empty())
                    .unwrap_or_else(||vec![json!({"type":"text","text":payload["text"].as_str().unwrap_or("")})]);
                let params = json!({"sessionId":self.conversation,"prompt":prompt});
                let id = self.send(Some(operation_id.clone()),None,Call::Prompt,"session/prompt",params,&mut effects);
                if let Some(wire) = self.wires.get_mut(&operation_id) { wire.kind = WireKind::Prompt { steer }; }
                if !steer { self.turn_id = Some(id); }
                self.in_progress = true;
                self.last_event = clock.monotonic_s;
                effects.push(Effect::StateChanged);
            }
            K::Interrupt => {
                if !self.in_progress { return Ok(vec![reply(Disposition::Accepted,json!({"interrupted":false}))]); }
                // O ACP pede que o cliente cancele as permissões abertas junto com o turno.
                for (id,_) in self.server_requests.clone() {
                    if self.answering.insert(id.clone()) {
                        effects.push(Effect::Write { frame:json!({"jsonrpc":"2.0","id":id,"result":{"outcome":{"outcome":"cancelled"}}}),operation_id:None });
                    }
                }
                self.wires.insert(operation_id.clone(),Wire { kind:WireKind::Cancel,request_id:None,server_request:None,server_epoch:None,interaction:None,final_result:false });
                effects.push(Effect::Write { frame:json!({"jsonrpc":"2.0","method":"session/cancel","params":{"sessionId":self.conversation}}),operation_id:Some(operation_id.clone()) });
                self.cancel_due = Some(clock.monotonic_s + CANCEL_LIMIT);
            }
            K::Select => {
                let expected = self.expected(&payload)?;
                let (id,request) = self.server_requests.iter()
                    .find(|(id,_)|native_target(InteractionKind::Approval,&self.conversation,&json!(id)) == expected.target)
                    .cloned().ok_or_else(||runtime_error(InteractionError::Changed))?;
                self.check_request(&expected,&id,&request)?;
                let options = request["params"]["options"].as_array().cloned().unwrap_or_default();
                let chosen = payload["option"].as_u64().and_then(|n|n.checked_sub(1)).and_then(|n|options.get(n as usize))
                    .and_then(|o|o["optionId"].as_str()).ok_or_else(||error("antigravity_answer_invalid","opção fora da pergunta"))?;
                self.answering.insert(id.clone());
                self.wires.insert(operation_id.clone(),Wire { kind:WireKind::Answer,request_id:None,server_request:Some(id.clone()),
                    server_epoch:self.request_epochs.get(&id).copied(),interaction:Some(expected),final_result:false });
                effects.push(Effect::Write { frame:json!({"jsonrpc":"2.0","id":id,"result":{"outcome":{"outcome":"selected","optionId":chosen}}}),
                    operation_id:Some(operation_id.clone()) });
            }
            K::SetModel => {
                let model = model_id(payload["model"].as_str().filter(|m|!m.is_empty()),payload["effort"].as_str().filter(|e|!e.is_empty()),self.model.as_deref())
                    .ok_or_else(||error("antigravity_model_invalid","modelo ausente"))?;
                if !self.ready { return Err(error("antigravity_not_ready","a conversa ainda não abriu")); }
                if !self.models.is_empty() && !self.models.iter().any(|m|m.id == model) {
                    return Err(error("antigravity_model_invalid","esse modelo não está na lista da conta"));
                }
                let params = json!({"sessionId":self.conversation,"modelId":model});
                self.send(Some(operation_id.clone()),None,Call::SetModel(model),"session/set_model",params,&mut effects);
            }
            K::SetPermissionMode => {
                let wanted = payload["mode"].as_str().and_then(PermissionMode::antigravity)
                    .ok_or_else(||error("erro_modo_desconhecido","modo desconhecido para o Antigravity"))?;
                // O bypass é flag do processo: entrar ou sair dele sobe o `agy acp` de novo.
                if wanted == PermissionMode::BypassPermissions || self.permission_mode == PermissionMode::BypassPermissions {
                    if self.in_progress { return Err(error("erro_permissao_ocupada","espere o turno acabar para trocar o modo")); }
                    effects.push(Effect::Respawn { operation_id:operation_id.clone(),reason:"restart".into(),
                        patch:json!({"permission_mode":wanted.name()}),reply:json!({"mode":wanted.name()}) });
                    return Ok(effects);
                }
                let mode = super::antigravity_launch::acp_mode(wanted).ok_or_else(unsupported)?;
                if !self.ready { return Err(error("antigravity_not_ready","a conversa ainda não abriu")); }
                let params = json!({"sessionId":self.conversation,"modeId":mode});
                self.send(Some(operation_id.clone()),None,Call::SetMode(wanted),"session/set_mode",params,&mut effects);
            }
            K::Restart | K::Reload => effects.push(Effect::Respawn { operation_id:operation_id.clone(),reason:"restart".into(),patch:Value::Null,reply:json!({}) }),
            K::ListModels => effects.push(reply(Disposition::Accepted,json!({"models":self.models.iter().map(|m|json!({"id":m.id,"name":m.name})).collect::<Vec<_>>(),
                "current":self.model}))),
            K::ReadSettings => effects.push(reply(Disposition::Accepted,json!({"model":self.model,"mode":self.mode,"permission_mode":self.permission_mode.name()}))),
            _ => return Err(unsupported()),
        }
        Ok(effects)
    }

    pub fn apply(&mut self,input:EngineInput,clock:ClockSample) -> Result<Vec<Effect>,RuntimeError> {
        self.clock = clock;
        let mut effects = Vec::new();
        match input {
            EngineInput::Line(line) => {
                if line["type"] == "cano_saiu" {
                    self.alive = false; self.in_progress = false; self.ready = false; self.initialized = false; self.turn_id = None;
                    self.clear_live(&mut effects);
                    self.state.problema = Some("headless_caiu".into());
                    effects.push(Effect::StateChanged);
                } else if line.get("method").is_some() {
                    self.agent_line(&line,&mut effects);
                } else if line.get("id").is_some() {
                    self.reply(&line,&mut effects);
                }
            }
            EngineInput::WriteAck { operation_id,outcome } => self.write_ack(&operation_id,outcome,&mut effects),
            EngineInput::PolicyResult { request_id,payload } => {
                if let Some(kind) = self.policies.remove(&request_id) && kind == "session.patch_meta" && payload["updated"] != true {
                    let code = payload["error"]["code"].as_str().unwrap_or("stale").to_owned();
                    tracing::warn!(session=%self.state.session,code=%code,"antigravity: o arquivo da sessão não aceitou a gravação");
                    effects.push(Effect::Diag { event:DiagEvent::AntigravitySidecarPatch,code });
                }
            }
            EngineInput::Tick => self.tick(&mut effects),
        }
        Ok(effects)
    }

    fn tick(&mut self,effects:&mut Vec<Effect>) {
        let now = self.clock.monotonic_s;
        if let Some(text) = self.preview.tick(now) { self.publish("preview",text,effects); }
        if let Some(text) = self.thinking.tick(now) { self.publish("thinking",text,effects); }
        let expired:Vec<String> = self.rpc.iter().filter(|(_,rpc)|!rpc.timed_out && rpc.deadline.is_some_and(|d|now >= d)).map(|(id,_)|id.clone()).collect();
        for id in expired {
            let Some(rpc) = self.rpc.get_mut(&id) else { continue };
            rpc.timed_out = true;
            let (op,parent) = (rpc.operation_id.clone(),rpc.parent.clone());
            if parent.is_some() && parent == self.booting {
                self.boot_failed("antigravity_conversa_nao_abriu","o agy não respondeu a abertura da conversa".into(),effects);
            } else if let Some(op) = op {
                effects.push(Effect::Reply { operation_id:op,disposition:Disposition::Unknown,payload:json!({"error":"o Antigravity não respondeu"}) });
            }
        }
        if self.in_progress && self.cancel_due.is_some_and(|due|now >= due) {
            self.cancel_due = None;
            self.counter += 1;
            tracing::warn!(session=%self.state.session,"antigravity: o turno não parou depois do cancelamento; o processo sobe de novo");
            effects.push(Effect::Respawn { operation_id:format!("cancel_restart:{}:{}",self.generation,self.counter),reason:"restart".into(),patch:Value::Null,reply:json!({}) });
        }
    }

    /// Notificação ou pedido do agente, lido pelo decodificador do crate.
    fn agent_line(&mut self,line:&Value,effects:&mut Vec<Effect>) {
        self.last_event = self.clock.monotonic_s;
        let now = self.clock.monotonic_s;
        let raw = line.to_string();
        for out in self.decoder.feed(&raw) {
            match out {
                // Pedido que não sabemos atender (ex.: `fs/read_text_file`) recebe erro: pendurado, prende o turno.
                acp::Out::Write(text) => if let Ok(frame) = serde_json::from_str::<Value>(&text) { effects.push(Effect::Write { frame,operation_id:None }); },
                acp::Out::Event(event) => match event {
                    acp::Event::PermissionAsked { rpc_id,.. } => {
                        let Ok(id) = serde_json::from_value::<RequestId>(rpc_id) else { continue };
                        if self.server_requests.iter().any(|(known,_)|*known == id) { continue; }
                        self.counter += 1;
                        self.request_epochs.insert(id.clone(),self.counter);
                        self.server_requests.push((id,line.clone()));
                        effects.push(Effect::StateChanged);
                    }
                    acp::Event::TextDelta(piece) => if let Some(text) = self.preview.append(&piece,now) { self.publish("preview",text,effects); },
                    acp::Event::ThoughtDelta(piece) => if let Some(text) = self.thinking.append(&piece,now) { self.publish("thinking",text,effects); },
                    acp::Event::ToolStarted { title,name,.. } => {
                        // Texto depois de uma ferramenta é outra mensagem; o definitivo vem do transcript.
                        if let Some(text) = self.preview.clear() { self.publish("preview",text,effects); }
                        let label = title.or(name).unwrap_or_default();
                        self.tool = Some(label.clone());
                        self.publish("tool",label,effects);
                    }
                    acp::Event::ToolUpdated { status,.. } => if matches!(status.as_deref(),Some("completed" | "failed")) && self.tool.take().is_some() {
                        self.publish("tool",String::new(),effects);
                    },
                    acp::Event::ModeChanged(mode) => { self.mode = Some(mode); effects.push(Effect::StateChanged); }
                    acp::Event::ModelChanged(model) => { self.model = Some(model); effects.push(Effect::StateChanged); }
                    acp::Event::ContextUsage { used,size } => self.token_usage = json!({"used":used,"size":size}),
                    _ => {}
                },
            }
        }
    }

    fn reply(&mut self,line:&Value,effects:&mut Vec<Effect>) {
        self.last_event = self.clock.monotonic_s;
        let key = match &line["id"] { Value::String(id) => id.clone(), other => other.to_string() };
        let Some(rpc) = self.rpc.remove(&key) else {
            // Prompt de antes da religação (o motor não o conhece mais): a resposta ainda fecha o turno.
            if self.in_progress && !self.prompt_open() && line["result"].get("stopReason").is_some() { self.turn_ended(line,effects); }
            return;
        };
        if let Some(op) = &rpc.operation_id && let Some(wire) = self.wires.get_mut(op) && wire.kind == WireKind::Rpc { wire.final_result = true; }
        let failure = line.get("error").filter(|e|!e.is_null()).map(|e|e["message"].as_str().unwrap_or("o Antigravity recusou").to_owned());
        let result = &line["result"];
        let booting = rpc.parent.is_some() && rpc.parent == self.booting;
        match rpc.call {
            Call::Prompt => self.turn_ended(line,effects),
            _ if rpc.timed_out && !booting => {}
            Call::Initialize | Call::Authenticate | Call::Open { .. } if !booting => {}
            Call::Initialize => match failure {
                Some(detail) => self.boot_failed("antigravity_conversa_nao_abriu",detail,effects),
                None => {
                    let parent = self.booting.clone();
                    self.send(parent.as_ref().map(|p|format!("{p}:authenticate")),parent,Call::Authenticate,"authenticate",json!({"methodId":acp::AUTH_PERSONAL}),effects);
                }
            },
            // Login recusado vira aviso de login; o Hangar nunca troca de conta sozinho.
            Call::Authenticate => match failure {
                Some(detail) => { self.state.login = true; self.boot_failed("antigravity_sem_login",detail,effects); }
                None => {
                    self.state.login = false;
                    self.tier = result["_meta"]["antigravity"]["tier"]["name"].as_str().map(str::to_owned);
                    let parent = self.booting.clone();
                    let op = parent.as_ref().map(|p|format!("{p}:open"));
                    if self.conversation.is_empty() {
                        self.send(op,parent,Call::Open { resumed:false },"session/new",json!({"cwd":self.cwd,"mcpServers":[]}),effects);
                    } else {
                        let params = json!({"sessionId":self.conversation,"cwd":self.cwd,"mcpServers":[]});
                        self.send(op,parent,Call::Open { resumed:true },"session/resume",params,effects);
                    }
                }
            },
            Call::Open { resumed } => match failure.or_else(||(!resumed && text(&result["sessionId"]).is_none()).then(||"o agy não devolveu a conversa".to_owned())) {
                Some(detail) => self.boot_failed("antigravity_conversa_nao_abriu",detail,effects),
                None => self.opened(result,effects),
            },
            Call::BootMode => {
                match failure {
                    Some(detail) => self.note(format!("O Antigravity não aceitou o modo de permissão pedido: {detail}"),effects),
                    None => self.mode = super::antigravity_launch::acp_mode(self.permission_mode).map(str::to_owned),
                }
                self.boot_next(effects);
            }
            Call::BootModel(model) => {
                match failure {
                    Some(detail) => self.note(format!("O Antigravity não aceitou o modelo pedido: {detail}"),effects),
                    None => self.model = Some(model),
                }
                self.boot_next(effects);
            }
            Call::SetModel(model) => {
                let Some(op) = rpc.operation_id else { return };
                match failure {
                    Some(detail) => effects.push(Effect::Reply { operation_id:op,disposition:Disposition::Rejected,payload:json!({"error":detail}) }),
                    None => {
                        self.model = Some(model.clone()); self.wanted_model = Some(model.clone());
                        self.policy("session.patch_meta",json!({"model":model}),effects);
                        effects.push(Effect::Reply { operation_id:op,disposition:Disposition::Accepted,payload:json!({"model":model}) });
                        effects.push(Effect::StateChanged);
                    }
                }
            }
            Call::SetMode(mode) => {
                let Some(op) = rpc.operation_id else { return };
                match failure {
                    Some(detail) => effects.push(Effect::Reply { operation_id:op,disposition:Disposition::Rejected,payload:json!({"error":detail}) }),
                    None => {
                        self.permission_mode = mode;
                        self.mode = super::antigravity_launch::acp_mode(mode).map(str::to_owned);
                        self.policy("session.patch_meta",json!({"permission_mode":mode.name()}),effects);
                        effects.push(Effect::Reply { operation_id:op,disposition:Disposition::Accepted,payload:json!({"mode":mode.name()}) });
                        effects.push(Effect::StateChanged);
                    }
                }
            }
        }
    }

    fn opened(&mut self,result:&Value,effects:&mut Vec<Effect>) {
        if let Some(id) = text(&result["sessionId"]) {
            if !hangar_antigravity::paths::valid_conversation_id(&id) {
                return self.boot_failed("antigravity_conversa_nao_abriu",format!("o agy devolveu uma conversa fora do formato: {id}"),effects);
            }
            self.conversation = id;
        }
        let models:Vec<acp::Choice> = result["models"]["availableModels"].as_array().into_iter().flatten()
            .filter_map(|c|Some(acp::Choice { id:c["modelId"].as_str()?.into(),name:c["name"].as_str().unwrap_or_default().into() })).collect();
        let modes:Vec<acp::Choice> = result["modes"]["availableModes"].as_array().into_iter().flatten()
            .filter_map(|c|Some(acp::Choice { id:c["id"].as_str()?.into(),name:c["name"].as_str().unwrap_or_default().into() })).collect();
        if !models.is_empty() { self.models = models; }
        if !modes.is_empty() { self.modes = modes; }
        if let Some(model) = text(&result["models"]["currentModelId"]) { self.model = Some(model); }
        if let Some(mode) = text(&result["modes"]["currentModeId"]) { self.mode = Some(mode); }
        if let Err(error) = self.bind_interactions() { tracing::warn!(session=%self.state.session,code=%error.code,"antigravity: cartões sem vínculo"); }
        self.policy("session.patch_meta",json!({"conversation_id":self.conversation}),effects);
        self.boot_next(effects);
    }

    fn turn_ended(&mut self,line:&Value,effects:&mut Vec<Effect>) {
        if self.prompt_open() { return; }
        self.in_progress = false; self.turn_id = None; self.cancel_due = None;
        self.clear_live(effects);
        // O agy encerra as permissões do turno junto com ele.
        for (id,_) in std::mem::take(&mut self.server_requests) { self.request_epochs.remove(&id); self.answering.remove(&id); }
        if let Some(detail) = line.get("error").filter(|e|!e.is_null()).map(|e|e["message"].as_str().unwrap_or("o turno falhou").to_owned()) {
            self.put_problem("headless_turno_erro",Some(detail.chars().take(300).collect()));
        } else {
            if self.state.problema.as_deref() == Some("headless_turno_erro") { self.state.problema = None; self.state.problema_detalhe = None; }
            match line["result"]["stopReason"].as_str() {
                Some("refusal") => self.note("O modelo recusou continuar este turno.".into(),effects),
                Some("max_tokens") => self.note("O turno parou no limite de tokens da resposta.".into(),effects),
                Some("max_turn_requests") => self.note("O turno parou no limite de passos do Antigravity.".into(),effects),
                _ => {}
            }
        }
        effects.push(Effect::WakeQueue);
        effects.push(Effect::StateChanged);
    }

    fn finalize(&mut self,operation_id:&str,done:bool) { if let Some(wire) = self.wires.get_mut(operation_id) { wire.final_result = done; } }

    fn write_ack(&mut self,operation_id:&str,outcome:WriteOutcome,effects:&mut Vec<Effect>) {
        let Some(wire) = self.wires.get(operation_id).filter(|wire|!wire.final_result) else { return };
        let (kind,request_id,server,epoch) = (wire.kind,wire.request_id.clone(),wire.server_request.clone(),wire.server_epoch);
        let reply = |disposition,payload| Effect::Reply { operation_id:operation_id.into(),disposition,payload };
        match kind {
            WireKind::Rpc => {
                if outcome != WriteOutcome::NotWritten { return; }
                self.finalize(operation_id,true);
                let rpc = request_id.and_then(|id|self.rpc.remove(&id));
                if let Some(rpc) = rpc && rpc.parent.is_some() && rpc.parent == self.booting {
                    self.boot_failed("antigravity_conversa_nao_abriu","o pedido de abertura não foi escrito".into(),effects);
                } else {
                    effects.push(reply(Disposition::Rejected,json!({"write_outcome":outcome})));
                }
            }
            WireKind::Prompt { steer } => {
                self.finalize(operation_id,outcome != WriteOutcome::Unknown);
                match outcome {
                    WriteOutcome::Written => effects.push(reply(Disposition::Accepted,json!({"write_outcome":outcome}))),
                    WriteOutcome::NotWritten => {
                        if let Some(id) = request_id { self.rpc.remove(&id); }
                        if !self.prompt_open() { self.in_progress = false; self.turn_id = None; }
                        effects.push(reply(if steer { Disposition::Rejected } else { Disposition::Deferred },json!({"write_outcome":outcome})));
                        effects.push(Effect::StateChanged);
                    }
                    WriteOutcome::Unknown => effects.push(reply(Disposition::Unknown,json!({"write_outcome":outcome}))),
                }
            }
            WireKind::Cancel => {
                self.finalize(operation_id,true);
                if outcome == WriteOutcome::NotWritten { self.cancel_due = None; }
                let disposition = match outcome { WriteOutcome::Written => Disposition::Accepted, WriteOutcome::NotWritten => Disposition::Rejected, WriteOutcome::Unknown => Disposition::Unknown };
                effects.push(reply(disposition,json!({"interrupted":outcome == WriteOutcome::Written})));
            }
            WireKind::Answer => {
                self.finalize(operation_id,outcome != WriteOutcome::Unknown);
                let Some(id) = server else { return };
                let same = self.request_epochs.get(&id).copied() == epoch;
                let disposition = match outcome {
                    WriteOutcome::Written => {
                        if same { self.server_requests.retain(|(key,_)|key != &id); self.answering.remove(&id); self.request_epochs.remove(&id); }
                        Disposition::Accepted
                    }
                    WriteOutcome::NotWritten => { if same { self.answering.remove(&id); } Disposition::Rejected }
                    WriteOutcome::Unknown => Disposition::Unknown,
                };
                effects.push(reply(disposition,json!({"write_outcome":outcome})));
                effects.push(Effect::StateChanged);
            }
        }
    }

    /// Retrato do cano na abertura do ator: pedidos abertos, turno, prévia e a conversa que o processo abriu.
    pub fn hydrate(&mut self,snapshot:CanoSnapshot) -> Result<Vec<Effect>,RuntimeError> {
        let mut effects = Vec::new();
        self.alive = snapshot.saiu.is_none();
        let opened = snapshot.init.as_deref().and_then(|raw|serde_json::from_str::<Value>(raw).ok());
        match opened.as_ref().and_then(|init|text(&init["result"]["sessionId"])) {
            Some(id) if self.conversation.is_empty() && hangar_antigravity::paths::valid_conversation_id(&id) => self.conversation = id,
            _ => {}
        }
        if opened.is_none() { self.initialized = false; self.ready = false; }
        else if self.initialized { self.ready = true; }
        for raw in &snapshot.pendentes {
            let line:Value = serde_json::from_str(raw).map_err(|_|error("antigravity_snapshot","pedido do retrato inválido"))?;
            if line["method"] == "session/request_permission" { self.agent_line(&line,&mut effects); }
        }
        self.in_progress = snapshot.aberto;
        if !snapshot.aberto { self.rpc.retain(|_,rpc|rpc.call != Call::Prompt); self.turn_id = None; }
        else if self.turn_id.is_none() { self.turn_id = Some(format!("agy:{}:retomado",self.generation)); }
        let prefix = &snapshot.inflight["acp"];
        if snapshot.aberto && prefix["complete"] == true {
            let now = self.clock.monotonic_s;
            if let Some(text) = self.preview.append(prefix["text"].as_str().unwrap_or(""),now) { self.publish("preview",text,&mut effects); }
            if let Some(text) = self.thinking.append(prefix["thinking"].as_str().unwrap_or(""),now) { self.publish("thinking",text,&mut effects); }
        }
        if let Err(error) = self.bind_interactions() { tracing::warn!(session=%self.state.session,code=%error.code,"antigravity: cartões sem vínculo"); }
        effects.push(Effect::StateChanged);
        Ok(effects)
    }

    /// Socket novo ao MESMO cano: a sessão ACP vive entre o cano e o agy e continua aberta.
    pub fn reattach(&mut self,snapshot:CanoSnapshot) -> Result<Vec<Effect>,RuntimeError> {
        // Pendente ilegível não pode virar "já respondido": o cartão sumiria com o agente esperando.
        let presented:Vec<Value> = snapshot.pendentes.iter().map(|raw|serde_json::from_str(raw)).collect::<Result<_,_>>()
            .map_err(|_|error("cano_snapshot_invalid","pedido pendente ilegível no cano"))?;
        let resolved:Vec<RequestId> = self.server_requests.iter().filter(|(_,request)|!presented.contains(request)).map(|(id,_)|id.clone()).collect();
        for id in &resolved { self.request_epochs.remove(id); }
        self.server_requests.retain(|(id,_)|!resolved.contains(id));
        self.answering.clear();
        let mut effects = Vec::new();
        self.clear_live(&mut effects);
        effects.extend(self.hydrate(snapshot)?);
        if !self.in_progress { effects.push(Effect::WakeQueue); }
        Ok(effects)
    }

    /// Só o socket com o cano caiu: o que não saiu volta não escrito, o que saiu fica incerto.
    pub fn transport_lost(&mut self,handed:&BTreeSet<String>) -> Vec<Effect> {
        let mut effects = Vec::new();
        let pending:Vec<(String,bool)> = self.wires.iter().filter(|(_,wire)|!wire.final_result).map(|(id,_)|(id.clone(),handed.contains(id))).collect();
        for (operation_id,sent) in pending {
            let outcome = if sent { WriteOutcome::Unknown } else { WriteOutcome::NotWritten };
            self.write_ack(&operation_id,outcome,&mut effects);
        }
        if let Some(parent) = self.booting.take() {
            self.rpc.retain(|_,rpc|rpc.parent.as_ref() != Some(&parent));
            effects.push(Effect::Reply { operation_id:parent,disposition:Disposition::Unknown,payload:json!({"error":"a conexão com o cano caiu na abertura"}) });
        }
        self.ready = false;
        effects.push(Effect::StateChanged);
        effects
    }

    /// Pedido restaurado do diário: a resposta tardia de um prompt ainda fecha o turno certo.
    pub fn restore_rpc(&mut self,operation_id:String,frame:&Value) {
        let (Some(id),Some("session/prompt")) = (frame["id"].as_str(),frame["method"].as_str()) else { return };
        if !id.starts_with(&format!("agy:{}:",self.generation)) || self.rpc.contains_key(id) { return; }
        self.rpc.insert(id.into(),Rpc { operation_id:Some(operation_id.clone()),parent:None,call:Call::Prompt,deadline:None,timed_out:true });
        self.wires.insert(operation_id,Wire { kind:WireKind::Prompt { steer:false },request_id:Some(id.into()),server_request:None,server_epoch:None,interaction:None,final_result:true });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const OPENED:&str = "9abf1fce-b58c-43c2-98be-8071a4843f8d";

    fn at(t:f64) -> ClockSample { ClockSample { monotonic_s:t,epoch_s:t } }

    fn engine(meta:Value) -> Engine {
        let mut base = json!({"name":"ag","cwd":"/tmp/agy-prova","brain_dir":"/h/.gemini/antigravity-cli/brain"});
        for (k,v) in meta.as_object().unwrap() { base[k] = v.clone(); }
        let mut engine = Engine::new_checked(base,1,at(0.0)).unwrap();
        engine.set_interactions("k1".into(),Arc::new(InteractionStore::default())).unwrap();
        engine
    }

    fn fixture(name:&str) -> Vec<Value> {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../hangar-antigravity/tests/fixtures").join(name);
        std::fs::read_to_string(path).unwrap().lines().filter(|l|!l.trim().is_empty()).map(|l|serde_json::from_str(l).unwrap()).collect()
    }

    fn frames(effects:&[Effect]) -> Vec<Value> {
        effects.iter().filter_map(|e|match e { Effect::Write { frame,.. } => Some(frame.clone()), _ => None }).collect()
    }

    fn replies(effects:&[Effect]) -> Vec<(String,Disposition)> {
        effects.iter().filter_map(|e|match e { Effect::Reply { operation_id,disposition,.. } => Some((operation_id.clone(),*disposition)), _ => None }).collect()
    }

    /// O pedido mais antigo ainda sem resposta (os ids são `agy:<geração>:<n>`).
    fn oldest(engine:&Engine) -> Option<String> {
        engine.rpc.keys().min_by_key(|id|id.rsplit(':').next().and_then(|n|n.parse::<u64>().ok())).cloned()
    }

    /// Repete a linha do agente: resposta gravada vai ao pedido mais antigo aberto (o agy responde em ordem).
    fn feed(engine:&mut Engine,mut line:Value,t:f64) -> Vec<Effect> {
        if line.get("method").is_none() && line.get("id").is_some() { line["id"] = json!(oldest(engine).expect("resposta sem pedido aberto")); }
        engine.apply(EngineInput::Line(line),at(t)).unwrap()
    }

    fn command(engine:&mut Engine,id:&str,kind:OperationKind,payload:Value) -> Result<Vec<Effect>,RuntimeError> {
        engine.command(RuntimeCommand { operation_id:id.into(),kind,payload },at(1.0))
    }

    fn written(engine:&mut Engine,id:&str) -> Vec<Effect> { engine.apply(EngineInput::WriteAck { operation_id:id.into(),outcome:WriteOutcome::Written },at(1.0)).unwrap() }

    fn opened(meta:Value) -> Engine {
        let mut engine = engine(meta);
        engine.bootstrap("boot".into()).unwrap();
        for line in fixture("acp-permission.jsonl").into_iter().take(3) { feed(&mut engine,line,0.5); }
        engine
    }

    #[test]
    fn the_bootstrap_opens_a_new_conversation_and_answers_the_boot_operation() {
        let mut engine = engine(json!({}));
        let start = engine.bootstrap("boot".into()).unwrap();
        assert_eq!(frames(&start)[0]["method"],"initialize");
        assert_eq!(frames(&start)[0]["params"]["clientCapabilities"]["fs"]["readTextFile"],false);
        let mut methods = Vec::new();
        let mut last = Vec::new();
        for line in fixture("acp-permission.jsonl").into_iter().take(3) {
            last = feed(&mut engine,line,0.5);
            methods.extend(frames(&last).iter().map(|f|f["method"].as_str().unwrap_or("").to_owned()));
        }
        assert_eq!(methods,["authenticate","session/new"]);
        assert_eq!(replies(&last),[("boot".to_owned(),Disposition::Accepted)]);
        assert!(last.iter().any(|e|matches!(e,Effect::Policy { kind,payload,.. } if kind == "session.patch_meta" && payload["conversation_id"] == OPENED)));
        assert!(last.contains(&Effect::WakeQueue));
        let view = engine.control_view();
        assert_eq!((view["ready"].as_bool(),view["conversation_id"].as_str()),(Some(true),Some(OPENED)));
        assert_eq!(view["rollout_path"],format!("/h/.gemini/antigravity-cli/brain/{OPENED}/.system_generated/logs/transcript_full.jsonl"));
        assert!(engine.deliverable());
    }

    #[test]
    fn a_known_conversation_is_resumed_and_the_chosen_mode_and_model_follow() {
        let id = "904fd70e-847b-49ed-9999-a13d7a4d9a98";
        let mut engine = engine(json!({"conversation_id":id,"permission_mode":"acceptEdits","model":"gemini-4-neon-high"}));
        engine.bootstrap("boot".into()).unwrap();
        feed(&mut engine,json!({"jsonrpc":"2.0","id":0,"result":{"protocolVersion":1}}),0.1);
        let open = frames(&feed(&mut engine,json!({"jsonrpc":"2.0","id":0,"result":{}}),0.2))[0].clone();
        assert_eq!((open["method"].as_str(),open["params"]["sessionId"].as_str()),(Some("session/resume"),Some(id)));
        let mode = frames(&feed(&mut engine,json!({"jsonrpc":"2.0","id":0,"result":{"sessionId":id}}),0.3))[0].clone();
        assert_eq!((mode["method"].as_str(),mode["params"]["modeId"].as_str()),(Some("session/set_mode"),Some("accept-edits")));
        let model = frames(&feed(&mut engine,json!({"jsonrpc":"2.0","id":0,"result":{}}),0.4))[0].clone();
        assert_eq!((model["method"].as_str(),model["params"]["modelId"].as_str()),(Some("session/set_model"),Some("gemini-4-neon-high")));
        let done = feed(&mut engine,json!({"jsonrpc":"2.0","id":0,"result":{}}),0.5);
        assert_eq!(replies(&done),[("boot".to_owned(),Disposition::Accepted)]);
        assert_eq!(engine.control_view()["current_model"],"gemini-4-neon-high");
    }

    #[test]
    fn a_refused_login_is_a_login_problem_and_nothing_else_is_sent() {
        let mut engine = engine(json!({}));
        engine.bootstrap("boot".into()).unwrap();
        feed(&mut engine,json!({"jsonrpc":"2.0","id":0,"result":{}}),0.1);
        let out = feed(&mut engine,json!({"jsonrpc":"2.0","id":0,"error":{"code":-32000,"message":"Authentication required"}}),0.2);
        assert!(frames(&out).is_empty());
        assert_eq!(replies(&out),[("boot".to_owned(),Disposition::Rejected)]);
        let state = engine.view();
        assert_eq!((state["problema"].as_str(),state["login"].as_bool()),(Some("antigravity_sem_login"),Some(true)));
        assert!(!engine.deliverable());
    }

    #[test]
    fn a_turn_with_a_permission_answered_on_the_card_ends_on_the_prompt_answer() {
        let mut engine = opened(json!({}));
        let sent = command(&mut engine,"in1",OperationKind::Input,json!({"text":"apague o arquivo"})).unwrap();
        let prompt = &frames(&sent)[0];
        assert_eq!((prompt["method"].as_str(),prompt["params"]["sessionId"].as_str()),(Some("session/prompt"),Some(OPENED)));
        assert_eq!(prompt["params"]["prompt"][0]["text"],"apague o arquivo");
        assert_eq!(replies(&written(&mut engine,"in1")),[("in1".to_owned(),Disposition::Accepted)],"a mensagem se resolve pela escrita");
        assert_eq!(engine.view()["state"],"working");
        assert!(!engine.deliverable());
        let rest:Vec<Value> = fixture("acp-permission.jsonl").into_iter().skip(3).collect();
        let (before,after) = rest.split_at(rest.iter().position(|l|l["method"] == "session/request_permission").unwrap() + 1);
        for line in before { feed(&mut engine,line.clone(),2.0); }
        let card = engine.view();
        assert_eq!(card["state"],"awaiting_input");
        assert_eq!(card["options"].as_array().map(Vec::len),Some(2));
        assert!(card["question"].as_str().unwrap().contains("rm -f /tmp/agy-prova/nao-existe.txt"));
        let interaction = card["interaction_id"].as_str().expect("cartão sem identidade").to_owned();
        assert_eq!(command(&mut engine,"s0",OperationKind::Select,json!({"option":2})).unwrap_err().code,"erro_interacao_sem_identidade");
        let answer = command(&mut engine,"s1",OperationKind::Select,json!({"option":2,"interaction_id":interaction})).unwrap();
        let frame = &frames(&answer)[0];
        assert_eq!((frame["id"].as_str(),frame["result"]["outcome"]["optionId"].as_str()),(Some("1"),Some("deny")),"o id volta em texto");
        assert_eq!(replies(&written(&mut engine,"s1")),[("s1".to_owned(),Disposition::Accepted)]);
        assert_eq!(engine.view()["state"],"working");
        let mut last = Vec::new();
        for line in after { last = feed(&mut engine,line.clone(),3.0); }
        assert_eq!(engine.view()["state"],"idle");
        assert!(last.contains(&Effect::WakeQueue));
        assert!(engine.deliverable());
    }

    #[test]
    fn a_text_turn_streams_the_preview_and_clears_it_at_the_end() {
        let mut engine = opened(json!({}));
        command(&mut engine,"in1",OperationKind::Input,json!({"text":"oi"})).unwrap();
        written(&mut engine,"in1");
        let mut published = String::new();
        for line in fixture("acp-text.jsonl").into_iter().filter(|l|l["method"] == "session/update") {
            for effect in feed(&mut engine,line,2.0) {
                if let Effect::Publish { channel,data } = effect && channel == "preview" { published = data["text"].as_str().unwrap_or("").into(); }
            }
        }
        for effect in engine.apply(EngineInput::Tick,at(9.0)).unwrap() {
            if let Effect::Publish { channel,data } = effect && channel == "preview" { published = data["text"].as_str().unwrap_or("").into(); }
        }
        assert!(!published.trim().is_empty(),"a prévia saiu");
        let end = feed(&mut engine,json!({"jsonrpc":"2.0","id":0,"result":{"stopReason":"end_turn"}}),10.0);
        assert!(end.iter().any(|e|matches!(e,Effect::Publish { channel,data } if channel == "preview" && data["text"] == "")),"a prévia some no fim");
    }

    #[test]
    fn a_message_during_a_turn_waits_and_a_steer_goes_as_a_second_prompt() {
        let mut engine = opened(json!({}));
        command(&mut engine,"in1",OperationKind::Input,json!({"text":"a"})).unwrap();
        assert_eq!(replies(&command(&mut engine,"in2",OperationKind::Input,json!({"text":"b"})).unwrap()),[("in2".to_owned(),Disposition::Deferred)]);
        let steer = command(&mut engine,"st1",OperationKind::Steer,json!({"text":"mais curto"})).unwrap();
        assert_eq!(frames(&steer)[0]["method"],"session/prompt");
        // A primeira resposta (o turno cortado pela orientação) não fecha o turno: a do prompt novo fecha.
        feed(&mut engine,json!({"jsonrpc":"2.0","id":0,"result":{"stopReason":"cancelled"}}),2.0);
        assert_eq!(engine.view()["state"],"working");
        feed(&mut engine,json!({"jsonrpc":"2.0","id":0,"result":{"stopReason":"end_turn"}}),3.0);
        assert_eq!(engine.view()["state"],"idle");
    }

    #[test]
    fn a_prompt_that_was_not_written_goes_back_to_the_queue() {
        let mut engine = opened(json!({}));
        command(&mut engine,"in1",OperationKind::Input,json!({"text":"a"})).unwrap();
        let out = engine.apply(EngineInput::WriteAck { operation_id:"in1".into(),outcome:WriteOutcome::NotWritten },at(1.0)).unwrap();
        assert_eq!(replies(&out),[("in1".to_owned(),Disposition::Deferred)]);
        assert!(engine.deliverable());
    }

    #[test]
    fn interrupt_cancels_and_a_turn_that_does_not_stop_restarts_the_process() {
        let mut engine = opened(json!({}));
        command(&mut engine,"in1",OperationKind::Input,json!({"text":"a"})).unwrap();
        written(&mut engine,"in1");
        let cancel = command(&mut engine,"int1",OperationKind::Interrupt,json!({})).unwrap();
        let frame = &frames(&cancel)[0];
        assert_eq!((frame["method"].as_str(),frame.get("id")),(Some("session/cancel"),None),"cancelamento é notificação");
        assert_eq!(replies(&written(&mut engine,"int1")),[("int1".to_owned(),Disposition::Accepted)]);
        assert!(engine.apply(EngineInput::Tick,at(5.0)).unwrap().iter().all(|e|!matches!(e,Effect::Respawn { .. })));
        let late = engine.apply(EngineInput::Tick,at(1.0 + CANCEL_LIMIT)).unwrap();
        assert!(late.iter().any(|e|matches!(e,Effect::Respawn { reason,.. } if reason == "restart")));
    }

    #[test]
    fn an_unexpected_agent_request_gets_an_error_instead_of_hanging_the_turn() {
        let mut engine = opened(json!({}));
        let out = feed(&mut engine,json!({"jsonrpc":"2.0","id":"7","method":"fs/read_text_file","params":{"path":"/etc/passwd"}}),1.0);
        let reply = &frames(&out)[0];
        assert_eq!((reply["id"].as_str(),reply["error"]["code"].as_i64()),(Some("7"),Some(-32601)));
    }

    #[test]
    fn the_cano_of_a_live_session_is_adopted_without_a_new_bootstrap() {
        let mut engine = engine(json!({"conversation_id":OPENED,"initialized":true,"ready":true}));
        let init = json!({"jsonrpc":"2.0","id":"agy:0:3","result":{"sessionId":OPENED}}).to_string();
        let snapshot = CanoSnapshot { kind:"cano_snapshot".into(),versao:2,pid:9,init:Some(init),aberto:true,pendentes:vec![],ultimo_result:None,
            rate_limit:None,stderr_tail:vec![],saiu:None,inflight:json!({"acp":{"complete":true,"text":"meio","thinking":""}}) };
        let out = engine.hydrate(snapshot).unwrap();
        assert!(out.iter().any(|e|matches!(e,Effect::Publish { channel,data } if channel == "preview" && data["text"] == "meio")));
        let view = engine.control_view();
        assert_eq!((view["initialized"].as_bool(),view["ready"].as_bool(),view["in_progress"].as_bool()),(Some(true),Some(true),Some(true)));
        // A resposta do prompt que era da vida anterior do ator ainda fecha o turno.
        engine.apply(EngineInput::Line(json!({"jsonrpc":"2.0","id":"agy:0:4","result":{"stopReason":"end_turn"}})),at(1.0)).unwrap();
        assert_eq!(engine.view()["state"],"idle");
    }

    #[test]
    fn a_fresh_process_without_an_open_conversation_needs_the_bootstrap() {
        let mut engine = engine(json!({"conversation_id":OPENED,"initialized":true,"ready":true}));
        let snapshot = CanoSnapshot { kind:"cano_snapshot".into(),versao:2,pid:9,init:None,aberto:false,pendentes:vec![],ultimo_result:None,
            rate_limit:None,stderr_tail:vec![],saiu:None,inflight:json!({}) };
        engine.hydrate(snapshot).unwrap();
        assert_eq!(engine.control_view()["initialized"],false);
        assert!(!engine.deliverable());
    }

    #[test]
    fn bypass_is_a_process_flag_and_the_other_modes_go_by_set_mode() {
        let mut engine = opened(json!({}));
        let bypass = command(&mut engine,"m1",OperationKind::SetPermissionMode,json!({"mode":"bypassPermissions"})).unwrap();
        assert!(bypass.iter().any(|e|matches!(e,Effect::Respawn { patch,.. } if patch["permission_mode"] == "bypassPermissions")));
        let plan = command(&mut engine,"m2",OperationKind::SetPermissionMode,json!({"mode":"plan"})).unwrap();
        assert_eq!(frames(&plan)[0]["params"]["modeId"],"plan");
        let done = feed(&mut engine,json!({"jsonrpc":"2.0","id":0,"result":{}}),2.0);
        assert_eq!(replies(&done),[("m2".to_owned(),Disposition::Accepted)]);
        assert!(done.iter().any(|e|matches!(e,Effect::Policy { payload,.. } if payload["permission_mode"] == "plan")));
        assert_eq!(command(&mut engine,"m3",OperationKind::SetPermissionMode,json!({"mode":"Full Access"})).unwrap_err().code,"erro_modo_desconhecido");
    }

    #[test]
    fn model_and_effort_become_one_agy_model_id() {
        assert_eq!(model_id(Some("gemini-4-neon"),Some("high"),None).as_deref(),Some("gemini-4-neon-high"));
        assert_eq!(model_id(None,Some("xhigh"),Some("gemini-4-neon-high")).as_deref(),Some("gemini-4-neon-xhigh"));
        assert_eq!(model_id(Some("gemini-3.8-flash"),None,Some("gemini-4-neon-high")).as_deref(),Some("gemini-3.8-flash"));
        assert_eq!(model_id(None,Some("low"),None),None);
    }

    #[test]
    fn set_model_goes_by_session_set_model_and_is_saved() {
        let mut engine = opened(json!({}));
        let out = command(&mut engine,"sm",OperationKind::SetModel,json!({"model":"gemini-4-neon-xhigh"})).unwrap();
        assert_eq!(frames(&out)[0]["method"],"session/set_model");
        let done = feed(&mut engine,json!({"jsonrpc":"2.0","id":0,"result":{}}),2.0);
        assert_eq!(replies(&done),[("sm".to_owned(),Disposition::Accepted)]);
        assert!(done.iter().any(|e|matches!(e,Effect::Policy { payload,.. } if payload["model"] == "gemini-4-neon-xhigh")));
        assert_eq!(command(&mut engine,"sx",OperationKind::SetModel,json!({"model":"nao-existe"})).unwrap_err().code,"antigravity_model_invalid");
    }
}
