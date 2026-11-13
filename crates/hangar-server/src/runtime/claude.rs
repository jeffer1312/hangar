use super::{LiveBuffer, protocol::*};
use crate::mods::{model::{ModsCall,ModsError,SurfaceEffect,missing},surface::Surface};
use crate::interactions::{ExpectedInteraction,InteractionBinding,InteractionError,InteractionKind,InteractionStore,native_target,runtime_error};
use std::sync::Arc;
use hangar_api::state::StateEvent;
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};

struct Waiter {
    operation_id: String,
    subtype: String,
    payload: Value,
    deadline: f64,
    timed_out: bool,
}

struct Wire {
    interaction: Option<ExpectedInteraction>,
    kind: String,
    request_id: Option<RequestId>,
    final_result: bool,
}

pub struct ClaudeEngine {
    interactions: Option<(String, Arc<InteractionStore>)>,
    metadata: Value,
    generation: u64,
    clock: ClockSample,
    counter: u64,
    alive: bool,
    /// Socket com o cano de pé. Fora, o processo segue vivo e a fila espera a religação.
    transport_up: bool,
    initialized: bool,
    initializing: bool,
    init_warning: Option<f64>,
    in_progress: bool,
    state: StateEvent,
    model: Option<String>,
    effort: Option<String>,
    permission_mode: Option<String>,
    previous_non_plan: Option<String>,
    restore_plan: Option<(String,String)>,
    pending: Vec<(RequestId,Value)>,
    question: Option<Value>,
    waiters: BTreeMap<RequestId,Waiter>,
    wires: BTreeMap<String,Wire>,
    retired_writes: BTreeSet<String>,
    policies: BTreeMap<RequestId,String>,
    last_format_request: Option<RequestId>,
    format_gate: FormatGate,
    reload_deadline: f64,
    rate_limit_info: Value,
    /// Limite do plano visto na mensagem sintética do turno; sai como efeito quando o turno termina.
    turn_limit: Option<UsageLimit>,
    /// O `error` da mensagem sintética do turno; o `result` não traz esse código.
    turn_error: Option<String>,
    commands: Option<Value>,
    terminal_commands: Value,
    preview: LiveBuffer,
    thinking: LiveBuffer,
    tool_input: LiveBuffer,
    tool_name: Option<String>,
    tool_visible: bool,
    label: Option<String>,
    /// `api_retry` em curso: vence os outros rótulos até a resposta voltar.
    retry_label: Option<String>,
    /// Hooks em curso por `hook_id`: o que passa de `HOOK_SLOW_S` vira rótulo.
    hooks_running: BTreeMap<String,RunningHook>,
    /// Falhas de cada hook no turno (`hook_name` → detalhe da primeira, total): a primeira vai à conversa, as outras só somam.
    hook_streaks: BTreeMap<String,(String,u32)>,
    /// Registros no diário por tipo nesta vida: passado o teto, o motor nem pede a gravação.
    diary_counts: BTreeMap<String,u32>,
    compacting: bool,
    turn_start: Option<f64>,
    label_deadline: Option<f64>,
    tokens_closed: u64,
    tokens_message: Option<u64>,
    gen_start: Option<f64>,
    token_chars: u64,
    thinking_start: Option<f64>,
    thought_s: f64,
    tasks: Vec<(String,Value)>,
    usage: Value,
    context_window: Option<u64>,
    cost: Option<f64>,
    effort_intent: Option<Value>,
    effort_deadline: Option<f64>,
    active_input: Option<String>,
    /// Último `/comando` escrito no processo: a CLI responde alguns sozinha, sem linha no transcript.
    slash_input: Option<String>,
    /// O que já foi avisado nesta vida do processo: tipos desconhecidos e as chaves de `notice_once`.
    unknown: BTreeSet<String>,
    /// Linhas do retrato do cano repassadas na adoção: a nota delas já está na conversa.
    replaying: bool,
    surface: Option<Surface>,
    /// Últimas linhas do stderr do processo: o motivo de uma queda (`not logged in`, CLI quebrada).
    stderr_tail: Vec<String>,
    /// O problema da queda está no arquivo da sessão: o próximo turno bom o apaga de lá.
    problem_persisted: bool,
    /// Ciclo de cada subagente (`local_agent`) por agentId: a última execução manda (`run_id` novo = retomado).
    subagents: BTreeMap<String,Value>,
    /// `uuid` de cada mensagem escrita → operação: o eco com o mesmo `uuid` prova a entrega. Teto pequeno porque
    /// prompt barrado por hook nunca ecoa.
    echoes: std::collections::VecDeque<(String,String)>,
    /// `task_id` do último `background_tasks_changed`: o trabalho de fundo segue vivo depois do `result`.
    background: BTreeSet<String>,
    /// Último evento da CLI ou comando nosso, no relógio do motor: daqui conta o tempo parado.
    active_at: f64,
    /// O cano parou de repassar a saída da CLI (`headless_cano_surdo`): entrega desligada até trocar o processo.
    deaf: bool,
    idle_compact: IdleCompact,
}

/// O que a compactação da sessão parada precisa saber do último turno visto nesta vida do processo.
#[derive(Default)]
struct IdleCompact {
    /// O `usage` veio de uma resposta desta vida, não do transcript relido: só aí o cache ainda pode estar quente.
    live: bool,
    /// Cache de 1 h no último turno que gravou cache; turno só de leitura não diz.
    one_hour: Option<bool>,
    /// Compactou desde o último uso: não há o que compactar.
    done: bool,
    /// Quando chegou o último `usage`: o prazo do cache conta da última chamada à API, não de qualquer evento.
    cache_at: f64,
    /// A compactação em curso ou feita foi a da sessão parada, não um pedido da pessoa.
    auto: bool,
    /// Já se pediu o `/compact` da parada desde o último uso: sem `compact_boundary`, não se pede de novo.
    tried: bool,
}

const ECHO_LIMIT: usize = 64;

/// Hook rápido não aparece: só o que passa disto vira rótulo.
const HOOK_SLOW_S: f64 = 3.0;
/// O mesmo teto por tipo do diário (`local_policy::UNKNOWN_PER_KIND`): cada pedido ao diário redesenha a vista.
const DIARY_PER_KIND: u32 = 30;

struct RunningHook { name:String, started:f64, shown:bool }

/// UUID estável da mensagem de uma operação (formato v5 sobre o SHA-1 do ID): reescrever o mesmo quadro
/// depois de reiniciar mantém o eco casável.
fn echo_uuid(operation_id:&str) -> String {
    let mut bytes = sha1_smol::Sha1::from(operation_id.as_bytes()).digest().bytes();
    bytes[6] = (bytes[6] & 0x0f) | 0x50; bytes[8] = (bytes[8] & 0x3f) | 0x80;
    let hex:String = bytes[..16].iter().map(|b|format!("{b:02x}")).collect();
    format!("{}-{}-{}-{}-{}",&hex[..8],&hex[8..12],&hex[12..16],&hex[16..20],&hex[20..32])
}

const STDERR_TAIL: usize = 10;

fn string(value:&Value) -> Option<String> { value.as_str().map(str::to_owned) }
fn clip(text:&str,max:usize) -> String {
    match text.char_indices().nth(max) { Some((at,_)) => format!("{}…",&text[..at]), None => text.to_owned() }
}
fn error(message:&str) -> RuntimeError { RuntimeError::new("claude_command",message) }
fn mode(value:&str) -> String { if value == "default" { "manual".into() } else { value.into() } }

impl ClaudeEngine {
    pub fn new(metadata:Value,generation:u64,clock:ClockSample) -> Self {
        let permission_mode = metadata["permission_mode"].as_str().map(mode);
        let previous_non_plan = string(&metadata["previous_non_plan"]);
        // A queda da vida anterior mora no arquivo da sessão (`problema: [código, detalhe]`, o formato do Python).
        let persisted = metadata["problema"].as_array().and_then(|pair|Some((pair.first()?.as_str()?.to_owned(),pair.get(1).and_then(Value::as_str).map(str::to_owned))));
        let state = StateEvent { session:metadata["name"].as_str().unwrap_or("").into(), state:"idle".into(),
            headless:true, claude_permission_mode:permission_mode.clone(),claude_previous_non_plan:previous_non_plan.clone(),
            status_line:string(&metadata["status_line"]),problema:persisted.as_ref().map(|(code,_)|code.clone()),
            problema_detalhe:persisted.as_ref().and_then(|(_,detail)|detail.clone()),..StateEvent::default() };
        let problem_persisted = persisted.is_some();
        let mut effort_intent = metadata["effort_intent"].is_object().then(||metadata["effort_intent"].clone());
        if let Some(intent) = effort_intent.as_mut() {
            if intent["status"] == "dispatching" { intent["status"] = json!("unknown"); }
        }
        let mut engine = Self { interactions:None,generation,clock,counter:metadata["runtime_counter"].as_u64().unwrap_or(0),alive:true,transport_up:true,
            initialized:metadata["initialized"] == true,initializing:metadata["initialized"] != true,
            init_warning:None,in_progress:false,state,model:string(&metadata["model"]),effort:string(&metadata["effort"]),
            permission_mode,previous_non_plan,restore_plan:None,pending:Vec::new(),question:None,
            waiters:BTreeMap::new(),wires:BTreeMap::new(),retired_writes:BTreeSet::new(),policies:BTreeMap::new(),last_format_request:None,format_gate:FormatGate::default(),reload_deadline:clock.monotonic_s+10.0,
            rate_limit_info:Value::Null,turn_limit:None,turn_error:None,commands:metadata.get("commands").filter(|commands|!commands.is_null()).cloned(),terminal_commands:metadata.get("terminal_commands").cloned().unwrap_or_else(||json!([])),
            preview:LiveBuffer::default(),thinking:LiveBuffer::default(),tool_input:LiveBuffer::default(),tool_name:None,tool_visible:false,
            label:None,retry_label:None,hooks_running:BTreeMap::new(),hook_streaks:BTreeMap::new(),diary_counts:BTreeMap::new(),compacting:false,turn_start:None,label_deadline:None,tokens_closed:0,tokens_message:None,gen_start:None,
            token_chars:0,thinking_start:None,thought_s:0.0,tasks:Vec::new(),usage:metadata.get("usage").cloned().unwrap_or(Value::Null),
            context_window:metadata["context_window"].as_u64(),cost:metadata["cost"].as_f64(),effort_intent,effort_deadline:None,active_input:None,slash_input:None,unknown:BTreeSet::new(),replaying:false,surface:None,stderr_tail:Vec::new(),problem_persisted,subagents:BTreeMap::new(),echoes:std::collections::VecDeque::new(),background:BTreeSet::new(),active_at:clock.monotonic_s,deaf:false,idle_compact:IdleCompact::default(),metadata };
        if let Some(controls) = engine.metadata["control_carry"].as_array() {
            for control in controls {
                let Ok(request_id) = serde_json::from_value::<RequestId>(control["request_id"].clone()) else { continue };
                let (Some(operation_id),Some(subtype)) = (control["operation_id"].as_str(),control["subtype"].as_str()) else { continue };
                engine.waiters.insert(request_id.clone(),Waiter { operation_id:operation_id.into(),subtype:subtype.into(),
                    payload:control["payload"].clone(),deadline:clock.monotonic_s,timed_out:true });
                engine.wires.insert(operation_id.into(),Wire { interaction:None,kind:"control".into(),request_id:Some(request_id),final_result:false });
            }
        }
        engine
    }

    pub fn write_interaction(&self,id:&str) -> Option<&ExpectedInteraction> { self.wires.get(id)?.interaction.as_ref() }

    pub fn interaction_source(&self,expected:&ExpectedInteraction) -> Option<ExpectedInteraction> {
        let native = expected.native_id(None).ok()?;
        let request = match expected.target.kind {
            InteractionKind::Approval => self.pending.iter().find(|(id,_)|json!(id) == native).map(|(_,request)|request),
            InteractionKind::Question => self.question.as_ref().filter(|question|question["request_id"] == native),
            _ => None,
        }?;
        let (key,_) = self.interactions.as_ref()?;
        let conversation = self.metadata["session_id"].as_str()?;
        Some(ExpectedInteraction { binding:InteractionBinding { key:key.clone(),conversation:conversation.into(),generation:self.generation },
            target:native_target(expected.target.kind.clone(),conversation,&native),interaction_id:expected.interaction_id.clone(),
            semantic:request.clone(),source_revision:None })
    }

    pub fn interaction_write_error(&self,id:&str) -> Option<RuntimeError> {
        let expected = self.wires.get(id)?.interaction.as_ref()?;
        let check = || -> Result<(),RuntimeError> {
            if self.expected(&json!({"interaction_id":expected.interaction_id}))? != *expected { return Err(runtime_error(InteractionError::Changed)); }
            let native = expected.native_id(None).map_err(runtime_error)?;
            let request = match expected.target.kind {
                InteractionKind::Approval => self.pending.iter().find(|(id,_)|json!(id) == native).map(|(_,request)|request),
                InteractionKind::Question => self.question.as_ref().filter(|q|q["request_id"] == native),
                _ => None,
            }.ok_or_else(||runtime_error(InteractionError::Changed))?;
            expected.check(&expected.target,request,None).map_err(runtime_error)
        };
        check().err()
    }

