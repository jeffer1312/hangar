//! Fontes locais da ponte do plugin; as nove rotas só são montadas no corte conjunto.
use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex, Weak};

use serde_json::{Map, Value, json};
use crate::interactions::InteractionBinding;
use crate::routes::AppState;
use crate::terminal_input::{CommandRequest, ProcessIo, TerminalBinding, TerminalIo};
use super::routes::{Discovery, Identity, MeasuredContext, SourceFuture, Sources};
use super::{PermissionGate, ServiceError};

#[derive(Default)]
pub struct RateStore { samples: Mutex<HashMap<String, (InteractionBinding, VecDeque<(u64, f64, f64)>)>> }

impl RateStore {
    pub fn record(&self, binding: &InteractionBinding, tokens: u64, seconds: f64, finished: f64) -> Result<(), ServiceError> {
        if binding.key.is_empty() || binding.conversation.is_empty() || !seconds.is_finite() || seconds < 0.0 || !finished.is_finite() {
            return Err(ServiceError("plugin_body_invalid"));
        }
        if tokens == 0 || seconds < 0.2 { return Ok(()); }
        let mut samples = self.samples.lock().map_err(|_| ServiceError("plugin_rate_unavailable"))?;
        let entry = samples.entry(binding.key.clone()).or_insert_with(|| (binding.clone(), VecDeque::new()));
        if entry.0 != *binding {
            if binding.generation <= entry.0.generation { return Err(ServiceError("plugin_conversation_mismatch")); }
            *entry = (binding.clone(), VecDeque::new());
        }
        if entry.1.len() == 10 { entry.1.pop_front(); }
        entry.1.push_back((tokens, seconds, finished));
        Ok(())
    }

    pub fn snapshot(&self, binding: &InteractionBinding, transcript_call_ts: Option<f64>) -> Map<String, Value> {
        let Some(ts) = transcript_call_ts.filter(|ts| ts.is_finite()) else { return Map::new() };
        let Ok(samples) = self.samples.lock() else {
            tracing::warn!(code = "plugin_rate_unavailable", "taxa: memória indisponível, não publica medição exata");
            return Map::new();
        };
        let Some((_, calls)) = samples.get(&binding.key).filter(|(life, _)| life == binding) else { return Map::new() };
        let Some(last) = calls.back().filter(|last| ts <= last.2 + 30.0) else { return Map::new() };
        let tokens: f64 = calls.iter().map(|(tokens, _, _)| *tokens as f64).sum();
        let seconds: f64 = calls.iter().map(|(_, seconds, _)| seconds).sum();
        let rounded = |value: f64| format!("{value:.1}").parse::<f64>().unwrap();
        json!({"tok_s_now":rounded(last.0 as f64 / last.1),"tok_s_recent":rounded(tokens / seconds),"tok_s_exact":true}).as_object().unwrap().clone()
    }

    pub fn overlay(&self, binding: &InteractionBinding, transcript_call_ts: Option<f64>, stats: &mut Value) {
        let Some(stats) = stats.as_object_mut() else { return };
        // A precisão antiga não é prova do plugin desta vida; os demais cálculos permanecem intactos.
        if stats.remove("tok_s_exact") == Some(json!(true)) { stats.remove("tok_s_now"); stats.remove("tok_s_recent"); }
        stats.extend(self.snapshot(binding, transcript_call_ts));
    }

    pub fn forget_binding(&self, binding: &InteractionBinding) {
        if let Ok(mut samples) = self.samples.lock() {
            if samples.get(&binding.key).is_some_and(|(life, _)| life == binding) { samples.remove(&binding.key); }
        } else { tracing::warn!(code = "plugin_rate_unavailable", "taxa: limpeza da vida recusada pela memória"); }
    }
    pub fn forget(&self, key: &str) {
        match self.samples.lock() {
            Ok(mut samples) => { samples.remove(key); },
            Err(_) => tracing::warn!(code = "plugin_rate_unavailable", "taxa: limpeza da chave recusada pela memória"),
        }
    }
}

