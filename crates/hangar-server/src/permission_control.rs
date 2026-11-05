//! Controle por teclado do Claude (BTab) e do Codex (seletor do `/permissions`); memória e reabertura
//! pertencem ao executor.
use crate::{runtime::protocol::RuntimeError, state::permission::parse_permission_mode,
    terminal_input::{IoFailure, PermissionSequence, TerminalDriver}};
use regex::Regex;
use serde::Serialize;
use serde_json::{Value, json};
use std::{sync::LazyLock, time::Duration};
use tokio::time::{Instant, timeout_at};

pub enum PermissionAction { Read { probe: bool }, Set { mode: String } }

const KEY_LIMIT: usize = 6;
const KEY_WAIT: Duration = Duration::from_secs(2);
const POLL: Duration = Duration::from_millis(20);
const POLL_MAX: Duration = Duration::from_millis(200);
const PROBE_LIMIT: Duration = Duration::from_secs(8);
const MODES: [&str; 6] = ["plan", "auto", "manual", "acceptEdits", "bypassPermissions", "dontAsk"];

struct Run {
    initial: &'static str,
    current: Option<&'static str>,
    last_confirmed: &'static str,
    modes: Vec<&'static str>,
    keys: usize,
    uncertain: bool,
    error: Option<&'static str>,
    cycle_closed: bool,
}
impl Run {
    fn failed(&mut self, failure: IoFailure) {
        self.error = Some(failure.code);
        if failure.may_have_written { self.current = None; self.uncertain = true; }
    }
    async fn step(&mut self, sequence: &PermissionSequence<'_>, deadline: Instant) -> bool {
        if Instant::now() >= deadline { self.error = Some("permission_probe_timeout"); return false; }
        match sequence.next(self.current.unwrap(), deadline).await {
            Ok(()) => self.keys += 1,
            Err(failure) => {
                if failure.may_have_written { self.keys += 1; }
                if failure.code == "permission_mode_changed" { self.current = None; }
                self.failed(failure); return false;
            }
        }
        let previous = self.current;
        // A tecla pode ter chegado; o modo de antes não prova o resultado da escrita.
        self.current = None;
        let until = deadline.min(Instant::now() + KEY_WAIT);
        let mut interval = POLL;
        loop {
            match timeout_at(until, sequence.capture()).await {
                Ok(Ok(screen)) => {
                    if let Some(mode) = parse_permission_mode(&screen) {
                        self.last_confirmed = mode;
                        if Some(mode) != previous { self.current = Some(mode); return true; }
                    }
                }
                Ok(Err(mut failure)) => {
                    failure.may_have_written = true; self.failed(failure); return false;
                }
                Err(_) => break,
            }
            if Instant::now() >= until { break; }
            tokio::time::sleep_until(until.min(Instant::now() + interval)).await;
            interval = interval.mul_f32(1.5).min(POLL_MAX);
        }
        self.failed(IoFailure { code: "permission_key_timeout", may_have_written: true });
        false
    }
    fn value(&self, probe: bool, target: Option<&str>, requires_reopen: bool) -> Value {
        let restored = self.current == Some(self.initial) && !self.uncertain;
        // Só o resultado final entra na memória; intermediários da sonda não viram base de plan.
        let previous_non_plan = self.current.filter(|m| *m != "plan")
            .or_else(|| (self.initial != "plan").then_some(self.initial));
        let mut value = json!({"ok": self.error.is_none() && !self.uncertain,
            "initial": self.initial, "current": self.current, "last_confirmed": self.last_confirmed,
            "previous_non_plan": previous_non_plan, "modes": self.modes,
            "sondavel": self.current.is_some_and(|m| m != "dontAsk"),
            "uncertain": self.uncertain, "changed": self.keys > 0, "error_code": self.error,
            "cycle_closed": self.cycle_closed, "requires_reopen": requires_reopen});
        if probe { value["restaurado"] = json!(restored); }
        if let Some(target) = target { value["mode"] = json!(self.current); value["requested"] = json!(target); }
        value
    }
}

/// Não relança processos nem guarda modos: o consumidor confirma o resultado na memória da conversa.
pub async fn claude(driver: &TerminalDriver, action: PermissionAction) -> Result<Value, RuntimeError> {
    let (probe, target) = match &action {
        PermissionAction::Read { probe } => (*probe, None),
        PermissionAction::Set { mode } => {
            if !MODES.contains(&mode.as_str()) {
                return Err(RuntimeError::new("erro_permissao_invalida", "modo de permissão inválido"));
            }
            (false, Some(mode.as_str()))
        }
    };
    let sequence = driver.permission_sequence().await;
    let screen = sequence.capture().await.map_err(|e| RuntimeError::new(e.code, "não consegui ler o terminal da sessão"))?;
    let initial = parse_permission_mode(&screen)
        .ok_or_else(|| RuntimeError::new("erro_permissao_leitura", "não consegui ler o modo atual no rodapé"))?;
    let mut run = Run { initial, current: Some(initial), last_confirmed: initial, modes: vec![],
        keys: 0, uncertain: false, error: None, cycle_closed: false };
    if target == Some(initial) || target.is_none() && (!probe || initial == "dontAsk") {
        return Ok(run.value(probe, target, false));
    }
    run.modes.push(initial);
    let deadline = Instant::now() + if probe { PROBE_LIMIT } else { KEY_WAIT * KEY_LIMIT as u32 };
    for _ in 0..KEY_LIMIT {
        if !run.step(&sequence, deadline).await { break; }
        let current = run.current.unwrap();
        if target == Some(current) { break; }
        if current == initial { run.cycle_closed = true; break; }
        let seen = run.modes.contains(&current);
        if !seen { run.modes.push(current); }
        if probe && seen { break; }
    }
    // Sem prova do último efeito, outra tecla não é restauração segura.
    if probe && run.current.is_some() && run.current != Some(initial) && !run.uncertain {
        let restore_deadline = Instant::now() + PROBE_LIMIT;
        for _ in 0..KEY_LIMIT {
            if !run.step(&sequence, restore_deadline).await || run.current == Some(initial) { break; }
        }
    }
    let requires_reopen = target == Some("bypassPermissions") && run.cycle_closed
        && !run.modes.contains(&"bypassPermissions") && !run.uncertain && run.error.is_none();
    if requires_reopen { run.error = Some("permission_reopen_required"); }
    else if target.is_some() && run.current != target && run.error.is_none()
        || probe && run.current != Some(initial) && run.error.is_none() {
        run.error = Some("erro_permissao_teto");
    }
    Ok(run.value(probe, target, requires_reopen))
}

// ── Codex: seletor do `/permissions` ────────────────────────────────────────────────────────────
// Por que o seletor e não o app-server: `backend/app/codex_permissions.py`.

const CODEX_TITLE: &str = "Update Model Permissions";
const CODEX_FOOTER: &str = "Press enter to confirm or esc to go back";
const CODEX_CONFIRM_TITLE: &str = "Enable full access?";
const CODEX_SETTLE: Duration = Duration::from_millis(300);
const CODEX_OPEN_SETTLE: Duration = Duration::from_millis(700);
const CODEX_NAV_GAP: Duration = Duration::from_millis(120);
const CODEX_OPEN_LIMIT: Duration = Duration::from_secs(2);
// A continuação da descrição não tem "N.": por isso a quebra de linha nunca vira modo.
static CODEX_LINE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^\s*([›❯]?)\s*(\d+)\.\s+(.+?)\s{2,}(.*)$").unwrap());

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct CodexPermissionOption {
    #[serde(rename = "numero")]
    pub number: u32,
    #[serde(rename = "nome")]
    pub name: String,
    #[serde(rename = "desc")]
    pub description: String,
    #[serde(rename = "atual")]
    pub current: bool,
    pub cursor: bool,
}