    pub fn write_is_current(&self,id:&str) -> bool { !self.retired_writes.contains(id) }

    /// Liga a superfície remota dos mods. Sem ela o motor segue como antes: nenhum `ui_*` sai e um
    /// `ui_copy` recebe a resposta vazia de pedido desconhecido.
    pub fn enable_surface(&mut self,prefix:String) { self.surface = Some(Surface::new(prefix)); }

    fn surface_out(effects:&mut Vec<Effect>,out:Vec<SurfaceEffect>) {
        effects.extend(out.into_iter().map(|effect|Effect::Surface { effect }));
    }

    /// Liga a superfície ao processo: no `initialize` que a oferece e ao religar a um processo já iniciado.
    fn start_surface(&mut self,effects:&mut Vec<Effect>) {
        let now = self.clock.monotonic_s;
        if let Some(surface) = self.surface.as_mut() { let out = surface.start(now); Self::surface_out(effects,out); }
    }

    /// Pedido de um app; a resposta sai depois, como `SurfaceEffect::Reply` com o mesmo `token`. `left_s`:
    /// quanto falta para quem pediu deixar de esperar.
    pub fn mods_call(&mut self,token:u64,call:ModsCall,left_s:f64,clock:ClockSample) -> Result<Vec<Effect>,ModsError> {
        self.clock = clock;
        let surface = self.surface.as_mut().ok_or_else(missing)?;
        let mut effects = Vec::new();
        Self::surface_out(&mut effects,surface.call(token,call,clock.monotonic_s,clock.monotonic_s+left_s));
        Ok(effects)
    }

    fn reset_conversation(&mut self,effects:&mut Vec<Effect>) {
        for (id,wire) in &mut self.wires {
            self.retired_writes.insert(id.clone());
            if !wire.final_result {
                effects.push(Effect::Reply { operation_id:id.clone(),disposition:Disposition::Unknown,
                    payload:json!({"reason":"conversation_changed"}) });
                wire.final_result = true;
            }
        }
        self.waiters.clear(); self.policies.clear(); self.last_format_request = None; self.format_gate.reset();
        self.pending.clear(); self.question = None; self.effort_intent = None; self.subagents.clear();
        self.tasks.clear(); self.background.clear(); self.label = None; self.drop_turn_notices();
        self.effort_deadline = None; self.active_input = None; self.restore_plan = None;
        self.in_progress = false; self.turn_start = None; self.label_deadline = None;
        self.clear_streams(effects);
    }

    pub fn view(&self) -> Value {
        let state = self.public_state();
        json!({"public_state":state,"alive":self.alive,"iniciando":self.initializing,"initialized":self.initialized,
            "in_progress":self.in_progress,"turn_started":self.turn_start,"pending":self.pending.iter().map(|(id,request)|json!({"request_id":id,"request":request})).collect::<Vec<_>>(),
            "question":self.question,"model":self.model,"effort":self.effort,"permission_mode":self.permission_mode,
            "previous_non_plan":self.previous_non_plan,"commands":self.commands,"terminal_commands":self.terminal_commands,
            "usage":self.usage,"context_window":self.context_window,"cost":self.cost,"effort_intent":self.effort_intent,
            "runtime_counter":self.counter,"conversation":self.metadata["session_id"],"deliverable":self.deliverable(),
            "subagents":self.subagents,"idle_compact_tokens":self.idle_compact_tokens(),
            "idle_compacted":self.idle_compact.auto && self.idle_compact.done})
    }

    pub(super) fn input_frame(operation_id:&str,payload:&Value) -> Value {
        let content=payload.get("content").cloned().unwrap_or_else(||json!([{"type":"text","text":payload["text"]}]));
        json!({"type":"user","uuid":echo_uuid(operation_id),"session_id":"","parent_tool_use_id":null,
            "message":{"role":"user","content":content}})
    }

    fn public_state(&self) -> StateEvent {
        let mut state = self.state.clone();
        state.claude_permission_mode = self.permission_mode.clone();
        state.claude_previous_non_plan = self.previous_non_plan.clone();
        state.codex_question = self.question.as_ref().and_then(|q|q.as_object().cloned());
        state.question = None; state.options = None; state.claude_plan_pending = None;
        // Processo morto não é "Sessão encerrada": a faixa mostra o problema e o próximo prompt sobe outro.
        state.state = if !self.alive { "idle" } else if !self.pending.is_empty() || self.question.is_some() { "awaiting_input" }
            else if self.in_progress || self.initializing { "working" } else { "idle" }.into();
        let mut label = self.label.clone();
        if self.initializing && !self.in_progress { label = Some("Iniciando sessão…".into()); }
        // Hook assíncrono começado com a sessão parada pode não responder: o rótulo ficaria preso até o próximo turno.
        if let Some(hook) = self.hooks_running.values().filter(|hook|hook.shown && (self.in_progress || self.initializing))
            .min_by(|a,b|a.started.total_cmp(&b.started)) {
            label = Some(format!("Rodando hook {}…",hook.name));
        }
        if let Some(retry) = &self.retry_label { label = Some(retry.clone()); }
        if state.state == "working" {
            if let Some(start) = self.turn_start {
                let seconds = (self.clock.monotonic_s - start).max(0.0) as u64;
                let elapsed = if seconds >= 60 { format!("{}m {}s",seconds/60,seconds%60) } else { format!("{seconds}s") };
                let mut counts = vec![elapsed];
                let tokens = self.tokens_closed + self.tokens_message.unwrap_or(self.token_chars/4);
                if tokens > 0 { counts.push(if tokens >= 1000 { format!("↓ {:.1}k tokens",tokens as f64/1000.0) } else { format!("↓ {tokens} tokens") }); }
                let thought = self.thought_s + self.thinking_start.map_or(0.0,|start|(self.clock.monotonic_s-start).max(0.0));
                if thought >= 1.0 { counts.push(format!("thought for {}s",thought as u64)); }
                label = Some(format!("{} ({})",label.as_deref().unwrap_or("Trabalhando…"),counts.join(" · ")));
            }
        }
        state.label = label;
        if self.question.is_none() {
            if let Some((_,request)) = self.pending.first() {
                if request["tool_name"] == "ExitPlanMode" {
                    state.question = Some("Aprovar o plano?".into());
                    state.options = Some(vec!["Aprovar plano".into(),"Continuar planejando".into()]);
                    state.claude_plan_pending = json!({"plan":request["input"]["plan"].as_str().unwrap_or(""),
                        "path":request["input"]["planFilePath"].as_str(),"tool_use_id":request["tool_use_id"].as_str()}).as_object().cloned();
                } else {
                    state.question = Some(permission_text(request));
                    let mut options = vec!["Permitir".into(),"Negar".into()];
                    if request["permission_suggestions"].as_array().is_some_and(|rules|rules.iter().any(Value::is_object)) { options.push("Sempre permitir".into()); }
                    state.options = Some(options);
                }
            }
        }
        self.stamp_shown(&mut state);
        state
    }

    /// O app só responde com o ID do cartão que viu: ele sai daqui, com o mesmo alvo e conteúdo que a resposta confere.
    // ponytail: o alvo respondido não é encerrado no store; a resposta já recusa ID de pedido que saiu.
    fn stamp_shown(&self,state:&mut StateEvent) {
        let (kind,native,semantic) = if let Some(question) = &self.question {
            (InteractionKind::Question,question["request_id"].clone(),question)
        } else if let Some((id,request)) = self.pending.first() {
            (InteractionKind::Approval,json!(id),request)
        } else { return };
        let observed = (|| {
            let (key,store) = self.interactions.as_ref().ok_or(InteractionError::Unavailable)?;
            if kind == InteractionKind::Question && state.codex_question.is_none() { return Err(InteractionError::Unavailable); }
            let conversation = self.metadata["session_id"].as_str().unwrap_or("");
            let binding = InteractionBinding { key:key.clone(),conversation:conversation.into(),generation:self.generation };
            store.observe(&binding,&native_target(kind.clone(),conversation,&native),Some(semantic.clone()))?.ok_or(InteractionError::Unavailable)
        })();
        match (observed,state.codex_question.as_mut()) {
            (Ok(id),Some(form)) if kind == InteractionKind::Question => { form.insert("interaction_id".into(),json!(id)); }
            (Ok(id),_) => state.interaction_id = Some(id),
            // Sem ID a resposta seria recusada: o cartão não pode parecer respondível.
            (Err(error),_) => {
                crate::interactions::mark_unidentified(state,error);
                state.options = None; state.codex_question = None; state.claude_plan_pending = None; state.interaction_id = None;
            }
        }
    }

    pub(super) fn deliverable(&self) -> bool { self.alive && self.transport_up && self.initialized && !self.initializing && !self.in_progress
        && self.pending.is_empty() && self.question.is_none() && self.active_input.is_none() }

    /// O `conversation` da vista, para quem só precisa dele.
    pub(super) fn conversation(&self) -> &str { self.metadata["session_id"].as_str().unwrap_or("") }

    pub fn forget_policy(&mut self,request_id:&RequestId) {
        // Pedido de status que falhou não pode barrar o próximo igual.
        if self.policies.remove(request_id).as_deref() == Some("format_status") { self.format_gate.reset(); }
    }

    pub(super) fn queue_problem(&mut self,detail:Option<String>) -> Vec<Effect> {
        let mut effects = Vec::new();
        if super::queue::queue_problem(&mut self.state,detail) { self.changed(&mut effects,false); }
        effects
    }

    fn policy(&mut self, kind:&str,payload:Value,effects:&mut Vec<Effect>) {
        self.counter += 1;
        let request_id = RequestId::String(format!("policy:{}:{}",self.generation,self.counter));
        if kind == "format_status" { self.last_format_request = Some(request_id.clone()); }
        self.policies.insert(request_id.clone(),kind.into());
        effects.push(Effect::Policy { kind:kind.into(),request_id,payload });
    }

    /// Problema que vem de fora do motor; o próximo turno bem-sucedido o apaga, como os dele.
    pub fn set_problem(&mut self,code:&str,detail:Option<String>) -> Vec<Effect> {
        self.state.problema = Some(code.into());
        self.state.problema_detalhe = detail.map(|detail|detail.chars().take(300).collect());
        let mut effects = Vec::new();
        self.changed(&mut effects,false);
        effects
    }

    /// Apaga só o problema `code`: o de outra origem que entrou depois fica.
    pub fn clear_problem(&mut self,code:&str) -> Vec<Effect> {
        let mut effects = Vec::new();
        if self.state.problema.as_deref() == Some(code) {
            self.state.problema = None; self.state.problema_detalhe = None;
            self.changed(&mut effects,false);
        }
        effects
    }

    /// O transcript provou o fim do turno que o cano não repassou (`result` perdido): o turno sai da vista, sem
    /// resposta inventada (a CLI já a gravou), e daqui em diante o fim de cada turno desta vida vem do transcript.
    pub fn transcript_turn_end(&mut self) -> Vec<Effect> {
        let mut effects = Vec::new();
        self.deaf = true;
        self.in_progress = false; self.turn_start = None; self.label_deadline = None; self.label = None;
        self.end_turn_notices(&mut effects);
        self.pending.clear(); self.question = None;
        if let Some(id) = self.active_input.take() {
            if let Some(wire) = self.wires.get_mut(&id) { wire.final_result = true; }
            effects.push(Effect::Reply { operation_id:id,disposition:Disposition::Accepted,payload:json!({"consumed":true}) });
        }
        self.clear_streams(&mut effects);
        self.changed(&mut effects,false);
        // Como no `result`: o esforço preparado sai antes da fila.
        if self.effort_intent.as_ref().is_some_and(|intent|intent["status"] == "prepared") { self.dispatch_effort(&mut effects); }
        else { effects.push(Effect::WakeQueue); }
        effects
    }

    /// A saída do cano já foi dada como perdida nesta vida: o fim do turno vem do transcript, sem prazo.
    pub fn deaf(&self) -> bool { self.deaf }
    pub fn active_input(&self) -> Option<&str> { self.active_input.as_deref() }
    /// Turno aberto com o processo vivo e o socket de pé: o que a vigília do transcript confere.
    pub fn turn_watch(&self) -> bool { self.in_progress && self.alive && self.transport_up }

    /// Linha da CLI chegou pelo cano: ele lê o stdout, e o fim do turno volta a ser o `result` (com o prazo de antes).
    pub fn heard(&mut self) -> Vec<Effect> {
        let mut effects = Vec::new();
        if std::mem::take(&mut self.deaf) { self.changed(&mut effects,false); }
        effects
    }

    /// Só o socket com o cano caiu; o processo segue. A fila deixa de entregar e o que não saiu volta
    /// como não escrito; o que pode ter saído (`handed`) fica incerto e a conferência pelo transcript decide.
    pub fn transport_lost(&mut self,handed:&BTreeSet<String>) -> Vec<Effect> {
        let mut effects = Vec::new();
        self.transport_up = false;
        let open:Vec<String> = self.wires.iter().filter(|(_,wire)|!wire.final_result).map(|(id,_)|id.clone()).collect();
        for operation_id in open {
            if handed.contains(&operation_id) { self.uncertain_wire(operation_id,&mut effects); }
            else { self.on_ack(operation_id,WriteOutcome::NotWritten,&mut effects); }
        }
        self.changed(&mut effects,false);
        effects
    }

