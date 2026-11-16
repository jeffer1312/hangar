//! Identidade no ingresso, no dono nativo e no primeiro efeito do formulário terminal.
use crate::fake::*;
use std::{path::{Path, PathBuf}, sync::{Arc, Mutex}, time::Duration};
use axum::http::StatusCode;
use hangar_server::interactions::{ExpectedInteraction, InteractionBinding, InteractionError, InteractionKind, native_target};
use hangar_server::routes::AppState;
use hangar_server::runtime::{gateway::RuntimeRegistry, protocol::{CanoBinding, RuntimeError, RuntimeReply, RuntimeTarget}};
use hangar_server::session_write::answer::*;
use hangar_server::state::ask;
use hangar_server::terminal_input::*;
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

fn binding() -> InteractionBinding { InteractionBinding { key:"k-s".into(), conversation:"sid".into(), generation:1 } }
fn expected(kind: InteractionKind) -> ExpectedInteraction {
    ExpectedInteraction { binding:binding(), target:native_target(kind,"sid",&json!(0)), interaction_id:"shown-a".into(), semantic:json!({}), source_revision:None }
}
fn body(value: Value) -> Body { parse_body(&value.to_string().into()).unwrap() }
fn option() -> Value { json!({"kind":"option","indices":[0],"labels":["first"]}) }

#[test]
fn chat_text_keeps_the_python_golden() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../backend/tests/fixtures/contract/session_write/askq_chat_text.json");
    let rows: Vec<Value> = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
    assert!(rows.len() >= 12);
    for row in rows {
        let questions: Vec<String> = row["questions"].as_array().into_iter().flatten().map(|q|q.as_str().unwrap().into()).collect();
        assert_eq!(chat_text(row["answers"].as_array().unwrap(), &questions), row["expect"].as_str().unwrap(), "{}", row["name"]);
    }
}

#[test]
fn terminal_plan_never_supplies_a_missing_public_id_from_the_pending_question() {
    let expected = expected(InteractionKind::Question);
    let missing = body(json!({"answers":[option()],"request_id":0}));
    let failed = terminal_plan(&missing, &expected, false).err().unwrap();
    assert_eq!((failed.0, failed.1["detail"]["code"].as_str()), (StatusCode::CONFLICT, Some("erro_interacao_sem_identidade")));
    let old = body(json!({"answers":[option()],"interaction_id":"old"}));
    assert_eq!(terminal_plan(&old,&expected,false).err().unwrap().1["detail"]["code"], "erro_interacao_desatualizada");
    let current = body(json!({"answers":[option()],"interaction_id":"shown-a"}));
    let Plan::Control(payload) = terminal_plan(&current,&expected,false).unwrap() else { panic!("controle") };
    assert_eq!(payload["interaction_id"], "shown-a");
    assert!(payload.get("request_id").is_none());
    let mut wrong_kind = expected.clone();
    wrong_kind.target.kind = InteractionKind::Approval;
    assert_eq!(terminal_plan(&current,&wrong_kind,false).err().unwrap().1["detail"]["code"], "erro_interacao_desatualizada");
    assert_eq!(terminal_plan(&current,&expected,true).err().unwrap().1["detail"]["code"], "erro_terminal_aberto");
}

#[test]
fn bodies_preserve_identity_and_native_zero_and_relay_only_normalization() {
    for invalid in [
        json!({"answers":[],"interaction_id":0}), json!({"answers":[],"interaction_id":true}),
        json!({"answers":[],"interaction_id":[],"request_id":0}), json!({"answers":[],"extra":1}),
        json!({"answers":[{"kind":"option","indices":["0"]}],"interaction_id":"old"}),
        json!({"answers":[],"request_id":1.5}), json!({"answers":[],"request_id":true}),
    ] { assert!(parse_body(&invalid.to_string().into()).is_none(), "{invalid}"); }
    let supplied = body(json!({"answers":[option()],"request_id":0,"interaction_id":"shown-a"}));
    assert_eq!(headless_command(&supplied)["request_id"], 0);
    assert_eq!(headless_command(&supplied)["interaction_id"], "shown-a");
    let wrong_native = body(json!({"answers":[option()],"request_id":"0","interaction_id":"shown-a"}));
    assert_eq!(terminal_plan(&wrong_native,&expected(InteractionKind::Question),false).err().unwrap().1["detail"]["code"], "erro_interacao_desatualizada");
}

#[test]
fn async_answer_delivery_preserves_the_explicit_checkbox_and_rejects_coercions() {
    let supplied = body(json!({"answers":[option()],"request_id":"async:t:q:0","interaction_id":"shown-a","steer":true}));
    assert_eq!(headless_command(&supplied)["steer"],true);
    for invalid in [json!("true"),json!(1),Value::Null] {
        assert!(parse_body(&json!({"answers":[],"steer":invalid}).to_string().into()).is_none());
    }
}

