//! Fatos locais do executor Claude, com o mesmo registro de interações e plugin.
use std::{io::{Read, Seek, SeekFrom}, path::{Path, PathBuf}, sync::{Arc, Mutex}, time::{Duration, Instant, SystemTime}};
use serde_json::Value;
use crate::interactions::{ExpectedInteraction, InteractionBinding, InteractionError, plugin_semantic, plugin_target};
use crate::list::procs::{ProcessView, SystemProcs};
use crate::plugin::{PluginQuestion, Store};
use crate::runtime::terminal::{NoFacts, TerminalTarget};
use crate::terminal_input::{self as input, CommandRequest, InputFacts, InputLimits, NativeMessage, PluginReply, PluginRequest, ServiceError, TerminalBinding, TerminalDriver, TerminalIo};

#[derive(Clone, Debug, PartialEq)]
pub struct TerminalMetadata {
    pub config_dir: PathBuf,
    pub agent_pid: u32,
    pub agent_birth: f64,
    pub legacy_import_after: Option<f64>,
    pub fingerprint: String,
    pub origin: String,
}
impl TerminalMetadata {
    pub fn from_descriptor(meta: &Value) -> Result<Self, ServiceError> {
        let invalid = || ServiceError("terminal_metadata");
        let text = |field: &str| meta[field].as_str().filter(|text| !text.is_empty() && !text.contains('\0')).map(str::to_owned).ok_or_else(invalid);
        let positive = |field: &str| meta[field].as_f64().filter(|number| number.is_finite() && *number > 0.0).ok_or_else(invalid);
        let legacy_import_after = match meta.get("legacy_import_after") {
            None | Some(Value::Null) => None, Some(_) => Some(positive("legacy_import_after")?),
        };
        let origin = match meta.get("message_origin") {
            None | Some(Value::Null) => format!("uds:hangar-{}", std::process::id()),
            Some(_) => text("message_origin")?,
        };
        if !origin.starts_with("uds:") || origin.len() <= 4 { return Err(invalid()); }
        Ok(Self { config_dir: text("config_dir")?.into(), agent_pid: meta["agent_pid"].as_u64().and_then(|pid| u32::try_from(pid).ok()).filter(|pid| *pid > 0).ok_or_else(invalid)?,
            agent_birth: positive("agent_birth")?, legacy_import_after, fingerprint: text("fingerprint")?, origin })
    }
    pub fn cut(&self, created: f64) -> f64 { self.legacy_import_after.filter(|cut| *cut > 0.0).unwrap_or(created).max(self.agent_birth) }

    /// Campos antigos ausentes são resolvidos pelo pane e processo, nunca pelo nome da sessão.
    pub async fn resolve(meta: &Value, binding: &TerminalBinding, transcript: &Path) -> Result<Self, ServiceError> {
        if let Ok(metadata) = Self::from_descriptor(meta) { return Ok(metadata); }
        let io = input::ProcessIo::default();
        let (program, prefix) = binding.mux_argv.split_first().ok_or(ServiceError("mux_missing"))?;
        let output = io.command(CommandRequest { program: program.clone(), args: prefix.iter().cloned().chain([
            "list-panes".into(), "-a".into(), "-F".into(), crate::list::mux::LIST_PANES_FORMAT.into()]).collect(), stdin: vec![] }).await.map_err(|error| ServiceError(error.code))?;
        if !output.success { return Err(ServiceError("terminal_metadata_mux_unavailable")); }
        let text = std::str::from_utf8(&output.stdout).map_err(|_| ServiceError("terminal_metadata_mux_unavailable"))?;
        if binding.windows && text.contains('\u{fffd}') { return Err(ServiceError("terminal_metadata_mux_unavailable")); }
        let (panes, dropped) = crate::list::mux::parse_list_panes(text);
        if dropped != 0 { return Err(ServiceError("terminal_metadata_mux_unavailable")); }
        let (meta, binding, transcript) = (meta.clone(), binding.clone(), transcript.to_owned());
        tokio::task::spawn_blocking(move || {
            let config = match meta["config_dir"].as_str() {
                Some(path) if !path.is_empty() && !path.contains('\0') => PathBuf::from(path),
                Some(_) => return Err(ServiceError("terminal_metadata")),
                None => transcript.ancestors().find(|path| path.file_name().is_some_and(|name| name == "projects"))
                    .and_then(Path::parent).map(Path::to_owned).ok_or(ServiceError("terminal_metadata_config_unavailable"))?,
            };
            let procs = SystemProcs::default();
            let children = procs.children(Duration::ZERO).map_err(|_| ServiceError("terminal_metadata_process_unavailable"))?;
            let mut resolver = crate::list::discover::Resolver::default();
            let rows = crate::list::discover::discover_panes(&panes, &procs, &children, &config.join("projects"), &mut resolver, &|_| false);
            let mut matches = rows.into_iter().filter(|row| row.name == binding.name && row.provider == "claude"
                && row.session_created == Some(binding.created)
                && panes.iter().any(|pane| pane.session == row.name && pane.pane_id == row.pane_id && pane.target().as_deref() == Some(&binding.pane)));
            let row = matches.next().ok_or(ServiceError("terminal_metadata_process_unavailable"))?;
            if matches.next().is_some() { return Err(ServiceError("terminal_metadata_ambiguous")); }
            let pid = row.agent_pid.and_then(|pid| u32::try_from(pid).ok()).filter(|pid| *pid > 0).ok_or(ServiceError("terminal_metadata_process_unavailable"))?;
            let birth = agent_birth(pid).ok_or(ServiceError("terminal_agent_unavailable"))?;
            let record = read_json(&config.join("sessions").join(format!("{pid}.json")), "terminal_native_record_unavailable")?;
            let proven = match record {
                Some(record) => record["pid"].as_u64() == Some(u64::from(pid)) && record["sessionId"].as_str() == Some(&binding.conversation),
                None => row.repl_sid.as_deref() == Some(&binding.conversation) && row.transcript.as_ref().is_some_and(|source| source.tracked && source.jsonl.as_deref().is_some_and(|path| Path::new(path) == transcript)),
            };
            if !proven { return Err(ServiceError("terminal_native_record_changed")); }
            let mut resolved = meta;
            for (field, value) in [("config_dir", serde_json::json!(config)), ("agent_pid", serde_json::json!(pid)), ("agent_birth", serde_json::json!(birth))] {
                if resolved.get(field).is_none_or(Value::is_null) { resolved[field] = value; }
            }
            if resolved["fingerprint"].is_null() {
                let pane_pid = row.pane_pid.ok_or(ServiceError("terminal_metadata_process_unavailable"))?;
                let pane_birth = procs.start_time(pane_pid).ok_or(ServiceError("terminal_metadata_process_unavailable"))?;
                resolved["fingerprint"] = serde_json::json!(format!("{}:{}:{}:{pane_birth}:{pid}:{birth}", binding.pane, binding.created, binding.conversation));
            }
            let metadata = Self::from_descriptor(&resolved)?;
            if metadata.agent_pid != pid || !same_birth(metadata.agent_birth, birth) { return Err(ServiceError("terminal_agent_changed")); }
            Ok(metadata)
        }).await.map_err(|_| ServiceError("terminal_metadata_process_unavailable"))?
    }
}