    /// Socket novo ao MESMO processo: o que só existia na conexão anterior sai e o retrato do cano repõe
    /// turno, perguntas e prévia, como na subida do servidor.
    pub fn reattach(&mut self,snapshot:CanoSnapshot) -> Result<Vec<Effect>,RuntimeError> {
        let mut effects = Vec::new();
        // O que podia ter saído já ficou incerto na queda: o que segue aberto nasceu com o socket fora e não saiu.
        let open:Vec<String> = self.wires.iter().filter(|(_,wire)|!wire.final_result).map(|(id,_)|id.clone()).collect();
        for operation_id in open { self.on_ack(operation_id,WriteOutcome::NotWritten,&mut effects); }
        self.waiters.clear();
        self.transport_up = true;
        self.in_progress = false; self.turn_start = None; self.label_deadline = None; self.active_input = None;
        self.drop_turn_notices();
        self.pending.clear(); self.question = None; self.clear_streams(&mut effects);
        effects.extend(self.hydrate(snapshot)?);
        Ok(effects)
    }

    fn uncertain_wire(&mut self,operation_id:String,effects:&mut Vec<Effect>) {
        if let Some(wire) = self.wires.get_mut(&operation_id) { wire.final_result = true; }
        effects.push(Effect::Reply { operation_id,disposition:Disposition::Unknown,payload:json!({"error":"a conexão com o cano caiu"}) });
    }

    fn changed(&mut self,effects:&mut Vec<Effect>,format:bool) {
        // A faixa recebe `isWorking`: começo e fim de turno pedem um desenho novo dela.
        let (now,working) = (self.clock.monotonic_s,self.in_progress);
        if let Some(surface) = self.surface.as_mut() { surface.set_working(working,now); }
        effects.push(Effect::StateChanged);
        if !format { return; }
        let payload = json!({"model":self.model,"effort":self.effort,"usage":self.usage,
            "context_window":self.context_window,"cost":self.cost,"rate_limit_info":self.rate_limit_info});
        if self.format_gate.due(&payload,self.clock.monotonic_s) { self.policy("format_status",payload,effects); }
    }

    fn publish(&self,channel:&str,text:String,effects:&mut Vec<Effect>) {
        effects.push(Effect::Publish { channel:channel.into(),data:json!({"session":self.state.session,
            "text":text,"md":true,"full":true,"vivo":true}) });
    }

    fn clear_streams(&mut self,effects:&mut Vec<Effect>) {
        if let Some(text) = self.preview.clear() { self.publish("preview",text,effects); }
        if let Some(text) = self.thinking.clear() { self.publish("thinking",text,effects); }
        self.clear_tool(effects);
        self.tool_name = None;
    }

    fn clear_tool(&mut self,effects:&mut Vec<Effect>) {
        self.tool_input.clear();
        if std::mem::take(&mut self.tool_visible) { self.publish("tool",String::new(),effects); }
    }

    fn start_turn(&mut self) {
        self.in_progress = true; self.turn_start = Some(self.clock.monotonic_s);
        self.label_deadline = Some(self.clock.monotonic_s+1.0); self.tokens_closed = 0; self.tokens_message = None;
        self.token_chars = 0; self.thinking_start = None; self.thought_s = 0.0; self.compacting = false;
        // Hook já lento antes do turno começou com a sessão parada (assíncrono que talvez nunca responda).
        self.hooks_running.retain(|_,hook|!hook.shown);
    }

    fn control(&mut self,operation_id:String,subtype:&str,payload:Value,effects:&mut Vec<Effect>) {
        self.counter += 1;
        let request_id = RequestId::String(format!("hangar:{}:{}",self.generation,self.counter));
        let mut request = payload.as_object().cloned().unwrap_or_default();
        request.insert("subtype".into(),json!(subtype));
        self.waiters.insert(request_id.clone(),Waiter { operation_id:operation_id.clone(),subtype:subtype.into(),
            payload:payload.clone(),deadline:self.clock.monotonic_s + if subtype == "initialize" { 180.0 } else { 15.0 },timed_out:false });
        self.wires.insert(operation_id.clone(),Wire { interaction:None,kind:"control".into(),request_id:Some(request_id.clone()),final_result:false });
        effects.push(Effect::Write { frame:json!({"type":"control_request","request_id":request_id,"request":request}),operation_id:Some(operation_id) });
    }

    pub fn restore_control(&mut self,operation_id:String,frame:&Value) {
        if frame["type"] != "control_request" {
            let kind = match frame["type"].as_str() {
                Some("control_response")=>"recovered_answer",
                Some("user") if self.effort_intent.as_ref().is_some_and(|intent|
                    operation_id == format!("{}:effort",intent["operation_id"].as_str().unwrap_or("")))=>"effort",
                Some("user")=>"input",
                _=>return,
            };
            if kind == "input" && frame["uuid"].is_string() { self.expect_echo(&operation_id); }
            self.wires.insert(operation_id,Wire { interaction:None,kind:kind.into(),request_id:None,final_result:false });
            return;
        }
        let (Ok(request_id),Some(subtype)) = (serde_json::from_value::<RequestId>(frame["request_id"].clone()),frame["request"]["subtype"].as_str()) else { return };
        if self.waiters.contains_key(&request_id) { return; }
        let mut payload = frame["request"].clone();
        if let Some(fields) = payload.as_object_mut() { fields.remove("subtype"); }
        self.waiters.insert(request_id.clone(),Waiter { operation_id:operation_id.clone(),subtype:subtype.into(),payload,
            deadline:self.clock.monotonic_s,timed_out:true });
        self.wires.insert(operation_id,Wire { interaction:None,kind:"control".into(),request_id:Some(request_id),final_result:false });
    }

    fn expect_echo(&mut self,operation_id:&str) -> String {
        let uuid = echo_uuid(operation_id);
        if !self.echoes.iter().any(|(known,_)|*known == uuid) {
            if self.echoes.len() >= ECHO_LIMIT { self.echoes.pop_front(); }
            self.echoes.push_back((uuid.clone(),operation_id.into()));
        }
        uuid
    }

    /// Resposta que o motor dá sozinho: cada uma com ID próprio, senão a segunda é recusada no despacho e a CLI espera.
    fn server_answer_id(&mut self) -> String {
        self.counter += 1;
        format!("server:{}:{}",self.generation,self.counter)
    }

    fn answer(&mut self,operation_id:String,request_id:RequestId,body:Value,effects:&mut Vec<Effect>) {
        self.wires.insert(operation_id.clone(),Wire { interaction:None,kind:"answer".into(),request_id:Some(request_id.clone()),final_result:false });
        effects.push(Effect::Write { frame:json!({"type":"control_response","response":{
            "subtype":"success","request_id":request_id,"response":body}}),operation_id:Some(operation_id) });
    }

    pub fn start_initialize(&mut self,operation_id:String) -> Result<Vec<Effect>,RuntimeError> {
        let mut effects = Vec::new();
        self.initialized = false; self.initializing = true;
        self.init_warning = Some(self.clock.monotonic_s+60.0);
        self.control(operation_id,"initialize",json!({}),&mut effects);
        self.changed(&mut effects,true);
        Ok(effects)
    }

    pub fn hydrate(&mut self,snapshot:CanoSnapshot) -> Result<Vec<Effect>,RuntimeError> {
        let mut effects = Vec::new();
        self.alive = snapshot.saiu.is_none();
        self.stderr_tail = snapshot.stderr_tail.iter().rev().take(STDERR_TAIL).rev().cloned().collect();
        self.replaying = true;
        let replayed:Result<(),RuntimeError> = snapshot.init.iter().chain(snapshot.rate_limit.iter()).try_for_each(|raw|{
            effects.extend(self.apply(EngineInput::Line(serde_json::from_str(raw).map_err(|_|error("snapshot inválido"))?),self.clock)?);
            Ok(())
        });
        self.replaying = false;
        replayed?;
        if snapshot.aberto { self.start_turn(); }
        for raw in snapshot.pendentes {
            effects.extend(self.apply(EngineInput::Line(serde_json::from_str(&raw).map_err(|_|error("pedido pendente inválido"))?),self.clock)?);
        }
        let prefix = &snapshot.inflight["claude"];
        if prefix["complete"] == false { self.preview.block(); self.thinking.block(); self.tool_input.block(); }
        else {
            if let Some(text) = self.preview.append(prefix["text"].as_str().unwrap_or(""),self.clock.monotonic_s) { self.publish("preview",text,&mut effects); }
            if let Some(text) = self.thinking.append(prefix["thinking"].as_str().unwrap_or(""),self.clock.monotonic_s) { self.publish("thinking",text,&mut effects); }
            self.tool_name = string(&prefix["tool"]["name"]);
            if let Some(text) = self.tool_input.append(prefix["tool"]["input"].as_str().unwrap_or(""),self.clock.monotonic_s) { self.publish_tool(text,&mut effects); }
        }
        if let Some(raw) = snapshot.ultimo_result {
            let result:Value = serde_json::from_str(&raw).map_err(|_|error("resultado do snapshot inválido"))?;
            self.apply_usage(&result,&mut effects);
        }
        self.changed(&mut effects,true);
        if self.usage.is_null() { self.policy("last_usage",json!({}),&mut effects); }
        self.policy("reload_stamp",json!({}),&mut effects);
        // Religado a um processo já iniciado (Rust reiniciado ou reconexão ao cano): liga de novo; o
        // `ui_attach` repetido é aceito e reenvia o rol (E3). Processo velho sem a capacidade recusa,
        // e a superfície desliga sem mais pedidos.
        // Só com o processo vivo (A15): com `saiu` no retrato, o `ui_attach` iria a um processo morto e
        // venceria em 15 s.
        if self.initialized && self.alive { self.start_surface(&mut effects); }
        Ok(effects)
    }

    pub(super) fn advance_generation(&mut self,generation:u64)->Result<(),RuntimeError> {
        if self.generation.checked_add(1)!=Some(generation) {return Err(error("geração da conversa mudou"));}
        self.generation=generation;
        self.bind_interactions(self.metadata["session_id"].as_str().unwrap_or(""))
    }
    pub fn set_interactions(&mut self,key:String,store:Arc<InteractionStore>) -> Result<(),RuntimeError> {
        self.interactions = Some((key,store));
        self.bind_interactions(self.metadata["session_id"].as_str().unwrap_or(""))
    }

    fn bind_interactions(&self,conversation:&str) -> Result<(),RuntimeError> {
        if !conversation.is_empty() && let Some((key,store)) = &self.interactions {
            store.bind(&InteractionBinding { key:key.clone(),conversation:conversation.into(),generation:self.generation }).map_err(runtime_error)?;
        }
        Ok(())
    }

    fn expected(&self,payload:&Value) -> Result<ExpectedInteraction,RuntimeError> {
        let supplied = payload["interaction_id"].as_str().filter(|id|!id.is_empty()).ok_or_else(||runtime_error(InteractionError::Missing))?;
        let (key,store) = self.interactions.as_ref().ok_or_else(||runtime_error(InteractionError::Unavailable))?;
        let conversation = self.metadata["session_id"].as_str().filter(|id|!id.is_empty()).ok_or_else(||runtime_error(InteractionError::Unavailable))?;
        store.expected(&InteractionBinding { key:key.clone(),conversation:conversation.into(),generation:self.generation },Some(supplied)).map_err(runtime_error)
    }