#[test]
fn interaction_errors_keep_codes_and_unknown_never_becomes_success_or_not_sent() {
    for error in [InteractionError::Missing, InteractionError::Changed, InteractionError::Unavailable] {
        let failed = Err(RuntimeError::new(error.code(),error.message()));
        for answer in [control_answer(&failed), headless_answer(false,&failed), headless_answer(true,&failed)] {
            assert_eq!(answer.0, error.status());
            assert_eq!(answer.1["detail"]["code"], error.code());
        }
    }
    let unknown = Ok(RuntimeReply { operation_id:"same-operation".into(), disposition:hangar_server::runtime::protocol::Disposition::Unknown, payload:json!({}) });
    for answer in [control_answer(&unknown),headless_answer(false,&unknown),headless_answer(true,&unknown)] {
        assert_eq!(answer.1["detail"]["code"], "erro_sem_confirmacao_resposta");
        assert_ne!(answer.0,StatusCode::OK);
    }
    // Sem terminal nada é digitado: a frase é a do canal da CLI, não a do texto no pane.
    let headless = headless_answer(false,&unknown).1["detail"]["msg"].as_str().unwrap().to_owned();
    assert!(headless.contains("A CLI não confirmou") && !headless.contains("por texto"),"{headless}");
}

#[test]
fn text_fallback_answers_like_the_python() {
    use hangar_server::runtime::protocol::Disposition;
    let reply = |disposition, code: &str| -> Result<RuntimeReply,RuntimeError> { Ok(RuntimeReply { operation_id:"op".into(), disposition, payload:json!({"code":code}) }) };
    // As teclas falharam e a resposta saiu por texto: 200 `fallback:true`.
    assert_eq!(control_answer(&reply(Disposition::Accepted,"answer_fallback_sent")), (StatusCode::OK, json!({"ok":true,"fallback":true})));
    // O texto não saiu (o picker não fechou): nunca diz que respondeu.
    let (status, body) = control_answer(&reply(Disposition::Unknown,"answer_fallback_not_delivered"));
    assert_eq!((status, body["detail"]["code"].clone()), (StatusCode::CONFLICT, json!("erro_resposta_nao_entregue")));
    let (status, body) = control_answer(&reply(Disposition::Unknown,"answer_fallback_unproved"));
    assert_eq!((status, body["detail"]["code"].clone()), (StatusCode::CONFLICT, json!("erro_drive_fallback_falhou")));
    // Texto da reserva: "pergunta → resposta", sem a linha do "conversar".
    let answers = vec![json!({"kind":"option","labels":["Sim"]}), json!({"kind":"text","value":"azul"}), json!({"kind":"chat"})];
    assert_eq!(fallback_text(&answers, &["Q1".into(),"Q2".into(),"Q3".into()]),
        "Respondendo as perguntas (o seletor de opções falhou, vai por texto):\n- Q1 → Sim\n- Q2 → azul");
    assert_eq!(fallback_text(&[json!({"kind":"chat"})], &[]), "");
}

#[test]
fn text_during_the_clear_barrier_is_refused_with_its_own_wait_code() {
    use hangar_server::session_write::input::{Params, classify, input_answer};
    // Fora da fila: a troca de conversa do `/clear` a esvaziaria calada. O app guarda o texto e mostra o motivo.
    let params = Params { text: "Olá", steer: false, terminal: true, codex: false, operation_id: "op" };
    let (sent, _) = classify(&params, &Err(RuntimeError::new("runtime_clear_barrier", "operação terminal conservada no diário")));
    let (status, body) = input_answer(&sent, false);
    assert_eq!((status.as_u16(), body["detail"]["code"].clone()), (400, json!("erro_aguarde_clear")));
    assert_eq!(body["detail"]["msg"], "Espere o /clear terminar e envie de novo.");
}

#[test]
fn codex_terminal_written_or_reserved_answer_is_uncertain_over_http() {
    use hangar_server::runtime::protocol::Disposition;
    use hangar_server::session_write::{codex::skip_answer, control::select_headless_answer};
    let reply = |payload:Value| -> Result<RuntimeReply,RuntimeError> { Ok(RuntimeReply { operation_id:"op".into(), disposition:Disposition::Unknown, payload }) };
    let written = reply(json!({"write_outcome":"written","decision_confirmed":false}));
    let reserved = reply(json!({"reserved":true,"entry_id":"async-answer:1:2","decision_confirmed":false}));
    for answer in [headless_answer(true,&written),select_headless_answer(&written),skip_answer(&written),headless_answer(true,&reserved)] {
        assert_eq!(answer.0, StatusCode::CONFLICT);
        assert_eq!(answer.1["detail"]["code"], "erro_sem_confirmacao_resposta");
    }
}

async fn cano(sink: Arc<Mutex<Vec<Value>>>) -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let (read,mut write) = tokio::io::split(stream);
        let mut reader = BufReader::new(read);
        let mut header = String::new();
        reader.read_line(&mut header).await.unwrap();
        assert_eq!(header,"secret-test\n");
        let snapshot = json!({"type":"cano_snapshot","versao":2,"pid":42,"init":null,"aberto":false,
            "pendentes":[],"ultimo_result":null,"rate_limit":null,"stderr_tail":[],"saiu":null,"inflight":{}});
        write.write_all(format!("{snapshot}\n").as_bytes()).await.unwrap();
        loop {
            let mut raw = String::new();
            if reader.read_line(&mut raw).await.unwrap_or(0) == 0 { break; }
            let envelope: Value = serde_json::from_str(&raw).unwrap();
            sink.lock().unwrap().push(envelope.clone());
            let ack = json!({"type":"cano_input_ack","operation_id":envelope["operation_id"],"outcome":"written"});
            if write.write_all(format!("{ack}\n").as_bytes()).await.is_err() { break; }
        }
    });
    format!("tcp:{address}")
}

