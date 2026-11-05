//! Modo confirmado no rodapé do Claude e memória compartilhada com as operações do executor Rust.
use std::{collections::HashMap, sync::LazyLock};

use regex::Regex;

static FOOTER: LazyLock<Regex> = LazyLock::new(|| Regex::new(
    r"(?i)(plan mode on|auto mode on|manual mode on|accept edits on|bypass permissions on|don['’]t ask on)").unwrap());

/// `_CANON` sobre a frase casada, normalizada como `_norm` (a frase só tem espaços simples).
fn canon(phrase: &str) -> Option<&'static str> {
    Some(match phrase.to_lowercase().replace('’', "'").as_str() {
        "plan mode on" => "plan",
        "auto mode on" => "auto",
        "manual mode on" => "manual",
        "accept edits on" => "acceptEdits",
        "bypass permissions on" => "bypassPermissions",
        "don't ask on" => "dontAsk",
        _ => return None,
    })
}

/// Última linha com o glifo (⏸/⏵) e a frase; sem nenhuma, a última com a frase (pane estreito
/// que cortou o glifo).
pub fn parse_permission_mode(pane: &str) -> Option<&'static str> {
    let lines = crate::terminal_state::lines(pane);
    let pass = |glyph: bool| lines.iter().rev()
        .filter(|l| !glyph || l.contains('⏸') || l.contains('⏵'))
        .find_map(|l| FOOTER.find(l).and_then(|m| canon(m.as_str())));
    pass(true).or_else(|| pass(false))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PermissionView {
    pub current: String,
    pub previous_non_plan: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MemoryRevision(u64);

/// Retratos confirmados por conversa, preservados durante sequências controladas de teclas.
#[derive(Debug, Default)]
pub struct PermissionMemory {
    confirmed: HashMap<String, PermissionView>,
    operations: HashMap<String, usize>,
    uncertain: std::collections::HashSet<String>,
    revisions: HashMap<String, u64>,
    /// Conversas vistas só em `plan`: a base fora do plano é um palpite, não uma leitura.
    guessed_base: std::collections::HashSet<String>,
    /// Ordem das bases confirmadas, da mais antiga: a memória guarda só as últimas `KEEP`.
    order: std::collections::VecDeque<String>,
}

/// O teto do Python (`_ultimos_nao_plan`): cada conversa nova entra e nenhuma sai sem ele.
const KEEP: usize = 200;

pub type PermissionStore = std::sync::Arc<std::sync::Mutex<PermissionMemory>>;

impl PermissionMemory {
    pub fn revision(&self, key: &str) -> MemoryRevision { MemoryRevision(self.revisions.get(key).copied().unwrap_or(0)) }
    fn advance(&mut self, key: &str) {
        let revision = self.revisions.entry(key.to_owned()).or_default();
        *revision = revision.checked_add(1).expect("revisão de permissão esgotada");
    }
    pub fn observe_capture(&mut self, key: &str, mode: &str, revision: MemoryRevision) -> Result<PermissionView, &'static str> {
        if self.revision(key) != revision { return Err("permission_capture_stale"); }
        if self.active(key) { return self.confirmed(key)?.ok_or("permission_mode_missing"); }
        self.uncertain.remove(key);
        Ok(self.observe(key, mode))
    }

    pub fn confirmed(&self, key: &str) -> Result<Option<PermissionView>, &'static str> {
        if self.uncertain.contains(key) { return Err("permission_mode_unknown"); }
        Ok(self.confirmed.get(key).cloned())
    }
    pub fn get(&self, key: &str) -> Option<PermissionView> { self.confirmed.get(key).filter(|_| !self.uncertain.contains(key)).cloned() }
    pub fn previous_non_plan(&self, key: &str) -> Option<String> { self.confirmed.get(key).map(|view| view.previous_non_plan.clone()) }
    pub fn finish_unknown(&mut self, key: &str) -> bool {
        let Some(depth) = self.operations.get_mut(key) else { return false };
        *depth -= 1;
        if *depth == 0 { self.operations.remove(key); }
        self.uncertain.insert(key.to_owned());
        self.advance(key);
        true
    }
    pub fn active(&self, key: &str) -> bool { self.operations.contains_key(key) }
    pub fn is_uncertain(&self, key: &str) -> bool { self.uncertain.contains(key) }

    pub fn observe(&mut self, key: &str, mode: &str) -> PermissionView {
        if self.operations.contains_key(key) || self.uncertain.contains(key) {
            return self.confirmed[key].clone();
        }
        if !self.confirmed.contains_key(key) && mode == "plan" { self.guessed_base.insert(key.to_owned()); }
        let view = self.confirmed.entry(key.to_owned()).or_insert_with(|| PermissionView {
            current: mode.to_owned(),
            previous_non_plan: "manual".into(),
        });
        view.current = mode.to_owned();
        if mode != "plan" {
            view.previous_non_plan = mode.to_owned();
            self.guessed_base.remove(key);
        }
        let view = view.clone();
        self.order.retain(|old| old != key);
        self.order.push_back(key.to_owned());
        if self.order.len() > KEEP && let Some(oldest) = self.order.pop_front() {
            // Em operação ou incerta, a entrada segue: o executor ainda vai concluí-la.
            if self.operations.contains_key(&oldest) || self.uncertain.contains(&oldest) { self.order.push_back(oldest); }
            else { self.confirmed.remove(&oldest); self.guessed_base.remove(&oldest); self.revisions.remove(&oldest); }
        }
        view
    }
    /// A base fora do plano foi lida no rodapé (e não suposta na primeira leitura já em `plan`).
    pub fn base_known(&self, key: &str) -> bool { !self.guessed_base.contains(key) }

    /// Sem leitura confirmada não há retrato para preservar: recusa antes de enviar teclas.
    pub fn begin(&mut self, key: &str) -> bool {
        if self.get(key).is_none() {
            return false;
        }
        *self.operations.entry(key.to_owned()).or_default() += 1;
        self.advance(key);
        true
    }

    /// Exige um `begin` bem-sucedido; só o último encerramento confirma o modo efetivo.
    pub fn finish(&mut self, key: &str, final_mode: &str) -> PermissionView {
        let depth = self.operations.get_mut(key).expect("operação de permissão deve estar ativa");
        *depth -= 1;
        if *depth == 0 {
            self.operations.remove(key);
            self.uncertain.remove(key);
        }
        self.advance(key);
        self.observe(key, final_mode)
    }
}