    pub fn command(&mut self,command:RuntimeCommand,clock:ClockSample) -> Result<Vec<Effect>,RuntimeError> {
        // O `/compact` da sessão parada reconfere aqui, na mesma vez do ator: o que chegou depois da vista cancela.
        if command.kind == OperationKind::Compact && let Some(after) = command.payload.get("idle_after_s") {
            let after = after.as_f64().ok_or_else(||error("idle_after_s precisa ser número"))?;
            if self.idle_compact_age(clock.monotonic_s).is_none_or(|age|age < after) {
                return Err(RuntimeError::new("idle_compact_stale","a sessão saiu da parada antes do /compact"));
            }
        }
        self.clock = clock;
        self.active_at = clock.monotonic_s;
        if !self.alive {
            // A queda levou os pedidos em aberto: responder a um deles é responder a algo que já não existe (409).
            if matches!(command.kind,OperationKind::Select | OperationKind::AnswerQuestions | OperationKind::SkipQuestion) {
                return Err(runtime_error(InteractionError::Changed));
            }
            return Err(error("processo não está vivo"));
        }
        let mut effects = Vec::new();
        let operation_id = command.operation_id;
        let payload = command.payload;
        match command.kind {
            OperationKind::Input | OperationKind::Steer => {
                if command.kind == OperationKind::Input && !self.deliverable() {
                    return Ok(vec![Effect::Reply { operation_id,disposition:Disposition::Deferred,payload:json!({}) }]);
                }
                let text = payload["text"].as_str().ok_or_else(||error("mensagem sem texto"))?;
                if text.trim_start().starts_with('/') { self.slash_input = Some(text.trim().to_owned()); }
                self.wires.insert(operation_id.clone(),Wire { interaction:None,kind:"input".into(),request_id:None,final_result:false });
                if !self.in_progress {
                    self.active_input = Some(operation_id.clone());
                    self.start_turn();
                }
                self.expect_echo(&operation_id);
                let frame=Self::input_frame(&operation_id,&payload);
                effects.push(Effect::Write { operation_id:Some(operation_id),frame });
            }
            OperationKind::Interrupt => {
                // Sem turno não há o que interromper: dizer isso, e não fingir um Parar que aconteceu.
                if !self.in_progress && self.pending.is_empty() && self.question.is_none() {
                    return Ok(vec![Effect::Reply { operation_id,disposition:Disposition::Accepted,payload:json!({"interrupted":false}) }]);
                }
                let mut requests:Vec<_> = self.pending.iter().map(|(id,_)|id.clone()).collect();
                if let Some(question) = &self.question {
                    requests.push(serde_json::from_value(question["request_id"].clone()).map_err(|_|error("ID da pergunta inválido"))?);
                }
                for (index,id) in requests.into_iter().enumerate() {
                    self.answer(format!("{operation_id}:deny:{index}"),id,json!({"behavior":"deny","message":"Interrompido pelo usuário."}),&mut effects);
                }
                self.restore_plan = None;
                self.control(operation_id.clone(),"interrupt",json!({}),&mut effects);
                // O Parar vale quando chega ao stdin (como no Python): esperar a resposta da CLI virava incerto.
                if let Some(wire) = self.wires.get_mut(&operation_id) { wire.kind = "interrupt".into(); }
                self.clear_streams(&mut effects);
            }
            OperationKind::Select => {
                let expected = self.expected(&payload)?;
                let (id,request) = self.pending.iter().find(|(id,_)|native_target(InteractionKind::Approval,&expected.binding.conversation,&json!(id)) == expected.target)
                    .cloned().ok_or_else(||runtime_error(InteractionError::Changed))?;
                expected.check(&native_target(InteractionKind::Approval,&expected.binding.conversation,&json!(id)),&request,None).map_err(runtime_error)?;
                if self.wires.values().any(|wire|wire.kind == "answer" && wire.request_id.as_ref() == Some(&id) && !wire.final_result) { return Err(runtime_error(InteractionError::Changed)); }
                let option = payload["option"].as_u64().filter(|n| (1..=3).contains(n)).ok_or_else(||error("opção inválida"))?;
                let suggestions:Vec<_> = request["permission_suggestions"].as_array().map(|rules|rules.iter().filter(|v|v.is_object()).cloned().collect()).unwrap_or_default();
                let allow = option == 1 || option == 3 && !suggestions.is_empty();
                let mut body = if allow { json!({"behavior":"allow","updatedInput":request.get("input").cloned().unwrap_or_else(||json!({}))}) }
                    else { json!({"behavior":"deny","message":if request["tool_name"] == "ExitPlanMode" {
                        "O usuário não aprovou o plano. Continue no modo plano e aguarde as instruções dele." } else { "Usuário recusou." }}) };
                if option == 3 && allow { body["updatedPermissions"] = json!(suggestions); }
                if request["tool_name"] == "ExitPlanMode" && allow {
                    self.restore_plan = self.previous_non_plan.as_ref().filter(|mode|mode.as_str() != "manual")
                        .map(|mode|(mode.clone(),operation_id.clone()));
                }
                self.answer(operation_id.clone(),id,body,&mut effects);
                self.wires.get_mut(&operation_id).unwrap().interaction = Some(expected);
            }
            OperationKind::AnswerQuestions | OperationKind::SkipQuestion => {
                let expected = self.expected(&payload)?;
                let question = self.question.as_ref().ok_or_else(||runtime_error(InteractionError::Changed))?;
                let request_id:RequestId = serde_json::from_value(expected.native_id(payload.get("request_id")).map_err(runtime_error)?)
                    .map_err(|_|runtime_error(InteractionError::Changed))?;
                expected.check(&native_target(InteractionKind::Question,&expected.binding.conversation,&json!(request_id)),question,None).map_err(runtime_error)?;
                if self.wires.values().any(|wire|wire.kind == "answer" && wire.request_id.as_ref() == Some(&request_id) && !wire.final_result) { return Err(runtime_error(InteractionError::Changed)); }
                let body = if command.kind == OperationKind::SkipQuestion { json!({"behavior":"deny","message":"O usuário prefere conversar antes de responder."}) }
                    else { answer_body(question,&payload["answers"])? };
                self.answer(operation_id.clone(),request_id,body,&mut effects);
                self.wires.get_mut(&operation_id).unwrap().interaction = Some(expected);
            }
            OperationKind::SetPermissionMode => {
                let requested = payload["mode"].as_str().ok_or_else(||error("modo não informado"))?;
                self.control(operation_id,"set_permission_mode",json!({"mode":requested}),&mut effects);
            }
            OperationKind::SetModel | OperationKind::SetEffort => {
                if let Some(model) = payload["model"].as_str().filter(|m|!m.is_empty()) {
                    self.control(operation_id.clone(),"set_model",json!({"model":model}),&mut effects);
                    if let Some(waiter) = self.waiters.values_mut().find(|waiter|waiter.operation_id == operation_id) {
                        waiter.payload = payload.clone();
                    }
                } else { self.request_effort(operation_id,&payload,&mut effects)?; }
            }
            OperationKind::ListModels => self.control(operation_id,"list_models",json!({}),&mut effects),
            OperationKind::Compact => {
                if !self.deliverable() { return Err(error("aguarde a sessão ficar ociosa")); }
                self.idle_compact.auto = payload.get("idle_after_s").is_some();
                self.idle_compact.tried |= self.idle_compact.auto;
                self.local_command(operation_id,"/compact",&mut effects);
            }
            OperationKind::StopTask => {
                let task_id = payload["task_id"].as_str().filter(|id|!id.is_empty()).ok_or_else(||error("task_id não informado"))?;
                self.control(operation_id.clone(),"stop_task",json!({"task_id":task_id}),&mut effects);
                // O `success` da CLI não prova a parada (id inventado também o recebe): responde na escrita.
                if let Some(wire) = self.wires.get_mut(&operation_id) { wire.kind = "stop_task".into(); }
            }
            OperationKind::Cwd => effects.push(Effect::Reply { operation_id,disposition:Disposition::Accepted,payload:self.metadata["cwd"].clone() }),
            OperationKind::Detach => effects.push(Effect::Stop { reason:"detach".into() }),
            _ => return Err(error("operação exige a barreira de lifecycle ou não é suportada pelo Claude")),
        }
        self.changed(&mut effects,true);
        Ok(effects)
    }

    fn local_command(&mut self,operation_id:String,text:&str,effects:&mut Vec<Effect>) {
        self.wires.insert(operation_id.clone(),Wire { interaction:None,kind:if text.starts_with("/effort ") { "effort" } else { "local" }.into(),request_id:None,final_result:false });
        self.start_turn();
        effects.push(Effect::Write { operation_id:Some(operation_id),frame:json!({"type":"user","session_id":"","parent_tool_use_id":null,
            "message":{"role":"user","content":[{"type":"text","text":text}]}}) });
    }

    fn request_effort(&mut self,operation_id:String,payload:&Value,effects:&mut Vec<Effect>) -> Result<(),RuntimeError> {
        let effort = payload["effort"].as_str().filter(|s|!s.is_empty()).ok_or_else(||error("esforço não informado"))?;
        if self.effort.as_deref() == Some(effort) {
            effects.push(Effect::Reply { operation_id,disposition:Disposition::Accepted,payload:json!({"applied":true}) });
            return Ok(());
        }
        self.effort_intent = Some(json!({"operation_id":operation_id,"value":effort,"status":"prepared"}));
        if self.deliverable() { self.dispatch_effort(effects); }
        else { effects.push(Effect::Reply { operation_id,disposition:Disposition::Deferred,payload:json!({"applied":false}) }); }
        Ok(())
    }

    fn dispatch_effort(&mut self,effects:&mut Vec<Effect>) {
        let Some(intent) = self.effort_intent.as_mut() else { return };
        if intent["status"] != "prepared" { return; }
        intent["status"] = json!("dispatching");
        self.effort_deadline = Some(self.clock.monotonic_s+15.0);
        let id = format!("{}:effort",intent["operation_id"].as_str().unwrap_or(""));
        let text = format!("/effort {}",intent["value"].as_str().unwrap_or(""));
        effects.push(Effect::StateChanged);
        self.local_command(id,&text,effects);
    }

    pub fn apply(&mut self,input:EngineInput,clock:ClockSample) -> Result<Vec<Effect>,RuntimeError> {
        self.clock = clock;
        let mut effects = Vec::new();
        match input {
            EngineInput::Line(event) => {
                // Batimento e stderr do cano chegam com a sessão parada: não são atividade (o Python também os ignorava).
                if !matches!(event["type"].as_str(),Some("keep_alive" | "cano_stderr")) { self.active_at = clock.monotonic_s; }
                self.on_line(event,&mut effects)?
            },
            EngineInput::WriteAck { operation_id,outcome } => self.on_ack(operation_id,outcome,&mut effects),
            EngineInput::Tick => self.on_tick(&mut effects),
            EngineInput::PolicyResult { request_id,payload } => {
                if let Some(kind) = self.policies.remove(&request_id) {
                    if kind == "format_status" && self.last_format_request.as_ref() != Some(&request_id) { return Ok(effects); }
                    match kind.as_str() {
                        "format_status" => {
                            self.state.status_line = string(&payload["status_line"]);
                            if payload.get("limit_reset").is_some() { self.state.limit_reset = string(&payload["limit_reset"]); }
                        }
                        "reload_stamp" => self.state.recarregar_motivo = string(&payload["reason"]),
                        "last_usage" => {
                            if self.usage.is_null() && payload["usage"].is_object() { self.usage = payload["usage"].clone(); }
                            self.changed(&mut effects,true);
                            return Ok(effects);
                        }
                        _ => {},
                    }
                    self.changed(&mut effects,false);
                }
            }
        }
        Ok(effects)
    }

    fn on_ack(&mut self,operation_id:String,outcome:WriteOutcome,effects:&mut Vec<Effect>) {
        let Some(wire) = self.wires.get_mut(&operation_id) else { return };
        if wire.final_result { return; }
        if wire.kind == "control" && outcome == WriteOutcome::Written { return; }
        if wire.kind == "effort" && outcome == WriteOutcome::Written { return; }
        let kind = wire.kind.clone();
        let request_id = wire.request_id.clone();
        if matches!(kind.as_str(),"interrupt" | "stop_task") && let Some(id) = &request_id { self.waiters.remove(id); }
        let disposition = match outcome { WriteOutcome::Written => Disposition::Accepted,
            WriteOutcome::NotWritten => Disposition::Rejected,WriteOutcome::Unknown => Disposition::Unknown };
        wire.final_result = outcome != WriteOutcome::Unknown;
        let interaction = wire.interaction.clone();
        if kind == "answer" && outcome == WriteOutcome::Written {
            self.pending.retain(|(id,request)|Some(id) != request_id.as_ref()
                || interaction.as_ref().is_some_and(|expected|expected.target.kind != InteractionKind::Approval || expected.semantic != *request));
            if self.question.as_ref().is_some_and(|question|
                serde_json::from_value::<RequestId>(question["request_id"].clone()).ok() == request_id
                    && interaction.as_ref().is_none_or(|expected|expected.target.kind == InteractionKind::Question && expected.semantic == *question)) {
                self.question = None;
            }
        }
        if kind == "input" && self.active_input.as_deref() == Some(operation_id.as_str()) && outcome == WriteOutcome::NotWritten {
            self.active_input = None; self.in_progress = false; self.turn_start = None; self.label_deadline = None;
        }
        if kind == "effort" && outcome != WriteOutcome::Written {
            self.effort_deadline = None;
            if let Some(intent) = self.effort_intent.as_mut() {
                if operation_id == format!("{}:effort",intent["operation_id"].as_str().unwrap_or("")) {
                    intent["status"] = json!(if outcome == WriteOutcome::NotWritten { "rejected" } else { "unknown" });
                }
            }
        }
        effects.push(Effect::Reply { operation_id,disposition,payload:json!({"write_outcome":outcome}) });
        self.changed(effects,true);
    }