#[tokio::test]
async fn missing_old_and_wrong_native_answers_write_nothing_and_never_relay() {
    let dir = tempfile::tempdir().unwrap();
    let sink = Arc::new(Mutex::new(Vec::new()));
    let (python, upstream) = spawn_fake().await;
    let state = AppState::new(config(upstream,"127.0.0.1"));
    let store = state.interactions.clone();
    let registry = crate::runtime_open::with_hubs(RuntimeRegistry::new("instance-test".into()).with_interactions(store.clone())
        .with_processes(Arc::new(crate::runtime_open::RecipeProcesses("sid"))));
    registry.open(RuntimeTarget { key:"k-s".into(),generation:1,name:"s".into(),provider:"claude".into(),
        metadata:json!({"name":"s","headless":true,"session_id":"sid","initialized":true}),
        binding:CanoBinding { pid:42,escuta:cano(sink.clone()).await,token:"secret-test".into(),versao:2 },
        lease_path:dir.path().join("lease"),state_path:dir.path().join("state"),projection_dir:dir.path().join("projection"),
        transcript:dir.path().join("s.jsonl"),created:0.0 }).await.unwrap();
    state.state.runtime.set(registry.clone()).ok().unwrap();
    let old = store.observe(&binding(),&native_target(InteractionKind::Question,"sid",&json!(0)),Some(json!({"question":"A"}))).unwrap().unwrap();
    store.observe(&binding(),&native_target(InteractionKind::Question,"sid",&json!(0)),Some(json!({"question":"B"}))).unwrap();
    let server = spawn_state(state).await;
    for (payload,code) in [(json!({"answers":[option()]}),"erro_interacao_sem_identidade"),
        (json!({"answers":[option()],"request_id":0,"interaction_id":old}),"erro_interacao_desatualizada")] {
        let response = client().post(format!("http://{server}/api/sessions/s/answer"))
            .header("authorization",format!("Bearer {OWNER}")).header("content-type","application/json").body(payload.to_string()).send().await.unwrap();
        assert_eq!(response.status().as_u16(),409);
        let body: Value = serde_json::from_str(&response.text().await.unwrap()).unwrap();
        assert_eq!(body["detail"]["code"],code);
    }
    assert_eq!(python.hits_to("/api/sessions/s/answer"),0);
    assert!(!sink.lock().unwrap().iter().any(|v|v["frame"].as_str().is_some_and(|frame|frame.contains("control_response"))));
    registry.close("k-s",1).await.unwrap();
}

fn terminal_binding() -> TerminalBinding {
    TerminalBinding { name:"test".into(),pane:"%7".into(),conversation:"sid".into(),generation:1,created:17,
        mux_argv:vec!["fake".into()],windows:false,clipboard_lock_path:None }
}
fn picker(title: &str) -> String { format!("{title}\n❯ 1. first\n  2. second\nEsc to cancel · to navigate") }
struct Services;
impl TerminalServices for Services {
    fn facts<'a>(&'a self,binding:&'a TerminalBinding) -> ServiceFuture<'a,InputFacts> {
        Box::pin(async move { Ok(InputFacts { binding:binding.clone(),ready:true,idle:true,open_question:false,
            plugin_live:false,plugin_user:false,clipboard_available:false,native:None,screen:None }) })
    }
    fn publish<'a>(&'a self,_:&'a TerminalBinding,_:PluginRequest) -> ServiceFuture<'a,PluginReply> {
        Box::pin(async { Ok(PluginReply::Unavailable) })
    }
    fn permission_guard<'a>(&'a self,_:&'a TerminalBinding) -> ServiceFuture<'a,()> { Box::pin(async { Ok(()) }) }
}
struct PaneIo { screen:Mutex<String>, writes:Mutex<Vec<CommandRequest>>, replacement:Mutex<Option<(PathBuf,Vec<u8>,String)>> }
impl PaneIo {
    fn new(screen:String) -> Self { Self { screen:Mutex::new(screen),writes:Mutex::new(vec![]),replacement:Mutex::new(None) } }
}
impl TerminalIo for PaneIo {
    fn command<'a>(&'a self,request:CommandRequest) -> IoFuture<'a,CommandOutput> {
        Box::pin(async move {
            let stdout = if request.args.iter().any(|s|s == "display-message") { b"test\t%7\t17\n".to_vec() }
                else if request.args.iter().any(|s|s == "capture-pane") { self.screen.lock().unwrap().as_bytes().to_vec() }
                else {
                    self.writes.lock().unwrap().push(request);
                    if let Some((path,bytes,screen)) = self.replacement.lock().unwrap().take() {
                        std::fs::write(path,bytes).unwrap(); *self.screen.lock().unwrap() = screen;
                    }
                    vec![]
                };
            Ok(CommandOutput { success:true,stdout })
        })
    }
    fn socket<'a>(&'a self,_:&'a NativeMessage,_:Vec<u8>) -> IoFuture<'a,WriteOutcome> { Box::pin(async { Ok(WriteOutcome::NotWritten) }) }
}
fn driver(io:Arc<PaneIo>,jsonl:PathBuf,expected:&mut ExpectedInteraction) -> TerminalDriver {
    let store = Arc::new(hangar_server::interactions::InteractionStore::default());
    store.bind(&expected.binding).unwrap();
    expected.interaction_id = store.observe_revision(&expected.binding,&expected.target,Some(expected.semantic.clone()),expected.source_revision.clone()).unwrap().unwrap();
    TerminalDriver::new(terminal_binding(),Arc::new(Services),io,InputLimits { settle:Duration::ZERO,
        literal_settle:Duration::ZERO,multiline_settle:Duration::ZERO,slash_settle:Duration::ZERO,
        proof_attempts:2,ready_attempts:2,cleanup_attempts:2 }).with_interaction_source(jsonl).with_interactions(store)
}
fn sidecar(jsonl:&Path, title:&str) -> PathBuf {
    let path = ask::sidecar_path(jsonl).unwrap();
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path,json!({"tool_input":{"questions":[
        {"header":"h","question":title,"multiSelect":false,"options":[{"label":"first"},{"label":"second"}]},
        {"header":"h2","question":"Other","multiSelect":false,"options":[{"label":"first"},{"label":"second"}]}]}}).to_string()).unwrap();
    path
}
fn terminal_expected(jsonl:&Path,title:&str) -> ExpectedInteraction {
    let (question,revision) = ask::read_revision(jsonl).unwrap().unwrap();
    ExpectedInteraction { semantic:interaction_semantic(&picker(title),Some(&question)).unwrap(),source_revision:Some(revision),..expected(InteractionKind::Question) }
}
fn answers() -> Vec<QuestionAnswer> {
    serde_json::from_value(json!([option(),{"kind":"chat","chat_index":3}])).unwrap()
}

