//! O mesmo módulo do produto, com E/S controlada e leitores/identidade reais.
pub use hangar_server::runtime::other_terminal::*;
pub use hangar_server::{interactions, runtime, terminal_input, terminal_state};
#[path = "../src/runtime/other_terminal/drive.rs"]
mod drive;
use futures_util::future::BoxFuture;
use interactions::{InteractionBinding, InteractionStore};
use runtime::protocol::RuntimeError;
use serde_json::{Value, json};
use std::{
    collections::VecDeque,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
};

fn target(dir: &std::path::Path) -> OtherTarget {
    let home = dir.to_owned();
    OtherTarget {
        key: "life-a".into(),
        generation: 1,
        name: "kimi-a".into(),
        provider: hangar_transcript::Provider::Kimi,
        binding: terminal_input::TerminalBinding {
            name: "kimi-a".into(),
            pane: "%1".into(),
            conversation: "sid".into(),
            generation: 1,
            created: 100,
            mux_argv: vec!["tmux".into()],
            windows: false,
            clipboard_lock_path: None,
        },
        pane_address: "%1".into(),
        dirs: hangar_server::list::discover_other::Dirs {
            home: home.clone(),
            claude: home.join(".claude"),
            codex_home: home.join(".codex"),
            pi_sessions: home.join("pi"),
            omp_config: home.join("omp"),
            omp_agent: home.join("omp/agent"),
            kimi_home: home.join("kimi"),
        },
        cwd: "project".into(),
        pane_pid: 123,
        agent_pid: 123,
        agent_birth: 100.5,
        transcript: home.join("wire.jsonl"),
        state_path: home.join("state.json"),
        lease_path: home.join("state.lock"),
        projection_dir: home.join("queue"),
        config_dirs: vec![],
        created: 100.0,
    }
}
fn composer(text: &str) -> OtherFrame {
    let measured = include_str!("../../../backend/tests/fixtures/pane_kimi_eco_e_dica.txt");
    let value = measured
        .lines()
        .map(|line| {
            if line.trim_start().starts_with("│ >") {
                format!(" │ > {text} │")
            } else {
                line.into()
            }
        })
        .collect::<Vec<_>>()
        .join("\n");
    OtherFrame {
        text: value.clone(),
        typed: Some(value),
        styled: None,
    }
}
#[derive(Default)]
struct Io {
    frames: Mutex<VecDeque<OtherFrame>>,
    writes: Mutex<Vec<(String, bool)>>,
    proofs: AtomicUsize,
    fail_proof: AtomicUsize,
    fail_write: AtomicUsize,
    dead: Arc<AtomicBool>,
    booting: AtomicBool,
    on_capture: Mutex<Option<Box<dyn FnOnce() + Send>>>,
    before_write: Mutex<Option<Box<dyn FnOnce() + Send>>>,
    after_write: Option<Arc<dyn Fn(usize, &str, bool) + Send + Sync>>,
    guarded: Mutex<Vec<bool>>,
}
impl Io {
    fn frames(frames: Vec<OtherFrame>) -> Self {
        Self {
            frames: Mutex::new(frames.into()),
            ..Default::default()
        }
    }
    fn writes(&self) -> Vec<(String, bool)> {
        self.writes.lock().unwrap().clone()
    }
}
impl OtherIo for Io {
    fn prove<'a>(&'a self, _target: &'a OtherTarget) -> BoxFuture<'a, Result<bool, RuntimeError>> {
        Box::pin(async move {
            let n = self.proofs.fetch_add(1, Ordering::SeqCst) + 1;
            Ok(!self.dead.load(Ordering::SeqCst) && self.fail_proof.load(Ordering::SeqCst) != n)
        })
    }
    fn capture<'a>(&'a self) -> BoxFuture<'a, Result<OtherFrame, RuntimeError>> {
        Box::pin(async move {
            if self.booting.load(Ordering::SeqCst) && self.writes.lock().unwrap().is_empty() {
                return Ok(frame("iniciando"));
            }
            if let Some(hook) = self.on_capture.lock().unwrap().take() {
                hook();
            }
            let mut frames = self.frames.lock().unwrap();
            if frames.len() > 1 {
                Ok(frames.pop_front().unwrap())
            } else {
                frames
                    .front()
                    .cloned()
                    .ok_or_else(|| RuntimeError::new("capture_unavailable", "captura indisponível"))
            }
        })
    }
    fn write<'a>(
        &'a self,
        target: &'a OtherTarget,
        text: &'a str,
        literal: bool,
        guard: Option<&'a OtherWriteGuard>,
    ) -> BoxFuture<'a, Result<(), RuntimeError>> {
        Box::pin(async move {
            tokio::task::yield_now().await;
            if let Some(hook) = self.before_write.lock().unwrap().take() {
                hook();
            }
            if let Some(guard) = guard {
                let latest = read_pending_interaction(target)?.ok_or_else(|| {
                    RuntimeError::new("erro_interacao_desatualizada", "pedido encerrado")
                })?;
                if latest.target != guard.target || latest.semantic != guard.semantic {
                    return Err(RuntimeError::new(
                        "erro_interacao_desatualizada",
                        "pedido trocado",
                    ));
                }
            }
            self.guarded.lock().unwrap().push(guard.is_some());
            let n = {
                let mut writes = self.writes.lock().unwrap();
                writes.push((text.into(), literal));
                writes.len()
            };
            if let Some(hook) = &self.after_write {
                hook(n, text, literal);
            }
            if self.fail_write.load(Ordering::SeqCst) == n {
                Err(RuntimeError::new("write_uncertain", "escrita incerta"))
            } else {
                Ok(())
            }
        })
    }
    fn release<'a>(&'a self) -> BoxFuture<'a, ()> {
        Box::pin(async {})
    }
}
fn pending(target: &OtherTarget, questions: Value) -> (InteractionStore, String) {
    std::fs::write(&target.transcript,format!("{}\n",json!({"type":"context.append_loop_event","event":{"type":"tool.call","toolCallId":"q1","name":"AskUserQuestion","args":{"questions":questions}}}))).unwrap();
    let pending = read_pending_interaction(target).unwrap().unwrap();
    let store = InteractionStore::default();
    let binding = InteractionBinding {
        key: target.key.clone(),
        generation: target.generation,
        conversation: target.binding.conversation.clone(),
    };
    store.bind(&binding).unwrap();
    let id = store
        .observe(&binding, &pending.target, Some(pending.semantic))
        .unwrap()
        .unwrap();
    (store, id)
}