#[derive(Clone)]
pub struct ModeSource { pub binding: InteractionBinding, pub transcript: PathBuf, pub confirmed: Option<String> }
pub type ModeReader = Arc<dyn Fn(String) -> input::ServiceFuture<'static, Option<ModeSource>> + Send + Sync>;
pub(crate) type TranscriptModes = crate::list::capped::Capped<PathBuf, ((SystemTime, u64), Option<String>)>;
pub struct LocalTerminalFacts {
    io: Arc<dyn TerminalIo>, plugin: Option<Arc<Store>>, modes: Option<ModeReader>,
    transcripts: Arc<Mutex<TranscriptModes>>,
}
impl LocalTerminalFacts {
    pub fn new(io: Arc<dyn TerminalIo>, plugin: Option<Arc<Store>>, modes: Option<ModeReader>) -> Self {
        Self { io, plugin, modes, transcripts: Arc::new(Mutex::new(TranscriptModes::default())) }
    }
    pub(crate) fn io_for_guard(&self) -> Arc<dyn TerminalIo> { self.io.clone() }
    pub fn binding(target: &TerminalTarget) -> InteractionBinding {
        InteractionBinding { key: target.key.clone(), conversation: target.binding.conversation.clone(), generation: target.generation }
    }
    /// Mesmo pane e mesma vida (agente e conversa); devolve o pid do pane.
    pub(crate) async fn check(&self, target: &TerminalTarget, binding: &TerminalBinding) -> Result<i64, ServiceError> {
        self.check_life(target, binding, true).await
    }
    /// Mesmo pane e mesmo processo do agente, sem provar a conversa: só para o que a entrega precisa
    /// fazer quando a prova da conversa sumiu (tirar o próprio texto, reabrir na mesma conversa).
    pub(crate) async fn check_agent(&self, target: &TerminalTarget, binding: &TerminalBinding) -> Result<i64, ServiceError> {
        self.check_life(target, binding, false).await
    }
    async fn check_life(&self, target: &TerminalTarget, binding: &TerminalBinding, prove_conversation: bool) -> Result<i64, ServiceError> {
        if binding != &target.binding || binding.generation != target.generation || binding.name != target.name
            || target.metadata.fingerprint.is_empty() || !target.created.is_finite() || target.created < 0.0 {
            return Err(ServiceError("terminal_binding_changed"));
        }
        let (program,prefix)=binding.mux_argv.split_first().ok_or(ServiceError("mux_missing"))?;
        let output=self.io.command(CommandRequest {program:program.clone(),args:prefix.iter().cloned().chain(["display-message".into(),"-p".into(),"-t".into(),binding.pane.clone(),"#{pane_pid}\t#{session_name}\t#{pane_id}\t#{session_created}".into()]).collect(),stdin:vec![]}).await.map_err(|error|ServiceError(error.code))?;
        if !output.success {return Err(ServiceError("terminal_pane_unavailable"));}
        let text=std::str::from_utf8(&output.stdout).map_err(|_|ServiceError("terminal_pane_unavailable"))?;
        let fields:Vec<_>=text.trim().split('\t').collect();
        if fields.len()!=4 || fields[1]!=binding.name || fields[3].parse::<u64>().ok()!=Some(binding.created) || (!binding.windows && fields[2]!=binding.pane) {return Err(ServiceError("terminal_binding_changed"));}
        let pane_pid=fields[0].parse::<i64>().ok().filter(|pid|*pid>0).ok_or(ServiceError("terminal_pane_unavailable"))?;
        let (metadata, conversation, transcript) = (target.metadata.clone(), binding.conversation.clone(), target.transcript.clone());
        // Sem o registro nativo, o `/pull` aberto do plugin desta vida prova a conversa (o Claude Code só o
        // aceita com o `session_id` dela), sem ler os arquivos abertos do processo.
        let plugin_proof = prove_conversation && self.plugin_proves(target);
        tokio::task::spawn_blocking(move || {
            let procs=SystemProcs::default();let children=procs.children(Duration::ZERO).map_err(|_|ServiceError("terminal_agent_unavailable"))?;
            if !crate::list::discover::descendants(pane_pid,&children).contains(&i64::from(metadata.agent_pid)) {return Err(ServiceError("terminal_agent_changed"));}
            let born = agent_birth(metadata.agent_pid).ok_or(ServiceError("terminal_agent_unavailable"))?;
            #[cfg(not(windows))]
            if procs.argv_checked(i64::from(metadata.agent_pid)).map_err(|_|ServiceError("terminal_agent_unavailable"))?.is_empty() {return Err(ServiceError("terminal_agent_unavailable"));}
            if !same_birth(born, metadata.agent_birth) { return Err(ServiceError("terminal_agent_changed")); }
            // Registro de outra conversa é troca de vida mesmo sem a prova da conversa.
            if native_record(&metadata, &conversation)?.is_none() && prove_conversation && !plugin_proof {
                current_transcript(&procs, &metadata, &conversation, &transcript)?;
            }
            Ok(pane_pid)
        }).await.map_err(|_| ServiceError("terminal_agent_unavailable"))?
    }
    fn plugin_proves(&self, target: &TerminalTarget) -> bool {
        // Plugin sem vínculo ou ilegível não prova nada: segue a leitura do processo.
        self.plugin.as_ref().is_some_and(|store| self.plugin_facts(target).is_ok_and(|facts| plugin_current(&facts, target.metadata.cut(target.created)))
            && store.conversation_proved(&Self::binding(target)).unwrap_or(false))
    }
    /// Sem o registro nativo, o `/pull` aberto do plugin é a prova da conversa: quem o entrega ao plugin
    /// (`Fill`) fica sem ela até ele voltar.
    async fn pull_is_the_proof(&self, target: &TerminalTarget) -> Result<bool, ServiceError> {
        let (metadata, conversation) = (target.metadata.clone(), target.binding.conversation.clone());
        tokio::task::spawn_blocking(move || native_record(&metadata, &conversation).map(|record| record.is_none()))
            .await.map_err(|_| ServiceError("terminal_native_record_unavailable"))?
    }
    /// Parada e sem pergunta aberta, conferindo só o pane e o processo do agente: a reabertura que devolve
    /// a prova da conversa não pode depender dela.
    pub(crate) async fn idle_agent(&self, target: &TerminalTarget) -> Result<(), ServiceError> {
        let binding = &target.binding;
        self.check_agent(target, binding).await?;
        let driver = TerminalDriver::new(binding.clone(), Arc::new(NoFacts), self.io.clone(), InputLimits::default());
        let screen = driver.facts_screen().await.map_err(|error| ServiceError(error.code))?;
        let (ready, idle) = input::input_state(&screen);
        if !ready || !idle { return Err(ServiceError("erro_sessao_trabalhando")); }
        let (files, store, frame) = (target.clone(), self.plugin.clone(), screen);
        let (question, hook) = tokio::task::spawn_blocking(move || {
            let question = match &store {
                Some(store) => crate::state::ask::observe_terminal(store.interactions(), &Self::binding(&files), &files.transcript, &frame).map(|question| question.is_some()),
                None => crate::state::ask::read_pending(&files.transcript).map(|question| question.is_some()),
            }.map_err(|code| ServiceError(crate::state::ask::failure_code(&code)))?;
            Ok::<_, ServiceError>((question, hook_state(&files.metadata, &files.binding.conversation, files.created)?))
        }).await.map_err(|_| ServiceError("terminal_files_unavailable"))??;
        let held = match &self.plugin { Some(store) => !store.pending_all(&Self::binding(target), Instant::now())?.is_empty(), None => false };
        if question || held { return Err(ServiceError("erro_sessao_esperando_resposta")); }
        if hook.is_some_and(|state| state != "idle") { return Err(ServiceError("erro_sessao_trabalhando")); }
        self.check_agent(target, binding).await.map(|_| ())
    }
    pub async fn read(&self, target: &TerminalTarget, binding: &TerminalBinding, text: &str) -> Result<InputFacts, ServiceError> {
        self.read_facts(target, binding, text, true).await
    }
    /// Releitura dentro da mesma entrega, com a vida já provada por uma leitura anterior dela: só a
    /// conferência do fim, que pega a troca de pane ou de vida havida desde então.
    pub(crate) async fn reread(&self, target: &TerminalTarget, binding: &TerminalBinding, text: &str) -> Result<InputFacts, ServiceError> {
        self.read_facts(target, binding, text, false).await
    }
    async fn read_facts(&self, target: &TerminalTarget, binding: &TerminalBinding, text: &str, lead: bool) -> Result<InputFacts, ServiceError> {
        if lead { self.check(target, binding).await?; }
        let driver = TerminalDriver::new(binding.clone(), Arc::new(NoFacts), self.io.clone(), InputLimits::default());
        let screen = driver.facts_screen().await.map_err(|error| ServiceError(error.code))?;
        let (ready, mut idle) = input::input_state(&screen);
        let (files, plugin_store, question_frame) = (target.clone(), self.plugin.clone(), screen.clone());
        let (native, question) = tokio::task::spawn_blocking(move || {
            let native = native_record(&files.metadata, &files.binding.conversation)?;
            let question = match plugin_store {
                Some(store) => crate::state::ask::observe_terminal(store.interactions(), &Self::binding(&files), &files.transcript, &question_frame).map(|question| question.is_some()),
                None => crate::state::ask::read_pending(&files.transcript).map(|question| question.is_some()),
            }.map_err(|code| ServiceError(crate::state::ask::failure_code(&code)))?;
            Ok::<_, ServiceError>((native, question))
        }).await.map_err(|_| ServiceError("terminal_files_unavailable"))??;
        let plugin = self.plugin_facts(target)?;
        let current = plugin_current(&plugin, target.metadata.cut(target.created));
        if let Some(state) = native.as_ref().and_then(|record| native_state(record["status"].as_str()?)) { idle = state == "idle"; }
        else if current && let Some(state) = plugin["plugin_state"]["state"].as_str().filter(|_| plugin["waiter_open"] == true || plugin["plugin_state"]["age_ms"].as_u64().is_some_and(|age| age <= 90_000)) { idle = state == "idle"; }
        else {
            let (metadata, conversation, created) = (target.metadata.clone(), binding.conversation.clone(), target.created);
            if let Some(state) = tokio::task::spawn_blocking(move || hook_state(&metadata, &conversation, created)).await.map_err(|_| ServiceError("terminal_hook_unavailable"))?? { idle = state == "idle"; }
        }
        let live = current && plugin["waiter_open"] == true;
        let open_question = question || plugin["questions"].as_array().is_some_and(|questions| !questions.is_empty());
        // Em qualquer SO, como o `_native_socket` do Python: quem não fala o socket (o escritor fora do Unix)
        // responde `NotWritten` e a entrega segue pelo teclado.
        let native = if let Some((sender, _)) = input::message_prefix(text)
            && let Some(socket) = native.as_ref().and_then(|record| record["messagingSocketPath"].as_str()).filter(|path| !path.is_empty())
            && Path::new(socket).try_exists().map_err(|_| ServiceError("terminal_native_socket_unavailable"))? {
            Some(NativeMessage { socket: socket.into(), origin: target.metadata.origin.clone(), sender: sender.into(),
                mode: self.message_class(target, sender, !text.starts_with("[painel:")).await?, message_id: None })
        } else { None };
        // A tela e os arquivos lidos acima valem só com a mesma vida conferida depois deles.
        let pane_pid = self.check(target, binding).await?;
        let clipboard_available = binding.windows && clipboard_available(pane_pid);
        Ok(InputFacts { binding: binding.clone(), ready, idle, open_question, plugin_live: live, plugin_user: live && has_mode(&plugin, "user"), clipboard_available, native, screen: Some(screen) })
    }
    pub fn plugin_facts(&self, target: &TerminalTarget) -> Result<Value, ServiceError> {
        match &self.plugin { Some(store) => store.facts(&Self::binding(target), Instant::now()), None => Ok(Value::Null) }
    }
    pub fn abandon_publication(&self, target: &TerminalTarget, publication_id: &str) -> Result<(), ServiceError> {
        match &self.plugin { Some(store) => store.abandon(&Self::binding(target), publication_id), None => Ok(()) }
    }
    pub async fn publish(&self, target: &TerminalTarget, binding: &TerminalBinding, request: PluginRequest) -> Result<PluginReply, ServiceError> {
        self.check(target, binding).await?;
        if !plugin_current(&self.plugin_facts(target)?, target.metadata.cut(target.created)) { return Ok(PluginReply::NotWritten); }
        // A prova da tela e a limpeza depois do `Fill` ficariam sem a conversa: a entrega vai pelo teclado,
        // que deixa o `/pull` aberto.
        if request.mode == input::PluginMode::Fill && self.pull_is_the_proof(target).await? { return Ok(PluginReply::NotWritten); }
        self.plugin.as_ref().ok_or(ServiceError("plugin_store_unavailable"))?.publish(&Self::binding(target), request).await
    }
    pub async fn plugin_question(&self, target: &TerminalTarget, expected: &ExpectedInteraction) -> Result<PluginQuestion, ServiceError> {
        self.check(target, &target.binding).await?;
        let binding = Self::binding(target);
        if expected.binding != binding { return Err(ServiceError(InteractionError::Changed.code())); }
        let facts = self.plugin_facts(target)?;
        if !plugin_current(&facts, target.metadata.cut(target.created)) || !has_mode(&facts, "receipt_v2") { return Err(ServiceError(InteractionError::Unavailable.code())); }
        let store = self.plugin.as_ref().ok_or(ServiceError(InteractionError::Unavailable.code()))?;
        let mut questions = store.pending_all(&binding, Instant::now())?.into_iter().filter(|question| {
            let kind = if question.id.starts_with("perm:") { crate::interactions::InteractionKind::Approval } else { crate::interactions::InteractionKind::Question };
            question.interaction_id == expected.interaction_id && plugin_target(kind, &question.id) == expected.target
        });
        let question = questions.next().ok_or(ServiceError(InteractionError::Changed.code()))?;
        if questions.next().is_some() { return Err(ServiceError(InteractionError::Unavailable.code())); }
        let semantic = plugin_semantic(&serde_json::to_value(&question).map_err(|_| ServiceError(InteractionError::Unavailable.code()))?);
        expected.check(&plugin_target(expected.target.kind.clone(), &question.id), &semantic, None).map_err(|error| ServiceError(error.code()))?;
        Ok(question)
    }
    async fn session_mode(&self, name: &str, target: Option<&TerminalTarget>) -> Result<Option<String>, ServiceError> {
        let source = match &self.modes { Some(read) => read(name.into()).await?, None => None };
        if let Some(source) = &source {
            if target.is_some_and(|target| source.binding != Self::binding(target) || source.transcript != target.transcript) { return Err(ServiceError("terminal_mode_binding_changed")); }
            if let Some(mode) = &source.confirmed { return Ok(Some(mode.clone())); }
        }
        let path = source.map(|source| source.transcript).or_else(|| target.map(|target| target.transcript.clone()));
        let Some(path) = path else { return Ok(None); }; let cache = self.transcripts.clone();
        tokio::task::spawn_blocking(move || transcript_mode(&path, &cache)).await.map_err(|_| ServiceError("terminal_mode_unavailable"))?
    }
    async fn message_class(&self, target: &TerminalTarget, sender: &str, sender_is_session: bool) -> Result<String, ServiceError> {
        let valid_name = !sender.is_empty() && !sender.starts_with('-') && !sender.ends_with('-') && sender.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-'));
        let sender_mode = if sender_is_session && valid_name { self.session_mode(sender, (sender == target.name).then_some(target)).await? } else { None };
        let mode = match sender_mode { Some(mode) => mode, None => match self.session_mode(&target.name, Some(target)).await? {
            Some(mode) => mode, None => { let agent = target.metadata.agent_pid; let config = target.metadata.config_dir.clone(); tokio::task::spawn_blocking(move || launch_or_account_mode(Some(agent), &config)).await.map_err(|_| ServiceError("terminal_account_mode_unavailable"))?? }
        }};
        Ok(if mode.to_ascii_lowercase().contains("bypass") { "bypass" } else { "prompting" }.into())
    }
}
/// O pane na mesma sessão de logon do servidor: só então a área de transferência é a mesma.
fn clipboard_available(pane_pid: i64) -> bool {
    u32::try_from(pane_pid).ok().and_then(process_session).zip(process_session(std::process::id())).is_some_and(|(theirs, ours)| ours > 0 && ours == theirs)
}
fn read_json(path: &Path, code: &'static str) -> Result<Option<Value>, ServiceError> {
    match std::fs::read(path) { Ok(bytes) => serde_json::from_slice(&bytes).map(Some).map_err(|_| ServiceError(code)), Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None), Err(_) => Err(ServiceError(code)) }
}
fn native_record(metadata: &TerminalMetadata, conversation: &str) -> Result<Option<Value>, ServiceError> {
    let record = read_json(&metadata.config_dir.join("sessions").join(format!("{}.json", metadata.agent_pid)), "terminal_native_record_unavailable")?;
    if let Some(record) = &record && (record["pid"].as_u64() != Some(u64::from(metadata.agent_pid)) || record["sessionId"].as_str() != Some(conversation)) { return Err(ServiceError("terminal_native_record_changed")); } Ok(record)
}
fn current_transcript(procs: &dyn ProcessView, metadata: &TerminalMetadata, conversation: &str, transcript: &Path) -> Result<(), ServiceError> {
    let unavailable = || ServiceError("terminal_conversation_unavailable");
    let projects = metadata.config_dir.join("projects").canonicalize().map_err(|_| unavailable())?;
    let expected = transcript.canonicalize().map_err(|_| unavailable())?;
    let fds = procs.fds_checked(i64::from(metadata.agent_pid)).map_err(|_| unavailable())?;
    let mut current = std::collections::BTreeSet::new();
    for path in fds.into_iter().filter(|path| path.extension().is_some_and(|extension| extension == "jsonl")) {
        let path = path.canonicalize().map_err(|_| unavailable())?;
        if path.starts_with(&projects) { current.insert(path); }
    }
    // O argv conserva o SID de abertura depois do /clear; só arquivo aberto prova a conversa atual.
    let mut current = current.into_iter();
    let path = current.next().ok_or_else(unavailable)?;
    if current.next().is_some() { return Err(unavailable()); }
    if path != expected || path.file_stem().and_then(|stem| stem.to_str()) != Some(conversation) {
        return Err(ServiceError("terminal_conversation_changed"));
    }
    Ok(())
}
fn native_state(status: &str) -> Option<&'static str> { match status { "idle" | "shell" => Some("idle"), "busy" => Some("working"), "waiting" => Some("awaiting_input"), _ => None } }
fn hook_state(metadata: &TerminalMetadata, conversation: &str, created: f64) -> Result<Option<String>, ServiceError> {
    let Some(record) = read_json(&metadata.config_dir.join(".hangar-state").join(format!("{conversation}.json")), "terminal_hook_unavailable")? else { return Ok(None); };
    let state = record["state"].as_str().ok_or(ServiceError("terminal_hook_invalid"))?;
    let ts = record["ts"].as_f64().filter(|ts| ts.is_finite()).ok_or(ServiceError("terminal_hook_invalid"))?;
    Ok((ts >= metadata.legacy_import_after.unwrap_or(created)).then(|| state.to_owned()))
}
pub(crate) fn plugin_current(facts: &Value, cut: f64) -> bool {
    facts["owner_instance"].as_str().and_then(|instance| instance.split('-').next()).and_then(|stamp| u64::from_str_radix(stamp, 36).ok()).is_some_and(|stamp| stamp as f64 / 1000.0 >= cut)
}
fn has_mode(facts: &Value, mode: &str) -> bool { facts["declared_modes"].as_array().is_some_and(|modes| modes.iter().any(|value| value.as_str() == Some(mode))) }
pub(crate) fn message_id(key: &str, operation: &str) -> String {
    let mut hash = sha1_smol::Sha1::new();hash.update(&[0x6b,0xa7,0xb8,0x11,0x9d,0xad,0x11,0xd1,0x80,0xb4,0x00,0xc0,0x4f,0xd4,0x30,0xc8]);hash.update(format!("hangar:{key}:{operation}").as_bytes());
    let mut bytes = hash.digest().bytes();bytes[6] = (bytes[6] & 0x0f) | 0x50;bytes[8] = (bytes[8] & 0x3f) | 0x80;
    let hex: String = bytes[..16].iter().map(|byte| format!("{byte:02x}")).collect();format!("{}-{}-{}-{}-{}", &hex[..8], &hex[8..12], &hex[12..16], &hex[16..20], &hex[20..])
}
pub(crate) fn transcript_mode(path: &Path, cache: &Mutex<TranscriptModes>) -> Result<Option<String>, ServiceError> {
    let mut file = match std::fs::File::open(path) { Ok(file) => file, Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None), Err(_) => return Err(ServiceError("terminal_transcript_mode_unavailable")) };
    let meta = file.metadata().map_err(|_| ServiceError("terminal_transcript_mode_unavailable"))?;let key = (meta.modified().map_err(|_| ServiceError("terminal_transcript_mode_unavailable"))?, meta.len());
    let mut cache = cache.lock().map_err(|_| ServiceError("terminal_transcript_mode_unavailable"))?;if let Some((version, mode)) = cache.get(path) && version == &key { return Ok(mode.clone()); }
    let start = key.1.saturating_sub(8 * 1024 * 1024);file.seek(SeekFrom::Start(start)).map_err(|_| ServiceError("terminal_transcript_mode_unavailable"))?;
    let mut bytes = Vec::new();file.take(key.1 - start).read_to_end(&mut bytes).map_err(|_| ServiceError("terminal_transcript_mode_unavailable"))?;
    let bytes = if start > 0 { bytes.iter().position(|byte| *byte == b'\n').map(|at| &bytes[at + 1..]).unwrap_or(&[]) } else { &bytes[..] };
    let mode = bytes.rsplit(|byte| *byte == b'\n').find_map(|raw| {
        if !raw.windows(b"\"permissionMode\"".len()).any(|window| window == b"\"permissionMode\"") { return None; }
        let value: Value = serde_json::from_slice(raw).ok()?;value["permissionMode"].as_str().filter(|mode| !mode.is_empty() && *mode != "plan").map(str::to_owned)
    });cache.insert(path.to_owned(), (key, mode.clone()));Ok(mode)
}
/// Modo sem confirmação na memória: o último que o próprio CLI gravou no transcript, senão o de abertura
/// e, por último, o da conta. Para a classe do recado que o Rust manda pronta.
pub(crate) fn unconfirmed_mode(transcript: &Path, agent: Option<u32>, config: &Path) -> Result<String, ServiceError> {
    match transcript_mode(transcript, &Mutex::new(TranscriptModes::default()))? { Some(mode) => Ok(mode), None => launch_or_account_mode(agent, config) }
}
/// Sessão recém-aberta ainda sem rodapé lido nem transcript: o `--permission-mode` com que o CLI nasceu
/// decide antes do `defaultMode` da conta, que diverge dele e faz o Claude reter o recado.
fn launch_or_account_mode(agent: Option<u32>, config: &Path) -> Result<String, ServiceError> {
    // argv ilegível = agente que já saiu: o recado não chega a ele, e a conta segue como antes.
    let argv = agent.and_then(|pid| SystemProcs::default().argv_checked(i64::from(pid)).ok());
    match argv.as_deref().and_then(launch_mode) { Some(mode) => Ok(mode), None => account_mode(config) }
}
/// `--allow-dangerously-skip-permissions` só põe o bypass no ciclo do Shift+Tab: não é o modo de abertura.
fn launch_mode(argv: &[String]) -> Option<String> {
    if argv.iter().skip(1).take_while(|arg| *arg != "--").any(|arg| arg == "--dangerously-skip-permissions") { return Some("bypassPermissions".into()); }
    let argv: Vec<std::ffi::OsString> = argv.iter().map(Into::into).collect();
    let (_, _, mode) = crate::runtime::claude_relaunch::flag(&argv, "--permission-mode").ok()??;
    canonical_mode(mode.to_str()?).filter(|mode| *mode != "plan").map(Into::into)
}
/// Nome do modo como o rodapé o mostra; `default` é o `manual` dele.
fn canonical_mode(mode: &str) -> Option<&'static str> {
    Some(match mode { "default" | "manual" => "manual", "plan" => "plan", "auto" => "auto", "acceptEdits" => "acceptEdits", "bypassPermissions" => "bypassPermissions", "dontAsk" => "dontAsk", _ => return None })
}
fn account_mode(config: &Path) -> Result<String, ServiceError> {
    let value = read_json(&config.join("settings.json"), "terminal_account_mode_unavailable")?;
    Ok(value.as_ref().and_then(|value| value["permissions"]["defaultMode"].as_str()).and_then(canonical_mode).unwrap_or("bypassPermissions").into())
}
pub(crate) fn same_birth(a: f64, b: f64) -> bool { if cfg!(any(target_os = "linux", windows)) { (a - b).abs() < 0.001 } else { a.floor() == b.floor() } }
#[cfg(not(windows))]
pub fn agent_birth(pid: u32) -> Option<f64> { SystemProcs::default().start_time(i64::from(pid)) }
#[cfg(windows)]
pub fn agent_birth(pid: u32) -> Option<f64> {
    #[repr(C)] struct FileTime { low:u32, high:u32 }
    #[link(name="kernel32")]
    unsafe extern "system" {
        #[link_name="OpenProcess"] fn open_process(access:u32,inherit:i32,pid:u32)->*mut std::ffi::c_void;
        #[link_name="GetProcessTimes"] fn get_process_times(handle:*mut std::ffi::c_void,created:*mut FileTime,exited:*mut FileTime,kernel:*mut FileTime,user:*mut FileTime)->i32;
        #[link_name="CloseHandle"] fn close_handle(handle:*mut std::ffi::c_void)->i32;
        #[link_name="WaitForSingleObject"] fn wait_for_single_object(handle:*mut std::ffi::c_void,millis:u32)->u32;
    }
    let handle=unsafe {open_process(0x0010_1000,0,pid)};if handle.is_null() {return None;}
    let (mut created,mut exited,mut kernel,mut user)=(FileTime {low:0,high:0},FileTime {low:0,high:0},FileTime {low:0,high:0},FileTime {low:0,high:0});
    let running=unsafe {wait_for_single_object(handle,0)}==258;let ok=running && unsafe {get_process_times(handle,&mut created,&mut exited,&mut kernel,&mut user)}!=0;unsafe {close_handle(handle);}
    let ticks=(u64::from(created.high)<<32)|u64::from(created.low);ok.then(||ticks as f64 / 10_000_000.0 - 11_644_473_600.0)
}
#[cfg(windows)]
fn process_session(pid: u32) -> Option<u32> {
    #[link(name = "kernel32")] unsafe extern "system" { fn ProcessIdToSessionId(pid: u32, session: *mut u32) -> i32; }
    let mut session = 0;(unsafe { ProcessIdToSessionId(pid, &mut session) } != 0).then_some(session)
}
#[cfg(not(windows))]
fn process_session(_: u32) -> Option<u32> { None }