#[tokio::test]
async fn fresh_pane_b_refuses_a_before_any_key_even_with_old_sidecar_and_monitor() {
    let dir = tempfile::tempdir().unwrap();
    let jsonl = dir.path().join("projects/p/sid.jsonl");
    sidecar(&jsonl,"Question A");
    let mut expected = terminal_expected(&jsonl,"Question A");
    let io = Arc::new(PaneIo::new(picker("Question B")));
    let result = driver(io.clone(),jsonl,&mut expected).guarded_answer(&expected,&answers()).await;
    assert_eq!(result.disposition,Disposition::Rejected);
    assert_eq!(result.code,InteractionError::Changed.code());
    assert!(io.writes.lock().unwrap().is_empty());
}

#[tokio::test]
async fn chat_b_during_closure_stays_on_disk_and_receives_no_text() {
    let dir = tempfile::tempdir().unwrap();
    let jsonl = dir.path().join("projects/p/sid.jsonl");
    let path = sidecar(&jsonl,"Question A");
    let a = std::fs::read(&path).unwrap();
    sidecar(&jsonl,"Question B");
    let b = std::fs::read(&path).unwrap();
    std::fs::write(&path,a).unwrap();
    // A revisão inclui o carimbo do arquivo: a pergunta exibida é a A regravada, não a primeira escrita.
    let mut expected = terminal_expected(&jsonl,"Question A");
    let io = Arc::new(PaneIo::new(picker("Question A")));
    *io.replacement.lock().unwrap() = Some((path.clone(),b.clone(),picker("Question B")));
    let result = driver(io.clone(),jsonl.clone(),&mut expected).guarded_answer_chat(&expected,&answers(),"same-operation").await;
    assert_eq!(result.disposition,Disposition::Unknown);
    assert_eq!(io.writes.lock().unwrap().len(),1,"só o Esc de A, sem colagem nem Enter para B");
    assert_eq!(std::fs::read(&path).unwrap(),b);
    ask::consume_revision(&jsonl,expected.source_revision.as_deref().unwrap()).unwrap();
    assert_eq!(ask::read_pending(&jsonl).unwrap().unwrap().questions[0].question,"Question B");
}

#[tokio::test]
async fn chat_timeout_or_unreadable_pane_never_sends_the_text() {
    for after in [picker("Question A"),String::new()] {
        let dir = tempfile::tempdir().unwrap();
        let jsonl = dir.path().join("projects/p/sid.jsonl");
        let path = sidecar(&jsonl,"Question A");
        let mut expected = terminal_expected(&jsonl,"Question A");
        let io = Arc::new(PaneIo::new(picker("Question A")));
        *io.replacement.lock().unwrap() = Some((path.clone(),std::fs::read(path).unwrap(),after));
        let result = driver(io.clone(),jsonl,&mut expected).guarded_answer_chat(&expected,&answers(),"one-operation").await;
        assert_eq!(result.disposition,Disposition::Unknown);
        assert_eq!(io.writes.lock().unwrap().len(),1);
    }
}