    fn on_line(&mut self,event:Value,effects:&mut Vec<Effect>) -> Result<(),RuntimeError> {
        let kind = event["type"].as_str().unwrap_or("");
        // O `api_retry` não diz de quem é (subagente, chamada de fundo): qualquer resposta que volta o encerra.
        if matches!(kind,"stream_event" | "assistant" | "user") && self.retry_label.take().is_some() { self.changed(effects,false); }
        if !event["parent_tool_use_id"].is_null() && !kind.starts_with("control_") && kind != "sdk_control_request" { return Ok(()); }
        // Turno que a CLI já tocava quando o motor abriu (Rust reiniciado, turno emendado pela própria
        // CLI) não passou por `start_turn`: sem isto a sessão fica livre e a fila entrega no meio dele.
        if !self.in_progress && turn_activity(kind,&event) { self.start_turn(); self.changed(effects,true); }
        match kind {
            "control_response" => {
                let response = &event["response"];
                let id:RequestId = serde_json::from_value(response["request_id"].clone()).map_err(|_|error("ID de controle inválido"))?;
                // Resposta a um pedido `ui_*`: é da superfície e não fecha operação da fila.
                let now = self.clock.monotonic_s;
                if let Some(surface) = self.surface.as_mut().filter(|surface|surface.owns(&id)) {
                    let out = surface.on_response(&id,response,now);
                    Self::surface_out(effects,out);
                    return Ok(());
                }
                if let Some(waiter) = self.waiters.remove(&id) {
                    let rejected = response["subtype"] == "error";
                    if let Some(wire) = self.wires.get_mut(&waiter.operation_id) { wire.final_result = true; }
                    if !rejected {
                        let result = response.get("response").cloned().unwrap_or_else(||json!({}));
                        match waiter.subtype.as_str() {
                            "initialize" => {
                                self.initialized = true; self.initializing = false; self.init_warning = None;
                                self.commands = result.get("commands").cloned();
                                // A vida subiu: a queda da anterior (na tela e no arquivo da sessão) não vale mais.
                                self.state.problema = None; self.state.problema_detalhe = None;
                                if std::mem::take(&mut self.problem_persisted) {
                                    effects.push(Effect::PersistClaude { operation_id:None,patch:json!({"problema":null}) });
                                }
                                if self.effort_intent.as_ref().is_some_and(|intent|intent["status"] == "prepared") { self.dispatch_effort(effects); }
                                else { effects.push(Effect::WakeQueue); }
                                // `ui_surface_v1`: o processo aceita o Hangar como superfície remota dos mods.
                                if result["capabilities"].as_array().is_some_and(|list|list.iter().any(|item|item == "ui_surface_v1")) {
                                    self.start_surface(effects);
                                }
                            }
                            "set_permission_mode" => {
                                let selected = result["mode"].as_str().or_else(||waiter.payload["mode"].as_str());
                                if let Some(selected) = selected { self.set_mode(selected,Some(waiter.operation_id.clone()),effects); }
                            }
                            "set_model" => {
                                self.model = string(&waiter.payload["model"]);
                                self.policy("session.patch_meta",json!({"model":self.model}),effects);
                                if !waiter.timed_out && waiter.payload["effort"].is_string() {
                                    self.request_effort(waiter.operation_id.clone(),&waiter.payload,effects)?;
                                }
                            }
                            _ => {},
                        }
                        if waiter.subtype != "set_model" || !waiter.payload["effort"].is_string() {
                            effects.push(Effect::Reply { operation_id:waiter.operation_id,disposition:Disposition::Accepted,payload:result });
                        } else if waiter.timed_out {
                            effects.push(Effect::Reply { operation_id:waiter.operation_id,disposition:Disposition::Unknown,
                                payload:json!({"phase_resolved":true,"applied":false,"remaining_phase":"effort"}) });
                        }
                    } else {
                        if waiter.subtype=="set_permission_mode" && waiter.timed_out {
                            effects.push(Effect::Reply {operation_id:waiter.operation_id,disposition:Disposition::Unknown,
                                payload:json!({"error":"permission_response_after_timeout","uncertain":true})});
                            self.changed(effects,true);
                            return Ok(());
                        }
                        if waiter.subtype == "initialize" {
                            self.initializing = false; self.initialized = false; self.init_warning = None;
                            self.state.problema = Some("headless_nao_subiu".into());
                            self.state.problema_detalhe = Some(response["error"].as_str().unwrap_or("initialize recusado").chars().take(300).collect());
                        }
                        self.restore_failed(&waiter,response["error"].as_str().unwrap_or("recusado"));
                        effects.push(Effect::Reply { operation_id:waiter.operation_id,disposition:Disposition::Rejected,
                            payload:json!({"error":response["error"],"permission_boot_rejected":waiter.subtype=="set_permission_mode" && waiter.payload["mode"]=="bypassPermissions"}) });
                    }
                    self.changed(effects,true);
                }
            }
            "system" => {
                if event["subtype"] == "init" && let Some(sid) = event["session_id"].as_str() {
                    if self.metadata["session_id"].as_str()==Some(sid) {self.bind_interactions(sid)?;}
                }
                self.on_system(&event,effects);
            },
            "stream_event" => self.on_stream(&event["event"],effects),
            "command_lifecycle" if event["state"] == "started" => { if !self.in_progress { self.start_turn(); self.changed(effects,true); } }
            "assistant" => {
                if let Some(limit) = usage_limit(&event,&self.rate_limit_info,self.clock.epoch_s) { self.turn_limit = Some(limit); }
                if let Some(error) = event["error"].as_str() { self.turn_error = Some(error.to_owned()); }
                if !event["local_command_source"].is_null() {
                    let text = text_blocks(&event["message"]["content"]);
                    // A CLI de hoje põe a saída no `local_command_source`, sem o comando: a resposta do `/effort` é
                    // reconhecida pelo texto dela; o nome do comando no campo fica para a CLI que o mandava.
                    let named = event["local_command_source"].as_str().is_some_and(|source|source.trim_start_matches('/').split_whitespace().next() == Some("effort"));
                    let effort_source = named || text.starts_with("Set effort level to ");
                    // Só o `/effort` do próprio Hangar fica sem rótulo; o digitado pela pessoa leva o `/` dela.
                    let mut answered_intent = named;
                    if let Some(intent) = self.effort_intent.clone().filter(|i|effort_source && (i["status"] == "dispatching" || i["status"] == "unknown")) {
                        answered_intent = true;
                        let value = intent["value"].as_str().unwrap_or("");
                        let child = format!("{}:effort",intent["operation_id"].as_str().unwrap_or(""));
                        if text.starts_with(&format!("Set effort level to {value}")) && self.wires.contains_key(&child) {
                            self.effort = Some(value.into()); self.effort_intent = None; self.effort_deadline = None;
                            if let Some(wire) = self.wires.get_mut(&child) { wire.final_result = true; }
                            effects.push(Effect::Reply { operation_id:child,disposition:Disposition::Accepted,payload:json!({"applied":true}) });
                            effects.push(Effect::Reply { operation_id:intent["operation_id"].as_str().unwrap_or("").into(),
                                disposition:Disposition::Accepted,payload:json!({"applied":true}) });
                            self.policy("session.patch_meta",json!({"effort":self.effort}),effects);
                        } else if self.wires.contains_key(&child) {
                            self.effort_deadline = None;
                            if let Some(pending) = self.effort_intent.as_mut() { pending["status"] = json!("rejected"); }
                            if let Some(wire) = self.wires.get_mut(&child) { wire.final_result = true; }
                            self.state.problema = Some("headless_turno_erro".into());
                            self.state.problema_detalhe = Some(format!("esforço {value:?} não aceito: {}",text.chars().take(200).collect::<String>()));
                            for operation_id in [child,intent["operation_id"].as_str().unwrap_or("").into()] {
                                effects.push(Effect::Reply { operation_id,disposition:Disposition::Rejected,payload:json!({"applied":false}) });
                            }
                        }
                    }
                    // `local_command_source` traz a saída, não o comando: quem respondeu é o último `/` escrito.
                    let source = if answered_intent { None } else { self.slash_input.take() };
                    if !text.trim().is_empty() {
                        self.policy("local_output",json!({"text":text,"source":source}),effects);
                    }
                } else {
                    self.slash_input = None;
                    let usage = &event["message"]["usage"];
                    if ["input_tokens","cache_read_input_tokens","cache_creation_input_tokens"].iter().any(|k|usage[*k].as_u64().unwrap_or(0) > 0) {
                        self.usage = usage.clone();
                        let creation = &usage["cache_creation"];
                        if creation["ephemeral_1h_input_tokens"].as_u64().unwrap_or(0) > 0 { self.idle_compact.one_hour = Some(true); }
                        else if creation["ephemeral_5m_input_tokens"].as_u64().unwrap_or(0) > 0 { self.idle_compact.one_hour = Some(false); }
                        self.idle_compact.live = true;
                        self.idle_compact.done = false;
                        self.idle_compact.cache_at = self.clock.monotonic_s;
                        // A chamada do próprio resumo não é uso novo: a compactação da parada segue a dela.
                        if !self.compacting { self.idle_compact.auto = false; self.idle_compact.tried = false; }
                    }
                    if let Some(blocks) = event["message"]["content"].as_array() {
                        for block in blocks {
                            match block["type"].as_str() {
                                Some("text") => { if let Some(text) = self.preview.clear() { self.publish("preview",text,effects); } }
                                Some("thinking") => { if let Some(text) = self.thinking.clear() { self.publish("thinking",text,effects); } }
                                Some("tool_use") => {
                                    self.label = Some(tool_label(block["name"].as_str().unwrap_or("tool"),&block["input"]));
                                    self.clear_tool(effects);
                                    self.tool_name = None;
                                }
                                _ => {},
                            }
                        }
                    }
                }
                self.changed(effects,true);
            }
            "control_request" | "sdk_control_request" => {
                let request_id:RequestId = serde_json::from_value(event["request_id"].clone()).map_err(|_|error("ID de pedido inválido"))?;
                let request = event.get("request").cloned().unwrap_or_else(||json!({}));
                // Cópia pedida por um mod: a superfície responde na hora, fora do diário.
                if request["subtype"] == "ui_copy" && let Some(surface) = self.surface.as_mut() {
                    let out = surface.on_copy(&event["request_id"],&request);
                    Self::surface_out(effects,out);
                    return Ok(());
                }
                let tool = request["tool_name"].as_str().unwrap_or("");
                if request["subtype"] != "can_use_tool" {
                    let id = self.server_answer_id();
                    self.answer(id,request_id,json!({}),effects);
                    let subtype = request["subtype"].as_str().unwrap_or("");
                    self.policy("unknown_private",json!({"kind":format!("control_request/{subtype}"),"event":event}),effects);
                    if self.unknown.insert(format!("control_request/{subtype}")) {
                        self.policy("local_output",json!({"text":format!("⚙️ A CLI pediu `{subtype}`; respondi vazio")}),effects);
                    }
                } else if tool == "AskUserQuestion" {
                    self.question = Some(json!({"provider":"claude","request_id":request_id,"questions":request["input"]["questions"]}));
                } else if self.permission_mode.as_deref() == Some("plan") && self.previous_non_plan.as_deref() == Some("bypassPermissions")
                    && !["ExitPlanMode","Edit","Write","MultiEdit","NotebookEdit"].contains(&tool) {
                    let id = self.server_answer_id();
                    self.answer(id,request_id,
                        json!({"behavior":"allow","updatedInput":request.get("input").cloned().unwrap_or_else(||json!({}))}),effects);
                } else if let Some((_,old)) = self.pending.iter_mut().find(|(id,_)|*id == request_id) { *old = request; }
                else { self.pending.push((request_id,request)); }
                self.changed(effects,true);
            }
            "control_cancel_request" => {
                let id:RequestId = serde_json::from_value(event["request_id"].clone()).map_err(|_|error("ID de cancelamento inválido"))?;
                // O pedido acabou antes da resposta (hook, Parar, fim do turno, prazo da CLI): no terminal isso aparece
                // como uma linha; aqui o cartão sumiria calado. Sem prova de quem decidiu, o texto não afirma.
                let mut note = self.pending.iter().find(|(request_id,_)|request_id == &id)
                    .map(|(_,request)|format!("⚙️ {} — encerrado antes da sua resposta",permission_text(request)));
                self.pending.retain(|(request_id,_)|request_id != &id);
                if let Some(question) = self.question.take_if(|q|serde_json::from_value::<RequestId>(q["request_id"].clone()).ok().as_ref() == Some(&id)) {
                    let first:String = question["questions"][0]["question"].as_str().unwrap_or("").chars().take(120).collect();
                    note = Some(format!("⚙️ Pergunta cancelada antes da resposta: {first}").trim_end_matches([':',' ']).to_owned());
                }
                self.changed(effects,true);
                if let Some(text) = note { self.policy("local_output",json!({"text":text}),effects); }
            }
            "result" => {
                self.in_progress = false; self.turn_start = None; self.label_deadline = None;
                self.end_turn_notices(effects);
                // O agente de fundo ainda vivo segue no rótulo; o resto acabou com o turno.
                let background = &self.background;
                self.tasks.retain(|(key,_)|background.contains(key));
                self.label = self.tasks_label();
                self.pending.clear(); self.question = None; self.restore_plan = None;
                if let Some(id) = self.active_input.take() {
                    if let Some(wire) = self.wires.get_mut(&id) { wire.final_result = true; }
                    effects.push(Effect::Reply { operation_id:id,disposition:Disposition::Accepted,payload:json!({"consumed":true}) });
                }
                let subtype = event["subtype"].as_str().unwrap_or("");
                let turn_error = self.turn_error.take();
                if let Some(code) = crate::session_write::limit_switch::auth_error(turn_error.as_deref(),event["api_error_status"].as_u64()) {
                    effects.push(Effect::AuthFailed { code });
                }
                let mut stop_reason = event["terminal_reason"].as_str().and_then(|reason|terminal_reason_text(reason,&event));
                if stop_reason.is_none() && event["terminal_reason"].as_str().is_some_and(|reason|!NORMAL_TERMINAL.contains(&reason)) {
                    self.diary("claude_terminal_reason",||json!({"terminal_reason":event["terminal_reason"]}),effects);
                }
                // A CLI manda o nome do comando ("cost", "clear"), não um booleano.
                let local = !event["local_command"].is_null() && event["local_command"] != false;
                let failed = subtype != "error_during_execution" && (event["is_error"] == true || subtype.starts_with("error"));
                if failed {
                    let logged_out = event["result"].as_str().unwrap_or("").to_lowercase().contains("not logged in");
                    self.state.problema = Some(if logged_out { "headless_sem_login" } else { "headless_turno_erro" }.into());
                    let limit = self.turn_limit.take().or_else(||usage_limit(&event,&self.rate_limit_info,self.clock.epoch_s));
                    // Limite de uso e falta de login já têm aviso próprio: o motivo genérico ("erro da API") só confundiria.
                    if logged_out || limit.is_some() { stop_reason = None; }
                    let reason = stop_reason.as_ref().map(|reason|format!("{reason} — ")).unwrap_or_default();
                    self.state.problema_detalhe = Some(format!("{reason}{subtype}: {}",clip(event["result"].as_str().unwrap_or(""),300)));
                    if let Some(limit) = limit { effects.push(Effect::UsageLimit { limit }); }
                }
                if let Some(reason) = &stop_reason { self.policy("local_output",json!({"text":format!("⚙️ Turno parado: {reason}")}),effects); }
                if !failed && subtype == "success" && !local {
                    self.state.problema = None; self.state.problema_detalhe = None;
                    if std::mem::take(&mut self.problem_persisted) {
                        effects.push(Effect::PersistClaude { operation_id:None,patch:json!({"problema":null}) });
                    }
                }
                self.turn_limit = None;
                if local { effects.push(Effect::ConfirmLocalCommands); }
                self.apply_usage(&event,effects);
                self.clear_streams(effects); self.changed(effects,true);
                if self.effort_intent.as_ref().is_some_and(|intent|intent["status"] == "prepared") { self.dispatch_effort(effects); }
                else { effects.push(Effect::WakeQueue); }
            }
            "rate_limit_event" => {
                self.rate_limit_info = event["rate_limit_info"].clone();
                self.state.limited = event["rate_limit_info"]["status"] == "rejected";
                if !self.state.limited { self.state.limit_reset = None; }
                self.policy("format_status",json!({"rate_limit_info":event["rate_limit_info"],"model":self.model,
                    "effort":self.effort,"usage":self.usage,"context_window":self.context_window,"cost":self.cost}),effects);
                self.changed(effects,false);
            }
            // O turno acabou junto: o estado e a faixa (`isWorking`) acompanham.
            "conversation_reset" => { self.reset_conversation(effects); self.changed(effects,true); }
            "cano_saiu" => {
                let working = self.in_progress || !self.pending.is_empty() || self.question.is_some();
                self.alive = false; self.in_progress = false; self.initializing = false;
                self.turn_start = None; self.label_deadline = None; self.init_warning = None; self.effort_deadline = None;
                self.pending.clear(); self.question = None; self.clear_streams(effects);
                // O trabalho de fundo morre com o processo.
                self.tasks.clear(); self.background.clear(); self.label = None; self.drop_turn_notices();
                for agent in self.subagents.values_mut().filter(|agent|agent["state"] == "running") { agent["state"] = json!("stopped"); }
                // Pedidos dos apps em aberto respondem com código e a interface dos mods some.
                if let Some(surface) = self.surface.as_mut() { let out = surface.on_exit(); Self::surface_out(effects,out); }
                // Só a saída limpa com a sessão parada (0 ou SIGTERM) não é queda. SIGKILL (OOM, kill -9) e qualquer
                // saída no meio do turno cortaram trabalho: queda, com o motivo (a 1ª linha da faixa é o stderr).
                // Sem código: o cano morreu sem dar o código (com ele vivo, socket caído ou envelope que o
                // servidor não conhece, o ator religa e isto não chega).
                let code = match event["rc"].as_i64() {
                    Some(rc) if rc == -9 || ![0,-15].contains(&rc) || working => Some(format!("rc={rc}")),
                    Some(_) => None,
                    None => Some("cano_closed sem código de saída".to_owned()),
                };
                if let Some(code) = code {
                    let reason:String = self.stderr_tail.join("\n").chars().take(280).collect();
                    let detail = if reason.is_empty() { code } else { format!("{reason}\n{code}") };
                    self.state.problema = Some("headless_processo_caiu".into());
                    self.state.problema_detalhe = Some(detail.clone());
                    self.problem_persisted = true;
                    effects.push(Effect::PersistClaude { operation_id:None,patch:json!({"problema":["headless_processo_caiu",detail]}) });
                }
                for (operation_id,wire) in &mut self.wires {
                    if !wire.final_result {
                        effects.push(Effect::Reply { operation_id:operation_id.clone(),disposition:Disposition::Unknown,payload:json!({"error":"cano encerrado"}) });
                    }
                }
                self.waiters.clear(); self.changed(effects,true);
            }
            "user" => {
                if event["isReplay"] == true && let Some(uuid) = event["uuid"].as_str()
                    && let Some(index) = self.echoes.iter().position(|(known,_)|known == uuid) {
                    let (_,operation_id) = self.echoes.remove(index).unwrap();
                    effects.push(Effect::InputEchoed { operation_id });
                }
                self.label = None; self.changed(effects,true);
            }
            "cano_stderr" => {
                if let Some(text) = event["linha"].as_str().map(str::trim_end).filter(|text|!text.is_empty()) {
                    self.stderr_tail.push(text.to_owned());
                    if self.stderr_tail.len() > STDERR_TAIL { self.stderr_tail.remove(0); }
                }
            }
            "keep_alive" | "tool_progress" | "command_lifecycle" => {},
            _ => {
                self.policy("unknown_private",json!({"kind":kind,"event":event}),effects);
                if self.unknown.insert(kind.into()) { self.policy("local_output",json!({"text":format!("⚙️ Evento desconhecido da CLI: {kind}")}),effects); }
            }
        }
        Ok(())
    }