/// Última leitura confirmada; durante a operação preserva o modo até o executor terminar.
#[derive(Debug, Default)]
pub struct Watch {
    key: Option<String>,
    sent: Option<String>,
    during_op: bool,
    /// `(modo, previous_non_plan)` da última resposta.
    pub current: Option<(String, String)>,
}

impl Watch {
    pub fn due(&self, key: &str, observed: &str, op: bool) -> bool {
        op || self.during_op || self.key.as_deref() != Some(key) || self.sent.as_deref() != Some(observed)
    }

    pub fn answered(&mut self, key: &str, observed: &str, op: bool, answer: (String, String)) {
        self.key = Some(key.to_owned());
        self.sent = Some(observed.to_owned());
        self.during_op = op;
        self.current = Some(answer);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Parity B6: a memória poda como o Python (200), sem perder a conversa em operação.
    #[test]
    fn memory_keeps_the_latest_conversations() {
        let mut memory = PermissionMemory::default();
        memory.observe("busy", "manual");
        assert!(memory.begin("busy"));
        for n in 0..KEEP + 5 { memory.observe(&format!("s{n}"), "auto"); }
        assert!(memory.get("s0").is_none(), "a mais antiga saiu");
        assert!(memory.get(&format!("s{}", KEEP + 4)).is_some());
        assert!(memory.confirmed("busy").unwrap().is_some(), "em operação fica");
        assert!(memory.confirmed.len() <= KEEP + 1);
    }

    #[tokio::test]
    async fn capture_before_unknown_finish_cannot_reconfirm_and_fresh_capture_can() {
        let memory: PermissionStore = std::sync::Arc::default();
        memory.lock().unwrap().observe("sid", "manual");
        assert!(memory.lock().unwrap().begin("sid"));
        let (captured_tx, captured_rx) = tokio::sync::oneshot::channel();
        let (observe_tx, observe_rx) = tokio::sync::oneshot::channel();
        use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
        let (read, mut write) = tokio::io::duplex(1024);
        let source = memory.clone();
        let task = tokio::spawn(async move {
            let revision = source.lock().unwrap().revision("sid");
            let mut reader = BufReader::new(read);
            let mut pane = String::new();
            reader.read_line(&mut pane).await.unwrap();
            captured_tx.send(()).unwrap();
            observe_rx.await.unwrap();
            let mode = parse_permission_mode(&pane).unwrap();
            let result = source.lock().unwrap().observe_capture("sid", mode, revision);
            (result, reader)
        });
        write.write_all("⏵ manual mode on\n".as_bytes()).await.unwrap();
        captured_rx.await.unwrap();
        assert!(memory.lock().unwrap().finish_unknown("sid"));
        observe_tx.send(()).unwrap();
        let (result, mut reader) = task.await.unwrap();
        assert_eq!(result.unwrap_err(), "permission_capture_stale");
        assert_eq!(memory.lock().unwrap().confirmed("sid"), Err("permission_mode_unknown"));
        let revision = memory.lock().unwrap().revision("sid");
        write.write_all("⏵ accept edits on\n".as_bytes()).await.unwrap();
        let mut fresh = String::new();
        reader.read_line(&mut fresh).await.unwrap();
        memory.lock().unwrap().observe_capture("sid", parse_permission_mode(&fresh).unwrap(), revision).unwrap();
        assert_eq!(memory.lock().unwrap().confirmed("sid").unwrap().unwrap().current, "acceptEdits");
    }

    #[test]
    fn operation_transitions_invalidate_frames_and_inside_operation_keeps_confirmed_mode() {
        let mut memory = PermissionMemory::default();
        memory.observe("sid", "manual");
        let before = memory.revision("sid");
        assert!(memory.begin("sid"));
        assert_eq!(memory.observe_capture("sid", "auto", before), Err("permission_capture_stale"));
        let inside = memory.revision("sid");
        assert_eq!(memory.observe_capture("sid", "auto", inside).unwrap().current, "manual");
        assert!(memory.begin("sid"));
        let nested = memory.revision("sid");
        memory.finish_unknown("sid");
        assert_eq!(memory.observe_capture("sid", "auto", nested), Err("permission_capture_stale"));
        let still_inside = memory.revision("sid");
        assert_eq!(memory.observe_capture("sid", "auto", still_inside), Err("permission_mode_unknown"));
        memory.finish("sid", "plan");
        assert_eq!(memory.observe_capture("sid", "manual", inside), Err("permission_capture_stale"));
        assert_eq!(memory.confirmed("sid").unwrap().unwrap().current, "plan");
        assert_eq!(memory.previous_non_plan("sid").as_deref(), Some("manual"));
    }

    #[test]
    fn uncertain_operation_requires_a_new_capture_after_all_nested_operations_end() {
        let mut memory = PermissionMemory::default();
        memory.observe("sid", "manual");
        assert!(memory.begin("sid"));
        assert!(memory.begin("sid"));
        assert!(memory.finish_unknown("sid"));
        assert!(memory.get("sid").is_none());
        assert_eq!(memory.previous_non_plan("sid").as_deref(), Some("manual"));
        memory.observe("sid", "manual");
        assert!(memory.get("sid").is_none(), "captura durante operação externa não confirma o modo");
        assert!(memory.finish_unknown("sid"));
        assert!(!memory.begin("sid"), "modo antigo não abre outra operação");
        memory.observe("sid", "acceptEdits");
        assert_eq!(memory.confirmed("sid"), Err("permission_mode_unknown"));
        let fresh = memory.revision("sid");
        memory.observe_capture("sid", "acceptEdits", fresh).unwrap();
        assert_eq!(memory.get("sid").unwrap().current, "acceptEdits");
        assert!(!memory.finish_unknown("sid"));
    }

    #[test]
    fn conversation_permission_probe_does_not_publish_intermediate_mode() {
        let mut memory = PermissionMemory::default();
        memory.observe("key", "bypassPermissions");
        memory.begin("key");
        assert_eq!(memory.observe("key", "manual").current, "bypassPermissions");
        let final_view = memory.finish("key", "plan");
        assert_eq!(final_view.current, "plan");
        assert_eq!(final_view.previous_non_plan, "bypassPermissions");
    }

    #[test]
    fn conversation_permission_remembers_each_confirmed_non_plan_mode() {
        let mut memory = PermissionMemory::default();
        assert_eq!(memory.observe("key", "plan"), PermissionView {
            current: "plan".into(),
            previous_non_plan: "manual".into(),
        });
        for mode in ["manual", "auto", "acceptEdits", "bypassPermissions", "dontAsk"] {
            let view = memory.observe("key", mode);
            assert_eq!(view.current, mode);
            assert_eq!(view.previous_non_plan, mode);
            assert_eq!(memory.observe("key", mode), view);
            let plan = memory.observe("key", "plan");
            assert_eq!(plan.current, "plan");
            assert_eq!(plan.previous_non_plan, mode);
            assert_eq!(memory.observe("key", "plan"), plan);
        }
    }

    #[test]
    fn conversation_permission_finishes_with_effective_mode_and_accepts_new_readings() {
        let mut memory = PermissionMemory::default();
        memory.observe("key", "bypassPermissions");
        for _ in 0..2 {
            memory.begin("key");
            assert_eq!(memory.observe("key", "manual").current, "bypassPermissions");
            let view = memory.finish("key", "auto");
            assert_eq!(view.current, "auto");
            assert_eq!(view.previous_non_plan, "auto");
            assert_eq!(memory.observe("key", "plan").previous_non_plan, "auto");
            assert_eq!(memory.observe("key", "bypassPermissions").current, "bypassPermissions");
        }
    }

    #[test]
    fn conversation_permission_keeps_conversations_independent() {
        let mut memory = PermissionMemory::default();
        let first = memory.observe("first", "bypassPermissions");
        memory.observe("second", "acceptEdits");
        memory.begin("first");
        let second = memory.observe("second", "plan");
        assert_eq!(second.current, "plan");
        assert_eq!(second.previous_non_plan, "acceptEdits");
        memory.begin("second");
        assert_eq!(memory.observe("first", "manual"), first);
        assert_eq!(memory.observe("second", "auto"), second);
        let finished = memory.finish("first", "plan");
        assert_eq!(finished.previous_non_plan, "bypassPermissions");
        assert_eq!(memory.observe("second", "manual"), second);
        assert_eq!(memory.finish("second", "plan"), second);
        assert_eq!(memory.observe("new-conversation", "plan").previous_non_plan, "manual");
    }

    #[test]
    fn conversation_permission_begin_requires_a_confirmed_reading() {
        let mut memory = PermissionMemory::default();
        assert!(!memory.begin("key"));
        assert!(!memory.begin("key"));
        let initial = memory.observe("key", "plan");
        assert_eq!(initial.current, "plan");
        assert_eq!(initial.previous_non_plan, "manual");
        assert!(memory.begin("key"));
        assert_eq!(memory.observe("key", "bypassPermissions"), initial);
        let finished = memory.finish("key", "auto");
        assert_eq!(finished.current, "auto");
        assert_eq!(finished.previous_non_plan, "auto");
        assert_eq!(memory.observe("key", "manual").current, "manual");
    }

    #[test]
    fn conversation_permission_nested_operations_publish_only_the_last_finish() {
        let mut memory = PermissionMemory::default();
        let initial = memory.observe("key", "bypassPermissions");
        assert!(memory.begin("key"));
        assert!(memory.begin("key"));
        assert_eq!(memory.finish("key", "auto"), initial);
        assert_eq!(memory.observe("key", "manual"), initial);
        let finished = memory.finish("key", "plan");
        assert_eq!(finished.current, "plan");
        assert_eq!(finished.previous_non_plan, "bypassPermissions");
        assert_eq!(memory.observe("key", "acceptEdits").previous_non_plan, "acceptEdits");
    }

    #[test]
    #[should_panic(expected = "operação de permissão deve estar ativa")]
    fn conversation_permission_finish_requires_an_active_operation() {
        let mut memory = PermissionMemory::default();
        memory.observe("key", "auto");
        memory.begin("key");
        memory.finish("key", "plan");
        memory.finish("key", "manual");
    }

    #[test]
    fn parse_reads_the_footer_mode() {
        let rows = [
            ("glyph_bypass", "texto\n⏵⏵ bypass permissions on (shift+tab to cycle)", Some("bypassPermissions")),
            ("glyph_plan", "⏸ plan mode on (shift+tab to cycle)", Some("plan")),
            ("last_glyph_wins", "⏸ plan mode on\n⏵⏵ accept edits on", Some("acceptEdits")),
            ("conversation_without_glyph_loses", "plan mode on\n⏵⏵ auto mode on", Some("auto")),
            ("no_glyph_fallback", "x\nmanual mode on", Some("manual")),
            ("upper_and_curly", "⏵⏵ DON\u{2019}T ASK ON", Some("dontAsk")),
            ("kelvin", "⏵⏵ don't as\u{212a} on", Some("dontAsk")),
            ("long_s", "⏵⏵ bypa\u{17f}s permi\u{17f}sions on", None),
            ("unknown", "⏵⏵ turbo mode on", None),
            ("empty", "", None),
            ("separators", "⏵⏵ plan mode on\u{2028}⏵⏵ auto mode on\u{1c}", Some("auto")),
        ];
        for (name, pane, expected) in rows {
            assert_eq!(parse_permission_mode(pane), expected, "{name}");
        }
    }

    #[test]
    fn asks_on_new_reading_during_operation_and_once_after() {
        let mut w = Watch::default();
        assert!(w.due("sid", "plan", false), "nada perguntado ainda");
        w.answered("sid", "plan", false, ("plan".into(), "manual".into()));
        assert!(!w.due("sid", "plan", false), "mesma leitura fora da operação");
        assert!(w.due("sid", "auto", false));
        assert!(w.due("outra", "plan", false), "session-id novo depois do /clear");
        assert!(w.due("sid", "plan", true), "toda rodada da operação");
        w.answered("sid", "plan", true, ("plan".into(), "manual".into()));
        assert!(w.due("sid", "plan", false), "uma vez quando a operação acaba");
        w.answered("sid", "plan", false, ("plan".into(), "manual".into()));
        assert!(!w.due("sid", "plan", false));
    }
}