#[tokio::test]
async fn chat_guard_survives_journal_buffer_and_clipboard_preparation() {
    struct Prepared { io:PaneIo, jsonl:PathBuf, stage:&'static str }
    fn empty_composer() -> String { format!("conversation\n{}\n❯ \n{}\n⏵⏵ bypass permissions", "─".repeat(30), "─".repeat(30)) }
    impl Prepared {
        fn replace(&self) { sidecar(&self.jsonl,"Question B"); *self.io.screen.lock().unwrap() = picker("Question B"); }
    }
    impl TerminalServices for Prepared {
        fn facts<'a>(&'a self,binding:&'a TerminalBinding) -> ServiceFuture<'a,InputFacts> {
            Box::pin(async move { Ok(InputFacts { binding:binding.clone(),ready:true,idle:true,open_question:false,
                plugin_live:false,plugin_user:false,clipboard_available:true,native:None,screen:None }) })
        }
        fn publish<'a>(&'a self,_:&'a TerminalBinding,_:PluginRequest) -> ServiceFuture<'a,PluginReply> { Box::pin(async { panic!("publicação sem guarda") }) }
        fn permission_guard<'a>(&'a self,_:&'a TerminalBinding) -> ServiceFuture<'a,()> { Box::pin(async { Ok(()) }) }
        fn writing<'a>(&'a self) -> ServiceFuture<'a,()> { Box::pin(async move { if self.stage == "journal" { self.replace(); } Ok(()) }) }
    }
    impl TerminalIo for Prepared {
        fn command<'a>(&'a self,request:CommandRequest) -> IoFuture<'a,CommandOutput> {
            Box::pin(async move {
                if request.args.iter().any(|arg|arg == "Escape" || arg == "\u{1b}[27;1;27;1;0;1_\u{1b}[27;1;27;0;0;1_") {
                    self.io.writes.lock().unwrap().push(request);
                    *self.io.screen.lock().unwrap() = empty_composer();
                    return Ok(CommandOutput { success:true,stdout:vec![] });
                }
                let preparation = self.stage == "buffer" && request.args.iter().any(|arg|arg == "load-buffer")
                    || self.stage == "clipboard" && request.program == "powershell.exe";
                let result = self.io.command(request).await;
                if preparation { self.replace(); }
                result
            })
        }
        fn socket<'a>(&'a self,_:&'a NativeMessage,_:Vec<u8>) -> IoFuture<'a,WriteOutcome> { Box::pin(async { Ok(WriteOutcome::NotWritten) }) }
    }
    for stage in ["journal","buffer","clipboard"] {
        let dir = tempfile::tempdir().unwrap();
        let jsonl = dir.path().join("projects/p/sid.jsonl");
        sidecar(&jsonl,"Question A");
        let mut expected = terminal_expected(&jsonl,"Question A");
        let store = Arc::new(hangar_server::interactions::InteractionStore::default());
        store.bind(&expected.binding).unwrap();
        expected.interaction_id = store.observe_revision(&expected.binding,&expected.target,Some(expected.semantic.clone()),expected.source_revision.clone()).unwrap().unwrap();
        let io = Arc::new(Prepared { io:PaneIo::new(picker("Question A")),jsonl:jsonl.clone(),stage });
        let mut binding = terminal_binding();
        if stage == "clipboard" { binding.windows = true; binding.pane = "=test:0.0".into(); binding.clipboard_lock_path = Some(dir.path().join("clip.lock")); }
        let driver = TerminalDriver::new(binding,io.clone(),io.clone(),InputLimits { settle:Duration::ZERO,literal_settle:Duration::ZERO,
            multiline_settle:Duration::ZERO,slash_settle:Duration::ZERO,proof_attempts:2,ready_attempts:2,cleanup_attempts:2 })
            .with_interaction_source(jsonl.clone()).with_interactions(store);
        let result = driver.guarded_answer_chat(&expected,&answers(),"op-a").await;
        assert_eq!(result.disposition,Disposition::Unknown,"{stage}");
        assert_eq!(result.code,"erro_sem_confirmacao_resposta","{stage}");
        let writes = io.io.writes.lock().unwrap();
        assert_eq!(writes.iter().filter(|request|request.args.iter().any(|arg|arg == "send-keys")).count(),1,"só Escape: {stage}");
        if stage == "clipboard" {
            let preparation = writes.iter().find(|request|request.program == "powershell.exe").expect("não percorreu preparo do clipboard");
            assert!(preparation.stdin.contains(&b'\n'),"multilinha real no PowerShell");
        }
        if stage == "buffer" { assert!(writes.iter().any(|request|request.args.iter().any(|arg|arg == "load-buffer")),"não percorreu preparo do buffer"); }
        assert!(!writes.iter().any(|request|request.args.iter().any(|arg|arg == "paste-buffer" || arg == "M-v")),"{stage}");
        assert_eq!(ask::read_pending(&jsonl).unwrap().unwrap().questions[0].question,"Question B");
    }
}