    /// Tarefa no rótulo; a mesma tarefa retomada (mesmo `task_id`) não entra duas vezes.
    fn start_task(&mut self,task:&Value) {
        let id = task["task_id"].as_str().map(str::to_owned).unwrap_or_else(||task["task_id"].to_string());
        if self.tasks.iter().any(|(key,_)|*key == id) { return; }
        self.tasks.push((id,json!({"kind":task["subagent_type"].as_str().or_else(||task["task_type"].as_str()).unwrap_or("agente"),"step":task["description"]})));
    }

    fn tasks_label(&self) -> Option<String> {
        self.tasks.last().map(|(_,task)| {
            let step = task["step"].as_str().unwrap_or("").trim_start_matches("Running ").trim();
            let label = if step.is_empty() { format!("{}…",task["kind"].as_str().unwrap_or("agente")) }
                else { format!("{}: {step}",task["kind"].as_str().unwrap_or("agente")) };
            let label = if self.tasks.len() > 1 { format!("{} agentes · {label}",self.tasks.len()) } else { label };
            label.chars().take(120).collect()
        })
    }

    /// Execução de subagente em curso (`task_started` ou a lista de `background_tasks_changed`); `local_bash` e
    /// outras tarefas ficam de fora. A mesma execução vista de novo não apaga o desfecho já gravado.
    fn track_subagent(&mut self,task:&Value) {
        if task["task_type"] != "local_agent" { return; }
        let (Some(id),run) = (task["task_id"].as_str(),task["run_id"].as_str().unwrap_or("")) else { return };
        let entry = self.subagents.entry(id.into()).or_insert(Value::Null);
        if entry["run_id"] != run { *entry = json!({"state":"running","run_id":run,"end_time":null}); }
    }

    /// Desfecho da execução atual (`task_updated` com `patch`, `task_notification`); de outra execução, ignorado.
    fn finish_subagent(&mut self,event:&Value) {
        let Some(entry) = event["task_id"].as_str().and_then(|id|self.subagents.get_mut(id)) else { return };
        if event["run_id"].as_str().is_some_and(|run|entry["run_id"] != run) { return; }
        let state = match event["patch"]["status"].as_str().or_else(||event["status"].as_str()) {
            Some("completed") => "completed",
            Some("failed") => "failed",
            Some("killed" | "cancelled" | "stopped") => "stopped",
            _ => "",
        };
        if !state.is_empty() { entry["state"] = json!(state); }
        if let Some(end) = event["patch"]["end_time"].as_u64() { entry["end_time"] = json!(end); }
    }

    fn restore_failed(&mut self,waiter:&Waiter,reason:&str) { self.restore_failed_for(&waiter.operation_id,&waiter.payload,reason); }

    /// A volta ao modo de base depois do plano aprovado falhou: a sessão segue no modo errado, e isso aparece.
    fn restore_failed_for(&mut self,operation_id:&str,payload:&Value,reason:&str) -> bool {
        if !operation_id.ends_with(":restore_mode") { return false; }
        let base = payload["mode"].as_str().unwrap_or("");
        self.state.problema = Some("headless_turno_erro".into());
        self.state.problema_detalhe = Some(format!("modo {base:?} não reaplicado após o plano: {}",reason.chars().take(200).collect::<String>()));
        true
    }

    fn set_mode(&mut self,selected:&str,operation_id:Option<String>,effects:&mut Vec<Effect>) {
        let selected = mode(selected);
        if selected != "plan" { self.previous_non_plan = Some(selected.clone()); }
        self.permission_mode = Some(selected);
        effects.push(Effect::PersistClaude {operation_id,patch:json!({"permission_mode":self.permission_mode,"previous_non_plan":self.previous_non_plan})});
    }

    fn on_system(&mut self,event:&Value,effects:&mut Vec<Effect>) {
        let subtype = event["subtype"].as_str().unwrap_or("");
        // Avisos da interface dos mods (`ui_panes`, `ui_invalidate`, `ui_toast`, `ui_status`): da superfície.
        if subtype.starts_with("ui_") {
            let now = self.clock.monotonic_s;
            if let Some(surface) = self.surface.as_mut() { let out = surface.on_notice(event,now); Self::surface_out(effects,out); }
            return;
        }
        if let Some(selected) = event["permissionMode"].as_str() {
            let restore = self.restore_plan.clone();
            self.set_mode(selected,None,effects);
            if selected != "plan" {
                if let Some((base,id)) = restore {
                    self.restore_plan = None;
                    if self.permission_mode.as_deref() != Some(base.as_str()) { self.control(format!("{id}:restore_mode"),"set_permission_mode",json!({"mode":base}),effects); }
                }
            }
        }
        match subtype {
            "init" => {
                if let Some(sid) = event["session_id"].as_str() {
                    if self.metadata["session_id"].as_str() != Some(sid) {
                        if self.metadata["session_id"].is_string() { self.reset_conversation(effects); }
                        self.metadata["session_id"] = json!(sid); self.clear_streams(effects);
                        effects.push(Effect::PersistClaude {operation_id:None,patch:json!({"session_id":sid,"permission_mode":self.permission_mode,"previous_non_plan":self.previous_non_plan})});
                    }
                }
                // Sessão com motor: o proxy pode devolver só o modelo base; a rota fixa é a escolha da sessão.
                if let Some(model) = event["model"].as_str().filter(|_|self.metadata["engine_account"].as_str().is_none_or(str::is_empty)) { self.model = Some(model.into()); }
                if event["terminal_slash_commands"].is_array() { self.terminal_commands = event["terminal_slash_commands"].clone(); }
                self.startup_notices(event,effects);
            }
            "status" => {
                if event["status"] == "compacting" { self.compacting = true; self.label = Some("Compactando…".into()); }
                else if event["status"].is_null() && event.get("permissionMode").is_none() && self.compacting { self.compacting = false; self.label = None; }
                else if event["status"] == "requesting" && self.in_progress && !self.compacting { self.label = Some("Pensando…".into()); }
            }
            "thinking_tokens" if self.in_progress && !self.compacting => self.label = Some("Pensando…".into()),
            "compact_boundary" => {
                self.idle_compact.done = true;
                if std::mem::take(&mut self.compacting) { self.label = None; }
            }
            // O conjunto vivo inteiro do trabalho de fundo: substitui o que se sabia dele (o que saiu terminou).
            "background_tasks_changed" => {
                let live:BTreeSet<String> = event["tasks"].as_array().into_iter().flatten().filter_map(|task|task["task_id"].as_str().map(str::to_owned)).collect();
                for task in event["tasks"].as_array().into_iter().flatten() {
                    self.track_subagent(task);
                    self.start_task(task);
                }
                let gone:Vec<String> = self.background.difference(&live).cloned().collect();
                self.tasks.retain(|(key,_)|!gone.contains(key));
                self.background = live;
                self.label = self.tasks_label();
            }
            "task_started" | "task_progress" | "task_notification" | "task_updated" => {
                if subtype == "task_started" { self.track_subagent(event); }
                else if subtype != "task_progress" { self.finish_subagent(event); }
                let id = event["task_id"].as_str().map(str::to_owned).unwrap_or_else(||event["task_id"].to_string());
                // Como no Python: o rótulo só muda quando a lista de subagentes ou o passo de um deles muda.
                let relabel = if subtype == "task_started" {
                    self.start_task(event);
                    true
                } else if subtype == "task_progress" {
                    match self.tasks.iter_mut().find(|(key,_)|*key == id) {
                        Some((_,task)) => { if let Some(step) = event["description"].as_str().filter(|s|!s.is_empty()) { task["step"] = json!(step); } true }
                        None => false,
                    }
                } else if event["status"].as_str().or_else(||event["patch"]["status"].as_str())
                    .is_some_and(|s|["completed","failed","killed","cancelled"].contains(&s)) { self.tasks.retain(|(key,_)|key != &id); true } else { false };
                if relabel { self.label = self.tasks_label(); }
            }
            "api_retry" => {
                self.diary("claude_api_retry",||event.clone(),effects);
                let seconds = event["retry_delay_ms"].as_u64().unwrap_or(0).div_ceil(1000).max(1);
                let why = event["error_status"].as_u64().map_or_else(||"sem resposta da API".to_owned(),|status|format!("erro {status}"));
                // Chamada de fundo com a sessão parada também tenta de novo, e o sucesso dela não limpa o rótulo:
                // guardado, apareceria no turno seguinte.
                if !self.in_progress { return; }
                self.retry_label = Some(format!("Nova tentativa {} de {} em {seconds} s · {why}",
                    event["attempt"].as_u64().unwrap_or(1),event["max_retries"].as_u64().unwrap_or(1)));
                self.changed(effects,false);
                return;
            }
            "hook_started" => {
                if let Some(id) = event["hook_id"].as_str() {
                    let name = event["hook_name"].as_str().or_else(||event["hook_event"].as_str()).unwrap_or("hook").to_owned();
                    self.hooks_running.insert(id.to_owned(),RunningHook { name,started:self.clock.monotonic_s,shown:false });
                }
                return;
            }
            "hook_response" => { self.on_hook_response(event,effects); return; }
            "hook_progress" => return,
            "notification" => {
                let text = event["text"].as_str().unwrap_or("");
                if matches!(event["priority"].as_str(),Some("high" | "immediate")) && !text.is_empty() {
                    self.notice_once(&format!("notification:{}:{text}",event["key"].as_str().unwrap_or("")),format!("⚙️ {text}"),effects);
                } else {
                    self.diary("claude_notification",||event.clone(),effects);
                }
                return;
            }
            "model_refusal_fallback" | "model_refusal_no_fallback" => {
                self.diary(&format!("claude_{subtype}"),||event.clone(),effects);
                let original = event["original_model"].as_str().unwrap_or("modelo");
                let text = match event["fallback_model"].as_str() {
                    Some(fallback) if subtype == "model_refusal_fallback" => {
                        let local = if event["scope"] == "local" { format!(" (só esta resposta; a sessão segue no {original})") } else { String::new() };
                        // O evento chega quando a CLI decide repetir: a resposta do outro modelo ainda pode falhar.
                        format!("⚙️ O {original} recusou; nova tentativa no {fallback}{local}")
                    }
                    _ => {
                        let why = event["api_refusal_explanation"].as_str().filter(|why|!why.is_empty()).map(|why|format!(": {}",clip(why,300))).unwrap_or_default();
                        format!("⚙️ O {original} recusou responder{why}")
                    }
                };
                self.policy("local_output",json!({"text":text}),effects);
                return;
            }
            _ => { self.diary(&format!("system/{subtype}"),||event.clone(),effects); return; }
        }
        self.changed(effects,true);
    }