/// Do título até o rodapé. O seletor é overlay e não vai ao histórico: fechado, nada casa.
fn codex_region<'a>(pane: &'a str, title: &str) -> Vec<&'a str> {
    let lines: Vec<&str> = pane.lines().collect();
    let Some(start) = lines.iter().rposition(|line| line.contains(title)) else { return vec![] };
    let end = lines[start..].iter().position(|line| line.contains(CODEX_FOOTER)).map_or(lines.len(), |i| start + i + 1);
    lines[start..end].to_vec()
}
fn codex_picker_open(pane: &str) -> bool { !codex_region(pane, CODEX_TITLE).is_empty() }
/// Só com o rodapé: no instante do título a lista ainda está sendo pintada.
fn codex_picker_drawn(pane: &str) -> bool { codex_region(pane, CODEX_TITLE).iter().any(|line| line.contains(CODEX_FOOTER)) }
fn codex_full_access_confirmation(pane: &str) -> bool { !codex_region(pane, CODEX_CONFIRM_TITLE).is_empty() }

pub fn parse_codex_modes(pane: &str) -> Vec<CodexPermissionOption> {
    codex_region(pane, CODEX_TITLE).into_iter().filter_map(|line| {
        let caps = CODEX_LINE.captures(line)?;
        let label = caps[3].trim();
        Some(CodexPermissionOption { number: caps[2].parse().ok()?, name: label.replace("(current)", "").trim().to_owned(),
            description: caps[4].trim().to_owned(), current: label.contains("(current)"), cursor: !caps[1].is_empty() })
    }).collect()
}

pub fn current_codex_mode(pane: &str) -> Option<String> {
    parse_codex_modes(pane).into_iter().find(|mode| mode.current).map(|mode| mode.name)
}

/// Seta e quantidade por POSIÇÃO na lista: a descrição ocupa de uma a três linhas de tela.
fn codex_steps(modes: &[CodexPermissionOption], target: &str) -> Option<(&'static str, usize)> {
    let from = modes.iter().position(|mode| mode.cursor)?;
    let to = modes.iter().position(|mode| mode.name.to_lowercase() == target.trim().to_lowercase())?;
    Some((if to > from { "Down" } else { "Up" }, from.abs_diff(to)))
}

fn codex_io(failure: IoFailure) -> RuntimeError {
    let message = match failure.code {
        "terminal_panel_open" => "Terminal aberto nesta sessão. Feche o painel pra trocar a permissão por aqui.",
        "stale_mux" | "identity_failed" | "stale_binding" | "facts_unavailable" => "o terminal da sessão não está aberto",
        _ => "não consegui dirigir o terminal da sessão",
    };
    RuntimeError::new(failure.code, message)
}
fn codex_refused(code: &str, message: &str) -> RuntimeError { RuntimeError::new(code, message) }

/// Menu alheio na tela (do Codex, a confirmação do Full Access, um `/permissions` aberto pelo usuário,
/// um overlay): a tecla seria resposta dele.
fn codex_foreign_menu(screen: &str) -> Result<(), RuntimeError> {
    let analysis = crate::terminal_state::analyze(screen);
    if analysis.overlay || analysis.codex_menu.is_some() || codex_full_access_confirmation(screen) || codex_picker_open(screen) {
        return Err(codex_refused("permission_menu_open", "há um menu aberto no terminal da sessão"));
    }
    Ok(())
}

/// Rascunho no composer do Codex (a última linha `›`): o Enter mandaria "<rascunho>/permissions" ao modelo.
/// O parser do composer do Claude não serve aqui: a TUI do Codex não tem as réguas dele.
fn codex_composer_empty(typed: &str) -> Result<(), RuntimeError> {
    let lines: Vec<&str> = typed.lines().collect();
    let Some(at) = lines.iter().rposition(|line| line.trim_start().starts_with('›')) else {
        return Err(codex_refused("permission_composer_unreadable", "não consegui ler a caixa de digitar do Codex"));
    };
    // Rascunho de várias linhas continua recuado logo abaixo do `›`, mesmo com a primeira vazia.
    let continued = lines[at + 1..].iter().take_while(|line| line.starts_with(char::is_whitespace) && !line.trim().is_empty()).next().is_some();
    if continued || !lines[at].trim_start().trim_start_matches('›').trim().is_empty() {
        return Err(codex_refused("permission_composer_busy", "há texto na caixa de digitar do Codex; envie ou apague antes de trocar a permissão"));
    }
    Ok(())
}

async fn codex_wait_drawn(sequence: &PermissionSequence<'_>) -> Result<Option<String>, RuntimeError> {
    let deadline = Instant::now() + CODEX_OPEN_LIMIT;
    loop {
        tokio::time::sleep(CODEX_SETTLE).await;
        let screen = sequence.screen().await.map_err(codex_io)?;
        if codex_picker_drawn(&screen) { return Ok(Some(screen)); }
        if Instant::now() >= deadline { return Ok(None); }
    }
}

/// Só envia o comando da TUI; quem chama confirma a troca de conversa pelo app-server.
pub async fn codex_clear(driver: &TerminalDriver) -> Result<(), RuntimeError> {
    let sequence = driver.permission_sequence().await;
    codex_foreign_menu(&sequence.screen().await.map_err(codex_io)?)?;
    if sequence.copy_mode().await.map_err(codex_io)? {
        return Err(codex_refused("codex_clear_copy_mode", "O terminal está em modo de cópia."));
    }
    codex_composer_empty(&sequence.typed_screen().await.map_err(codex_io)?)
        .map_err(|error|codex_refused(&error.code,"Deixe a caixa de digitar do Codex vazia antes de limpar a conversa."))?;
    sequence.type_text("/clear").await.map_err(codex_io)?;
    tokio::time::sleep(CODEX_SETTLE).await;
    if !codex_clear_typed(&sequence.typed_screen().await.map_err(codex_io)?) {
        return Err(codex_refused("codex_clear_unproved", "Não consegui confirmar o /clear na caixa de digitar."));
    }
    sequence.press("Enter").await.map_err(codex_io)?;
    tokio::time::sleep(CODEX_OPEN_SETTLE).await;
    // O autocomplete pode consumir o Enter; só o nosso comando ainda intacto autoriza outro.
    if codex_clear_typed(&sequence.typed_screen().await.map_err(codex_io)?) {
        codex_foreign_menu(&sequence.screen().await.map_err(codex_io)?)?;
        sequence.press("Enter").await.map_err(codex_io)?;
    }
    Ok(())
}

fn codex_clear_typed(screen: &str) -> bool {
    let lines: Vec<&str> = screen.lines().collect();
    let Some(at) = lines.iter().rposition(|line|line.trim_start().starts_with('›')) else { return false };
    lines[at].trim_start().trim_start_matches('›').trim() == "/clear"
        && !lines[at + 1..].iter().take_while(|line|!line.trim().is_empty()).any(|line|line.starts_with(char::is_whitespace))
}

/// O pane com o seletor inteiro na tela. Sai sem nada aberto, ou com o seletor aberto por nós.
async fn codex_open(sequence: &PermissionSequence<'_>) -> Result<String, RuntimeError> {
    let screen = sequence.screen().await.map_err(codex_io)?;
    codex_foreign_menu(&screen)?;
    if sequence.copy_mode().await.map_err(codex_io)? {
        return Err(codex_refused("permission_copy_mode", "o terminal da sessão está em modo de cópia"));
    }
    codex_composer_empty(&sequence.typed_screen().await.map_err(codex_io)?)?;
    sequence.type_text("/permissions").await.map_err(codex_io)?;
    tokio::time::sleep(CODEX_SETTLE).await;
    sequence.press("Enter").await.map_err(codex_io)?;
    if let Some(screen) = codex_wait_drawn(sequence).await? { return Ok(screen); }
    // O autocomplete pode comer o 1º Enter. O 2º só com prova de que nada está aberto: num seletor ou
    // diálogo ele confirmaria a linha sob o cursor.
    let screen = sequence.screen().await.map_err(codex_io)?;
    if codex_picker_open(&screen) {
        let _ = sequence.escape().await;
        return Err(codex_refused("permission_picker_partial", "o seletor de permissões do Codex abriu pela metade"));
    }
    // Diálogo alheio: sem Esc, ele é de quem está no terminal.
    codex_foreign_menu(&screen)?;
    sequence.press("Enter").await.map_err(codex_io)?;
    if let Some(screen) = codex_wait_drawn(sequence).await? { return Ok(screen); }
    let _ = sequence.escape().await;
    Err(codex_refused("permission_picker_not_opened", "o seletor de permissões do Codex não abriu"))
}