#[tokio::test]
async fn guarded_text_keeps_a_across_editor_open_and_rejects_same_title_b() {
    use std::sync::atomic::{AtomicBool,AtomicUsize,Ordering};
    struct EditorIo {
        cursor:Mutex<usize>, opened:AtomicBool, submitted:AtomicBool,
        captures:AtomicUsize, changed:AtomicBool, writes:Mutex<Vec<CommandRequest>>, change:&'static str,
    }
    impl EditorIo {
        fn pane(&self) -> String {
            if self.submitted.load(Ordering::Acquire) { return "conversation".into(); }
            let opened = self.opened.load(Ordering::Acquire);
            if opened {
                let capture = self.captures.fetch_add(1,Ordering::AcqRel);
                if self.change == "after_open" && capture == 0 || self.change == "before_text" && capture == 1
                    || self.change == "unreadable" { self.changed.store(true,Ordering::Release); }
            }
            if self.change == "unreadable" && self.changed.load(Ordering::Acquire) { return String::new(); }
            let labels = if self.changed.load(Ordering::Acquire) { ["allow","deny","Type something."] } else { ["first","second","Type something."] };
            let cursor = *self.cursor.lock().unwrap();
            let options = labels.iter().enumerate().map(|(index,label)|format!("{} {}. {label}",if index + 1 == cursor { "❯" } else { " " },index + 1)).collect::<Vec<_>>().join("\n");
            format!("Question A\n{options}\n{}",if opened { "Text editor\nEnter to submit · Esc to cancel" } else { "Esc to cancel · to navigate" })
        }
    }
    impl TerminalIo for EditorIo {
        fn command<'a>(&'a self,request:CommandRequest) -> IoFuture<'a,CommandOutput> {
            Box::pin(async move {
                let stdout = if request.args.iter().any(|arg|arg == "display-message") { b"test\t%7\t17\n".to_vec() }
                    else if request.args.iter().any(|arg|arg == "capture-pane") { self.pane().into_bytes() }
                    else {
                        match request.args.last().map(String::as_str) {
                            Some("Down") => *self.cursor.lock().unwrap() += 1,
                            Some("\r") => if self.opened.swap(true,Ordering::AcqRel) { self.submitted.store(true,Ordering::Release); },
                            Some("custom answer") => assert!(self.opened.load(Ordering::Acquire)),
                            other => panic!("efeito inesperado: {other:?}"),
                        }
                        self.writes.lock().unwrap().push(request);
                        vec![]
                    };
                Ok(CommandOutput { success:true,stdout })
            })
        }
        fn socket<'a>(&'a self,_:&'a NativeMessage,_:Vec<u8>) -> IoFuture<'a,WriteOutcome> { Box::pin(async { Ok(WriteOutcome::NotWritten) }) }
    }
    for change in ["none","after_open","before_text","unreadable"] {
        let dir = tempfile::tempdir().unwrap();
        let jsonl = dir.path().join("projects/p/sid.jsonl");
        let path = sidecar(&jsonl,"Question A");
        std::fs::write(&path,json!({"tool_input":{"questions":[{"header":"h","question":"Question A","options":[{"label":"first"},{"label":"second"}]}]}}).to_string()).unwrap();
        let io = Arc::new(EditorIo { cursor:Mutex::new(1),opened:AtomicBool::new(false),submitted:AtomicBool::new(false),
            captures:AtomicUsize::new(0),changed:AtomicBool::new(false),writes:Mutex::new(vec![]),change });
        let (question,revision) = ask::read_revision(&jsonl).unwrap().unwrap();
        let mut expected = ExpectedInteraction { semantic:interaction_semantic(&io.pane(),Some(&question)).unwrap(),source_revision:Some(revision),..expected(InteractionKind::Question) };
        let store = Arc::new(hangar_server::interactions::InteractionStore::default());
        store.bind(&expected.binding).unwrap();
        expected.interaction_id = store.observe_revision(&expected.binding,&expected.target,Some(expected.semantic.clone()),expected.source_revision.clone()).unwrap().unwrap();
        let driver = TerminalDriver::new(terminal_binding(),Arc::new(Services),io.clone(),InputLimits { settle:Duration::ZERO,literal_settle:Duration::ZERO,
            multiline_settle:Duration::ZERO,slash_settle:Duration::ZERO,proof_attempts:2,ready_attempts:2,cleanup_attempts:2 })
            .with_interaction_source(jsonl.clone()).with_interactions(store.clone());
        let answers:Vec<QuestionAnswer> = serde_json::from_value(json!([{"kind":"text","value":"custom answer","type_index":2}])).unwrap();
        let result = driver.guarded_answer(&expected,&answers).await;
        let writes = io.writes.lock().unwrap();
        let keys = writes.iter().map(|request|request.args.last().unwrap().as_str()).collect::<Vec<_>>();
        if change == "none" {
            assert_eq!(result.disposition,Disposition::Accepted);
            assert_eq!(keys,vec!["Down","Down","\r","custom answer","\r"]);
        } else {
            assert_eq!(result.disposition,Disposition::Unknown,"{change}");
            assert_eq!(keys,vec!["Down","Down","\r"],"nenhum texto/Enter depois da espera: {change}");
            if change != "unreadable" { assert_eq!(result.code,InteractionError::Changed.code()); }
        }
        assert_eq!(ask::read_pending(&jsonl).unwrap().unwrap().questions[0].question,"Question A");
        assert_eq!(store.expected(&expected.binding,Some(&expected.interaction_id)).unwrap(),expected,"Store antigo não prova o pane novo");
    }
}