    fn on_hook_response(&mut self,event:&Value,effects:&mut Vec<Effect>) {
        // Sucesso em tipo próprio: o teto do diário por tipo não pode esconder falha e bloqueio.
        let kind = if event["outcome"] == "success" { "claude_hook_ok" } else { "claude_hook" };
        self.diary(kind,||{
            let mut logged = event.clone();
            for field in ["stdout","stderr","output"] {
                if let Some(text) = event[field].as_str() { logged[field] = json!(clip(text,2000)); }
            }
            logged
        },effects);
        let hook = event["hook_id"].as_str().and_then(|id|self.hooks_running.remove(id));
        if hook.as_ref().is_some_and(|hook|hook.shown) { self.changed(effects,false); }
        let name = event["hook_name"].as_str().map(str::to_owned).or(hook.map(|hook|hook.name)).unwrap_or_else(||"hook".into());
        // `exit 2` é o hook decidindo (bloqueio). Nos eventos de ferramenta e de parada o motivo vai ao modelo, e o
        // prompt barrado já aparece como bolha retida; nos outros a CLI o mostraria só à pessoa, e sem terminal sumiria.
        let shown = ["PreToolUse","PostToolUse","Stop","SubagentStop","UserPromptSubmit"].contains(&name.split(':').next().unwrap_or(""));
        if event["outcome"] != "error" || (event["exit_code"].as_i64() == Some(2) && shown) { return; }
        let reason = crate::transcript::hook_failure_reason(["stderr","output"].iter().filter_map(|field|event[*field].as_str()),
            event["exit_code"].as_i64());
        // Por nome, não por motivo: o `stderr` muda a cada chamada (arquivo, linha) e viraria uma nota por ferramenta.
        if let Some(streak) = self.hook_streaks.get_mut(&name) { streak.1 += 1; return; }
        let detail = format!("{name}: {reason}");
        self.hook_notice(&detail,effects);
        // Fora de turno (`SessionStart`, `Notification` parada) não há turno para somar.
        if self.in_progress { self.hook_streaks.insert(name,(detail,1)); }
    }

    /// O evento só é montado se ainda cabe: passado o teto, hook com saída grande não custa cópia.
    fn diary(&mut self,kind:&str,event:impl FnOnce() -> Value,effects:&mut Vec<Effect>) {
        match self.diary_counts.get_mut(kind) {
            Some(count) if *count >= DIARY_PER_KIND => return,
            Some(count) => *count += 1,
            None => { self.diary_counts.insert(kind.to_owned(),1); }
        }
        self.policy("unknown_private",json!({"kind":kind,"event":event()}),effects);
    }

    /// Fim do turno: o rótulo de nova tentativa e os hooks abertos saem, e as somas de falha viram aviso.
    fn end_turn_notices(&mut self,effects:&mut Vec<Effect>) {
        self.retry_label = None; self.hooks_running.clear(); self.flush_hook_streaks(effects);
    }

    /// Processo, conexão ou conversa trocados: o que era do turno some sem aviso.
    fn drop_turn_notices(&mut self) {
        self.retry_label = None; self.hooks_running.clear(); self.hook_streaks.clear();
    }

    /// Fim do turno: a falha que repetiu ganha um aviso com o total. Vários hooks dividem o mesmo nome
    /// (`PostToolUse:Bash`), então o sucesso de um não fecha a soma do outro.
    fn flush_hook_streaks(&mut self,effects:&mut Vec<Effect>) {
        for (detail,count) in std::mem::take(&mut self.hook_streaks).into_values() {
            if count > 1 { self.hook_notice(&format!("{detail} ({count}× neste turno)"),effects); }
        }
    }

    fn hook_notice(&mut self,detail:&str,effects:&mut Vec<Effect>) {
        self.policy("local_output",json!({"text":format!("Hook falhou — {detail}"),"hook_error":detail,"notice":"hook_failed"}),effects);
    }

    /// Aviso que vale uma vez por vida do processo: o relançamento mostra de novo, a adoção pelo Rust não.
    fn notice_once(&mut self,key:&str,text:String,effects:&mut Vec<Effect>) -> bool {
        let new = self.unknown.insert(format!("notice:{key}"));
        if new && !self.replaying { self.policy("local_output",json!({"text":text}),effects); }
        new
    }

    /// MCP e plugin que não subiram, como o terminal avisa na abertura.
    /// Uma nota só: os plugins instalados trazem dezenas de conectores sem login, e uma nota por servidor
    /// enchia a conversa a cada processo novo. Os erros de cada servidor ficam no diário.
    fn startup_notices(&mut self,init:&Value,effects:&mut Vec<Effect>) {
        // O servidor que a pessoa configurou vem antes dos conectores dos plugins, que ficam no "e mais N".
        let servers = |status:&str| -> Vec<&str> {
            let mut names:Vec<&str> = init["mcp_servers"].as_array().into_iter().flatten()
                .filter(|server|server["status"] == status).map(|server|server["name"].as_str().unwrap_or("?")).collect();
            names.sort_by_key(|name|name.starts_with("plugin:"));
            names
        };
        let mut lines = Vec::new();
        for (status,one,many) in [("failed","Servidor MCP que não subiu","Servidores MCP que não subiram"),
            ("needs-auth","Servidor MCP que precisa de login","Servidores MCP que precisam de login")] {
            let names = servers(status);
            if names.is_empty() { continue; }
            let shown:Vec<String> = names.iter().take(5).map(|name|format!("`{name}`")).collect();
            let rest = if names.len() > 5 { format!(" e mais {}",names.len()-5) } else { String::new() };
            lines.push(format!("{}: {}{rest}",if names.len() == 1 { one } else { many },shown.join(", ")));
        }
        for plugin in init["plugin_errors"].as_array().into_iter().flatten() {
            lines.push(format!("O plugin `{}` não carregou: {}",plugin["plugin"].as_str().unwrap_or("?"),clip(plugin["message"].as_str().unwrap_or(""),300)));
        }
        if lines.is_empty() { return; }
        let text = format!("⚙️ {}",lines.join("\n"));
        // O `init` chega a cada turno: o diário só registra o problema novo.
        if self.notice_once(&text.clone(),text,effects) && !self.replaying {
            self.diary("claude_init_problem",||json!({"mcp_servers":init["mcp_servers"],"plugin_errors":init["plugin_errors"]}),effects);
        }
    }

    fn on_stream(&mut self,event:&Value,effects:&mut Vec<Effect>) {
        match event["type"].as_str() {
            Some("message_start") => {
                self.tokens_closed += self.tokens_message.unwrap_or(self.token_chars/4);
                self.tokens_message = None; self.token_chars = 0;
                self.gen_start = Some(self.clock.monotonic_s);
                if self.turn_start.is_none() { self.start_turn(); }
                self.changed(effects,true);
            }
            Some("content_block_start") => {
                let block = &event["content_block"];
                match block["type"].as_str() {
                    Some("text") => { if let Some(text) = self.preview.clear() { self.publish("preview",text,effects); } self.label = None; }
                    Some("thinking") => { self.thinking.unblock(); self.thinking_start = Some(self.clock.monotonic_s); self.label = Some("Pensando…".into()); }
                    Some("tool_use" | "server_tool_use" | "mcp_tool_use") => {
                        self.tool_input.clear(); self.tool_name = string(&block["name"]);
                        self.tool_visible = true;
                        self.label = Some(tool_label(self.tool_name.as_deref().unwrap_or("tool"),&json!({})));
                        self.publish("tool",crate::transcript::pyjson::dumps(&json!({"nome":self.tool_name.as_deref().unwrap_or("tool"),"input":{}}),false),effects);
                    }
                    _ => {},
                }
                self.changed(effects,true);
            }
            Some("content_block_delta") => {
                let delta = &event["delta"];
                let piece = delta["text"].as_str().or_else(||delta["thinking"].as_str()).or_else(||delta["partial_json"].as_str()).unwrap_or("");
                self.token_chars += piece.chars().count() as u64;
                match delta["type"].as_str() {
                    Some("text_delta") => { if let Some(text) = self.preview.append(piece,self.clock.monotonic_s) { self.publish("preview",text,effects); } }
                    Some("thinking_delta") => { if let Some(text) = self.thinking.append(piece,self.clock.monotonic_s) { self.publish("thinking",text,effects); } }
                    Some("input_json_delta") if self.tool_name.is_some() => {
                        if let Some(text) = self.tool_input.append(piece,self.clock.monotonic_s) { self.publish_tool(text,effects); }
                    }
                    _ => {},
                }
            }
            Some("content_block_stop") => {
                if let Some(text) = self.preview.flush(self.clock.monotonic_s) { self.publish("preview",text,effects); }
                if let Some(text) = self.thinking.flush(self.clock.monotonic_s) { self.publish("thinking",text,effects); }
                if let Some(text) = self.tool_input.flush(self.clock.monotonic_s) { self.publish_tool(text,effects); }
                self.tool_name = None; self.tool_input.clear();
                if let Some(start) = self.thinking_start.take() { self.thought_s += self.clock.monotonic_s-start; }
            }
            Some("message_delta") => {
                self.tokens_message = event["usage"]["output_tokens"].as_u64();
                // tok/s real: do message_start ao fim, com o output_tokens que só chega aqui.
                if let (Some(tokens),Some(start),true) = (self.tokens_message,self.gen_start.take(),self.metadata["session_id"].is_string()) {
                    effects.push(Effect::Publish { channel:"rate".into(),data:json!({"tokens":tokens,
                        "seconds":(self.clock.monotonic_s-start).max(0.0),"conversation":self.metadata["session_id"]}) });
                }
            }
            _ => {},
        }
    }

    fn publish_tool(&mut self,text:String,effects:&mut Vec<Effect>) {
        let Some(name) = self.tool_name.clone() else { return };
        let input = partial_input(&text);
        self.label = Some(tool_label(&name,&input));
        self.tool_visible = true;
        self.publish("tool",crate::transcript::pyjson::dumps(&json!({"nome":name,"input":input}),false),effects);
        self.changed(effects,true);
    }