/// Como o Python: o app segura a permissão em qualquer modo, menos nos que não perguntam (`auto`,
/// `dontAsk`); modo ainda desconhecido segura também. O app faz tudo; o terminal é o fundo.
pub fn permission_gate(app_present: bool, terminal_attached: Option<bool>, mode: Option<&str>) -> PermissionGate {
    let mode_without_dialog = matches!(mode, Some("auto" | "dontAsk"));
    PermissionGate { app_present, terminal_attached: terminal_attached.unwrap_or(true), mode_without_dialog }
}

/// Relógio da última resposta principal; usa o mesmo parser do transcript, não mtime.
pub fn last_call_ts(jsonl: &std::path::Path, conversation: &str) -> Option<f64> {
    let size = std::fs::metadata(jsonl).ok()?.len();
    crate::tail::backfill(jsonl, conversation, crate::transcript::Provider::Claude, None, size).iter().rev().find_map(|frame| {
        let text = std::str::from_utf8(frame).ok()?;
        let data = text.lines().find_map(|line| line.strip_prefix("data: "))?;
        let event: Value = serde_json::from_str(data).ok()?;
        matches!(event["kind"].as_str(), Some("assistant_msg" | "tool_use" | "thinking"))
            .then(|| event["ts"].as_f64()).flatten().filter(|ts| ts.is_finite())
    })
}

pub struct NativeSources { state: Weak<AppState>, io: Arc<dyn TerminalIo> }

/// Sessão aberta fora do app não traz a chave do lançamento: o `whoami` entrega a chave do terminal, como na sem terminal.
fn terminal_plugin_key(launched: Option<String>, terminal_key: &str) -> String {
    launched.filter(|key| !key.is_empty()).unwrap_or_else(|| terminal_key.to_owned())
}

impl NativeSources {
    pub fn new(state: &Arc<AppState>) -> Self { Self { state: Arc::downgrade(state), io: Arc::new(ProcessIo::default()) } }
    fn state(&self) -> Result<Arc<AppState>, ServiceError> { self.state.upgrade().ok_or(ServiceError("plugin_source_unbound")) }

    async fn identity(&self, name: &str) -> Result<Identity, ServiceError> {
        let state = self.state()?;
        let registry = state.state.runtime.get().ok_or(ServiceError("plugin_runtime_absent"))?;
        let binding = registry.interaction_binding_readonly(name).await.map_err(|_| ServiceError("plugin_identity_missing"))?;
        let target = registry.terminal_target(name).await.map_err(|_| ServiceError("plugin_identity_ambiguous"))?;
        let plugin_key = match target {
            Some(target) => {
                if target.key != binding.key || target.generation != binding.generation || target.binding.conversation != binding.conversation {
                    return Err(ServiceError("plugin_conversation_mismatch"));
                }
                terminal_plugin_key(target.plugin_key, &binding.key)
            }
            None => binding.key.clone(),
        };
        // A vida já foi ativada pelo dono. O leitor não toma uma vida por bind.
        state.plugin.facts(&binding, std::time::Instant::now())?;
        Ok(Identity { name: name.into(), binding, plugin_key })
    }

    async fn command(&self, binding: &TerminalBinding, args: Vec<String>) -> Result<String, ServiceError> {
        let Some((program, prefix)) = binding.mux_argv.split_first() else { return Err(ServiceError("plugin_mux_unavailable")) };
        let args = prefix.iter().cloned().chain(args).collect();
        let output = self.io.command(CommandRequest { program: program.clone(), args, stdin: Vec::new() }).await
            .map_err(|_| ServiceError("plugin_mux_unavailable"))?;
        if !output.success { return Err(ServiceError("plugin_mux_unavailable")); }
        let text = String::from_utf8(output.stdout).map_err(|_| ServiceError("plugin_mux_unreadable"))?;
        if binding.windows && text.contains('\u{fffd}') { return Err(ServiceError("plugin_mux_unreadable")); }
        Ok(text)
    }