#[tokio::test(start_paused = true)]
async fn drive_prompt_proves_own_input_before_enter_and_cleared_composer_afterwards() {
    let dir = tempfile::tempdir().unwrap();
    let target = target(dir.path());
    std::fs::write(&target.transcript, "").unwrap();
    let text = "resposta completa para a sessão";
    let initial = composer("");
    let io = Io::frames(vec![composer(text), composer("")]);
    let result = drive::prompt(&target, &io, text, Some(&initial)).await;
    assert_eq!(result.disposition, terminal_input::Disposition::Accepted);
    assert_eq!(
        io.writes(),
        vec![(text.into(), true), ("Enter".into(), false)]
    );
}

#[tokio::test(start_paused = true)]
async fn drive_answer_validates_every_choice_before_the_first_effect() {
    let dir = tempfile::tempdir().unwrap();
    let target = target(dir.path());
    let (store, id) = pending(
        &target,
        json!([{"question":"Quais?","options":[{"label":"Rust"},{"label":"Python"}]}]),
    );
    let io = Io::default();
    let error=drive::control(&target,&io,&store,"op","answer_questions",&json!({"interaction_id":id,"answers":[{"kind":"option","indices":[0,0],"multi":true,"labels":["Rust"]}]}),None).await.err().expect("índice repetido deve ser recusado");
    assert_eq!(error.code, "invalid_answers");
    assert!(io.writes().is_empty());
}

const CHOICE: &str = "  question\n\n   Cor    Submit\n\n  ? Qual cor?\n\n     [1] Azul\n   → [2] Verde\n     [3] Other\n\n   ↑↓ select  1-3 / ↵ choose  ←/→/tab switch  esc cancel";
const TEXT: &str = "  question\n\n  ? Qual cor?\n    Type your answer, then press Enter to save.\n\n   → [3] Other:\n\n   type answer  ↵ save  tab switch  esc cancel";
const REVIEW: &str = "  question\n\n  Review your answer before submit\n\n   Q  Qual cor?\n   → Verde\n\n  Ready to submit your answers?\n\n   → [1] Submit\n     [2] Cancel\n\n   ↑↓ select  1/2 choose  ↵ confirm  ←/→/tab switch  esc cancel";
const APPROVAL: &str = "  ▶ Ready to build with this plan?\n\n  ▶ 1. Approve\n    2. Reject\n    3. Revise\n\n  ↑/↓ select · 1/2/3 choose · ↵ confirm\n";
fn frame(text: &str) -> OtherFrame {
    OtherFrame {
        text: text.into(),
        typed: Some(text.into()),
        styled: None,
    }
}
fn qs() -> Value {
    json!([{"question":"Qual cor?","options":[{"label":"Azul"},{"label":"Verde"}]}])
}
fn append(path: &std::path::Path, value: Value) {
    use std::io::Write;
    writeln!(
        std::fs::OpenOptions::new().append(true).open(path).unwrap(),
        "{value}"
    )
    .unwrap();
}
fn answer_result(path: &std::path::Path, id: &str) {
    append(
        path,
        json!({"type":"context.append_loop_event","event":{"type":"tool.result","toolCallId":id,"result":{"output":"Verde"}}}),
    );
}
fn option(id: &str) -> Value {
    json!({"interaction_id":id,"answers":[{"kind":"option","indices":[1],"labels":["Verde"]}]})
}
fn approval(target: &OtherTarget) -> (InteractionStore, String) {
    std::fs::write(&target.transcript,format!("{}\n",json!({"type":"interaction.request","agentId":"main","id":"a1","kind":"approval","request":{"turnId":8,"toolCallId":"tool_x","toolName":"ExitPlanMode","display":{"kind":"plan_review","plan":"# plano"}}}))).unwrap();
    let pending = read_pending_interaction(target).unwrap().unwrap();
    let store = InteractionStore::default();
    let bound = InteractionBinding {
        key: target.key.clone(),
        conversation: target.binding.conversation.clone(),
        generation: target.generation,
    };
    store.bind(&bound).unwrap();
    let id = store
        .observe(&bound, &pending.target, Some(pending.semantic))
        .unwrap()
        .unwrap();
    (store, id)
}