/// Movimento e Enter sobre o seletor aberto; erro aqui sai com UM Esc, dado por quem chama.
async fn codex_select(sequence: &PermissionSequence<'_>, screen: &str, requested: &str) -> Result<(), RuntimeError> {
    let modes = parse_codex_modes(screen);
    let Some((key, count)) = codex_steps(&modes, requested) else {
        let names = modes.iter().map(|mode| mode.name.as_str()).collect::<Vec<_>>().join(", ");
        let names = if names.is_empty() { "nenhum".to_owned() } else { names };
        return Err(codex_refused("erro_modo_desconhecido", &format!("modo desconhecido: {requested} (o seletor tem: {names})")));
    };
    for _ in 0..count {
        sequence.press(key).await.map_err(codex_io)?;
        tokio::time::sleep(CODEX_NAV_GAP).await;
    }
    tokio::time::sleep(CODEX_SETTLE).await;
    // A navegação que escorregou faria o Enter aplicar OUTRO modo, calado.
    let selected = parse_codex_modes(&sequence.screen().await.map_err(codex_io)?).into_iter().find(|mode| mode.cursor).map(|mode| mode.name);
    if selected.as_deref().map(|value| value.to_lowercase()) != Some(requested.trim().to_lowercase()) {
        return Err(RuntimeError::new("permission_cursor_mismatch", "O cursor não está no modo solicitado."));
    }
    sequence.press("Enter").await.map_err(codex_io)
}

/// A confirmação do Full Access já veio ou não vem? Estouro esperando por ela não é "não vem": seguir
/// digitaria `/permissions` por cima de um diálogo que pode chegar em seguida.
async fn codex_full_access(sequence: &PermissionSequence<'_>, requested: &str) -> Result<bool, RuntimeError> {
    if codex_full_access_confirmation(&sequence.screen().await.map_err(codex_io)?) { return Ok(true); }
    if !requested.trim().to_lowercase().contains("full access") { return Ok(false); }
    let deadline = Instant::now() + CODEX_OPEN_LIMIT;
    while Instant::now() < deadline {
        tokio::time::sleep(CODEX_SETTLE).await;
        if codex_full_access_confirmation(&sequence.screen().await.map_err(codex_io)?) { return Ok(true); }
    }
    Err(codex_refused("permission_full_access_unconfirmed", "o Codex não mostrou a confirmação do Full Access — confira o terminal da sessão"))
}

/// O que ficou, pelo `(current)` do seletor reaberto: a linha "Permissions updated" do histórico pode
/// ser da troca anterior.
async fn codex_confirm(sequence: &PermissionSequence<'_>, requested: &str) -> Result<String, RuntimeError> {
    // O nosso seletor ainda na tela seria recusado como menu alheio: espera ele fechar, sem tecla.
    let deadline = Instant::now() + CODEX_OPEN_LIMIT;
    while codex_picker_open(&sequence.screen().await.map_err(codex_io)?) {
        if Instant::now() >= deadline {
            return Err(codex_refused("permission_picker_stuck", "o seletor de permissões não fechou depois da confirmação — confira o terminal da sessão"));
        }
        tokio::time::sleep(CODEX_SETTLE).await;
    }
    let screen = codex_open(sequence).await?;
    let current = current_codex_mode(&screen);
    sequence.escape().await.map_err(codex_io)?;
    let current = current.ok_or_else(|| codex_refused("permission_unreadable", "não deu pra ler o modo de permissão depois da troca"))?;
    if current.to_lowercase() != requested.trim().to_lowercase() {
        return Err(codex_refused("permission_not_applied", &format!("o Codex ficou em {current}, não em {requested}")));
    }
    Ok(current)
}