    async fn attached(&self, binding: &TerminalBinding) -> Option<bool> {
        if binding.windows { return Some(true); }
        let raw = self.command(binding, vec!["list-clients".into(), "-t".into(), format!("={}", binding.name), "-F".into(),
            "#{client_control_mode}\t#{client_tty}".into()]).await.ok()?;
        Some(raw.lines().any(|line| { let fields: Vec<_> = line.split('\t').collect(); fields.len() != 2 || fields[0] != "1" }))
    }
}

impl Sources for NativeSources {
    fn enabled(&self) -> SourceFuture<'_, bool> {
        Box::pin(async move {
            let path = self.state()?.accounts.env.claude_base.join("runtime-config.json");
            tokio::task::spawn_blocking(move || match std::fs::read(path) {
                Ok(bytes) => {
                    let value: Value = serde_json::from_slice(&bytes).map_err(|_| ServiceError("plugin_config_invalid"))?;
                    match value.get("claude_function_hooks") {
                        Some(Value::Bool(enabled)) => Ok(*enabled), None => Ok(true), _ => Err(ServiceError("plugin_config_invalid")),
                    }
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(true),
                Err(_) => Err(ServiceError("plugin_config_unavailable")),
            }).await.map_err(|_| ServiceError("plugin_config_unavailable"))?
        })
    }
    fn resolve<'a>(&'a self, name: &'a str, key: Option<&'a str>) -> SourceFuture<'a, Identity> {
        Box::pin(async move {
            if let Ok(identity) = self.identity(name).await {
                if key.is_none_or(|key| key == identity.plugin_key || key == identity.binding.key) { return Ok(identity); }
            }
            let Some(key) = key else { return Err(ServiceError("plugin_identity_missing")) };
            let state = self.state()?;
            let rows = state.list.discover(None).await.map_err(|_| ServiceError("plugin_discovery_unavailable"))?;
            let mut found = None;
            for row in rows.iter().filter(|row| row.provider == "claude") {
                if let Ok(identity) = self.identity(&row.name).await {
                    if key == identity.plugin_key || key == identity.binding.key {
                        if found.replace(identity).is_some() { return Err(ServiceError("plugin_identity_ambiguous")); }
                    }
                }
            }
            found.ok_or(ServiceError("plugin_identity_missing"))
        })
    }
    fn discovery(&self) -> SourceFuture<'_, Vec<Discovery>> {
        Box::pin(async move {
            let state = self.state()?;
            let registry = state.state.runtime.get().ok_or(ServiceError("plugin_runtime_absent"))?;
            let rows = state.list.discover(None).await.map_err(|_| ServiceError("plugin_discovery_unavailable"))?;
            let mut facts = Vec::new();
            for row in rows.iter().filter(|row| row.provider == "claude" && !row.headless) {
                // Sem entrada de terminal no runtime (aberta à mão, em troca de vida) a linha não é candidata;
                // falhar aqui deixaria todo plugin sem identidade por causa de uma sessão alheia.
                let Some(target) = registry.terminal_target(&row.name).await.map_err(|_| ServiceError("plugin_identity_ambiguous"))? else { continue };
                let identity = match self.identity(&row.name).await {
                    Ok(identity) => identity,
                    Err(ServiceError(code)) => {
                        if crate::warn_limit::allow(Some(&row.name), code) {
                            tracing::warn!(session = %row.name, code, "whoami: sessão sem identidade do plugin fica de fora da descoberta");
                        }
                        continue;
                    }
                };
                let raw = self.command(&target.binding, vec!["display-message".into(), "-p".into(), "-t".into(), target.binding.pane.clone(),
                    "#{session_name}\t#{session_created}\t#{pane_id}\t#{socket_path}\t#{session_id}\t#{pid}".into()]).await?;
                let fields: Vec<_> = raw.trim_end_matches(['\r', '\n']).split('\t').collect();
                if fields.len() != 6 || fields[0] != target.name || fields[1].parse::<u64>().ok() != Some(target.binding.created)
                    || fields[2].is_empty() || fields[3].is_empty() || fields[4].is_empty() || fields[5].parse::<u64>().is_err()
                    || !target.binding.windows && fields[2] != target.binding.pane {
                    return Err(ServiceError("plugin_discovery_unavailable"));
                }
                if self.identity(&row.name).await?.binding != identity.binding { return Err(ServiceError("plugin_conversation_mismatch")); }
                facts.push(Discovery { identity, pane: Some(fields[2].into()), socket: Some(fields[3].into()),
                    mux_session: Some(fields[4].into()), psmux_pid: target.binding.windows.then(|| fields[5].into()) });
            }
            Ok(facts)
        })
    }
    fn permission_gate<'a>(&'a self, identity: &'a Identity) -> SourceFuture<'a, PermissionGate> {
        Box::pin(async move {
            let state = self.state()?;
            let registry = state.state.runtime.get().ok_or(ServiceError("plugin_runtime_absent"))?;
            if registry.interaction_binding_readonly(&identity.name).await.map_err(|_| ServiceError("plugin_identity_missing"))? != identity.binding {
                return Err(ServiceError("plugin_conversation_mismatch"));
            }
            let target = registry.terminal_target(&identity.name).await.map_err(|_| ServiceError("plugin_identity_ambiguous"))?;
            let attached = if state.term.is_active(&identity.name) { Some(true) } else {
                // Sem terminal o pedido é do canal nativo (can_use_tool): o plugin nunca segura.
                match target { Some(target) => self.attached(&target.binding).await, None => Some(true) }
            };
            let memory = state.state.permissions.lock().map_err(|_| ServiceError("permission_store_unavailable"))?;
            // Modo incerto (troca em curso) também segura: só `auto`/`dontAsk` confirmados soltam.
            let mode = match memory.confirmed(&identity.binding.conversation) {
                Ok(mode) => mode,
                Err(code) => {
                    if crate::warn_limit::allow(Some(&identity.binding.key), code) {
                        tracing::warn!(key = %identity.binding.key, session = %identity.name, code, "portão da permissão: modo incerto; o app segura");
                    }
                    None
                }
            };
            Ok(permission_gate(state.list.owner_clients() > 0 || state.list.python_apps() > 0 || state.side.hubs.app_present(), attached,
                mode.as_ref().map(|mode| mode.current.as_str())))
        })
    }
    fn band_present<'a>(&'a self, identity: &'a Identity) -> SourceFuture<'a, bool> {
        Box::pin(async move {
            let state = self.state()?;
            if self.identity(&identity.name).await?.binding != identity.binding { return Err(ServiceError("plugin_conversation_mismatch")); }
            Ok(state.mods.band_present(&identity.name))
        })
    }
    fn context<'a>(&'a self, identity: &'a Identity, context: MeasuredContext) -> SourceFuture<'a, ()> {
        Box::pin(async move {
            let state = self.state()?;
            let registry = state.state.runtime.get().ok_or(ServiceError("plugin_runtime_absent"))?;
            if self.identity(&identity.name).await?.binding != identity.binding { return Err(ServiceError("plugin_conversation_mismatch")); }
            publish_context(registry, &identity.name, &identity.binding, context, || state.list.invalidate_context(&identity.name)).await
        })
    }
    fn rate<'a>(&'a self, identity: &'a Identity, tokens: u64, seconds: f64) -> SourceFuture<'a, ()> {
        Box::pin(async move {
            let state = self.state()?;
            if self.identity(&identity.name).await?.binding != identity.binding { return Err(ServiceError("plugin_conversation_mismatch")); }
            state.side.hubs.plugin_rates().record(&identity.binding, tokens, seconds, crate::state::monitor::wall_now())
        })
    }
    /// Todo fim de turno, e cada desfecho vai ao diário: a troca de conta calada já passou despercebida.
    fn turn_error<'a>(&'a self, identity: &'a Identity) -> SourceFuture<'a, ()> {
        Box::pin(async move {
            let state = self.state()?;
            let registry = state.state.runtime.get().ok_or(ServiceError("plugin_runtime_absent"))?;
            let name = &identity.name;
            let report = |code: &str, reason: &'static str| state.diag.report("session.limit_check", name, code, reason);
            match registry.turn_limit(name, &identity.binding, crate::state::monitor::wall_now()).await {
                Ok(crate::runtime::gateway::TurnLimit::NotTerminal) => report("limit_check_not_terminal", "fim de turno sem terminal: o limite é conferido pelo ator"),
                Ok(crate::runtime::gateway::TurnLimit::Checked { limit: None, .. }) => report("limit_check_none", "fim de turno conferido: sem aviso de limite no transcript"),
                Ok(crate::runtime::gateway::TurnLimit::Checked { limit: Some(_), sent: true, .. }) => report("limit_check_sent", "fim de turno: aviso de limite entregue ao vigia da troca de conta"),
                Ok(crate::runtime::gateway::TurnLimit::Checked { limit: Some(_), sent: false, .. }) => report("limit_check_unsent", "fim de turno: aviso de limite achado e não entregue ao vigia"),
                Err(error) => {
                    report(&error.code, "fim de turno: o limite de uso não foi conferido");
                    return Err(ServiceError("limit_check_failed"));
                }
            }
            Ok(())
        })
    }
}