#[cfg(test)]
mod tests {
    use super::*;
    use crate::terminal_input::{CommandOutput, IoFuture, WriteOutcome};
    use serde_json::json;
    struct FrameIo(Vec<u8>);
    impl TerminalIo for FrameIo {
        fn command<'a>(&'a self, request: CommandRequest) -> IoFuture<'a, CommandOutput> { Box::pin(async move {
            let stdout = match request.args[0].as_str() { "capture-pane" => self.0.clone(), "display-message" if request.args.last().is_some_and(|arg| arg == "#{pane_pid}\t#{session_name}\t#{pane_id}\t#{session_created}") => format!("{}\tsession\t%1\t1\n",std::process::id()).into_bytes(), "display-message" => b"session\t%1\t1\n".to_vec(), _ => panic!("fatos locais não escrevem") };Ok(CommandOutput { success: true, stdout })
        }) }
        fn socket<'a>(&'a self, _: &'a NativeMessage, _: Vec<u8>) -> IoFuture<'a, WriteOutcome> { Box::pin(async { panic!("fatos locais não entregam") }) }
    }
    fn target(config: &Path) -> TerminalTarget {
        let pid = std::process::id();TerminalTarget { key:"key".into(), generation:1, name:"session".into(),
            binding:TerminalBinding { name:"session".into(),pane:"%1".into(),conversation:"sid".into(),generation:1,created:1,mux_argv:vec!["fake".into()],windows:false,clipboard_lock_path:None },
            lease_path:config.join("lease"),state_path:config.join("state"),projection_dir:config.join("projection"),transcript:config.join("projects/project/sid.jsonl"),created:1.0,plugin_key:None,
            metadata:TerminalMetadata {config_dir:config.into(),agent_pid:pid,agent_birth:agent_birth(pid).unwrap(),legacy_import_after:Some(1.0),fingerprint:"fixture-life".into(),origin:"uds:existing-inbox".into()} }
    }
    fn frame() -> Arc<dyn TerminalIo> { Arc::new(FrameIo("────────────────────────────────\n❯ \n────────────────────────────────\n".as_bytes().to_vec())) }
    #[test]
    fn descriptor_origin_is_optional_but_proof_fields_are_not() {
        let meta=json!({"config_dir":"config","agent_pid":42,"agent_birth":123.5,"fingerprint":"life","message_origin":"uds:existing"});
        assert_eq!(TerminalMetadata::from_descriptor(&meta).unwrap().origin,"uds:existing");
        for field in ["config_dir","agent_pid","agent_birth","fingerprint"] {let mut missing=meta.clone();missing.as_object_mut().unwrap().remove(field);assert!(TerminalMetadata::from_descriptor(&missing).is_err());}
        let mut missing=meta.clone();missing.as_object_mut().unwrap().remove("message_origin");assert_eq!(TerminalMetadata::from_descriptor(&missing).unwrap().origin,format!("uds:hangar-{}",std::process::id()));
        let mut wrong=meta;wrong["message_origin"]=json!("peer");assert!(TerminalMetadata::from_descriptor(&wrong).is_err());
    }
    #[test]
    fn native_record_absent_is_distinct_from_unreadable_or_other_life() {
        let dir=tempfile::tempdir().unwrap();let target=target(dir.path());std::fs::create_dir(dir.path().join("sessions")).unwrap();let path=dir.path().join("sessions").join(format!("{}.json",target.metadata.agent_pid));
        assert!(native_record(&target.metadata,"sid").unwrap().is_none());std::fs::write(&path,"invalid").unwrap();assert_eq!(native_record(&target.metadata,"sid").unwrap_err().0,"terminal_native_record_unavailable");
        for record in [json!({"pid":target.metadata.agent_pid+1,"sessionId":"sid","messagingSocketPath":"other"}),json!({"pid":target.metadata.agent_pid,"sessionId":"other","messagingSocketPath":"other"})] {std::fs::write(&path,record.to_string()).unwrap();assert_eq!(native_record(&target.metadata,"sid").unwrap_err().0,"terminal_native_record_changed");}
        std::fs::write(&path,json!({"pid":target.metadata.agent_pid,"sessionId":"sid","messagingSocketPath":"validated"}).to_string()).unwrap();assert_eq!(native_record(&target.metadata,"sid").unwrap().unwrap()["messagingSocketPath"],"validated");
    }
    #[cfg(target_os="linux")]
    #[tokio::test]
    async fn missing_native_record_requires_one_current_open_transcript() {
        let dir=tempfile::tempdir().unwrap();let target=target(dir.path());let local=LocalTerminalFacts::new(frame(),None,None);
        std::fs::create_dir_all(target.transcript.parent().unwrap()).unwrap();std::fs::write(&target.transcript,"").unwrap();
        assert_eq!(local.check(&target,&target.binding).await.unwrap_err().0,"terminal_conversation_unavailable");
        let a=std::fs::File::open(&target.transcript).unwrap();assert!(local.check(&target,&target.binding).await.is_ok());
        let path=target.transcript.parent().unwrap().join("sid-B.jsonl");std::fs::write(&path,"").unwrap();let _b=std::fs::File::open(&path).unwrap();
        assert_eq!(local.check(&target,&target.binding).await.unwrap_err().0,"terminal_conversation_unavailable");
        drop(a);assert_eq!(local.check(&target,&target.binding).await.unwrap_err().0,"terminal_conversation_changed");
    }
    #[tokio::test]
    async fn birth_binding_and_capture_failures_never_become_ready() {
        let dir=tempfile::tempdir().unwrap();let mut target=target(dir.path());let local=LocalTerminalFacts::new(frame(),None,None);
        std::fs::create_dir(dir.path().join("sessions")).unwrap();
        std::fs::write(dir.path().join("sessions").join(format!("{}.json",target.metadata.agent_pid)),json!({"pid":target.metadata.agent_pid,"sessionId":"sid"}).to_string()).unwrap();
        target.metadata.agent_birth+=1.0;assert_eq!(local.read(&target,&target.binding,"").await.unwrap_err().0,"terminal_agent_changed");target.metadata.agent_birth-=1.0;
        let mut binding=target.binding.clone();binding.generation+=1;assert_eq!(local.read(&target,&binding,"").await.unwrap_err().0,"terminal_binding_changed");
        let local=LocalTerminalFacts::new(Arc::new(FrameIo(vec![0xff])),None,None);assert_eq!(local.read(&target,&target.binding,"").await.unwrap_err().0,"capture_utf8");
        target.binding.windows=true;target.binding.pane="=session:0.0".into();let local=LocalTerminalFacts::new(frame(),None,None);
        // O pane do teste é este processo: a sessão de logon do servidor, que só serve fora da sessão 0.
        assert_eq!(local.read(&target,&target.binding,"").await.unwrap().clipboard_available,interactive_session());
    }
    #[test]
    fn clipboard_needs_a_pane_whose_logon_session_is_readable() {
        assert!(!clipboard_available(-1));assert!(!clipboard_available(i64::from(u32::MAX)));
        assert_eq!(clipboard_available(i64::from(std::process::id())),interactive_session());
    }
    /// Sessão de logon deste processo legível e fora da 0 (a dos serviços); nunca fora do Windows.
    fn interactive_session() -> bool { process_session(std::process::id()).is_some_and(|session| session > 0) }
    #[tokio::test]
    async fn native_then_current_plugin_then_hook_decide_idle() {
        use crate::plugin::Pull;
        let dir=tempfile::tempdir().unwrap();let mut target=target(dir.path());let binding=LocalTerminalFacts::binding(&target);let plugin=Arc::new(Store::new(Arc::new(crate::interactions::InteractionStore::default())));plugin.interactions().bind(&binding).unwrap();plugin.bind(&binding).unwrap();
        let now=SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs_f64();let mut stamp=(now*1000.0) as u64;let mut instance=String::new();while stamp>0 {instance.insert(0,char::from_digit((stamp%36) as u32,36).unwrap());stamp/=36;}
        let poll=plugin.begin_pull(&binding,Pull {instance,modos:vec!["user".into(),"receipt_v2".into()],estado:Some("working".into()),session_id:Some("sid".into())},Instant::now()).unwrap();
        std::fs::create_dir(dir.path().join("sessions")).unwrap();std::fs::create_dir(dir.path().join(".hangar-state")).unwrap();let native=dir.path().join("sessions").join(format!("{}.json",target.metadata.agent_pid));
        std::fs::write(&native,json!({"pid":target.metadata.agent_pid,"sessionId":"sid","status":"idle"}).to_string()).unwrap();std::fs::write(dir.path().join(".hangar-state/sid.json"),"invalid").unwrap();
        let local=LocalTerminalFacts::new(frame(),Some(plugin.clone()),None);let facts=local.read(&target,&target.binding,"").await.unwrap();assert!(facts.ready && facts.idle && facts.plugin_live);
        std::fs::write(&native,json!({"pid":target.metadata.agent_pid,"sessionId":"sid"}).to_string()).unwrap();
        assert!(!local.read(&target,&target.binding,"").await.unwrap().idle);target.metadata.legacy_import_after=Some(now+1.0);
        std::fs::write(dir.path().join(".hangar-state/sid.json"),json!({"state":"idle","ts":now+2.0}).to_string()).unwrap();assert!(local.read(&target,&target.binding,"").await.unwrap().idle);
        drop(poll);plugin.forget("key").unwrap();assert!(local.read(&target,&target.binding,"").await.is_err());
    }
    #[cfg(target_os="linux")]
    #[tokio::test]
    async fn open_plugin_pull_proves_the_conversation_without_the_native_record() {
        use crate::plugin::Pull;
        let dir=tempfile::tempdir().unwrap();let target=target(dir.path());let binding=LocalTerminalFacts::binding(&target);
        std::fs::create_dir_all(target.transcript.parent().unwrap()).unwrap();std::fs::write(&target.transcript,"").unwrap();
        let plugin=Arc::new(Store::new(Arc::new(crate::interactions::InteractionStore::default())));plugin.interactions().bind(&binding).unwrap();plugin.bind(&binding).unwrap();
        let local=LocalTerminalFacts::new(frame(),Some(plugin.clone()),None);
        // Sem registro nativo nem o transcript aberto pelo processo, e sem plugin: não há prova.
        assert_eq!(local.check(&target,&target.binding).await.unwrap_err().0,"terminal_conversation_unavailable");
        let now=SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs_f64();let mut stamp=(now*1000.0) as u64;let mut instance=String::new();while stamp>0 {instance.insert(0,char::from_digit((stamp%36) as u32,36).unwrap());stamp/=36;}
        let poll=plugin.begin_pull(&binding,Pull {instance,modos:vec!["fill".into()],estado:Some("idle".into()),session_id:Some("sid".into())},Instant::now()).unwrap();
        assert!(local.check(&target,&target.binding).await.is_ok(),"o /pull aberto prova a conversa");
        // `session.end` (o `/clear`) depois de o /pull começar: a prova acaba, mesmo com outro estado depois.
        plugin.record_state(&binding,"fim".into(),Some("clear".into()),Instant::now()).unwrap();
        plugin.record_state(&binding,"idle".into(),None,Instant::now()).unwrap();
        assert_eq!(local.check(&target,&target.binding).await.unwrap_err().0,"terminal_conversation_unavailable");
        drop(poll);
    }
    /// Pane que troca de tela a cada Enter enviado, como o formulário do TUI.
    struct FormIo { screens: Vec<String>, at: Mutex<usize>, keys: Mutex<Vec<String>> }
    impl TerminalIo for FormIo {
        fn command<'a>(&'a self, request: CommandRequest) -> IoFuture<'a, CommandOutput> { Box::pin(async move {
            let stdout = match request.args[0].as_str() {
                "capture-pane" => self.screens[*self.at.lock().unwrap()].clone().into_bytes(),
                "display-message" if request.args.last().is_some_and(|arg| arg.starts_with("#{pane_pid}\t")) => format!("{}\tsession\t%1\t1\n", std::process::id()).into_bytes(),
                "display-message" => b"session\t%1\t1\n".to_vec(),
                "send-keys" => {
                    let key = request.args.last().unwrap().clone();
                    if key == "\r" { let mut at = self.at.lock().unwrap(); *at = (*at + 1).min(self.screens.len() - 1); }
                    self.keys.lock().unwrap().push(key);
                    Vec::new()
                }
                other => panic!("comando inesperado: {other}"),
            };
            Ok(CommandOutput { success: true, stdout })
        }) }
        fn socket<'a>(&'a self, _: &'a NativeMessage, _: Vec<u8>) -> IoFuture<'a, WriteOutcome> { Box::pin(async { panic!("resposta não usa o socket") }) }
    }
    /// Os fatos de verdade (`LocalTerminalFacts::read`) atrás do executor, como no runtime.
    struct FormFacts { local: LocalTerminalFacts, target: TerminalTarget }
    impl input::TerminalServices for FormFacts {
        fn facts<'a>(&'a self, binding: &'a TerminalBinding) -> input::ServiceFuture<'a, InputFacts> { Box::pin(self.local.read(&self.target, binding, "")) }
        fn publish<'a>(&'a self, _: &'a TerminalBinding, _: PluginRequest) -> input::ServiceFuture<'a, PluginReply> { Box::pin(async { panic!("resposta pelo pane não publica no plugin") }) }
        fn permission_guard<'a>(&'a self, _: &'a TerminalBinding) -> input::ServiceFuture<'a, ()> { Box::pin(async { Ok(()) }) }
    }
    #[tokio::test]
    async fn two_question_form_concludes_through_local_facts_and_replacement_is_refused() {
        let dir=tempfile::tempdir().unwrap();let target=target(dir.path());let binding=LocalTerminalFacts::binding(&target);
        std::fs::create_dir(dir.path().join("sessions")).unwrap();
        std::fs::write(dir.path().join("sessions").join(format!("{}.json",target.metadata.agent_pid)),json!({"pid":target.metadata.agent_pid,"sessionId":"sid","status":"idle"}).to_string()).unwrap();
        let sidecar=crate::state::ask::sidecar_path(&target.transcript).unwrap();std::fs::create_dir_all(sidecar.parent().unwrap()).unwrap();
        let cor=json!({"question":"Cor?","header":"Cor","multiSelect":false,"options":[{"label":"Azul"},{"label":"Verde"}]});
        let pasta=json!({"question":"Pasta?","header":"Pasta","multiSelect":false,"options":[{"label":"A"},{"label":"B"}]});
        let form_a=json!({"tool_input":{"questions":[cor,pasta.clone()]}}).to_string();let form_b=json!({"tool_input":{"questions":[pasta]}}).to_string();
        let foot="\nEnter to select · ↑/↓ to navigate · Esc to cancel\n";
        let step_one=format!("   Cor?\n\n ❯ 1. Azul\n   2. Verde\n   3. Type something.\n   4. Chat about this\n{foot}");
        let step_two=format!("   Pasta?\n\n ❯ 1. A\n   2. B\n   3. Type something.\n   4. Chat about this\n{foot}");
        let review="Review your answers\n\n → Azul\n → A\n\n ❯ 1. Submit answers\n   2. Cancel\n\nEsc to cancel\n".to_string();
        let closed="────────────────────────────────\n❯ \n────────────────────────────────\n".to_string();
        let other=format!("   Outra?\n\n ❯ 1. X\n   2. Y\n{foot}");
        let answers:Vec<input::QuestionAnswer>=serde_json::from_value(json!([{"kind":"option","indices":[0],"labels":["Azul"]},{"kind":"option","indices":[0],"labels":["A"]}])).unwrap();
        let limits=InputLimits {settle:Duration::ZERO,literal_settle:Duration::ZERO,multiline_settle:Duration::ZERO,slash_settle:Duration::ZERO,proof_attempts:2,ready_attempts:2,cleanup_attempts:2};
        // Formulário A publicado na primeira etapa, como a lista e o monitor fazem.
        let run=|screens:Vec<String>| {
            std::fs::write(&sidecar,&form_a).unwrap();
            let interactions=Arc::new(crate::interactions::InteractionStore::default());let plugin=Arc::new(Store::new(interactions.clone()));
            interactions.bind(&binding).unwrap();plugin.bind(&binding).unwrap();
            let id=crate::state::ask::observe_terminal(&interactions,&binding,&target.transcript,&step_one).unwrap().unwrap().interaction_id.unwrap();
            let expected=interactions.expected(&binding,Some(&id)).unwrap();
            let io=Arc::new(FormIo {screens,at:Mutex::new(0),keys:Mutex::new(Vec::new())});
            let facts=Arc::new(FormFacts {local:LocalTerminalFacts::new(io.clone(),Some(plugin),None),target:target.clone()});
            let driver=TerminalDriver::new(target.binding.clone(),facts,io.clone(),limits.clone()).with_interactions(interactions.clone()).with_interaction_source(target.transcript.clone());
            (interactions,expected,io,driver)
        };
        // Etapa 1 → etapa 2 → revisão → composer: conclui com o ID publicado na etapa 1.
        let (interactions,expected,io,driver)=run(vec![step_one.clone(),step_two.clone(),review,closed]);
        let result=driver.guarded_answer(&expected,&answers).await;
        assert_eq!((result.disposition,result.code.as_str()),(input::Disposition::Accepted,"control_submitted"));
        assert_eq!(*io.keys.lock().unwrap(),["\r","\r","\r"]);
        assert_eq!(interactions.expected(&binding,Some(&expected.interaction_id)).unwrap(),expected);
        // Menu fora do formulário: é menu do pane, não esta pergunta; o ID do formulário não o responde.
        let (_,expected,io,driver)=run(vec![other]);
        assert!(driver.capture().await.is_ok());
        assert_eq!(driver.guarded_answer(&expected,&answers).await.disposition,input::Disposition::Rejected);
        assert!(io.keys.lock().unwrap().is_empty());
        // Troca real A→B na etapa 2: o ID de A não responde B.
        let (_,expected,io,driver)=run(vec![step_two]);
        std::fs::write(&sidecar,&form_b).unwrap();
        let result=driver.guarded_answer(&expected,&answers).await;
        assert_eq!((result.disposition,result.code.as_str()),(input::Disposition::Rejected,crate::interactions::InteractionError::Changed.code()));
        assert!(io.keys.lock().unwrap().is_empty());
    }
    #[tokio::test]
    async fn menu_beside_another_question_and_cut_preview_labels_still_answer() {
        let dir=tempfile::tempdir().unwrap();let target=target(dir.path());let binding=LocalTerminalFacts::binding(&target);
        std::fs::create_dir(dir.path().join("sessions")).unwrap();
        std::fs::write(dir.path().join("sessions").join(format!("{}.json",target.metadata.agent_pid)),json!({"pid":target.metadata.agent_pid,"sessionId":"sid","status":"idle"}).to_string()).unwrap();
        let sidecar=crate::state::ask::sidecar_path(&target.transcript).unwrap();std::fs::create_dir_all(sidecar.parent().unwrap()).unwrap();
        let foot="\nEnter to select · ↑/↓ to navigate · Esc to cancel\n";
        let closed="────────────────────────────────\n❯ \n────────────────────────────────\n".to_string();
        let limits=InputLimits {settle:Duration::ZERO,literal_settle:Duration::ZERO,multiline_settle:Duration::ZERO,slash_settle:Duration::ZERO,proof_attempts:2,ready_attempts:2,cleanup_attempts:2};
        let setup=|screens:Vec<String>| {
            let interactions=Arc::new(crate::interactions::InteractionStore::default());let plugin=Arc::new(Store::new(interactions.clone()));
            interactions.bind(&binding).unwrap();plugin.bind(&binding).unwrap();
            let io=Arc::new(FormIo {screens,at:Mutex::new(0),keys:Mutex::new(Vec::new())});
            let facts=Arc::new(FormFacts {local:LocalTerminalFacts::new(io.clone(),Some(plugin),None),target:target.clone()});
            let driver=TerminalDriver::new(target.binding.clone(),facts,io.clone(),limits.clone()).with_interactions(interactions.clone()).with_interaction_source(target.transcript.clone());
            (interactions,io,driver)
        };
        // Sidecar de outra pergunta e uma permissão na tela: os botões do pane respondem.
        std::fs::write(&sidecar,json!({"tool_input":{"questions":[{"question":"Cor?","header":"Cor","multiSelect":false,"options":[{"label":"Azul"},{"label":"Verde"}]}]}}).to_string()).unwrap();
        let menu=format!("   Permitir Bash?\n\n ❯ 1. Yes\n   2. No\n{foot}");
        let (interactions,io,driver)=setup(vec![menu.clone(),closed.clone()]);
        let id=crate::state::ask::observe_menu(&interactions,&binding,&target.transcript,&menu).unwrap().expect("o menu ganha identidade");
        let result=driver.guarded_select(&interactions.expected(&binding,Some(&id)).unwrap(),1,true).await;
        assert_eq!((result.disposition,result.code.as_str()),(input::Disposition::Accepted,"selected"));
        assert_eq!(*io.keys.lock().unwrap(),["\r"]);
        // Prévia: o pane corta o rótulo; a resposta segue pelo mesmo casamento da publicação.
        std::fs::write(&sidecar,json!({"tool_input":{"questions":[{"question":"Layout?","header":"L","multiSelect":false,
            "options":[{"label":"Grade larga","preview":"x"},{"label":"Lista","preview":"y"}]}]}}).to_string()).unwrap();
        let cut=format!("   Layout?\n\n ❯ 1. Grade lar\n   2. Lista\n   3. Type something.\n   4. Chat about this\n{foot}");
        let (interactions,io,driver)=setup(vec![cut.clone(),closed]);
        let id=crate::state::ask::observe_terminal(&interactions,&binding,&target.transcript,&cut).unwrap().unwrap().interaction_id.unwrap();
        let answers:Vec<input::QuestionAnswer>=serde_json::from_value(json!([{"kind":"option","indices":[0],"labels":["Grade larga"]}])).unwrap();
        let result=driver.guarded_answer(&interactions.expected(&binding,Some(&id)).unwrap(),&answers).await;
        assert_eq!((result.disposition,result.code.as_str()),(input::Disposition::Accepted,"answered"));
        assert_eq!(*io.keys.lock().unwrap(),["\r"]);
    }
    /// Picker que o Esc fecha e composer que mostra o que foi digitado até o Enter.
    struct FallbackIo { picker: String, composer: Mutex<Option<String>>, buffer: Mutex<String>, keys: Mutex<Vec<String>> }
    impl TerminalIo for FallbackIo {
        fn command<'a>(&'a self, request: CommandRequest) -> IoFuture<'a, CommandOutput> { Box::pin(async move {
            let rule = "─".repeat(32);
            let stdout = match request.args[0].as_str() {
                "capture-pane" => match &*self.composer.lock().unwrap() { None => self.picker.clone(), Some(text) => format!("{rule}\n❯ {text}\n{rule}\n") }.into_bytes(),
                "display-message" if request.args.last().is_some_and(|arg| arg.starts_with("#{pane_pid}\t")) => format!("{}\tsession\t%1\t1\n", std::process::id()).into_bytes(),
                "display-message" => b"session\t%1\t1\n".to_vec(),
                // Texto de várias linhas vai colado: o buffer carregado entra no composer.
                "load-buffer" => { *self.buffer.lock().unwrap() = String::from_utf8(request.stdin.clone()).unwrap(); Vec::new() }
                "paste-buffer" => {
                    let text = self.buffer.lock().unwrap().clone();
                    if let Some(composer) = self.composer.lock().unwrap().as_mut() { *composer = text.clone(); }
                    self.keys.lock().unwrap().push(text);
                    Vec::new()
                }
                "send-keys" => {
                    let key = request.args.last().unwrap().clone();
                    let mut composer = self.composer.lock().unwrap();
                    if key == "Escape" { *composer = Some(String::new()); }
                    else if key == "\r" { if composer.is_some() { *composer = Some(String::new()); } }
                    else if request.args.contains(&"-l".into()) && let Some(text) = composer.as_mut() { *text = key.clone(); }
                    self.keys.lock().unwrap().push(key);
                    Vec::new()
                }
                other => panic!("comando inesperado: {other}"),
            };
            Ok(CommandOutput { success: true, stdout })
        }) }
        fn socket<'a>(&'a self, _: &'a NativeMessage, _: Vec<u8>) -> IoFuture<'a, WriteOutcome> { Box::pin(async { panic!("resposta não usa o socket") }) }
    }
    #[tokio::test]
    async fn keys_that_fail_on_the_same_picker_fall_back_to_a_text_answer() {
        let dir=tempfile::tempdir().unwrap();let target=target(dir.path());let binding=LocalTerminalFacts::binding(&target);
        std::fs::create_dir(dir.path().join("sessions")).unwrap();
        std::fs::write(dir.path().join("sessions").join(format!("{}.json",target.metadata.agent_pid)),json!({"pid":target.metadata.agent_pid,"sessionId":"sid","status":"idle"}).to_string()).unwrap();
        let sidecar=crate::state::ask::sidecar_path(&target.transcript).unwrap();std::fs::create_dir_all(sidecar.parent().unwrap()).unwrap();
        std::fs::write(&sidecar,json!({"tool_input":{"questions":[{"question":"Cor?","header":"Cor","multiSelect":false,"options":[{"label":"Azul"},{"label":"Verde"}]}]}}).to_string()).unwrap();
        // O cursor não anda (as setas não chegam): as teclas falham com o mesmo picker na tela.
        let picker="   Cor?\n\n ❯ 1. Azul\n   2. Verde\n   3. Type something.\n   4. Chat about this\n\nEnter to select · ↑/↓ to navigate · Esc to cancel\n".to_string();
        let interactions=Arc::new(crate::interactions::InteractionStore::default());let plugin=Arc::new(Store::new(interactions.clone()));
        interactions.bind(&binding).unwrap();plugin.bind(&binding).unwrap();
        let id=crate::state::ask::observe_terminal(&interactions,&binding,&target.transcript,&picker).unwrap().unwrap().interaction_id.unwrap();
        let expected=interactions.expected(&binding,Some(&id)).unwrap();
        let io=Arc::new(FallbackIo {picker,composer:Mutex::new(None),buffer:Mutex::new(String::new()),keys:Mutex::new(Vec::new())});
        let facts=Arc::new(FormFacts {local:LocalTerminalFacts::new(io.clone(),Some(plugin),None),target:target.clone()});
        let limits=InputLimits {settle:Duration::ZERO,literal_settle:Duration::ZERO,multiline_settle:Duration::ZERO,slash_settle:Duration::ZERO,proof_attempts:2,ready_attempts:2,cleanup_attempts:2};
        let driver=TerminalDriver::new(target.binding.clone(),facts,io.clone(),limits).with_interactions(interactions.clone()).with_interaction_source(target.transcript.clone());
        let answers:Vec<input::QuestionAnswer>=serde_json::from_value(json!([{"kind":"option","indices":[1],"labels":["Verde"]}])).unwrap();
        let keys=driver.guarded_answer(&expected,&answers).await;
        assert_eq!((keys.disposition,keys.code.as_str()),(input::Disposition::Rejected,"cursor_drift"));
        let fallback=driver.guarded_answer_fallback(&expected,&answers,"op").await.expect("o mesmo picker na tela: há reserva");
        assert_eq!((fallback.disposition,fallback.code.as_str()),(input::Disposition::Accepted,"answer_fallback_sent"));
        let sent=io.keys.lock().unwrap().clone();
        let escape=sent.iter().position(|k|k=="Escape").unwrap();
        assert!(sent[escape+1].contains("- Cor? → Verde"),"{sent:?}");assert_eq!(sent.last().unwrap(),"\r");
        // Com o composer na tela (o picker já fechou), nenhum Esc às cegas: fica a recusa.
        let before=io.keys.lock().unwrap().len();
        assert!(driver.guarded_answer_fallback(&expected,&answers,"op2").await.is_none());
        assert_eq!(io.keys.lock().unwrap().len(),before);
    }
    #[tokio::test]
    async fn sender_mode_and_target_life_are_checked_without_actor_calls() {
        let dir=tempfile::tempdir().unwrap();let target=target(dir.path());let queries=Arc::new(Mutex::new(Vec::new()));let (recorded,own)=(queries.clone(),target.clone());
        let modes:ModeReader=Arc::new(move |name| {recorded.lock().unwrap().push(name.clone());let own=own.clone();Box::pin(async move {Ok(Some(if name=="peer" {ModeSource {binding:InteractionBinding {key:"peer-key".into(),conversation:"peer-sid".into(),generation:3},transcript:own.metadata.config_dir.join("peer.jsonl"),confirmed:Some("manual".into())}}else {ModeSource {binding:LocalTerminalFacts::binding(&own),transcript:own.transcript,confirmed:Some("bypassPermissions".into())}}))})});
        let local=LocalTerminalFacts::new(frame(),None,Some(modes));assert_eq!(local.message_class(&target,"peer",true).await.unwrap(),"prompting");assert_eq!(local.message_class(&target,"peer",false).await.unwrap(),"bypass");assert_eq!(*queries.lock().unwrap(),vec!["peer","session"]);
        let mut source=ModeSource {binding:LocalTerminalFacts::binding(&target),transcript:target.transcript.clone(),confirmed:Some("bypassPermissions".into())};source.binding.generation+=1;let stale:ModeReader=Arc::new(move |_| {let source=source.clone();Box::pin(async move {Ok(Some(source))})});let local=LocalTerminalFacts::new(frame(),None,Some(stale));assert!(local.message_class(&target,"painel",false).await.is_err());
    }
    #[tokio::test]
    async fn unknown_sender_or_target_mode_never_falls_back_to_account() {
        let dir=tempfile::tempdir().unwrap();let target=target(dir.path());
        std::fs::write(dir.path().join("settings.json"),json!({"permissions":{"defaultMode":"bypassPermissions"}}).to_string()).unwrap();
        std::fs::create_dir_all(target.transcript.parent().unwrap()).unwrap();std::fs::write(&target.transcript,"{\"permissionMode\":\"bypassPermissions\"}\n").unwrap();
        let unknown:ModeReader=Arc::new(|_|Box::pin(async {Err(ServiceError("permission_mode_unknown"))}));
        let local=LocalTerminalFacts::new(frame(),None,Some(unknown));
        assert_eq!(local.message_class(&target,"peer",true).await.unwrap_err().0,"permission_mode_unknown");
        assert_eq!(local.message_class(&target,"aviso",false).await.unwrap_err().0,"permission_mode_unknown");
    }
    #[test]
    fn launch_mode_reads_the_permission_flag_the_cli_was_started_with() {
        let argv=|args:&[&str]| args.iter().map(|arg|(*arg).to_owned()).collect::<Vec<_>>();
        assert_eq!(launch_mode(&argv(&["claude","--session-id","s","--permission-mode","bypassPermissions"])).as_deref(),Some("bypassPermissions"));
        assert_eq!(launch_mode(&argv(&["claude","--permission-mode=acceptEdits"])).as_deref(),Some("acceptEdits"));
        assert_eq!(launch_mode(&argv(&["claude","--dangerously-skip-permissions"])).as_deref(),Some("bypassPermissions"));
        assert_eq!(launch_mode(&argv(&["claude","--permission-mode","default"])).as_deref(),Some("manual"));
        // Só põe o bypass no ciclo; o CLI nasce no padrão da conta.
        assert_eq!(launch_mode(&argv(&["claude","--allow-dangerously-skip-permissions"])),None);
        // Nascido em `plan`, a base fora do plano não está no argv.
        assert_eq!(launch_mode(&argv(&["claude","--permission-mode","plan"])),None);
        assert_eq!(launch_mode(&argv(&["claude","--permission-mode"])),None);
        assert_eq!(launch_mode(&argv(&["claude"])),None);
        // Depois do `--` é o prompt, não flag; flag repetida não decide.
        assert_eq!(launch_mode(&argv(&["claude","--","--permission-mode","bypassPermissions"])),None);
        assert_eq!(launch_mode(&argv(&["claude","--permission-mode","auto","--permission-mode","bypassPermissions"])),None);
    }
    #[test]
    fn transcript_account_and_hook_body_preserve_errors_and_shape() {
        let dir=tempfile::tempdir().unwrap();let path=dir.path().join("sid.jsonl");let cache=Mutex::new(TranscriptModes::default());std::fs::write(&path,"{\"permissionMode\":\"bypassPermissions\"}\n{\"permissionMode\":\"plan\"}\ninvalid\n").unwrap();assert_eq!(transcript_mode(&path,&cache).unwrap().as_deref(),Some("bypassPermissions"));
        std::fs::write(dir.path().join("settings.json"),"invalid").unwrap();assert!(account_mode(dir.path()).is_err());
        assert_eq!(input::plugin_answer_body(&json!([{"question":"A"}]),&json!([{"kind":"text","value":" primeira "}])).unwrap(),json!({"answers":{"A":"primeira"}}));
        assert!(input::plugin_answer_body(&json!([{"question":"A"}]),&json!([{"kind":"chat"}])).unwrap()["deny"].is_string());
        // Rótulo não prova a opção: índice fora da pergunta ou rótulo trocado não chega ao plugin.
        let ab=json!([{"question":"Q","options":[{"label":"A"},{"label":"B"}]}]);
        for invalid in [json!({"kind":"option","indices":[99],"labels":["C"]}),json!({"kind":"option","indices":[2],"labels":["B"]}),
            json!({"kind":"option","indices":[0],"labels":["C"]}),json!({"kind":"option","indices":[0],"labels":["B"]}),
            json!({"kind":"option","indices":[0,1],"labels":["B","A"]}),json!({"kind":"option","labels":["A"]})] {
            assert_eq!(input::plugin_answer_body(&ab,&json!([invalid])).unwrap_err().0,"terminal_answer_invalid","{invalid}");
        }
        assert_eq!(input::plugin_answer_body(&ab,&json!([{"kind":"option","indices":[1],"labels":["B"]}])).unwrap(),json!({"answers":{"Q":"B"}}));
        assert_eq!(input::plugin_answer_body(&ab,&json!([{"kind":"option","indices":[1],"labels":[" B "]}])).unwrap(),json!({"answers":{"Q":"B"}}),"espaço no rótulo não recusa");
        assert_eq!(input::plugin_answer_body(&ab,&json!([{"kind":"option","indices":[0,1],"labels":["A","B"]}])).unwrap(),json!({"answers":{"Q":"A, B"}}));
        assert!(plugin_current(&json!({"owner_instance":"rs-owner"}),1.0));assert!(!plugin_current(&json!({"owner_instance":"rs-owner"}),1.001));
    }
}