    fn apply_usage(&mut self,result:&Value,effects:&mut Vec<Effect>) {
        if let Some(cost) = result["total_cost_usd"].as_f64() { self.cost = Some(cost); }
        if let Some(models) = result["modelUsage"].as_object() {
            if let Some((model,usage)) = models.iter().filter(|(_,u)|u["contextWindow"].as_u64().is_some())
                .min_by_key(|(_,u)|std::cmp::Reverse(["inputTokens","cacheReadInputTokens","cacheCreationInputTokens"].iter().map(|key|u[*key].as_u64().unwrap_or(0)).sum::<u64>())) {
                self.context_window = usage["contextWindow"].as_u64();
                if self.model.is_none() { self.model = Some(model.clone()); }
                self.policy("session.patch_meta",json!({"context_window":self.context_window}),effects);
            }
        }
    }

    fn on_tick(&mut self,effects:&mut Vec<Effect>) {
        let now = self.clock.monotonic_s;
        if now >= self.reload_deadline {
            self.reload_deadline = now + 10.0;
            self.policy("reload_stamp",json!({}),effects);
        }
        if let Some(text) = self.preview.tick(now) { self.publish("preview",text,effects); }
        if let Some(text) = self.thinking.tick(now) { self.publish("thinking",text,effects); }
        if let Some(text) = self.tool_input.tick(now) { self.publish_tool(text,effects); }
        if self.init_warning.is_some_and(|deadline|now >= deadline) {
            self.init_warning = None;
            self.state.problema = Some("headless_sem_resposta".into()); self.changed(effects,true);
        }
        if self.effort_deadline.is_some_and(|deadline|now >= deadline) {
            self.effort_deadline = None;
            if let Some(intent) = self.effort_intent.as_mut() {
                intent["status"] = json!("unknown");
                let parent = intent["operation_id"].as_str().unwrap_or("").to_owned();
                self.state.problema = Some("headless_turno_erro".into());
                self.state.problema_detalhe = Some("esforço sem confirmação da CLI".into());
                for operation_id in [format!("{parent}:effort"),parent] {
                    effects.push(Effect::Reply { operation_id,disposition:Disposition::Unknown,payload:json!({"applied":false}) });
                }
                self.changed(effects,true);
            }
        }
        let mut expired = Vec::new();
        for waiter in self.waiters.values_mut() {
            if !waiter.timed_out && now >= waiter.deadline {
                waiter.timed_out = true;
                effects.push(Effect::Reply { operation_id:waiter.operation_id.clone(),disposition:Disposition::Unknown,
                    payload:json!({"error":"controle sem resposta"}) });
                expired.push((waiter.operation_id.clone(),waiter.payload.clone()));
            }
        }
        for (operation_id,payload) in expired {
            if self.restore_failed_for(&operation_id,&payload,"sem resposta da CLI") { self.changed(effects,true); }
        }
        if self.label_deadline.is_some_and(|deadline|now >= deadline) {
            self.label_deadline = self.turn_start.map(|_|now+1.0); self.changed(effects,false);
        }
        let mut slow = false;
        for hook in self.hooks_running.values_mut().filter(|hook|!hook.shown && now >= hook.started+HOOK_SLOW_S) { hook.shown = true; slow = true; }
        if slow { self.changed(effects,false); }
        // Prazos da superfície: janela dos desenhos, nova ligação e pedidos vencidos.
        let out = self.surface.as_mut().map(|surface|surface.tick(now)).unwrap_or_default();
        Self::surface_out(effects,out);
    }

    /// Segundos parada desde o último evento da CLI ou comando nosso, se nada está em aberto; `None` = não estaciona.
    /// Sem o socket a vista não sabe do turno nem das perguntas do processo: conta como aberta.
    pub fn park_idle(&self,now:f64) -> Option<f64> {
        let open = !self.alive || !self.transport_up || self.initializing || self.in_progress || !self.pending.is_empty() || self.question.is_some()
            || self.effort_intent.as_ref().is_some_and(|intent|intent["status"] == "prepared" || intent["status"] == "dispatching")
            || !self.background.is_empty();
        (!open).then(||now - self.active_at)
    }

    /// Contexto do último turno quando compactar parada ainda aproveita o cache de 1 h; `None` = nada a fazer.
    fn idle_compact_tokens(&self) -> Option<u64> {
        let state = &self.idle_compact;
        if !state.live || state.done || state.tried || state.one_hour != Some(true) { return None; }
        Some(["input_tokens","cache_read_input_tokens","cache_creation_input_tokens"].iter().map(|k|self.usage[*k].as_u64().unwrap_or(0)).sum())
    }

    /// Segundos desde a última chamada à API, com a sessão parada e algo a compactar; `None` = não compacta.
    pub fn idle_compact_age(&self,now:f64) -> Option<f64> {
        self.idle_compact_tokens()?;
        self.park_idle(now)?;
        Some(now - self.idle_compact.cache_at)
    }

    pub fn next_deadline(&self) -> Option<f64> {
        [self.preview.deadline(),self.thinking.deadline(),self.tool_input.deadline(),self.init_warning,self.label_deadline,self.effort_deadline,Some(self.reload_deadline),
            self.surface.as_ref().and_then(Surface::deadline)]
            .into_iter().flatten().chain(self.waiters.values().filter(|w|!w.timed_out).map(|w|w.deadline))
            .chain(self.hooks_running.values().filter(|hook|!hook.shown).map(|hook|hook.started+HOOK_SLOW_S))
            .min_by(f64::total_cmp)
    }
}

/// Por que o turno parou, quando não foi o fim normal nem a pessoa parando.
const NORMAL_TERMINAL:[&str;5] = ["completed","aborted_streaming","aborted_tools","background_requested","tool_deferred"];

fn terminal_reason_text(reason:&str,result:&Value) -> Option<String> {
    Some(match reason {
        _ if NORMAL_TERMINAL.contains(&reason) => return None,
        "api_error" => return Some(result["api_error_status"].as_u64().map_or_else(||"erro da API".into(),|status|format!("erro da API ({status})"))),
        "blocking_limit" => "a conversa chegou ao limite do contexto",
        "rapid_refill_breaker" => "a compactação automática entrou em laço",
        "prompt_too_long" => "a conversa passou do tamanho do contexto",
        "image_error" => "a API recusou uma imagem",
        "model_error" => "erro do modelo",
        "malformed_tool_use_exhausted" => "o modelo errou o formato da ferramenta várias vezes",
        "stop_hook_prevented" | "hook_stopped" => "um hook parou o turno",
        "max_turns" => "limite de passos do turno",
        "budget_exhausted" => "orçamento do turno esgotado",
        "structured_output_retry_exhausted" => "a resposta estruturada falhou várias vezes",
        "tool_deferred_unavailable" => "a ferramenta adiada não está disponível",
        "turn_setup_failed" => "o turno não conseguiu começar",
        // Motivo novo de uma CLI futura pode ser normal: viraria nota em todo turno. Fica só no diário.
        _ => return None,
    }.to_owned())
}

fn permission_text(request:&Value) -> String {
    let detail = request["description"].as_str().or_else(||request["input"]["command"].as_str())
        .or_else(||request["input"]["file_path"].as_str()).or_else(||request["input"]["path"].as_str()).unwrap_or("");
    let detail = if detail.chars().count() > 200 { format!("{}…",detail.chars().take(200).collect::<String>()) } else { detail.into() };
    format!("Permitir {}? {detail}",request["tool_name"].as_str().unwrap_or("ferramenta")).trim().into()
}

fn turn_activity(kind:&str,event:&Value) -> bool {
    match kind {
        "stream_event" | "tool_progress" => true,
        "assistant" => event["local_command_source"].is_null(),
        "user" => event["message"]["content"].as_array().is_some_and(|blocks|blocks.iter().any(|block|block["type"] == "tool_result")),
        _ => false,
    }
}

/// Limite do plano na mensagem sintética: `usage_limit_reached` (CLI de hoje) ou, no CLI antigo, `rate_limit`
/// com a última cota recusada ainda valendo. O aperto temporário também é `rate_limit`, sem cota recusada.
fn usage_limit(event:&Value,last:&Value,now:f64) -> Option<UsageLimit> {
    let params = &event["api_error_params"]["rate_limit_info"];
    let info = if event["api_error"] == "usage_limit_reached" { if params.is_object() { params } else { last } }
        else if event["api_error"].is_null() && event["error"] == "rate_limit" && last["status"] == "rejected" && last["resetsAt"].as_f64().is_some_and(|reset|reset > now) { last }
        else { return None };
    Some(UsageLimit::from_info(info))
}

fn text_blocks(blocks:&Value) -> String {
    blocks.as_array().map(|blocks|blocks.iter().filter(|b|b["type"] == "text").filter_map(|b|b["text"].as_str())
        .collect::<Vec<_>>().join("\n")).unwrap_or_default()
}

fn partial_input(raw:&str) -> Value {
    if let Ok(value) = serde_json::from_str::<Value>(raw) { return if value.is_object() { value } else { json!({}) }; }
    static FIELDS:std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    let fields = FIELDS.get_or_init(||regex::Regex::new(r#""(command|file_path|notebook_path|path|pattern|url|query|description|prompt)"\s*:\s*"((?:[^"\\]|\\.)*)"#).unwrap());
    let mut input = serde_json::Map::new();
    for captures in fields.captures_iter(raw) {
        let value = serde_json::from_str::<Value>(&format!("\"{}\"",&captures[2])).unwrap_or_else(|_|json!(&captures[2]));
        input.entry(captures[1].to_owned()).or_insert(value);
    }
    Value::Object(input)
}

fn tool_label(name:&str,input:&Value) -> String {
    for key in ["command","file_path","notebook_path","path","pattern","url","query","description","prompt"] {
        if let Some(value) = input[key].as_str().map(str::trim).filter(|s|!s.is_empty()) {
            let value = value.lines().next().unwrap_or("");
            let value = if matches!(key,"file_path" | "notebook_path") { value.trim_end_matches(['\\','/']).rsplit(['\\','/']).next().unwrap_or(value) } else { value };
            let value = if value.chars().count() > 80 { format!("{}…",value.chars().take(80).collect::<String>()) } else { value.into() };
            return format!("{name}: {value}");
        }
    }
    format!("{name}…")
}

fn answer_body(question:&Value,answers:&Value) -> Result<Value,RuntimeError> {
    let questions = question["questions"].as_array().ok_or_else(||error("perguntas inválidas"))?;
    let answers = answers.as_array().filter(|answers|answers.len() == questions.len()).ok_or_else(||error("responda a todas as perguntas"))?;
    let mut response = serde_json::Map::new();
    let mut chat = Vec::new();
    for (question,answer) in questions.iter().zip(answers) {
        let title = question["question"].as_str().ok_or_else(||error("pergunta inválida"))?;
        let text = match answer["kind"].as_str() {
            Some("chat") => { chat.push(title); continue; }
            Some("text") => answer["value"].as_str().unwrap_or("").trim().to_owned(),
            Some("option") => {
                let indices = answer["indices"].as_array().ok_or_else(||error("opções inválidas"))?;
                if question["multiSelect"] != true && indices.len() != 1 { return Err(error("escolha uma opção")); }
                let options = question["options"].as_array().ok_or_else(||error("opções inválidas"))?;
                let labels:Result<Vec<_>,_> = indices.iter().map(|index| {
                    index.as_u64().and_then(|index|options.get(index as usize)).and_then(|o|o["label"].as_str()).ok_or_else(||error("opção inválida"))
                }).collect();
                labels?.join(", ")
            }
            _ => return Err(error("responda a todas as perguntas")),
        };
        if text.is_empty() { return Err(error("responda a todas as perguntas")); }
        response.insert(title.into(),json!(text));
    }
    if chat.is_empty() { Ok(json!({"behavior":"allow","updatedInput":{"questions":questions,"answers":response}})) }
    else {
        let mut lines = if chat.len() == 1 { vec![format!("Sobre «{}» ele prefere conversar antes de responder.",chat[0])] }
            else { let mut lines = vec!["Sobre estas perguntas ele prefere conversar antes de responder:".into()]; lines.extend(chat.iter().map(|q|format!("- {q}"))); lines };
        if !response.is_empty() { lines.push("Ele já respondeu:".into()); lines.extend(response.iter().map(|(q,a)|format!("- {q} → {}",a.as_str().unwrap_or("")))); }
        lines.push("Não repita a pergunta: responda em texto e aguarde a mensagem dele.".into());
        Ok(json!({"behavior":"deny","message":lines.join("\n")}))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn question_that_is_not_a_form_is_shown_unanswerable() {
        let clock = ClockSample { monotonic_s:10.0,epoch_s:1_800_000_010.0 };
        let mut engine = ClaudeEngine::new(json!({"name":"session","session_id":"sid","initialized":true}),1,clock);
        engine.set_interactions("key".into(),Arc::new(InteractionStore::default())).unwrap();
        engine.question = Some(json!("não é formulário"));
        let state = engine.public_state();
        assert_eq!(state.problema.as_deref(),Some("runtime_falhou"));
        assert_eq!(state.problema_detalhe.as_deref(),Some("erro_interacao_indisponivel"));
        assert!(state.codex_question.is_none() && state.interaction_id.is_none() && state.options.is_none());
    }
}