/// Grava a medida no transcript da conversa vigente, com ou sem terminal. A vida é conferida antes e
/// depois da gravação; a lista só é invalidada quando as duas conferências batem com quem mediu.
pub(crate) async fn publish_context(registry: &crate::runtime::gateway::RuntimeRegistry, name: &str, binding: &InteractionBinding,
    context: MeasuredContext, invalidate: impl FnOnce()) -> Result<(), ServiceError> {
    let current = || async move { registry.context_transcript(name).await.map_err(|_| ServiceError("plugin_context_source_missing")) };
    let (live, transcript) = current().await?;
    if live != *binding { return Err(ServiceError("plugin_conversation_mismatch")); }
    let path = transcript.with_extension("context.json");
    let content = json!({"used":context.used,"window":context.window}).to_string();
    tokio::task::spawn_blocking(move || crate::runtime::queue::atomic_write(&path, content.as_bytes()))
        .await.map_err(|_| ServiceError("plugin_context_write_failed"))?.map_err(|_| ServiceError("plugin_context_write_failed"))?;
    if current().await? != (live, transcript) { return Err(ServiceError("plugin_conversation_mismatch")); }
    invalidate();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn binding(generation: u64) -> InteractionBinding {
        InteractionBinding { key: "k".into(), conversation: "sid".into(), generation }
    }

    /// Sessão com terminal aberta fora do app (restauração do boot, wrapper do shell): sem a chave do lançamento,
    /// o plugin se acha pelo `whoami` com a chave do terminal.
    #[test]
    fn terminal_without_launch_key_uses_the_terminal_key() {
        assert_eq!(terminal_plugin_key(Some("lancamento".into()), "terminal"), "lancamento");
        assert_eq!(terminal_plugin_key(None, "terminal"), "terminal");
        assert_eq!(terminal_plugin_key(Some(String::new()), "terminal"), "terminal");
    }

    #[test]
    fn gate_requires_presence_and_detachment() {
        for (app, attached) in [(false, Some(false)), (true, Some(true)), (true, None)] {
            let gate = permission_gate(app, attached, Some("manual"));
            assert!(!gate.app_present || gate.terminal_attached);
        }
        let gate = permission_gate(true, Some(false), Some("manual"));
        assert!(gate.app_present && !gate.terminal_attached && !gate.mode_without_dialog);
    }

    /// Decisão do usuário (B4/P23), como o Python: o app segura em qualquer modo, menos `auto`/`dontAsk`;
    /// modo ainda desconhecido e `plan` também seguram.
    #[test]
    fn app_holds_in_every_mode_but_auto_and_dont_ask() {
        for mode in [None, Some("manual"), Some("acceptEdits"), Some("plan"), Some("bypassPermissions")] {
            assert!(!permission_gate(true, Some(false), mode).mode_without_dialog, "{mode:?}");
        }
        for mode in ["auto", "dontAsk"] {
            assert!(permission_gate(true, Some(false), Some(mode)).mode_without_dialog, "{mode}");
        }
    }

    #[test]
    fn rates_are_kept_without_chat_but_not_between_lives_or_newer_responses() {
        let rates = RateStore::default();
        let first = binding(1);
        rates.record(&first, 100, 2.0, 10.0).unwrap();
        assert_eq!(rates.snapshot(&first, Some(10.0))["tok_s_now"], 50.0);
        assert!(rates.snapshot(&first, Some(41.0)).is_empty());
        assert!(rates.snapshot(&binding(2), Some(10.0)).is_empty());
        assert!(rates.snapshot(&first, None).is_empty(), "sem prova do transcript não aplica taxa antiga");
        rates.record(&first, 100, 0.1, 12.0).unwrap();
        assert_eq!(rates.snapshot(&first, Some(10.0))["tok_s_now"], 50.0);
        rates.forget("k");
        assert!(rates.snapshot(&first, Some(10.0)).is_empty());
    }

    #[test]
    fn old_cleanup_and_old_report_cannot_replace_new_life_rates() {
        let rates = RateStore::default();
        let first = binding(1);
        let second = binding(2);
        rates.record(&first, 100, 2.0, 10.0).unwrap();
        rates.record(&second, 200, 2.0, 12.0).unwrap();
        rates.forget_binding(&first);
        assert!(rates.record(&first, 300, 2.0, 14.0).is_err());
        assert_eq!(rates.snapshot(&second, Some(12.0))["tok_s_now"], 100.0);
    }

    #[test]
    fn unavailable_exact_rate_preserves_estimates_and_other_stats() {
        let rates = RateStore::default();
        let mut stats = json!({"tokens":321,"tok_s_now":9.0,"tok_s_exact":false});
        rates.overlay(&binding(1), None, &mut stats);
        assert_eq!(stats["tok_s_now"], 9.0);
        assert_eq!(stats["tokens"], 321);
    }

    #[test]
    fn windows_duplicate_pane_and_pid_is_ambiguous_not_the_first_session() {
        let one = Identity { name:"one".into(), binding:binding(1), plugin_key:"p-one".into() };
        let two = Identity { name:"two".into(), binding:InteractionBinding { key:"other".into(), ..binding(1) }, plugin_key:"p-two".into() };
        let rows = vec![Discovery {identity:one,pane:Some("%1".into()),socket:None,mux_session:None,psmux_pid:Some("123".into())},
            Discovery {identity:two,pane:Some("%1".into()),socket:None,mux_session:None,psmux_pid:Some("123".into())}];
        let body = super::super::routes::Whoami { chave:"".into(),pane:Some("%1".into()),nome:Some("one".into()),
            tmux:Some("/tmp/psmux-123/socket,0,0".into()),session_id:Some("sid".into()) };
        assert_eq!(super::super::routes::discover(&body,&rows).unwrap_err().0,"plugin_identity_ambiguous");
    }

    #[test]
    fn stats_overlay_keeps_the_existing_payload_and_removes_stale_exact_rate() {
        let rates = RateStore::default();
        let life = binding(1);
        rates.record(&life, 100, 2.0, 10.0).unwrap();
        let mut stats = json!({"tokens":321,"turns":4,"tok_s_now":9.0,"tok_s_exact":true});
        rates.overlay(&life, Some(10.0), &mut stats);
        assert_eq!(stats["tokens"], 321);
        assert_eq!(stats["turns"], 4);
        assert_eq!(stats["tok_s_now"], 50.0);
        rates.overlay(&life, Some(50.0), &mut stats);
        assert!(stats.get("tok_s_exact").is_none());
    }
}