#[tokio::test(start_paused = true)]
async fn drive_prompt_rejects_controls_before_any_effect() {
    let dir = tempfile::tempdir().unwrap();
    let target = target(dir.path());
    let io = Io::default();
    for text in ["teste\0", "teste\u{1b}", ""] {
        let result = drive::prompt(&target, &io, text, None).await;
        assert_eq!(result.disposition, terminal_input::Disposition::Rejected);
        assert_eq!(result.stage, terminal_input::DeliveryStage::Validate);
    }
    assert!(io.writes().is_empty());
    assert_eq!(io.proofs.load(Ordering::SeqCst), 0);
}
#[tokio::test(start_paused = true)]
async fn drive_prompt_defers_a_real_draft_without_erasing_it() {
    let dir = tempfile::tempdir().unwrap();
    let target = target(dir.path());
    std::fs::write(&target.transcript, "").unwrap();
    let io = Io::default();
    let result = drive::prompt(
        &target,
        &io,
        "mensagem",
        Some(&composer("rascunho do usuário")),
    )
    .await;
    assert_eq!(result.disposition, terminal_input::Disposition::Deferred);
    assert_eq!(result.code, "composer_busy");
    assert!(io.writes().is_empty());
}
#[tokio::test(start_paused = true)]
async fn drive_prompt_new_placeholder_proves_own_paste_without_looking_at_history() {
    let dir = tempfile::tempdir().unwrap();
    let target = target(dir.path());
    std::fs::write(&target.transcript, "").unwrap();
    let io = Io::frames(vec![composer("[paste #2 400 chars]"), composer("")]);
    let text = "conteúdo longo que ficou recolhido pela interface";
    let result = drive::prompt(&target, &io, text, Some(&composer(""))).await;
    assert_eq!(result.disposition, terminal_input::Disposition::Accepted);
    assert_eq!(
        io.writes(),
        vec![(text.into(), true), ("Enter".into(), false)]
    );
}
#[tokio::test(start_paused = true)]
async fn drive_prompt_transport_failure_never_submits_even_if_text_looks_present() {
    let dir = tempfile::tempdir().unwrap();
    let target = target(dir.path());
    std::fs::write(&target.transcript, "").unwrap();
    let text = "esta é a mensagem completa enviada";
    let io = Io::frames(vec![composer("rascunho alheio")]);
    io.fail_write.store(1, Ordering::SeqCst);
    let result = drive::prompt(&target, &io, text, Some(&composer(""))).await;
    assert_eq!(result.disposition, terminal_input::Disposition::Unknown);
    assert_eq!(result.cleanup, terminal_input::Cleanup::Unproved);
    assert_eq!(io.writes(), vec![(text.into(), true)]);
}
#[tokio::test(start_paused = true)]
async fn drive_prompt_cleans_only_proved_own_partial_input_before_allowing_retry() {
    let dir = tempfile::tempdir().unwrap();
    let target = target(dir.path());
    std::fs::write(&target.transcript, "").unwrap();
    let text = "esta é a mensagem completa enviada";
    let io = Io::frames(vec![composer("esta é a mensagem"), composer("")]);
    io.fail_write.store(1, Ordering::SeqCst);
    let result = drive::prompt(&target, &io, text, Some(&composer(""))).await;
    assert_eq!(result.disposition, terminal_input::Disposition::Deferred);
    assert_eq!(result.cleanup, terminal_input::Cleanup::Proved);
    assert_eq!(
        io.writes(),
        vec![(text.into(), true), ("C-u".into(), false)]
    );
}
#[tokio::test(start_paused = true)]
async fn drive_prompt_after_possible_enter_stays_unknown_without_cleanup_or_retyping() {
    let dir = tempfile::tempdir().unwrap();
    let target = target(dir.path());
    std::fs::write(&target.transcript, "").unwrap();
    let text = "texto completo ainda visível";
    let io = Io::frames(vec![composer(text)]);
    let result = drive::prompt(&target, &io, text, Some(&composer(""))).await;
    assert_eq!(result.disposition, terminal_input::Disposition::Unknown);
    assert_eq!(result.stage, terminal_input::DeliveryStage::SubmitProof);
    assert_eq!(
        io.writes(),
        vec![(text.into(), true), ("Enter".into(), false)]
    );
}
#[tokio::test(start_paused = true)]
async fn drive_prompt_loss_of_life_after_text_prevents_enter() {
    let dir = tempfile::tempdir().unwrap();
    let target = target(dir.path());
    std::fs::write(&target.transcript, "").unwrap();
    let text = "mensagem completa";
    let mut io = Io::frames(vec![composer(text)]);
    let dead = io.dead.clone();
    io.after_write = Some(Arc::new(move |_, _, _| dead.store(true, Ordering::SeqCst)));
    let result = drive::prompt(&target, &io, text, Some(&composer(""))).await;
    assert_eq!(result.disposition, terminal_input::Disposition::Unknown);
    assert_eq!(io.writes(), vec![(text.into(), true)]);
}
#[tokio::test(start_paused = true)]
async fn drive_prompt_native_question_blocks_input_even_outside_visible_picker() {
    let dir = tempfile::tempdir().unwrap();
    let target = target(dir.path());
    let _pending = pending(&target, qs());
    let io = Io::default();
    let result = drive::prompt(&target, &io, "outra mensagem", Some(&composer(""))).await;
    assert_eq!(result.disposition, terminal_input::Disposition::Deferred);
    assert_eq!(result.code, "question_open");
    assert!(io.writes().is_empty());
}
#[tokio::test(start_paused = true)]
async fn drive_answer_number_then_review_is_confirmed_by_the_same_tool_id() {
    let dir = tempfile::tempdir().unwrap();
    let target = target(dir.path());
    let (store, id) = pending(&target, qs());
    let mut io = Io::frames(vec![frame(CHOICE), frame(REVIEW)]);
    let path = target.transcript.clone();
    io.after_write = Some(Arc::new(move |n, _, _| {
        if n == 2 {
            answer_result(&path, "q1")
        }
    }));
    let response = drive::control(
        &target,
        &io,
        &store,
        "answer-op",
        "answer_questions",
        &option(&id),
        Some(&frame(CHOICE)),
    )
    .await;
    assert!(response.is_ok());
    let response = response.ok().unwrap();
    assert_eq!(
        response.reply.disposition,
        runtime::protocol::Disposition::Accepted
    );
    assert_eq!(response.reply.payload["fallback"], false);
    assert!(response.fallback_text.is_none());
    assert_eq!(io.writes(), vec![("2".into(), true), ("1".into(), true)]);
    assert!(io.guarded.lock().unwrap().iter().all(|guard| *guard));
}
#[tokio::test(start_paused = true)]
async fn drive_answer_numeric_free_text_is_text_after_the_other_option() {
    let dir = tempfile::tempdir().unwrap();
    let target = target(dir.path());
    let (store, id) = pending(&target, qs());
    let mut io = Io::frames(vec![frame(CHOICE), frame(TEXT), frame(TEXT), frame(REVIEW)]);
    let path = target.transcript.clone();
    io.after_write = Some(Arc::new(move |n, _, _| {
        if n == 4 {
            answer_result(&path, "q1")
        }
    }));
    let response = drive::control(
        &target,
        &io,
        &store,
        "text-op",
        "answer_questions",
        &json!({"interaction_id":id,"answers":[{"kind":"text","value":"123"}]}),
        Some(&frame(CHOICE)),
    )
    .await;
    assert!(response.is_ok());
    assert_eq!(
        response.ok().unwrap().reply.disposition,
        runtime::protocol::Disposition::Accepted
    );
    assert_eq!(
        io.writes(),
        vec![
            ("3".into(), true),
            ("123".into(), true),
            ("Enter".into(), false),
            ("1".into(), true)
        ]
    );
}
#[tokio::test(start_paused = true)]
async fn drive_answer_multi_select_uses_tab_instead_of_enter() {
    let dir = tempfile::tempdir().unwrap();
    let target = target(dir.path());
    let (store, id) = pending(&target, qs());
    let multi = CHOICE.replace("↵ choose", "↵ toggle");
    let mut io = Io::frames(vec![
        frame(&multi),
        frame(&multi),
        frame(&multi),
        frame(REVIEW),
    ]);
    let path = target.transcript.clone();
    io.after_write = Some(Arc::new(move |n, _, _| {
        if n == 4 {
            answer_result(&path, "q1")
        }
    }));
    let response=drive::control(&target,&io,&store,"multi-op","answer_questions",&json!({"interaction_id":id,"answers":[{"kind":"option","indices":[0,1],"multi":true,"labels":["Azul","Verde"]}]}),Some(&frame(&multi))).await;
    assert!(response.is_ok());
    assert_eq!(
        response.ok().unwrap().reply.disposition,
        runtime::protocol::Disposition::Accepted
    );
    assert_eq!(
        io.writes(),
        vec![
            ("1".into(), true),
            ("2".into(), true),
            ("Tab".into(), false),
            ("1".into(), true)
        ]
    );
}
#[tokio::test(start_paused = true)]
async fn drive_answer_result_of_another_id_does_not_confirm_or_trigger_duplicate_fallback() {
    let dir = tempfile::tempdir().unwrap();
    let target = target(dir.path());
    let (store, id) = pending(&target, qs());
    let mut io = Io::frames(vec![frame(CHOICE), frame(REVIEW), composer("")]);
    let path = target.transcript.clone();
    io.after_write = Some(Arc::new(move |n, _, _| {
        if n == 2 {
            answer_result(&path, "other-call")
        }
    }));
    let response = drive::control(
        &target,
        &io,
        &store,
        "unknown-op",
        "answer_questions",
        &option(&id),
        Some(&frame(CHOICE)),
    )
    .await;
    assert!(response.is_ok());
    let response = response.ok().unwrap();
    assert_eq!(
        response.reply.disposition,
        runtime::protocol::Disposition::Unknown
    );
    assert!(response.fallback_text.is_none());
    assert_eq!(io.writes(), vec![("2".into(), true), ("1".into(), true)]);
}
#[tokio::test(start_paused = true)]
async fn drive_answer_drift_before_submit_returns_only_an_intention_for_the_same_actor_queue() {
    let dir = tempfile::tempdir().unwrap();
    let target = target(dir.path());
    let (store, id) = pending(&target, qs());
    let io = Io::frames(vec![
        frame(CHOICE),
        frame(CHOICE),
        frame(CHOICE),
        composer(""),
    ]);
    let response = drive::control(
        &target,
        &io,
        &store,
        "fallback-op",
        "answer_questions",
        &option(&id),
        Some(&frame(CHOICE)),
    )
    .await;
    assert!(response.is_ok());
    let response = response.ok().unwrap();
    assert_eq!(
        response.reply.disposition,
        runtime::protocol::Disposition::Deferred
    );
    assert_eq!(
        response.fallback_text.as_deref(),
        Some("Respondendo à pergunta: Verde")
    );
    assert_eq!(
        io.writes(),
        vec![("2".into(), true), ("Escape".into(), false)]
    );
}
#[tokio::test(start_paused = true)]
async fn drive_question_changed_during_capture_is_rejected_before_a_key() {
    let dir = tempfile::tempdir().unwrap();
    let target = target(dir.path());
    let (store, id) = pending(&target, qs());
    let io = Io::frames(vec![frame(CHOICE)]);
    let path = target.transcript.clone();
    *io.on_capture.lock().unwrap() = Some(Box::new(move || std::fs::write(&path, "").unwrap()));
    let response = drive::control(
        &target,
        &io,
        &store,
        "stale-op",
        "answer_questions",
        &option(&id),
        Some(&frame(CHOICE)),
    )
    .await;
    assert!(response.is_err());
    assert!(io.writes().is_empty());
}
#[tokio::test(start_paused = true)]
async fn drive_question_guard_is_carried_inside_the_io_write_boundary() {
    let dir = tempfile::tempdir().unwrap();
    let target = target(dir.path());
    let (store, id) = pending(&target, qs());
    let io = Io::frames(vec![frame(CHOICE)]);
    let path = target.transcript.clone();
    *io.before_write.lock().unwrap() = Some(Box::new(move || std::fs::write(&path, "").unwrap()));
    let response = drive::control(
        &target,
        &io,
        &store,
        "stale-in-write",
        "answer_questions",
        &option(&id),
        Some(&frame(CHOICE)),
    )
    .await;
    assert!(response.is_err());
    assert_eq!(response.err().unwrap().code, "erro_interacao_desatualizada");
    assert!(io.writes().is_empty());
}
#[tokio::test(start_paused = true)]
async fn drive_approval_literal_digit_requires_matching_wire_resolution() {
    let dir = tempfile::tempdir().unwrap();
    let target = target(dir.path());
    let (store, id) = approval(&target);
    let mut io = Io::default();
    let path = target.transcript.clone();
    io.after_write = Some(Arc::new(move |_, _, _| {
        append(
            &path,
            json!({"type":"interaction.resolved","id":"a1","response":{"decision":"approved"}}),
        )
    }));
    let response = drive::control(
        &target,
        &io,
        &store,
        "select-op",
        "select",
        &json!({"interaction_id":id,"option":1}),
        Some(&frame(APPROVAL)),
    )
    .await;
    assert!(response.is_ok());
    assert_eq!(
        response.ok().unwrap().reply.disposition,
        runtime::protocol::Disposition::Accepted
    );
    assert_eq!(io.writes(), vec![("1".into(), true)]);
}
#[tokio::test(start_paused = true)]
async fn drive_approval_feedback_is_confirmed_by_the_text_field_for_that_request() {
    let dir = tempfile::tempdir().unwrap();
    let target = target(dir.path());
    let (store, id) = approval(&target);
    let io = Io::frames(vec![frame("Type feedback")]);
    let response = drive::control(
        &target,
        &io,
        &store,
        "feedback-op",
        "select",
        &json!({"interaction_id":id,"option":3}),
        Some(&frame(APPROVAL)),
    )
    .await;
    assert!(response.is_ok());
    let response = response.ok().unwrap();
    assert_eq!(
        response.reply.disposition,
        runtime::protocol::Disposition::Accepted
    );
    assert_eq!(response.reply.payload["feedback_pendente"], true);
    assert_eq!(io.writes(), vec![("3".into(), true)]);
}
#[tokio::test(start_paused = true)]
async fn drive_approval_refuses_a_numeric_choice_in_an_existing_feedback_field() {
    let dir = tempfile::tempdir().unwrap();
    let target = target(dir.path());
    let (store, id) = approval(&target);
    let io = Io::default();
    let response = drive::control(
        &target,
        &io,
        &store,
        "feedback-again",
        "select",
        &json!({"interaction_id":id,"option":1}),
        Some(&frame("Type feedback")),
    )
    .await;
    assert!(response.is_err());
    assert!(io.writes().is_empty());
}
#[tokio::test(start_paused = true)]
async fn drive_raw_keys_use_the_io_vocabulary_and_reject_arbitrary_controls() {
    let dir = tempfile::tempdir().unwrap();
    let target = target(dir.path());
    let store = InteractionStore::default();
    let io = Io::default();
    for (kind, key) in [
        ("navigation_key", "PageUp"),
        ("interactive_key", "Backspace"),
        ("interactive_key", "Delete"),
    ] {
        let response = drive::control(
            &target,
            &io,
            &store,
            "key-op",
            kind,
            &json!({"key":key}),
            None,
        )
        .await;
        assert!(response.is_ok());
        assert_eq!(
            response.ok().unwrap().reply.disposition,
            runtime::protocol::Disposition::Accepted
        );
    }
    let response = drive::control(
        &target,
        &io,
        &store,
        "bad-key",
        "navigation_key",
        &json!({"key":"C-c"}),
        None,
    )
    .await;
    assert!(response.is_err());
    assert_eq!(
        io.writes(),
        vec![
            ("PageUp".into(), false),
            ("Backspace".into(), false),
            ("Delete".into(), false)
        ]
    );
}
#[tokio::test(start_paused = true)]
async fn drive_steer_without_a_tui_queue_never_presses_ctrl_s() {
    let dir = tempfile::tempdir().unwrap();
    let target = target(dir.path());
    let store = InteractionStore::default();
    let io = Io::default();
    let response = drive::control(
        &target,
        &io,
        &store,
        "steer-op",
        "steer_queue",
        &json!({}),
        Some(&composer("")),
    )
    .await;
    assert!(response.is_ok());
    assert_eq!(response.ok().unwrap().reply.payload["steered"], false);
    assert!(io.writes().is_empty());
}