#[tokio::test]
async fn native_chat_without_previous_answers_keeps_the_native_option() {
    let dir = tempfile::tempdir().unwrap();
    let jsonl = dir.path().join("projects/p/sid.jsonl");
    let path = sidecar(&jsonl,"Question A");
    std::fs::write(&path,json!({"tool_input":{"questions":[{"header":"h","question":"Question A","options":[]}]}}).to_string()).unwrap();
    // O seletor do Claude sempre tem duas linhas ou mais; com o cursor no "Chat about this", só o Enter.
    let screen = "Question A\n❯ 1. Chat about this\n  2. Type something.\nEsc to cancel · to navigate".to_owned();
    let (question,revision) = ask::read_revision(&jsonl).unwrap().unwrap();
    let mut expected = ExpectedInteraction { semantic:interaction_semantic(&screen,Some(&question)).unwrap(),source_revision:Some(revision),..expected(InteractionKind::Question) };
    let io = Arc::new(PaneIo::new(screen));
    *io.replacement.lock().unwrap() = Some((path.clone(),std::fs::read(path).unwrap(),"conversation".into()));
    let answers:Vec<QuestionAnswer> = serde_json::from_value(json!([{"kind":"chat","chat_index":0}])).unwrap();
    let result = driver(io.clone(),jsonl,&mut expected).guarded_answer_chat(&expected,&answers,"op-chat").await;
    assert_eq!(result.disposition,Disposition::Accepted);
    let writes = io.writes.lock().unwrap();
    assert_eq!(writes.len(),1);
    assert_eq!(writes[0].args.last().unwrap(),"\r");
}

#[test]
fn consuming_a_keeps_b_and_hook_removal_discards_consumption() {
    let dir = tempfile::tempdir().unwrap();
    let jsonl = dir.path().join("projects/p/sid.jsonl");
    let path = sidecar(&jsonl,"A");
    let (_,a) = ask::read_revision(&jsonl).unwrap().unwrap();
    let bytes = std::fs::read(&path).unwrap();
    sidecar(&jsonl,"B");
    ask::consume_revision(&jsonl,&a).unwrap();
    assert_eq!(ask::read_pending(&jsonl).unwrap().unwrap().questions[0].question,"B");
    std::fs::remove_file(&path).unwrap();
    assert!(ask::read_pending(&jsonl).unwrap().is_none());
    std::fs::write(path,bytes).unwrap();
    assert!(ask::read_pending(&jsonl).unwrap().is_some());
}

#[test]
fn claude_select_targets_second_request_and_rechecks_before_the_wire_write() {
    use hangar_server::runtime::{claude::ClaudeEngine,protocol::{ClockSample,EngineInput,OperationKind,RuntimeCommand,Effect}};
    let clock = ClockSample {monotonic_s:1.0,epoch_s:1.0};
    let mut engine = ClaudeEngine::new(json!({"name":"s","session_id":"sid","initialized":true}),1,clock);
    let store = Arc::new(hangar_server::interactions::InteractionStore::default());
    engine.set_interactions("k-s".into(),store.clone()).unwrap();
    let first = json!({"subtype":"can_use_tool","tool_name":"Bash","input":{"command":"first"}});
    let second = json!({"subtype":"can_use_tool","tool_name":"Bash","input":{"command":"second"}});
    for (id,request) in [(json!("first"),first),(json!(0),second.clone())] {
        engine.apply(EngineInput::Line(json!({"type":"control_request","request_id":id,"request":request})),clock).unwrap();
    }
    let token = store.observe(&binding(),&native_target(InteractionKind::Approval,"sid",&json!(0)),Some(second)).unwrap().unwrap();
    let effects = engine.command(RuntimeCommand {operation_id:"choose-zero".into(),kind:OperationKind::Select,
        payload:json!({"option":1,"interaction_id":token})},clock).unwrap();
    assert!(effects.iter().any(|effect|matches!(effect,Effect::Write {frame,..} if frame["response"]["request_id"] == 0)));
    assert!(engine.interaction_write_error("choose-zero").is_none());
    // A fonte ainda conserva A; a pendência real já mudou antes de o diário liberar a escrita.
    engine.apply(EngineInput::Line(json!({"type":"control_request","request_id":0,
        "request":{"subtype":"can_use_tool","tool_name":"Bash","input":{"command":"replacement"}}})),clock).unwrap();
    assert_eq!(engine.interaction_write_error("choose-zero").unwrap().code,InteractionError::Changed.code());
}