/// Lê (`{modes, current}`) ou troca (`{current}`, o que ficou) o modo do Codex com terminal, inteiro
/// na trava do pane. Não relança nada: sandbox e aprovação trocam ao vivo pela TUI.
pub async fn codex(driver: &TerminalDriver, action: PermissionAction) -> Result<Value, RuntimeError> {
    let sequence = driver.permission_sequence().await;
    let screen = codex_open(&sequence).await?;
    let requested = match action {
        PermissionAction::Read { .. } => {
            let (modes, current) = (parse_codex_modes(&screen), current_codex_mode(&screen));
            sequence.escape().await.map_err(codex_io)?;
            return Ok(json!({"modes": modes, "current": current}));
        }
        PermissionAction::Set { mode } => mode,
    };
    if let Err(error) = codex_select(&sequence, &screen, &requested).await {
        let _ = sequence.escape().await;
        return Err(error);
    }
    if codex_full_access(&sequence, &requested).await? {
        sequence.press("Enter").await.map_err(codex_io)?;
        tokio::time::sleep(CODEX_OPEN_SETTLE).await;
    }
    Ok(json!({"current": codex_confirm(&sequence, &requested).await?}))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::terminal_input::*;
    use std::{collections::VecDeque, sync::{Arc, Mutex}};
    use tokio::time::Instant;

    #[test]
    fn clear_retries_enter_only_for_the_exact_command_in_the_composer() {
        assert!(codex_clear_typed("histórico\n› /clear\n\n  dicas"));
        for screen in ["› \n\n/clear", "› /clear outra\n", "› /clear\n  rascunho", "› /clear\n\n› novo texto", "/clear"] {
            assert!(!codex_clear_typed(screen), "{screen}");
        }
    }

    fn binding(windows: bool) -> TerminalBinding {
        TerminalBinding { name: "test".into(), pane: if windows { "=test:0.0" } else { "%7" }.into(),
            conversation: "uuid".into(), generation: 1, created: 17,
            mux_argv: vec!["fake".into(), "-L".into(), "isolated".into()], windows, clipboard_lock_path: None }
    }
    fn screen(mode: &str) -> String {
        let footer = match mode {
            "manual" => "x\nmanual mode on",
            "plan" => "⏸ plan mode on (shift+tab to cycle)",
            "auto" => "plan mode on\n⏵⏵ auto mode on",
            "acceptEdits" => "⏸ plan mode on\n⏵⏵ accept edits on",
            "bypassPermissions" => "texto\n⏵⏵ bypass permissions on (shift+tab to cycle)",
            "dontAsk" => "⏵⏵ DON\u{2019}T ASK ON",
            _ => "⏵⏵ turbo mode on",
        };
        format!("history\n{}\n❯ \n{}\n{footer}\n", "─".repeat(30), "─".repeat(30))
    }
    struct Services { facts: Mutex<InputFacts>, panel: Mutex<bool>, guard_count: Mutex<usize>, panel_after: Mutex<Option<usize>> }
    impl Services {
        fn new(windows: bool) -> Self {
            Self { facts: Mutex::new(InputFacts { binding: binding(windows), ready: true, idle: true,
                open_question: false, plugin_live: false, plugin_user: false, clipboard_available: false, native: None, screen: None }),
                panel: Mutex::new(false), guard_count: Mutex::new(0), panel_after: Mutex::new(None) }
        }
    }
    impl TerminalServices for Services {
        fn facts<'a>(&'a self, _: &'a TerminalBinding) -> ServiceFuture<'a, InputFacts> {
            Box::pin(async { Ok(self.facts.lock().unwrap().clone()) })
        }
        fn publish<'a>(&'a self, _: &'a TerminalBinding, _: PluginRequest) -> ServiceFuture<'a, PluginReply> {
            Box::pin(async { Ok(PluginReply::Unavailable) })
        }
        fn permission_guard<'a>(&'a self, _: &'a TerminalBinding) -> ServiceFuture<'a, ()> {
            Box::pin(async {
                let mut count = self.guard_count.lock().unwrap(); *count += 1;
                if *self.panel.lock().unwrap() || self.panel_after.lock().unwrap().is_some_and(|n| *count >= n) {
                    Err(ServiceError("terminal_panel_open"))
                } else { Ok(()) }
            })
        }
    }
    // Mesmo contrato de TerminalIo usado pelos testes do escritor, com o rodapé alterado pela tecla.
    struct FakeIo {
        mode: Mutex<String>, steps: Mutex<VecDeque<String>>, calls: Mutex<Vec<(CommandRequest, Instant)>>,
        first_capture_after_key: Mutex<Option<Instant>>, key_at: Mutex<Option<Instant>>,
        failure: Mutex<Option<(usize, bool)>>, capture_failure: Mutex<bool>, stale_after: Mutex<Option<usize>>,
        menu_after: Mutex<Option<usize>>, menu_screen: Mutex<Option<String>>, capture_delay: Mutex<Duration>, block_key: Mutex<bool>,
        entered: tokio::sync::Notify, release: tokio::sync::Notify, windows: bool, neighbor: Mutex<String>,
    }
    impl FakeIo {
        fn new(initial: &str, steps: &[&str], windows: bool) -> Self {
            Self { mode: Mutex::new(initial.into()), steps: Mutex::new(steps.iter().map(|s| s.to_string()).collect()),
                calls: Mutex::new(vec![]), first_capture_after_key: Mutex::new(None), key_at: Mutex::new(None),
                failure: Mutex::new(None), capture_failure: Mutex::new(false), stale_after: Mutex::new(None),
                menu_after: Mutex::new(None), menu_screen: Mutex::new(None), capture_delay: Mutex::new(Duration::ZERO), block_key: Mutex::new(false),
                entered: tokio::sync::Notify::new(), release: tokio::sync::Notify::new(), windows, neighbor: Mutex::new("dontAsk".into()) }
        }
        fn keys(&self) -> Vec<CommandRequest> {
            self.calls.lock().unwrap().iter().filter(|(r, _)| r.args.iter().any(|a| a == "send-keys"))
                .map(|(r, _)| r.clone()).collect()
        }
        fn btab_count(&self) -> usize { self.keys().iter().filter(|r| r.args.last().is_some_and(|a| a == "BTab")).count() }
    }
    impl TerminalIo for FakeIo {
        fn command<'a>(&'a self, r: CommandRequest) -> IoFuture<'a, CommandOutput> {
            Box::pin(async move {
                self.calls.lock().unwrap().push((r.clone(), Instant::now()));
                if r.args.iter().any(|a| a == "display-message") {
                    let stale = self.stale_after.lock().unwrap().is_some_and(|n| self.btab_count() >= n);
                    let stdout = if r.args.last().is_some_and(|a| a.contains("mouse_sgr_flag")) { "1|1|0|150|45\n".to_string() }
                        else { format!("test\t{}\t{}\n", if self.windows { "%1" } else { "%7" }, if stale { 18 } else { 17 }) };
                    return Ok(CommandOutput { success: true, stdout: stdout.into_bytes() });
                }
                if r.args.iter().any(|a| a == "capture-pane") {
                    if self.key_at.lock().unwrap().is_some() && self.first_capture_after_key.lock().unwrap().is_none() {
                        *self.first_capture_after_key.lock().unwrap() = Some(Instant::now());
                    }
                    if *self.capture_failure.lock().unwrap() && self.btab_count() > 0 {
                        return Err(IoFailure { code: "capture_failed", may_have_written: false });
                    }
                    let delay = *self.capture_delay.lock().unwrap();
                    if self.btab_count() > 0 && !delay.is_zero() { tokio::time::sleep(delay).await; }
                    let menu = self.menu_after.lock().unwrap().is_some_and(|n| self.btab_count() >= n);
                    let stdout = if menu { self.menu_screen.lock().unwrap().clone()
                        .unwrap_or_else(|| "Choose a model\n❯ 1. Sonnet\n  2. Opus\nEnter to confirm · Esc to cancel".into()) }
                        else { screen(&self.mode.lock().unwrap()) };
                    return Ok(CommandOutput { success: true, stdout: stdout.into_bytes() });
                }
                if r.args.last().is_some_and(|a| a == "BTab") {
                    *self.key_at.lock().unwrap() = Some(Instant::now());
                    let blocked = *self.block_key.lock().unwrap();
                    if blocked { self.entered.notify_one(); self.release.notified().await; }
                    if let Some((n, may_have_written)) = *self.failure.lock().unwrap() {
                        if self.btab_count() == n { return Err(IoFailure { code: "mux_effect_failed", may_have_written }); }
                    }
                    if let Some(mode) = self.steps.lock().unwrap().pop_front() {
                        let target = r.args.iter().position(|a| a == "-t").map(|i| r.args[i+1].as_str());
                        if self.windows && target == Some("%1") { *self.neighbor.lock().unwrap() = mode; }
                        else { *self.mode.lock().unwrap() = mode; }
                    }
                }
                Ok(CommandOutput { success: true, stdout: vec![] })
            })
        }
        fn socket<'a>(&'a self, _: &'a NativeMessage, _: Vec<u8>) -> IoFuture<'a, WriteOutcome> {
            Box::pin(async { Ok(WriteOutcome::NotWritten) })
        }
    }
    fn driver(io: Arc<FakeIo>, services: Arc<Services>) -> TerminalDriver {
        TerminalDriver::new(binding(io.windows), services, io, InputLimits::default())
    }

    #[tokio::test]
    async fn read_is_literal_and_never_presses_keys() {
        let io = Arc::new(FakeIo::new("plan", &[], false));
        let result = claude(&driver(io.clone(), Arc::new(Services::new(false))), PermissionAction::Read { probe: false }).await.unwrap();
        assert_eq!(result["current"], "plan"); assert_eq!(result["modes"], json!([]));
        assert!(result["previous_non_plan"].is_null()); assert!(io.keys().is_empty());
    }
    #[tokio::test(start_paused = true)]
    async fn first_observation_is_immediate_and_working_turn_is_allowed() {
        let io = Arc::new(FakeIo::new("manual", &["auto"], false));
        let services = Arc::new(Services::new(false));
        services.facts.lock().unwrap().idle = false; services.facts.lock().unwrap().ready = false;
        let result = claude(&driver(io.clone(), services), PermissionAction::Set { mode: "auto".into() }).await.unwrap();
        assert_eq!(result["current"], "auto"); assert_eq!(result["mode"], "auto");
        assert_eq!(io.btab_count(), 1);
        assert_eq!(*io.first_capture_after_key.lock().unwrap(), *io.key_at.lock().unwrap());
    }
    #[tokio::test]
    async fn probe_discovers_real_cycle_and_restores() {
        let io = Arc::new(FakeIo::new("manual", &["acceptEdits", "plan", "auto", "manual"], false));
        let result = claude(&driver(io.clone(), Arc::new(Services::new(false))), PermissionAction::Read { probe: true }).await.unwrap();
        assert_eq!(result["current"], "manual"); assert_eq!(result["restaurado"], true);
        assert_eq!(result["modes"], json!(["manual", "acceptEdits", "plan", "auto"]));
        assert_eq!(io.btab_count(), 4);
    }
    #[tokio::test]
    async fn restoration_failure_reports_proved_mode_not_original() {
        let io = Arc::new(FakeIo::new("manual", &["auto", "plan", "auto"], false));
        *io.failure.lock().unwrap() = Some((4, true));
        let result = claude(&driver(io.clone(), Arc::new(Services::new(false))), PermissionAction::Read { probe: true }).await.unwrap();
        assert_eq!(result["last_confirmed"], "auto"); assert_eq!(result["restaurado"], false);
        assert!(result["current"].is_null()); assert_eq!(result["uncertain"], true);
        assert_eq!(result["error_code"], "mux_effect_failed"); assert_eq!(io.btab_count(), 4);
    }
    #[tokio::test]
    async fn restoration_refused_without_effect_keeps_the_proved_mode() {
        let io = Arc::new(FakeIo::new("manual", &["auto", "plan", "auto"], false));
        *io.failure.lock().unwrap() = Some((4, false));
        let result = claude(&driver(io.clone(), Arc::new(Services::new(false))), PermissionAction::Read { probe: true }).await.unwrap();
        assert_eq!(result["current"], "auto"); assert_eq!(result["last_confirmed"], "auto");
        assert_eq!(result["restaurado"], false); assert_eq!(result["uncertain"], false);
        assert_eq!(result["ok"], false); assert_eq!(io.btab_count(), 4);
    }
    #[tokio::test]
    async fn unreadable_footer_is_an_error_without_a_key_or_fallback_mode() {
        let io = Arc::new(FakeIo::new("unknown", &[], false));
        let result = claude(&driver(io.clone(), Arc::new(Services::new(false))), PermissionAction::Read { probe: true }).await;
        assert_eq!(result.unwrap_err().code, "erro_permissao_leitura"); assert!(io.keys().is_empty());
    }
    #[tokio::test(start_paused = true)]
    async fn discovery_stops_at_eight_seconds_and_does_not_blindly_restore() {
        let io = Arc::new(FakeIo::new("manual", &["auto", "plan", "acceptEdits", "manual"], false));
        *io.capture_delay.lock().unwrap() = Duration::from_millis(1900);
        let started = Instant::now();
        let result = claude(&driver(io.clone(), Arc::new(Services::new(false))), PermissionAction::Read { probe: true }).await.unwrap();
        assert_eq!(Instant::now() - started, Duration::from_secs(8)); assert_eq!(io.btab_count(), 3);
        assert_eq!(result["uncertain"], true); assert_eq!(result["ok"], false); assert_eq!(result["restaurado"], false);
        assert!(result["current"].is_null()); assert_eq!(result["last_confirmed"], "plan");
    }
    #[tokio::test]
    async fn dont_ask_is_not_probed_and_invalid_target_is_not_written() {
        for action in [PermissionAction::Read { probe: true }, PermissionAction::Set { mode: "invented".into() }] {
            let io = Arc::new(FakeIo::new("dontAsk", &[], false));
            let result = claude(&driver(io.clone(), Arc::new(Services::new(false))), action).await;
            if let Ok(value) = result { assert_eq!(value["current"], "dontAsk"); assert_eq!(value["sondavel"], false); }
            else { assert_eq!(result.unwrap_err().code, "erro_permissao_invalida"); }
            assert!(io.keys().is_empty());
        }
    }
    #[tokio::test(start_paused = true)]
    async fn unchanged_footer_times_out_without_retry_or_reopening() {
        let io = Arc::new(FakeIo::new("manual", &[], false)); let started = Instant::now();
        let result = claude(&driver(io.clone(), Arc::new(Services::new(false))), PermissionAction::Set { mode: "bypassPermissions".into() }).await.unwrap();
        assert_eq!(result["uncertain"], true); assert_eq!(result["last_confirmed"], "manual");
        assert_ne!(result["requires_reopen"], true); assert_eq!(io.btab_count(), 1);
        assert_eq!(Instant::now() - started, Duration::from_secs(2));
    }
    #[tokio::test]
    async fn only_a_closed_cycle_without_bypass_requests_reopening() {
        let io = Arc::new(FakeIo::new("manual", &["acceptEdits", "plan", "auto", "manual"], false));
        let result = claude(&driver(io.clone(), Arc::new(Services::new(false))), PermissionAction::Set { mode: "bypassPermissions".into() }).await.unwrap();
        assert_eq!(result["current"], "manual"); assert_eq!(result["requires_reopen"], true);
        assert_eq!(result["uncertain"], false); assert_eq!(result["error_code"], "permission_reopen_required");
        assert_eq!(io.btab_count(), 4);
    }
    #[tokio::test(start_paused = true)]
    async fn probe_budget_has_a_separate_bounded_restoration() {
        let io = Arc::new(FakeIo::new("manual", &["auto", "plan", "auto", "manual"], false));
        *io.capture_delay.lock().unwrap() = Duration::from_millis(1200);
        let started = Instant::now();
        let result = claude(&driver(io.clone(), Arc::new(Services::new(false))), PermissionAction::Read { probe: true }).await.unwrap();
        assert_eq!(result["restaurado"], true); assert_eq!(result["current"], "manual");
        assert!(Instant::now() - started > Duration::from_secs(8));
        let calls = io.calls.lock().unwrap();
        let key_times: Vec<_> = calls.iter().filter(|(r,_)| r.args.last().is_some_and(|s| s == "BTab")).map(|(_,t)| *t).collect();
        assert_eq!(key_times.len(), 4); assert!(Instant::now() - started < Duration::from_secs(16));
        assert!(key_times[..3].iter().all(|t| *t - started < Duration::from_secs(8)));
    }
    #[tokio::test]
    async fn six_keys_are_a_hard_limit_even_for_dont_ask_target() {
        let io = Arc::new(FakeIo::new("manual", &["auto", "plan", "acceptEdits", "bypassPermissions", "plan", "auto", "dontAsk"], false));
        let result = claude(&driver(io.clone(), Arc::new(Services::new(false))), PermissionAction::Set { mode: "dontAsk".into() }).await.unwrap();
        assert_eq!(io.btab_count(), 6); assert_eq!(result["current"], "auto");
        assert_eq!(result["error_code"], "erro_permissao_teto"); assert_ne!(result["requires_reopen"], true);
    }
    #[tokio::test]
    async fn restoration_also_stops_after_six_keys_with_the_effective_mode() {
        let io = Arc::new(FakeIo::new("manual", &["auto", "plan", "auto", "plan", "auto", "plan", "auto", "plan", "auto"], false));
        let result = claude(&driver(io.clone(), Arc::new(Services::new(false))), PermissionAction::Read { probe: true }).await.unwrap();
        assert_eq!(io.btab_count(), 9); assert_eq!(result["current"], "auto");
        assert_eq!(result["restaurado"], false); assert_eq!(result["ok"], false); assert_eq!(result["error_code"], "erro_permissao_teto");
    }
    #[tokio::test]
    async fn panel_menu_and_changed_life_refuse_before_the_next_effect() {
        let io = Arc::new(FakeIo::new("manual", &[], false)); let services = Arc::new(Services::new(false));
        *services.panel.lock().unwrap() = true;
        assert_eq!(claude(&driver(io.clone(), services), PermissionAction::Read { probe: true }).await.unwrap_err().code, "terminal_panel_open");
        assert!(io.keys().is_empty());
        for (menu, stale) in [(Some(0), None), (None, Some(0)), (Some(1), None), (None, Some(1))] {
            let io = Arc::new(FakeIo::new("manual", &["auto"], false));
            *io.menu_after.lock().unwrap() = menu; *io.stale_after.lock().unwrap() = stale;
            let result = claude(&driver(io.clone(), Arc::new(Services::new(false))), PermissionAction::Read { probe: true }).await;
            if menu == Some(0) || stale == Some(0) { assert!(result.is_err()); assert!(io.keys().is_empty()); }
            else { let result = result.unwrap(); assert_eq!(result["uncertain"], true); assert_eq!(io.btab_count(), 1); }
        }
    }
    #[tokio::test]
    async fn long_menu_with_old_mode_in_history_never_receives_btab() {
        let trailing = (0..10).map(|i| format!("model detail {i}\n")).collect::<String>();
        let pane = format!("{}\nChoose a model\n❯ 1. Sonnet\n  2. Opus\nEnter to confirm · Esc to cancel\n{trailing}", screen("manual"));
        let analysis = crate::terminal_state::analyze(&pane);
        assert!(!analysis.overlay);
        assert_eq!(analysis.options, Some(vec!["Sonnet".into(), "Opus".into()]));
        assert_eq!(parse_permission_mode(&pane), Some("manual"));
        for action in [PermissionAction::Read { probe: true }, PermissionAction::Set { mode: "plan".into() }] {
            let io = Arc::new(FakeIo::new("manual", &["plan"], false));
            *io.menu_after.lock().unwrap() = Some(0); *io.menu_screen.lock().unwrap() = Some(pane.clone());
            let services = Arc::new(Services::new(false));
            services.facts.lock().unwrap().idle = false; services.facts.lock().unwrap().ready = false;
            let error = claude(&driver(io.clone(), services), action).await.unwrap_err();
            assert_eq!(error.code, "permission_menu_open");
            assert_eq!(io.btab_count(), 0); assert!(io.keys().is_empty());
            assert!(io.calls.lock().unwrap().iter().any(|(request, _)| request.args.iter().any(|arg| arg == "capture-pane")));
        }
    }
    #[tokio::test]
    async fn panel_opening_between_observation_and_effect_is_rechecked() {
        for (guard_count, keys, uncertain) in [(3, 0, false), (4, 1, true)] {
            let io = Arc::new(FakeIo::new("manual", &["auto"], false)); let services = Arc::new(Services::new(false));
            *services.panel_after.lock().unwrap() = Some(guard_count);
            let result = claude(&driver(io.clone(), services), PermissionAction::Set { mode: "auto".into() }).await.unwrap();
            assert_eq!(result["ok"], false); assert_eq!(result["uncertain"], uncertain);
            assert_eq!(result["error_code"], "terminal_panel_open"); assert_eq!(io.btab_count(), keys);
        }
    }
    #[tokio::test]
    async fn capture_failure_after_key_is_explicitly_uncertain() {
        let io = Arc::new(FakeIo::new("manual", &["auto"], false)); *io.capture_failure.lock().unwrap() = true;
        let result = claude(&driver(io.clone(), Arc::new(Services::new(false))), PermissionAction::Set { mode: "auto".into() }).await.unwrap();
        assert!(result["current"].is_null()); assert_eq!(result["last_confirmed"], "manual");
        assert_eq!(result["uncertain"], true); assert_eq!(result["error_code"], "capture_failed"); assert_eq!(io.btab_count(), 1);
    }
    #[tokio::test]
    async fn input_cannot_interleave_inside_permission_sequence() {
        let io = Arc::new(FakeIo::new("manual", &["plan", "manual"], false)); *io.block_key.lock().unwrap() = true;
        let d = Arc::new(driver(io.clone(), Arc::new(Services::new(false))));
        let probe = { let d = d.clone(); tokio::spawn(async move { claude(&d, PermissionAction::Read { probe: true }).await }) };
        io.entered.notified().await;
        let input = { let d = d.clone(); tokio::spawn(async move { d.text("input concorrente").await }) };
        tokio::task::yield_now().await; assert_eq!(io.keys().len(), 1);
        *io.block_key.lock().unwrap() = false; io.release.notify_one();
        assert_eq!(probe.await.unwrap().unwrap()["restaurado"], true); input.await.unwrap();
        let keys = io.keys(); assert_eq!(keys.len(), 3);
        assert_eq!(keys.iter().map(|r| r.args.last().unwrap().as_str()).collect::<Vec<_>>(), vec!["BTab", "BTab", "input concorrente"]);
    }
    #[tokio::test]
    async fn windows_targets_are_complete_and_mods_btab_stays_denied() {
        let io = Arc::new(FakeIo::new("manual", &["plan"], true)); let d = driver(io.clone(), Arc::new(Services::new(true)));
        assert_eq!(d.mods_keys(&["BTab"]).await.unwrap_err().code, "key_not_allowed");
        assert!(io.keys().is_empty());
        assert_eq!(claude(&d, PermissionAction::Set { mode: "plan".into() }).await.unwrap()["current"], "plan");
        let calls = io.calls.lock().unwrap();
        for (r, _) in calls.iter() {
            let target = r.args.iter().position(|a| a == "-t").unwrap();
            assert_eq!(r.args[target + 1], "=test:0.0"); assert!(!r.args.iter().any(|a| a == "%1"));
        }
        drop(calls);
        assert_eq!(io.btab_count(), 1); assert_eq!(*io.neighbor.lock().unwrap(), "dontAsk");
    }

    // ── Codex ───────────────────────────────────────────────────────────────────────────────────
    const CODEX_PICKER: &str = include_str!("../../../backend/tests/fixtures/pane_codex_permissions.txt");
    const CODEX_CONFIRM: &str = include_str!("../../../backend/tests/fixtures/pane_codex_permissions_confirma.txt");
    const NAMES: [&str; 3] = ["Ask for approval", "Approve for me", "Full Access"];
    const WINDOWS_ESCAPE: &str = "\u{1b}[27;1;27;1;0;1_\u{1b}[27;1;27;0;0;1_";

    #[test]
    fn conversation_codex_permissions_read_the_live_fixture() {
        let pane = include_str!("../../../backend/tests/fixtures/pane_codex_permissions.txt");
        let modes = parse_codex_modes(pane);
        assert!(!modes.is_empty());
        assert_eq!(modes.iter().filter(|mode| mode.current).count(), 1);
        assert_eq!(modes.iter().filter(|mode| mode.cursor).count(), 1);
        assert!(current_codex_mode(pane).is_some());
        for mode in &modes {
            let public = serde_json::to_value(mode).unwrap();
            assert_eq!(public["numero"], mode.number);
            assert_eq!(public["nome"], mode.name);
            assert_eq!(public["desc"], mode.description);
            assert_eq!(public["atual"], mode.current);
            assert_eq!(public["cursor"], mode.cursor);
        }
    }

    #[test]
    fn conversation_codex_permissions_parse_like_the_python_reader() {
        let modes = parse_codex_modes(CODEX_PICKER);
        assert_eq!(modes.iter().map(|m| m.name.as_str()).collect::<Vec<_>>(), NAMES);
        assert_eq!(modes.iter().map(|m| m.number).collect::<Vec<_>>(), [1, 2, 3]);
        assert!(modes.iter().all(|m| !m.description.is_empty()), "a quebra da descrição não vira modo");
        assert_eq!(current_codex_mode(CODEX_PICKER).as_deref(), Some("Full Access"));
        assert_eq!(modes.iter().filter(|m| m.cursor).map(|m| m.name.as_str()).collect::<Vec<_>>(), ["Full Access"]);
        assert_eq!(codex_steps(&modes, "Ask for approval"), Some(("Up", 2)));
        assert_eq!(codex_steps(&modes, "approve for me "), Some(("Up", 1)));
        assert_eq!(codex_steps(&modes, "Full Access"), Some(("Up", 0)));
        assert_eq!(codex_steps(&modes, "Modo Que Não Existe"), None);
        assert!(parse_codex_modes("› ola\n  1. isso nao e o picker   descricao\n").is_empty());
        assert!(!codex_picker_open("qualquer coisa") && current_codex_mode("qualquer coisa").is_none());
        let half = CODEX_PICKER.split("Press enter to confirm").next().unwrap();
        assert!(codex_picker_open(half) && !codex_picker_drawn(half) && codex_picker_drawn(CODEX_PICKER));
        assert!(codex_full_access_confirmation(CODEX_CONFIRM) && !codex_full_access_confirmation(CODEX_PICKER));
        assert!(!codex_picker_drawn(CODEX_CONFIRM));
    }

    /// TUI do Codex simulada: `/permissions`+Enter abre o seletor, setas movem, Enter aplica (Full Access
    /// pede a segunda confirmação), Esc fecha.
    #[derive(Default)]
    struct Tui {
        mode: usize, cursor: usize, picker: bool, confirm: bool, typed: String,
        /// Enter que abre o seletor: 1 normal, 2 com o autocomplete comendo o primeiro, 0 nunca.
        enters_to_open: usize, enters: usize,
        /// Capturas até a confirmação do Full Access aparecer; `None` com `confirm_never` = nunca.
        confirm_delay: usize, confirm_pending: Option<usize>, confirm_never: bool,
        slip: bool, half_drawn: bool, foreign: Option<String>, enter_shows: Option<String>,
        stale_after_keys: Option<usize>, keys: usize,
    }
    struct CodexIo { windows: bool, tui: Mutex<Tui>, calls: Mutex<Vec<CommandRequest>>,
        block_first_key: Mutex<bool>, entered: tokio::sync::Notify, release: tokio::sync::Notify }
    impl CodexIo {
        fn new(mode: &str, windows: bool) -> Self {
            let mode = NAMES.iter().position(|n| *n == mode).unwrap();
            Self { windows, tui: Mutex::new(Tui { mode, cursor: mode, enters_to_open: 1, ..Default::default() }),
                calls: Mutex::new(vec![]), block_first_key: Mutex::new(false),
                entered: tokio::sync::Notify::new(), release: tokio::sync::Notify::new() }
        }
        fn with(self, change: impl FnOnce(&mut Tui)) -> Self { change(&mut self.tui.lock().unwrap()); self }
        fn keys(&self) -> Vec<String> {
            self.calls.lock().unwrap().iter().filter(|r| r.args.iter().any(|a| a == "send-keys")).map(|r| {
                let last = r.args.last().unwrap().as_str();
                match last { "\r" => "Enter".into(), WINDOWS_ESCAPE => "Escape".into(), other => other.into() }
            }).collect()
        }
        fn render(tui: &mut Tui) -> String {
            if let Some(screen) = &tui.foreign { return screen.clone(); }
            if let Some(left) = tui.confirm_pending {
                if left == 0 { tui.confirm = true; tui.confirm_pending = None; } else { tui.confirm_pending = Some(left - 1); }
            }
            if tui.confirm { return CODEX_CONFIRM.into(); }
            if !tui.picker { return format!("• pronto\n\n› {}\n", tui.typed); }
            let descriptions = ["Codex can read and edit files in the current workspace, and run\n                            commands.",
                "Only ask for actions detected as potentially unsafe.", "Codex can edit files outside this workspace."];
            let mut out = String::from("history\n\n  Update Model Permissions\n\n");
            for (i, name) in NAMES.iter().enumerate() {
                let label = if i == tui.mode { format!("{name} (current)") } else { (*name).to_owned() };
                // Como a TUI: a coluna da descrição fica a dois espaços do rótulo mais longo, `(current)` incluído.
                out.push_str(&format!("{}{}. {label:<26}  {}\n", if i == tui.cursor { "› " } else { "  " }, i + 1, descriptions[i]));
            }
            if !tui.half_drawn { out.push_str("\n  Press enter to confirm or esc to go back\n"); }
            out
        }
        fn press(tui: &mut Tui, key: &str) {
            match key {
                "Escape" => { tui.picker = false; tui.confirm = false; tui.typed.clear(); }
                "Up" if tui.picker && !tui.slip => tui.cursor = tui.cursor.saturating_sub(1),
                "Down" if tui.picker && !tui.slip => tui.cursor = (tui.cursor + 1).min(NAMES.len() - 1),
                "Enter" if tui.confirm => { tui.confirm = false; tui.mode = 2; }
                "Enter" if tui.picker => {
                    tui.picker = false;
                    if tui.cursor == 2 && tui.mode != 2 { if !tui.confirm_never { tui.confirm_pending = Some(tui.confirm_delay); } }
                    else { tui.mode = tui.cursor; }
                }
                "Enter" if tui.typed == "/permissions" => {
                    tui.enters += 1;
                    if let Some(screen) = tui.enter_shows.take() { tui.foreign = Some(screen); }
                    else if tui.enters_to_open != 0 && tui.enters >= tui.enters_to_open { tui.picker = true; tui.cursor = tui.mode; tui.typed.clear(); }
                }
                "Enter" if tui.typed == "/clear" => {
                    tui.enters += 1;
                    if tui.enters >= tui.enters_to_open { tui.typed.clear(); }
                }
                "Up" | "Down" | "Enter" => {}
                text => tui.typed.push_str(text),
            }
        }
    }
    impl TerminalIo for CodexIo {
        fn command<'a>(&'a self, r: CommandRequest) -> IoFuture<'a, CommandOutput> {
            Box::pin(async move {
                self.calls.lock().unwrap().push(r.clone());
                let out = |s: String| Ok(CommandOutput { success: true, stdout: s.into_bytes() });
                if r.args.iter().any(|a| a == "display-message") {
                    if r.args.last().is_some_and(|a| a.contains("pane_in_mode")) { return out("1|0|0|150|45\n".into()); }
                    let tui = self.tui.lock().unwrap();
                    let stale = tui.stale_after_keys.is_some_and(|n| tui.keys >= n);
                    return out(format!("test\t{}\t{}\n", if self.windows { "%1" } else { "%7" }, if stale { 18 } else { 17 }));
                }
                if r.args.iter().any(|a| a == "capture-pane") { return out(Self::render(&mut self.tui.lock().unwrap())); }
                if r.args.iter().any(|a| a == "send-keys") {
                    let first = std::mem::take(&mut *self.block_first_key.lock().unwrap());
                    if first { self.entered.notify_one(); self.release.notified().await; }
                    let last = r.args.last().unwrap().as_str();
                    let key = match last { "\r" => "Enter", WINDOWS_ESCAPE => "Escape", other => other };
                    let mut tui = self.tui.lock().unwrap();
                    tui.keys += 1;
                    Self::press(&mut tui, key);
                }
                out(String::new())
            })
        }
        fn socket<'a>(&'a self, _: &'a NativeMessage, _: Vec<u8>) -> IoFuture<'a, WriteOutcome> {
            Box::pin(async { Ok(WriteOutcome::NotWritten) })
        }
    }
    fn codex_driver(io: Arc<CodexIo>, services: Arc<Services>) -> TerminalDriver {
        TerminalDriver::new(binding(io.windows), services, io, InputLimits::default())
    }
    async fn run_codex(io: &Arc<CodexIo>, action: PermissionAction) -> Result<Value, RuntimeError> {
        codex(&codex_driver(io.clone(), Arc::new(Services::new(io.windows))), action).await
    }

    #[tokio::test]
    async fn clear_uses_the_tui_and_retries_only_the_untouched_command() {
        for (windows,enters) in [(false,1),(false,2),(true,1),(true,2)] {
            let io = Arc::new(CodexIo::new("Full Access", windows).with(|tui|tui.enters_to_open = enters));
            let driver = codex_driver(io.clone(),Arc::new(Services::new(windows)));
            codex_clear(&driver).await.unwrap();
            assert_eq!(io.keys(),std::iter::once("/clear".to_owned()).chain(std::iter::repeat_n("Enter".into(),enters)).collect::<Vec<_>>());
            assert!(io.tui.lock().unwrap().typed.is_empty());
        }
    }

    #[tokio::test]
    async fn clear_preserves_the_terminal_draft_and_an_open_menu() {
        for menu in [false,true] {
            let io = Arc::new(CodexIo::new("Full Access",false).with(|tui| {
                if menu { tui.picker = true; } else { tui.typed = "rascunho da pessoa".into(); }
            }));
            let driver = codex_driver(io.clone(),Arc::new(Services::new(false)));
            assert!(codex_clear(&driver).await.is_err());
            assert!(io.keys().is_empty());
        }
    }
    fn set(mode: &str) -> PermissionAction { PermissionAction::Set { mode: mode.into() } }

    #[tokio::test(start_paused = true)]
    async fn conversation_codex_permissions_read_lists_and_closes_with_one_escape() {
        let io = Arc::new(CodexIo::new("Full Access", false));
        let result = run_codex(&io, PermissionAction::Read { probe: true }).await.unwrap();
        assert_eq!(result["current"], "Full Access");
        assert_eq!(result["modes"].as_array().unwrap().len(), 3);
        assert_eq!(result["modes"][1], json!({"numero": 2, "nome": "Approve for me", "desc": "Only ask for actions detected as potentially unsafe.", "atual": false, "cursor": false}));
        assert_eq!(io.keys(), ["/permissions", "Enter", "Escape"]);
        assert!(!io.tui.lock().unwrap().picker);
    }

    #[tokio::test(start_paused = true)]
    async fn conversation_codex_permissions_set_moves_rereads_and_returns_what_stayed() {
        let io = Arc::new(CodexIo::new("Full Access", false));
        let result = run_codex(&io, set("Approve for me")).await.unwrap();
        assert_eq!(result, json!({"current": "Approve for me"}));
        // Sem Full Access, nenhuma espera por diálogo nem tecla dele; a releitura fecha com um Esc.
        assert_eq!(io.keys(), ["/permissions", "Enter", "Up", "Enter", "/permissions", "Enter", "Escape"]);
        assert_eq!(io.tui.lock().unwrap().mode, 1);
    }

    #[tokio::test(start_paused = true)]
    async fn conversation_codex_permissions_wait_for_a_late_full_access_dialog() {
        let io = Arc::new(CodexIo::new("Approve for me", false).with(|t| t.confirm_delay = 3));
        let result = run_codex(&io, set("Full Access")).await.unwrap();
        assert_eq!(result["current"], "Full Access");
        // O `/permissions` da releitura só sai depois de o diálogo ser respondido.
        assert_eq!(io.keys(), ["/permissions", "Enter", "Down", "Enter", "Enter", "/permissions", "Enter", "Escape"]);
    }

    #[tokio::test(start_paused = true)]
    async fn conversation_codex_permissions_full_access_timeout_never_types_over_the_dialog() {
        let io = Arc::new(CodexIo::new("Approve for me", false).with(|t| t.confirm_never = true));
        let started = Instant::now();
        let error = run_codex(&io, set("Full Access")).await.unwrap_err();
        assert_eq!(error.code, "permission_full_access_unconfirmed");
        assert!(error.message.contains("confira o terminal"));
        assert!(Instant::now() - started >= CODEX_OPEN_LIMIT);
        assert_eq!(io.keys(), ["/permissions", "Enter", "Down", "Enter"], "nem Esc nem outro /permissions");
    }

    #[tokio::test(start_paused = true)]
    async fn conversation_codex_permissions_cursor_off_target_refuses_before_enter() {
        let io = Arc::new(CodexIo::new("Full Access", false).with(|t| t.slip = true));
        let error = run_codex(&io, set("Ask for approval")).await.unwrap_err();
        assert_eq!(error.code, "permission_cursor_mismatch");
        assert_eq!(io.keys(), ["/permissions", "Enter", "Up", "Up", "Escape"]);
        assert_eq!(io.tui.lock().unwrap().mode, 2);
    }

    #[tokio::test(start_paused = true)]
    async fn conversation_codex_permissions_unknown_mode_closes_and_is_a_distinct_code() {
        let io = Arc::new(CodexIo::new("Full Access", false));
        let error = run_codex(&io, set("Inventado")).await.unwrap_err();
        assert_eq!(error.code, "erro_modo_desconhecido");
        assert!(error.message.contains("Ask for approval, Approve for me, Full Access"));
        assert_eq!(io.keys(), ["/permissions", "Enter", "Escape"]);
    }

    #[tokio::test(start_paused = true)]
    async fn conversation_codex_permissions_refusals_before_selection_press_nothing() {
        // Diálogo do Full Access pendurado, `/permissions` aberto pelo usuário, outro menu do Codex.
        let codex_menu = "Allow command?\n› 1. Yes\n  2. No\n\n  Press enter to confirm or esc to go back\n".to_owned();
        for foreign in [CODEX_CONFIRM.to_owned(), CODEX_PICKER.to_owned(), codex_menu] {
            let io = Arc::new(CodexIo::new("Full Access", false).with(|t| t.foreign = Some(foreign)));
            for action in [PermissionAction::Read { probe: false }, set("Approve for me")] {
                assert_eq!(run_codex(&io, action).await.unwrap_err().code, "permission_menu_open");
            }
            assert!(io.keys().is_empty());
        }
        // Painel real aberto.
        let io = Arc::new(CodexIo::new("Full Access", false)); let services = Arc::new(Services::new(false));
        *services.panel.lock().unwrap() = true;
        assert_eq!(codex(&codex_driver(io.clone(), services), set("Approve for me")).await.unwrap_err().code, "terminal_panel_open");
        assert!(io.keys().is_empty());
        // Rascunho no composer (inclusive o texto que um `/term-input` deixou antes da tecla) e composer ilegível.
        for (screen, code) in [("• pronto\n\n› rascunho do dono\n", "permission_composer_busy"),
            ("• pronto\n\n›\n  segunda linha do rascunho\n", "permission_composer_busy"), ("sem caixa de digitar\n", "permission_composer_unreadable")] {
            let io = Arc::new(CodexIo::new("Full Access", false).with(|t| t.foreign = Some(screen.into())));
            for action in [PermissionAction::Read { probe: false }, set("Approve for me")] {
                assert_eq!(run_codex(&io, action).await.unwrap_err().code, code);
            }
            assert!(io.keys().is_empty());
        }
        let io = Arc::new(CodexIo::new("Full Access", false).with(|t| t.typed = "rascunho".into()));
        assert_eq!(run_codex(&io, set("Approve for me")).await.unwrap_err().code, "permission_composer_busy");
        assert!(io.keys().is_empty());
        // Pane que sumiu (outro nascimento com o mesmo nome).
        let io = Arc::new(CodexIo::new("Full Access", false).with(|t| t.stale_after_keys = Some(0)));
        assert_eq!(run_codex(&io, set("Approve for me")).await.unwrap_err().code, "stale_mux");
        assert!(io.keys().is_empty());
    }

    #[tokio::test(start_paused = true)]
    async fn conversation_codex_permissions_second_enter_only_with_nothing_open() {
        // O autocomplete comeu o primeiro Enter.
        let io = Arc::new(CodexIo::new("Full Access", false).with(|t| t.enters_to_open = 2));
        assert_eq!(run_codex(&io, PermissionAction::Read { probe: false }).await.unwrap()["current"], "Full Access");
        assert_eq!(io.keys(), ["/permissions", "Enter", "Enter", "Escape"]);
        // Seletor pela metade: Esc, nunca o segundo Enter.
        let io = Arc::new(CodexIo::new("Full Access", false).with(|t| t.half_drawn = true));
        assert_eq!(run_codex(&io, PermissionAction::Read { probe: false }).await.unwrap_err().code, "permission_picker_partial");
        assert_eq!(io.keys(), ["/permissions", "Enter", "Escape"]);
        // Diálogo que não é nosso no lugar do seletor: nem Enter nem Esc nele.
        let io = Arc::new(CodexIo::new("Full Access", false).with(|t| t.enter_shows = Some(CODEX_CONFIRM.into())));
        assert_eq!(run_codex(&io, PermissionAction::Read { probe: false }).await.unwrap_err().code, "permission_menu_open");
        assert_eq!(io.keys(), ["/permissions", "Enter"]);
        // Nunca abre: um Esc depois do segundo Enter.
        let io = Arc::new(CodexIo::new("Full Access", false).with(|t| t.enters_to_open = 0));
        assert_eq!(run_codex(&io, PermissionAction::Read { probe: false }).await.unwrap_err().code, "permission_picker_not_opened");
        assert_eq!(io.keys(), ["/permissions", "Enter", "Enter", "Escape"]);
    }

    #[tokio::test(start_paused = true)]
    async fn conversation_codex_permissions_panel_or_vanished_pane_mid_sequence() {
        // Guarda 1: leitura inicial; 2: digitar; 3: Enter; 4: seletor desenhado; 5: a seta.
        let io = Arc::new(CodexIo::new("Full Access", false)); let services = Arc::new(Services::new(false));
        *services.panel_after.lock().unwrap() = Some(5);
        let error = codex(&codex_driver(io.clone(), services), set("Approve for me")).await.unwrap_err();
        assert_eq!(error.code, "terminal_panel_open");
        assert_eq!(io.keys(), ["/permissions", "Enter", "Escape"], "o Esc de fechar não depende do painel");
        // O pane some depois do Enter: nada mais é escrito.
        let io = Arc::new(CodexIo::new("Full Access", false).with(|t| t.stale_after_keys = Some(2)));
        assert_eq!(run_codex(&io, PermissionAction::Read { probe: false }).await.unwrap_err().code, "stale_mux");
        assert_eq!(io.keys(), ["/permissions", "Enter"]);
    }

    #[tokio::test]
    async fn conversation_codex_permissions_share_the_pane_lock_with_keys() {
        let io = Arc::new(CodexIo::new("Full Access", false)); *io.block_first_key.lock().unwrap() = true;
        let serial = Arc::new(tokio::sync::Mutex::new(()));
        let selector = Arc::new(codex_driver(io.clone(), Arc::new(Services::new(false))).with_serial(serial.clone()));
        let keys = Arc::new(codex_driver(io.clone(), Arc::new(Services::new(false))).with_serial(serial));
        let read = { let d = selector.clone(); tokio::spawn(async move { codex(&d, PermissionAction::Read { probe: false }).await }) };
        io.entered.notified().await;
        let typed = { let d = keys.clone(); tokio::spawn(async move { d.text("concorrente").await }) };
        tokio::task::yield_now().await; assert_eq!(io.keys().len(), 1);
        io.release.notify_one();
        assert_eq!(read.await.unwrap().unwrap()["current"], "Full Access"); typed.await.unwrap();
        assert_eq!(io.keys(), ["/permissions", "Enter", "Escape", "concorrente"]);
    }

    #[tokio::test(start_paused = true)]
    async fn conversation_codex_permissions_windows_targets_are_complete() {
        let io = Arc::new(CodexIo::new("Full Access", true));
        assert_eq!(run_codex(&io, set("Approve for me")).await.unwrap()["current"], "Approve for me");
        for request in io.calls.lock().unwrap().iter() {
            let target = request.args.iter().position(|a| a == "-t").unwrap();
            assert_eq!(request.args[target + 1], "=test:0.0"); assert!(!request.args.iter().any(|a| a == "%1"));
        }
        assert!(io.calls.lock().unwrap().iter().any(|r| r.args.last().is_some_and(|a| a == WINDOWS_ESCAPE)));
    }
}