#[tokio::test(start_paused = true)]
async fn drive_windows_multiline_uses_the_same_complete_text_and_never_a_second_submit() {
    let dir = tempfile::tempdir().unwrap();
    let mut target = target(dir.path());
    target.binding.windows = true;
    target.pane_address = "=kimi-a:0.0".into();
    std::fs::write(&target.transcript, "").unwrap();
    let text = "primeira linha completa\nsegunda linha completa";
    let io = Io::frames(vec![composer(text), composer("")]);
    let result = drive::prompt(&target, &io, text, Some(&composer(""))).await;
    assert_eq!(result.disposition, terminal_input::Disposition::Accepted);
    assert_eq!(
        io.writes(),
        vec![(text.into(), true), ("Enter".into(), false)]
    );
}
#[tokio::test(start_paused = true)]
async fn drive_slash_preserves_two_enters_only_while_the_same_composer_is_safe() {
    let dir = tempfile::tempdir().unwrap();
    let target = target(dir.path());
    std::fs::write(&target.transcript, "").unwrap();
    let io = Io::frames(vec![composer("/help"), composer(""), composer("")]);
    let result = drive::prompt(&target, &io, "/help", Some(&composer(""))).await;
    assert_eq!(result.disposition, terminal_input::Disposition::Accepted);
    assert_eq!(
        io.writes(),
        vec![
            ("/help".into(), true),
            ("Enter".into(), false),
            ("Enter".into(), false)
        ]
    );
}
#[tokio::test(start_paused = true)]
async fn drive_pi_warning_is_not_a_user_draft_in_the_physical_fallback() {
    let dir = tempfile::tempdir().unwrap();
    let mut target = target(dir.path());
    target.provider = hangar_transcript::Provider::Pi;
    std::fs::write(&target.transcript, "").unwrap();
    let notice = "Subagent async grouped result intercom delivery was not acknowledged for group '/tmp/pi-subagents-example'.";
    let text = "mensagem após aviso de subagente";
    let io = Io::frames(vec![composer(text), composer("")]);
    let result = drive::prompt(&target, &io, text, Some(&composer(notice))).await;
    assert_eq!(result.disposition, terminal_input::Disposition::Accepted);
    assert_eq!(io.writes().first(), Some(&(text.into(), true)));
}
#[derive(Clone)]
struct Trace(Arc<Mutex<Vec<u8>>>);
impl std::io::Write for Trace {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
#[tokio::test(start_paused = true)]
async fn drive_readiness_timeout_attempts_input_and_warns_once_until_the_exact_life_is_forgotten() {
    let bytes = Trace(Arc::default());
    let writer = bytes.clone();
    tracing::subscriber::set_global_default(
        tracing_subscriber::fmt()
            .without_time()
            .with_ansi(false)
            .with_max_level(tracing::Level::WARN)
            .with_writer(move || writer.clone())
            .finish(),
    )
    .unwrap();
    let dir = tempfile::tempdir().unwrap();
    let mut target = target(dir.path());
    target.key = "readiness-unique".into();
    std::fs::write(&target.transcript, "").unwrap();
    drive::forget(&target);
    for _ in 0..2 {
        let io = Io::frames(vec![composer("mensagem de prontidão"), composer("")]);
        io.booting.store(true, Ordering::SeqCst);
        let result = drive::prompt(
            &target,
            &io,
            "mensagem de prontidão",
            Some(&frame("iniciando")),
        )
        .await;
        assert_eq!(result.disposition, terminal_input::Disposition::Unknown);
        assert_eq!(
            io.writes(),
            vec![
                ("mensagem de prontidão".into(), true),
                ("Enter".into(), false)
            ]
        );
    }
    let output = String::from_utf8(bytes.0.lock().unwrap().clone()).unwrap();
    assert_eq!(output.matches("other_terminal_ready_timeout").count(), 1);
    let mut other = target.clone();
    other.binding.conversation = "outra".into();
    drive::forget(&other);
    drive::forget(&target);
    let io = Io::frames(vec![composer("mensagem de prontidão"), composer("")]);
    io.booting.store(true, Ordering::SeqCst);
    drive::prompt(
        &target,
        &io,
        "mensagem de prontidão",
        Some(&frame("iniciando")),
    )
    .await;
    let output = String::from_utf8(bytes.0.lock().unwrap().clone()).unwrap();
    assert_eq!(output.matches("other_terminal_ready_timeout").count(), 2);
    assert!(!output.contains("mensagem de prontidão"));
    drive::forget(&target);
}

proptest::proptest! {
    #![proptest_config(proptest::test_runner::Config::with_cases(24))]
    #[test]
    fn drive_external_answer_choices_never_panic(indices in proptest::collection::vec(0u64..8,0..5),multi in proptest::prelude::any::<bool>(),text in ".{0,48}") {
        let runtime=tokio::runtime::Builder::new_current_thread().enable_all().start_paused(true).build().unwrap();
        runtime.block_on(async {
            let dir=tempfile::tempdir().unwrap();let target=target(dir.path());let (store,id)=pending(&target,qs());let io=Io::frames(vec![frame(CHOICE)]);
            let invalid=indices.is_empty() || indices.iter().any(|index|*index>=2)
                || indices.iter().collect::<std::collections::BTreeSet<_>>().len()!=indices.len()
                || indices.len()>1 && !multi;
            let result=drive::control(&target,&io,&store,"fuzz-op","answer_questions",&json!({"interaction_id":id,"answers":[{"kind":"option","indices":indices,"multi":multi,"labels":[text]}]}),Some(&frame(CHOICE))).await;
            if invalid {
                assert_eq!(result.err().expect("plano inválido antes de efeitos").code,"invalid_answers");
                assert!(io.writes().is_empty());
            } else {
                let result=result.expect("a escolha foi tentada, mas não houve Review nem recibo");
                assert_eq!(result.reply.disposition,runtime::protocol::Disposition::Unknown);
                assert!(result.fallback_text.is_none());
            }
        });
    }
    #[test]
    fn drive_external_control_payloads_never_bypass_validation(payload in ".{0,128}") {
        let runtime=tokio::runtime::Builder::new_current_thread().enable_all().start_paused(true).build().unwrap();
        runtime.block_on(async {
            let dir=tempfile::tempdir().unwrap();let target=target(dir.path());let store=InteractionStore::default();let io=Io::default();
            let value=serde_json::from_str::<Value>(&payload).unwrap_or(Value::String(payload));
            let result=drive::control(&target,&io,&store,"fuzz-control","answer_questions",&value,None).await;
            assert!(result.is_err());assert!(io.writes().is_empty());
        });
    }
}

fn bind_observed(target: &OtherTarget, pending: PendingInteraction) -> (InteractionStore, String) {
    let store = InteractionStore::default();
    let bound = InteractionBinding {
        key: target.key.clone(),
        conversation: target.binding.conversation.clone(),
        generation: target.generation,
    };
    store.bind(&bound).unwrap();
    let id = store
        .observe(&bound, &pending.target, Some(pending.semantic))
        .unwrap()
        .unwrap();
    (store, id)
}
fn pi_pending(target: &OtherTarget) -> (InteractionStore, String) {
    let question = json!({"question":"Qual cor?","options":[{"label":"Azul"},{"label":"Verde"}]});
    let (tool, args) = if target.provider == hangar_transcript::Provider::Omp {
        ("ask", json!({"questions":[question]}))
    } else {
        ("question", question)
    };
    std::fs::write(&target.transcript, format!("{}\n",json!({"message":{"role":"assistant","content":[{"type":"toolCall","name":tool,"id":"p1","arguments":args}]}}))).unwrap();
    bind_observed(target, read_pending_interaction(target).unwrap().unwrap())
}
fn pi_picker(cursor: usize) -> OtherFrame {
    let lines = ["Azul", "Verde", "Type something."]
        .iter()
        .enumerate()
        .map(|(i, label)| {
            format!(
                "{}{}. {label}",
                if cursor == i + 1 { "> " } else { "  " },
                i + 1
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    frame(&format!(
        "● Question Qual cor?\n\n{lines}\n↑↓ navigate • Enter to select • Esc to cancel\n{}\n status\n",
        "─".repeat(60)
    ))
}
#[tokio::test(start_paused = true)]
async fn drive_pi_native_answer_rechecks_cursor_and_guards_each_effect() {
    let dir = tempfile::tempdir().unwrap();
    let mut target = target(dir.path());
    target.provider = hangar_transcript::Provider::Pi;
    let (store, id) = pi_pending(&target);
    let io = Io::frames(vec![pi_picker(2), composer("")]);
    let result = drive::control(
        &target,
        &io,
        &store,
        "op",
        "answer_questions",
        &option(&id),
        Some(&pi_picker(1)),
    )
    .await
    .unwrap();
    assert_eq!(
        result.reply.disposition,
        runtime::protocol::Disposition::Accepted
    );
    assert_eq!(
        io.writes(),
        vec![("Down".into(), false), ("Enter".into(), false)]
    );
    assert!(io.guarded.lock().unwrap().iter().all(|guarded| *guarded));
}
#[tokio::test(start_paused = true)]
async fn drive_pi_native_free_text_goes_through_the_other_option() {
    let dir = tempfile::tempdir().unwrap();
    let mut target = target(dir.path());
    target.provider = hangar_transcript::Provider::Pi;
    let (store, id) = pi_pending(&target);
    let io = Io::frames(vec![pi_picker(3), composer("")]);
    let result = drive::control(
        &target,
        &io,
        &store,
        "op",
        "answer_questions",
        &json!({"interaction_id":id,"answers":[{"kind":"text","value":"123"}]}),
        Some(&pi_picker(1)),
    )
    .await
    .unwrap();
    assert_eq!(
        result.reply.disposition,
        runtime::protocol::Disposition::Accepted
    );
    assert_eq!(
        io.writes(),
        vec![
            ("Down".into(), false),
            ("Down".into(), false),
            ("Enter".into(), false),
            ("123".into(), true),
            ("Enter".into(), false)
        ]
    );
}
#[tokio::test(start_paused = true)]
async fn drive_omp_native_answer_uses_only_the_live_box_cursor() {
    let dir = tempfile::tempdir().unwrap();
    let mut target = target(dir.path());
    target.provider = hangar_transcript::Provider::Omp;
    let (store, id) = pi_pending(&target);
    let measured = include_str!("../../../backend/tests/fixtures/pane_omp_ask_picker.txt");
    let selected_first = measured
        .lines()
        .map(|line| {
            if line.contains("  Verde") {
                line.replace("  Verde", "   Verde")
            } else if line.contains("│    Azul") {
                line.replacen("│    Azul", "│   Azul", 1)
            } else {
                line.into()
            }
        })
        .collect::<Vec<_>>()
        .join("\n");
    let io = Io::frames(vec![frame(&selected_first), composer("")]);
    let result = drive::control(
        &target,
        &io,
        &store,
        "op",
        "select",
        &json!({"interaction_id":id,"option":1}),
        Some(&frame(measured)),
    )
    .await
    .unwrap();
    assert_eq!(
        result.reply.disposition,
        runtime::protocol::Disposition::Accepted
    );
    assert_eq!(
        io.writes(),
        vec![("Up".into(), false), ("Enter".into(), false)]
    );
    assert!(io.guarded.lock().unwrap().iter().all(|guarded| *guarded));
}
fn menu_picker() -> OtherFrame {
    frame("❯ 1. [ ] Azul\n  2. [ ] Verde\nEnter to select · ↑/↓ to navigate · Esc to cancel")
}
fn menu_store(target: &OtherTarget, frame: &OtherFrame) -> (InteractionStore, String) {
    let analysis = terminal_state::analyze(&frame.text);
    bind_observed(
        target,
        menu_interaction(analysis.question, analysis.options.unwrap()),
    )
}
#[tokio::test(start_paused = true)]
async fn drive_menu_toggle_does_not_claim_submission_and_submit_requires_its_tab() {
    let dir = tempfile::tempdir().unwrap();
    let mut target = target(dir.path());
    target.provider = hangar_transcript::Provider::Pi;
    let picker = menu_picker();
    let (store, id) = menu_store(&target, &picker);
    let io = Io::frames(vec![picker.clone()]);
    let result = drive::control(
        &target,
        &io,
        &store,
        "toggle",
        "select",
        &json!({"interaction_id":id,"option":1}),
        Some(&picker),
    )
    .await
    .unwrap();
    assert_eq!(result.reply.payload["submitted"], false);
    assert_eq!(io.writes(), vec![("Enter".into(), false)]);
    let io = Io::frames(vec![
        frame("❯ 1. Submit answers\nEnter to select · ↑/↓ to navigate · Esc to cancel"),
        composer(""),
    ]);
    let result = drive::control(
        &target,
        &io,
        &store,
        "submit",
        "submit_selected",
        &json!({"interaction_id":id}),
        Some(&picker),
    )
    .await
    .unwrap();
    assert_eq!(
        result.reply.disposition,
        runtime::protocol::Disposition::Accepted
    );
    assert_eq!(
        io.writes(),
        vec![("Right".into(), false), ("Enter".into(), false)]
    );
    let io = Io::frames(vec![picker.clone()]);
    let result = drive::control(
        &target,
        &io,
        &store,
        "submit",
        "submit_selected",
        &json!({"interaction_id":id}),
        Some(&picker),
    )
    .await
    .unwrap();
    assert_eq!(
        result.reply.disposition,
        runtime::protocol::Disposition::Unknown
    );
    assert_eq!(io.writes(), vec![("Right".into(), false)]);
}

#[tokio::test(start_paused = true)]
async fn drive_pi_new_paste_and_image_placeholders_remain_proof_after_notice_normalization() {
    for placeholder in [
        "[paste #2 400 chars]",
        "[Pasted text #2 +3 lines]",
        "[Image #2]",
    ] {
        let dir = tempfile::tempdir().unwrap();
        let mut target = target(dir.path());
        target.provider = hangar_transcript::Provider::Pi;
        std::fs::write(&target.transcript, "").unwrap();
        let io = Io::frames(vec![composer(placeholder), composer("")]);
        let initial = composer("");
        let result = drive::prompt(
            &target,
            &io,
            "mensagem que aparece recolhida no composer",
            Some(&initial),
        )
        .await;
        assert_eq!(
            result.disposition,
            terminal_input::Disposition::Accepted,
            "{placeholder}"
        );
        assert_eq!(io.writes().len(), 2);
        assert_eq!(io.writes()[1], ("Enter".into(), false));
    }
}