#[test]
fn codex_native_zero_keeps_question_and_approval_independent_and_checks_revision() {
    use hangar_server::runtime::{codex::Engine,protocol::{ClockSample,EngineInput,OperationKind,RuntimeCommand,Effect}};
    let clock = ClockSample {monotonic_s:1.0,epoch_s:1.0};
    let mut engine = Engine::new(json!({"name":"s","thread_id":"sid","initialized":true,"ready":true}),1,clock);
    let store = Arc::new(hangar_server::interactions::InteractionStore::default());
    engine.set_interactions("k-s".into(),store.clone()).unwrap();
    let approval = json!({"id":"approval","method":"item/commandExecution/requestApproval","params":{"threadId":"sid","command":"pwd"}});
    engine.apply(EngineInput::Line(approval),clock).unwrap();
    let question = json!({"id":0,"method":"item/tool/requestUserInput","params":{"threadId":"sid","questions":[
        {"id":"q","header":"h","question":"Choose","options":[{"label":"first","description":""}]}]}});
    engine.apply(EngineInput::Line(question.clone()),clock).unwrap();
    let revision = engine.request_revision(&serde_json::from_value(json!(0)).unwrap()).unwrap();
    let token = store.observe_revision(&binding(),&native_target(InteractionKind::Question,"sid",&json!(0)),Some(question.clone()),Some(revision)).unwrap().unwrap();
    let command = |native| RuntimeCommand {operation_id:"answer-zero".into(),kind:OperationKind::AnswerQuestions,
        payload:json!({"interaction_id":token,"request_id":native,"answers":[{"kind":"option","question_id":"q","indices":[0]}]})};
    assert_eq!(engine.command(command(json!("0")),clock).unwrap_err().code,InteractionError::Changed.code());
    let effects = engine.command(command(json!(0)),clock).unwrap();
    assert!(effects.iter().any(|effect|matches!(effect,Effect::Write {frame,..} if frame["id"] == 0)));
    assert!(engine.control_view()["pending"].as_array().unwrap().iter().any(|p|p["request_id"] == "approval"));
    assert!(engine.interaction_write_error("answer-zero").is_none());
    let mut replacement = question;
    replacement["params"]["questions"][0]["question"] = json!("Replacement");
    engine.apply(EngineInput::Line(replacement),clock).unwrap();
    assert_eq!(engine.interaction_write_error("answer-zero").unwrap().code,InteractionError::Changed.code());
}

#[test]
fn skip_question_requires_the_displayed_interaction_and_exact_native_target() {
    use hangar_server::runtime::{codex::Engine,protocol::{ClockSample,EngineInput,OperationKind,RuntimeCommand,Effect}};
    let clock = ClockSample {monotonic_s:1.0,epoch_s:1.0};
    let mut engine = Engine::new(json!({"name":"s","thread_id":"sid","initialized":true,"ready":true}),1,clock);
    let store = Arc::new(hangar_server::interactions::InteractionStore::default());
    engine.set_interactions("k-s".into(),store.clone()).unwrap();
    let request = json!({"id":0,"method":"mcpServer/elicitation/request","params":{"threadId":"sid","serverName":"db","mode":"form","message":"Name?",
        "requestedSchema":{"type":"object","properties":{"name":{"type":"string"}}}}});
    engine.apply(EngineInput::Line(request.clone()),clock).unwrap();
    let target = native_target(InteractionKind::Question,"sid",&json!(0));
    let revision = engine.request_revision(&serde_json::from_value(json!(0)).unwrap()).unwrap();
    let old = store.observe_revision(&binding(),&target,Some(request.clone()),Some(revision.clone())).unwrap().unwrap();
    let mut b = request.clone(); b["params"]["message"] = json!("Name B?");
    let current = store.observe_revision(&binding(),&target,Some(b.clone()),Some(revision)).unwrap().unwrap();
    for (interaction_id,request_id,code) in [(Value::Null,json!(0),"erro_interacao_sem_identidade"),
        (json!(old),json!(0),"erro_interacao_desatualizada"),(json!(current),json!("0"),"erro_interacao_desatualizada")] {
        let command = RuntimeCommand {operation_id:"skip".into(),kind:OperationKind::SkipQuestion,payload:json!({"request_id":request_id,"interaction_id":interaction_id})};
        assert_eq!(engine.command(command,clock).unwrap_err().code,code);
    }
    // Volta ao retrato real de A para provar a seleção atual sem completar o ID pelo pedido.
    let current = store.observe_revision(&binding(),&target,Some(request),Some(engine.request_revision(&serde_json::from_value(json!(0)).unwrap()).unwrap())).unwrap().unwrap();
    let effects = engine.command(RuntimeCommand {operation_id:"skip-current".into(),kind:OperationKind::SkipQuestion,
        payload:json!({"request_id":0,"interaction_id":current})},clock).unwrap();
    assert!(effects.iter().any(|effect|matches!(effect,Effect::Write {frame,..} if frame["id"] == 0 && frame["result"]["action"] == "cancel")));
    assert!(engine.interaction_write_error("skip-current").is_none());
}

#[test]
fn invalid_codex_answer_is_a_bad_answer_not_a_send_failure() {
    // Como o `ValueError` do Python: resposta vazia, fora das opções ou de pergunta já respondida é 409
    // `erro_codex_resposta_invalida`; o 503 fica para o envio que falhou.
    let invalid = Err(RuntimeError::new(hangar_server::runtime::codex::INVALID_ANSWER, "resposta vazia"));
    let (status, body) = headless_answer(true, &invalid);
    assert_eq!((status, body["detail"]["code"].as_str()), (StatusCode::CONFLICT, Some("erro_codex_resposta_invalida")));
    let (status, body) = headless_answer(true, &Err(RuntimeError::new("codex_command", "runtime sem terminal indisponível")));
    assert_eq!((status, body["detail"]["code"].as_str()), (StatusCode::SERVICE_UNAVAILABLE, Some("erro_codex_resposta_envio")));
}
