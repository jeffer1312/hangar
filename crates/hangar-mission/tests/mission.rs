//! Mission Control pela API do dono: vínculo, etapas, portão, evidências e configuração das etapas.
use axum::body::{Body, to_bytes};
use axum::extract::ConnectInfo;
use axum::http::{Request, StatusCode};
use hangar_core::auth::{Auth, TrustedHosts};
use hangar_core::diag::DiagClient;
use hangar_core::edge::Edge;
use hangar_mission::Store;
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};
use tower::ServiceExt;

#[derive(Default)]
struct Notices(Mutex<Vec<(String, String)>>, Mutex<Vec<Vec<String>>>, Mutex<std::collections::BTreeMap<String,String>>);

impl hangar_mission::http::Notify for Notices {
    fn identity(&self,session: String) -> std::pin::Pin<Box<dyn std::future::Future<Output=Result<Option<String>,hangar_mission::store::Error>> + Send>> {
        let key=self.2.lock().unwrap().get(&session).cloned();Box::pin(async move {Ok(key)})
    }
    fn groups(&self) -> std::pin::Pin<Box<dyn std::future::Future<Output=Result<Vec<hangar_mission::model::Group>,hangar_mission::store::Error>> + Send>> {
        let keys=self.2.lock().unwrap().clone();
        let groups=self.1.lock().unwrap().iter().enumerate().map(|(index,members)|hangar_mission::model::Group {id:format!("g{index}"),members:members.clone(),keys:keys.clone()}).collect();
        Box::pin(async move {Ok(groups)})
    }
    fn send(&self, session: String, text: String) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>> {
        self.0.lock().unwrap().push((session, text));
        Box::pin(async {})
    }
}

const TOKEN: &str = "dono-da-prova";
const ITEM: &str = "/api/mission/items/help/PM%3A1%2F2";

#[tokio::test]
async fn group_members_share_the_ticket_and_reports_keep_the_real_author() {
    let bench=Bench::new();
    *bench.notices.2.lock().unwrap()=[("coordinator".into(),"coord-life".into()),("backend".into(),"backend-life".into()),("frontend".into(),"frontend-life".into())].into();
    bench.link("coordinator").await;
    *bench.notices.1.lock().unwrap()=vec![vec!["coordinator".into(),"backend".into()]];
    let (status,snapshot)=bench.send("GET","/api/mission",None).await;
    assert_eq!(status,StatusCode::OK);
    assert_eq!(snapshot["items"][0]["sessions"],json!(["backend","coordinator"]));
    let (status,saved)=bench.send("PATCH",ITEM,Some(json!({"action":"repository","path":"/projects/server","key":"PM-1","title":"Imprimir","url":"https://help.invalid/PM-1"}))).await;
    assert_eq!(status,StatusCode::OK);
    assert_eq!(saved["item"]["sessions"],json!(["backend","coordinator"]),"salvar o repositório conserva participantes na resposta do PATCH");
    let (status,reply)=bench.send("POST","/api/mission/report",Some(json!({"session":"backend","key":"backend-life","stage":"execucao","note":"Servidor implementado"}))).await;
    assert_eq!(status,StatusCode::OK,"{reply}");
    assert_eq!(reply["item"]["session"],"coordinator");
    assert_eq!(reply["item"]["history"].as_array().unwrap().last().unwrap()["by"],"backend");
    bench.notices.1.lock().unwrap()[0].push("frontend".into());
    let (status,_)=bench.send("POST","/api/mission/report",Some(json!({"session":"frontend","key":"frontend-life","note":"Interface implementada"}))).await;
    assert_eq!(status,StatusCode::OK,"um membro criado depois também participa");
    bench.notices.1.lock().unwrap().clear();
    let (status,_)=bench.send("POST","/api/mission/report",Some(json!({"session":"frontend","key":"frontend-life","note":"Já saiu"}))).await;
    assert_eq!(status,StatusCode::OK,"sair do grupo conserva o trabalho associado");
}

#[tokio::test]
async fn repository_is_saved_on_its_ticket_before_any_session_exists() {
    let bench=Bench::new();
    let body=json!({"action":"repository","path":"/projects/server","key":"PM-1","title":"Imprimir","url":"https://help.invalid/PM-1"});
    let (status,reply)=bench.send("PATCH",ITEM,Some(body)).await;
    assert_eq!(status,StatusCode::OK,"{reply}");
    assert!(reply["item"]["session"].is_null());
    bench.link("coordinator").await;
    let (_,snapshot)=bench.send("GET","/api/mission",None).await;
    assert_eq!(snapshot["items"][0]["repository"],"/projects/server");
    assert!(bench.notices.0.lock().unwrap().iter().all(|(_,text)|!text.contains("/projects/server")),"salvar a pasta não envia ação");
}

#[tokio::test]
async fn participant_identity_survives_the_coordinator_and_rejects_a_reused_name() {
    let bench=Bench::new();
    let (coordinator,backend,recreated)=("a".repeat(32),"b".repeat(32),"c".repeat(32));
    *bench.notices.1.lock().unwrap()=vec![vec!["coordinator".into(),"backend".into()]];
    *bench.notices.2.lock().unwrap()=[("coordinator".into(),coordinator),("backend".into(),backend.clone())].into();
    bench.link("coordinator").await;
    let uri=format!("/api/mission/session?session=backend&key={backend}&group_id=g0");
    assert_eq!(bench.send("GET",&uri,None).await.0,StatusCode::OK);
    *bench.notices.1.lock().unwrap()=vec![vec!["backend".into()]];
    assert_eq!(bench.send("GET",&uri,None).await.0,StatusCode::OK,"o vínculo permanece sem a coordenadora");
    bench.notices.2.lock().unwrap().insert("coordinator".into(),recreated.clone());
    bench.notices.1.lock().unwrap().push(vec!["coordinator".into(),"unrelated".into()]);
    assert_eq!(bench.send("GET",&format!("/api/mission/session?session=coordinator&key={recreated}&group_id=g1"),None).await.0,StatusCode::NOT_FOUND);
    let (_,snapshot)=bench.send("GET","/api/mission",None).await;
    assert!(!snapshot["items"][0]["sessions"].as_array().unwrap().contains(&json!("unrelated")));
    assert_eq!(bench.send("PATCH",ITEM,Some(json!({"action":"stage","stage":"validacao"}))).await.0,StatusCode::OK);
    assert_eq!(bench.send("PATCH",ITEM,Some(json!({"action":"approve","stage":"validacao"}))).await.0,StatusCode::OK);
    let (status,_) = bench.send("PATCH",ITEM,Some(json!({"action":"delivered"}))).await;
    assert_eq!(status,StatusCode::OK);
    let (status,reply)=bench.send("POST","/api/mission/report",Some(json!({"session":"backend","key":backend,"note":"Nota final depois da entrega"}))).await;
    assert_eq!(status,StatusCode::OK,"{reply}");
}

#[tokio::test]
async fn manual_adoption_captures_life_before_the_first_group() {
    let bench=Bench::new();
    bench.notices.2.lock().unwrap().insert("coordinator".into(),"original-life".into());
    let linked=bench.link("coordinator").await;
    assert_eq!(linked["session_key"],"original-life");
    bench.notices.2.lock().unwrap().insert("coordinator".into(),"new-life".into());
    *bench.notices.1.lock().unwrap()=vec![vec!["coordinator".into(),"unrelated".into()]];
    assert_eq!(bench.send("GET","/api/mission/session?session=coordinator&key=new-life&group_id=g0",None).await.0,StatusCode::NOT_FOUND);
    let (_,snapshot)=bench.send("GET","/api/mission",None).await;
    assert!(!snapshot["items"][0]["sessions"].as_array().unwrap().contains(&json!("unrelated")));
}

#[tokio::test]
async fn explicit_reassignment_moves_the_group_and_a_recreated_worker_can_rejoin() {
    let bench=Bench::new();
    *bench.notices.2.lock().unwrap()=[("coordinator".into(),"coord-life".into()),("backend".into(),"worker-life".into())].into();
    *bench.notices.1.lock().unwrap()=vec![vec!["coordinator".into(),"backend".into()]];
    bench.link("coordinator").await;
    let uri="/api/mission/items/help/T-2";
    let body=json!({"key":"T-2","title":"Outro trabalho","url":"https://example.invalid/2","session":"backend","session_key":"worker-life"});
    assert_eq!(bench.send("PUT",uri,Some(body.clone())).await.0,StatusCode::OK);
    let (_,snapshot)=bench.send("GET","/api/mission",None).await;
    let previous:hangar_mission::model::Item=serde_json::from_value(snapshot["items"][0].clone()).unwrap();
    assert!(previous.sessions.is_empty());assert!(previous.group_ids.is_empty());assert!(previous.session.is_none());
    let (status,read)=bench.send("GET","/api/mission/session?session=coordinator&key=coord-life&group_id=g0",None).await;
    assert_eq!(status,StatusCode::OK);assert_eq!(read["item"]["key"],"T-2");
    bench.notices.2.lock().unwrap().insert("backend".into(),"worker-recreated".into());
    assert_eq!(bench.send("GET","/api/mission/session?session=backend&key=worker-recreated&group_id=g0",None).await.0,StatusCode::OK);
    assert_eq!(bench.send("GET","/api/mission/session?session=backend&key=worker-life&group_id=g0",None).await.0,StatusCode::NOT_FOUND);
    assert_eq!(bench.send("PUT",uri,Some(body)).await.0,StatusCode::CONFLICT,"a escolha antiga não associa outra vida");
}

#[test]
fn delayed_report_cannot_undo_a_stage_change_or_restore_an_old_association() {
    use hangar_mission::store::{Link, OwnerChange, Report, ReportBinding};
    let dir=tempfile::tempdir().unwrap();
    let store=Store::new(dir.path().join("mission"));
    let link=||Link {key:"T-1".into(),title:"Trabalho".into(),url:"https://example.invalid/1".into(),session:"worker".into()};
    let (item,_)=store.link_owner("help","1",link(),Some("life-1"),&[]).unwrap();
    let revision=item.history.len();
    store.owner_change("help","1",OwnerChange::Stage {stage:"execucao".into()}).unwrap();
    let report=||Report {stage:Some("planejamento".into()),note:Some("Relato atrasado".into()),evidence:Vec::new()};
    let binding=||Some(ReportBinding {session_key:Some("life-1".into()),history_revision:revision});
    assert_eq!(store.report_imported("help","1","worker",report(),Vec::new(),binding()).unwrap_err().code,"ticket_stage_changed");
    let note=Report {stage:None,note:Some("Nota complementar".into()),evidence:Vec::new()};
    assert_eq!(store.report_imported("help","1","worker",note,Vec::new(),binding()).unwrap().stage.as_deref(),Some("execucao"));
    store.link_owner("help","2",link(),Some("life-1"),&[]).unwrap();
    assert_eq!(store.report_imported("help","1","worker",report(),Vec::new(),binding()).unwrap_err().code,"ticket_association_changed");
}

struct Bench { dir: tempfile::TempDir, app: axum::Router, notices: Arc<Notices> }

impl Bench {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let edge = Edge {
            trusted: TrustedHosts::parse("127.0.0.1"),
            auth: Auth::configured(TOKEN),
            diag: DiagClient::new("127.0.0.1:9".parse().unwrap(), String::new()).with_diary(Some(dir.path().join("diary"))),
        };
        let notices = Arc::new(Notices::default());
        let app = hangar_mission::http::router::<()>(edge, Arc::new(Store::new(dir.path().join("mission"))), notices.clone());
        Self { dir, app, notices }
    }

    async fn raw(&self, method: &str, uri: &str, token: Option<&str>, body: Body) -> (StatusCode, Vec<u8>, Option<String>) {
        let mut request = Request::builder().method(method).uri(uri);
        if let Some(token) = token { request = request.header("authorization", format!("Bearer {token}")); }
        let mut request = request.header("content-type", "application/json").body(body).unwrap();
        request.extensions_mut().insert(ConnectInfo("127.0.0.1:40000".parse::<std::net::SocketAddr>().unwrap()));
        let response = self.app.clone().oneshot(request).await.unwrap();
        let status = response.status();
        let mime = response.headers().get("content-type").and_then(|v| v.to_str().ok()).map(str::to_owned);
        (status, to_bytes(response.into_body(), 64 * 1024 * 1024).await.unwrap().to_vec(), mime)
    }

    async fn send(&self, method: &str, uri: &str, body: Option<Value>) -> (StatusCode, Value) {
        let body = body.map_or_else(Body::empty, |body| Body::from(body.to_string()));
        let (status, bytes, _) = self.raw(method, uri, Some(TOKEN), body).await;
        (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
    }

    async fn link(&self, session: &str) -> Value {
        let (status, body) = self.send("PUT", ITEM, Some(json!({"key": "PM-1", "title": "Imprimir", "url": "https://help.invalid/pm/PM-1", "session": session}))).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        body["item"].clone()
    }

    /// Os avisos saem numa tarefa à parte: espera até chegarem `n`.
    async fn notices(&self, n: usize) -> Vec<(String, String)> {
        for _ in 0..200 {
            let seen = self.notices.0.lock().unwrap().clone();
            if seen.len() >= n { return seen; }
            tokio::task::yield_now().await;
        }
        self.notices.0.lock().unwrap().clone()
    }

    fn file(&self, name: &str, bytes: &[u8]) -> String {
        let path = self.dir.path().join(name);
        std::fs::write(&path, bytes).unwrap();
        path.to_string_lossy().into_owned()
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn empty_server_offers_the_default_stages_and_refuses_strangers() {
    let bench = Bench::new();
    let (status, body) = bench.send("GET", "/api/mission", None).await;
    assert_eq!(status, StatusCode::OK);
    let ids: Vec<&str> = body["stages"].as_array().unwrap().iter().map(|s| s["id"].as_str().unwrap()).collect();
    assert_eq!(ids, ["planejamento", "execucao", "revisao", "validacao", "entrega"]);
    assert_eq!(body["items"], json!([]));
    for token in [None, Some("errado")] {
        let (status, _, _) = bench.raw("GET", "/api/mission", token, Body::empty()).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
    }
}

#[tokio::test]
async fn stage_workflow_prompt_is_optional_and_survives_a_round_trip() {
    let bench = Bench::new();
    let (_, current) = bench.send("GET", "/api/mission", None).await;
    let mut stages = current["stages"].clone();
    stages[0]["prompt"] = json!("Confira o fluxo atual antes de propor a implementação.");
    let (status, body) = bench.send("PUT", "/api/mission/stages", Some(json!({"stages": stages}))).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (_, stored) = bench.send("GET", "/api/mission", None).await;
    assert_eq!(stored["stages"][0]["prompt"], json!("Confira o fluxo atual antes de propor a implementação."));
    assert!(stored["stages"][1].get("prompt").is_none_or(Value::is_null));
}

#[tokio::test]
async fn stage_workflow_configuration_survives_rename_reorder_and_an_older_client() {
    let bench = Bench::new();
    let (_, current) = bench.send("GET", "/api/mission", None).await;
    let mut configured = current["stages"].clone();
    configured[0]["prompt"] = json!("Confira o fluxo atual.");
    configured[0]["actions"] = json!({"help": ["start-pm"]});
    let (status, _) = bench.send("PUT", "/api/mission/stages", Some(json!({"stages": configured}))).await;
    assert_eq!(status, StatusCode::OK);
    let (_, stored) = bench.send("GET", "/api/mission", None).await;
    assert_eq!(stored["stages"][0]["actions"]["help"], json!(["start-pm"]));
    let mut older = current["stages"].as_array().unwrap().clone();
    older[0]["name"] = json!("Entendimento");
    older.swap(0, 1);
    let (status, body) = bench.send("PUT", "/api/mission/stages", Some(json!({"stages": older}))).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let stage = body["stages"].as_array().unwrap().iter().find(|s| s["id"] == "planejamento").unwrap();
    assert_eq!(stage["name"], json!("Entendimento"));
    assert_eq!(stage["prompt"], json!("Confira o fluxo atual."));
    assert_eq!(stage["actions"]["help"], json!(["start-pm"]));
}

#[tokio::test]
async fn stage_workflow_config_rejects_invalid_values_and_can_be_cleared_explicitly() {
    let bench = Bench::new();
    let (_, current) = bench.send("GET", "/api/mission", None).await;
    let mut stages = current["stages"].clone();
    stages[0]["prompt"] = json!("Instrução opcional.");
    stages[0]["actions"] = json!({"help": ["start-pm"]});
    bench.send("PUT", "/api/mission/stages", Some(json!({"stages": stages}))).await;
    for (field, invalid) in [("prompt", json!("x".repeat(4001))), ("prompt", json!("a\u{0000}b")),
        ("actions", json!({"../conector": ["start-pm"]})), ("actions", json!({"help": ["../acao"]})),
        ("actions", json!({"help": ["start-pm", "start-pm"]}))] {
        let mut bad = stages.clone();
        bad[0][field] = invalid;
        let (status, _) = bench.send("PUT", "/api/mission/stages", Some(json!({"stages": bad}))).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "campo {field}");
    }
    let (_, stored) = bench.send("GET", "/api/mission", None).await;
    assert_eq!(stored["stages"][0]["prompt"], json!("Instrução opcional."));
    stages[0]["prompt"] = json!("");
    stages[0]["actions"] = json!({});
    let (status, cleared) = bench.send("PUT", "/api/mission/stages", Some(json!({"stages": stages}))).await;
    assert_eq!(status, StatusCode::OK);
    assert!(cleared["stages"][0]["prompt"].as_str().unwrap_or_default().is_empty());
    assert_eq!(cleared["stages"][0]["actions"], json!({}));
}

#[tokio::test]
async fn stage_workflow_migrates_the_legacy_file_without_losing_ticket_history() {
    let bench = Bench::new();
    bench.link("pm-1").await;
    let path = bench.dir.path().join("mission/mission.json");
    let mut legacy: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    legacy["version"] = json!(1);
    std::fs::write(&path, serde_json::to_vec(&legacy).unwrap()).unwrap();
    let (status, snapshot) = bench.send("GET", "/api/mission", None).await;
    assert_eq!(status, StatusCode::OK);
    let mut stages = snapshot["stages"].clone();
    stages[0]["prompt"] = json!("Contexto opcional.");
    let (status, _) = bench.send("PUT", "/api/mission/stages", Some(json!({"stages": stages}))).await;
    assert_eq!(status, StatusCode::OK);
    let saved: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    assert_eq!(saved["version"], json!(3), "o marcador impede que um servidor antigo descarte campos novos");
    let (_, snapshot) = bench.send("GET", "/api/mission", None).await;
    let item = snapshot["items"].as_array().unwrap().iter().find(|i| i["session"] == "pm-1").unwrap();
    assert_eq!(item["key"], json!("PM-1"));
    assert!(item["history"].as_array().unwrap().iter().any(|event| event["kind"] == "linked"));
}

#[tokio::test(flavor = "multi_thread")]
async fn the_agent_moves_stages_but_waits_for_the_owner_at_the_gate() {
    let bench = Bench::new();
    let item = bench.link("pm-1").await;
    assert_eq!((item["key"].clone(), item["session"].clone()), (json!("PM-1"), json!("pm-1")));

    let report = |stage: &str| json!({"session": "pm-1", "stage": stage, "note": format!("indo para {stage}")});
    let (status, body) = bench.send("POST", "/api/mission/report", Some(report("validacao"))).await;
    assert_eq!((status, body["item"]["stage"].clone()), (StatusCode::OK, json!("validacao")), "entrar no portão é livre");

    let (status, body) = bench.send("POST", "/api/mission/report", Some(report("entrega"))).await;
    assert_eq!((status, body["detail"]["code"].clone()), (StatusCode::CONFLICT, json!("mission_gate_pending")), "{body}");

    let (status, body) = bench.send("PATCH", ITEM, Some(json!({"action": "approve"}))).await;
    assert_eq!((status, body["item"]["stage"].clone()), (StatusCode::OK, json!("entrega")), "aprovar leva à etapa seguinte");
    assert_eq!(body["item"]["approved"], json!(["validacao"]));

    let (status, body) = bench.send("PATCH", ITEM, Some(json!({"action": "changes", "note": "o print não mostra o botão"}))).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["item"]["stage"], json!("validacao"), "recusar volta uma etapa");
    assert_eq!(body["item"]["approved"], json!([]), "voltar para o portão fecha o portão");
    let (status, body) = bench.send("POST", "/api/mission/report", Some(report("entrega"))).await;
    assert_eq!((status, body["detail"]["code"].clone()), (StatusCode::CONFLICT, json!("mission_gate_pending")), "{body}");
    let (status, body) = bench.send("PATCH", ITEM, Some(json!({"action": "changes", "note": "ainda falta"}))).await;
    assert_eq!((status, body["item"]["stage"].clone(), body["item"]["approved"].clone()), (StatusCode::OK, json!("revisao"), json!([])));

    let (status, body) = bench.send("POST", "/api/mission/report", Some(json!({"session": "outra", "stage": "execucao"}))).await;
    assert_eq!((status, body["detail"]["code"].clone()), (StatusCode::NOT_FOUND, json!("mission_session_not_linked")));
    let (status, body) = bench.send("POST", "/api/mission/report", Some(json!({"session": "pm-1", "stage": "inventada"}))).await;
    assert_eq!((status, body["detail"]["code"].clone()), (StatusCode::BAD_REQUEST, json!("invalid_params")));
}

#[tokio::test(flavor = "multi_thread")]
async fn evidence_is_copied_served_and_removed_with_the_ticket() {
    let bench = Bench::new();
    bench.link("pm-1").await;
    let png = bench.file("tela.png", b"\x89PNG prova");
    let video = bench.file("fluxo.mp4", b"mp4 de prova");
    let script = bench.file("rodar.sh", b"echo");

    let (status, body) = bench.send("POST", "/api/mission/report", Some(json!({"session": "pm-1",
        "evidence": [{"path": png, "caption": "tela de impressão"}, {"path": video}]}))).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let evidence = body["item"]["evidence"].as_array().unwrap().clone();
    assert_eq!(evidence.len(), 2);
    assert_eq!((evidence[0]["mime"].clone(), evidence[0]["caption"].clone(), evidence[0]["by"].clone()),
        (json!("image/png"), json!("tela de impressão"), json!("pm-1")));
    std::fs::remove_file(&png).unwrap();

    let id = evidence[0]["id"].as_str().unwrap();
    let (status, bytes, mime) = bench.raw("GET", &format!("/api/mission/evidence/{id}"), Some(TOKEN), Body::empty()).await;
    assert_eq!((status, bytes.as_slice(), mime.as_deref()), (StatusCode::OK, &b"\x89PNG prova"[..], Some("image/png")), "a cópia sobrevive ao original");

    let (status, body) = bench.send("POST", "/api/mission/report", Some(json!({"session": "pm-1", "evidence": [{"path": script}]}))).await;
    assert_eq!((status, body["detail"]["code"].clone()), (StatusCode::BAD_REQUEST, json!("invalid_params")), "só imagem, vídeo, PDF ou texto");
    let (status, _) = bench.send("POST", "/api/mission/report", Some(json!({"session": "pm-1", "evidence": [{"path": "relativo.png"}]}))).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    let (status, bytes, _) = bench.raw("POST", &format!("{ITEM}/evidence?name=gravado.webm&caption=do%20dono"), Some(TOKEN), Body::from("webm")).await;
    assert_eq!(status, StatusCode::OK, "{}", String::from_utf8_lossy(&bytes));
    let body: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(body["item"]["evidence"][2]["by"], json!("dono"));

    let (status, _) = bench.send("DELETE", ITEM, None).await;
    assert_eq!(status, StatusCode::OK);
    let (status, _, _) = bench.raw("GET", &format!("/api/mission/evidence/{id}"), Some(TOKEN), Body::empty()).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let left = std::fs::read_dir(bench.dir.path().join("mission/evidence")).unwrap().count();
    assert_eq!(left, 0, "apagar o ticket leva os arquivos");
}

#[tokio::test(flavor = "multi_thread")]
async fn one_session_one_ticket_and_custom_stages_drop_what_left() {
    let bench = Bench::new();
    bench.link("pm-1").await;
    let (status, _) = bench.send("PUT", "/api/mission/items/help/PM-2", Some(json!({"key": "PM-2", "title": "", "url": "https://help.invalid/pm/PM-2", "session": "pm-1"}))).await;
    assert_eq!(status, StatusCode::OK);
    let (_, body) = bench.send("GET", "/api/mission", None).await;
    let sessions: Vec<Value> = body["items"].as_array().unwrap().iter().map(|i| i["session"].clone()).collect();
    assert_eq!(sessions, [Value::Null, json!("pm-1")], "a sessão sai do ticket anterior");

    bench.send("PATCH", "/api/mission/items/help/PM-2", Some(json!({"action": "stage", "stage": "revisao"}))).await;
    let stages = json!({"stages": [{"id": "fazer", "name": "Fazer"}, {"id": "conferir", "name": "Conferir", "gate": true, "statuses": ["Em Testes"]}]});
    let (status, body) = bench.send("PUT", "/api/mission/stages", Some(stages)).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["items"][1]["stage"], Value::Null, "etapa que saiu volta a ser deduzida");
    assert_eq!(body["stages"][1]["statuses"], json!(["Em Testes"]));

    for bad in [json!({"stages": []}), json!({"stages": [{"id": "Maiúscula", "name": "x"}]}), json!({"stages": [{"id": "a", "name": "x"}, {"id": "a", "name": "y"}]})] {
        let (status, body) = bench.send("PUT", "/api/mission/stages", Some(bad)).await;
        assert_eq!((status, body["detail"]["code"].clone()), (StatusCode::BAD_REQUEST, json!("invalid_params")));
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn linking_and_requested_changes_reach_the_session_only_when_saved() {
    let bench = Bench::new();
    bench.link("pm-1").await;
    bench.link("pm-1").await;
    let notices = bench.notices(1).await;
    assert_eq!(notices.len(), 1, "religar a mesma sessão não repete o aviso");
    assert!(notices[0].0 == "pm-1" && notices[0].1.contains("PM-1") && notices[0].1.contains("`mission`"), "{notices:?}");

    // Etapa só deduzida na tela: o dono aprova e recusa a etapa que viu.
    let (status, body) = bench.send("PATCH", ITEM, Some(json!({"action": "changes", "note": "falta o print"}))).await;
    assert_eq!((status, body["detail"]["code"].clone()), (StatusCode::BAD_REQUEST, json!("invalid_params")));
    assert_eq!(bench.notices(2).await.len(), 1, "recusa sem gravar não manda nada");
    let (status, body) = bench.send("PATCH", ITEM, Some(json!({"action": "approve", "stage": "validacao"}))).await;
    assert_eq!((status, body["item"]["stage"].clone()), (StatusCode::OK, json!("entrega")), "{body}");
    let (status, body) = bench.send("PATCH", ITEM, Some(json!({"action": "changes", "note": "o vídeo cortou", "stage": "revisao"}))).await;
    assert_eq!((status, body["item"]["stage"].clone()), (StatusCode::OK, json!("validacao")), "a etapa gravada vence a mostrada");
    let notices = bench.notices(2).await;
    assert_eq!(notices[1], ("pm-1".to_owned(), "Ajustes pedidos pelo dono no ticket PM-1, na etapa Entrega: o vídeo cortou".to_owned()));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_failing_batch_leaves_no_copy_and_same_names_never_collide() {
    let bench = Bench::new();
    bench.link("pm-1").await;
    let (a, b) = (bench.dir.path().join("a"), bench.dir.path().join("b"));
    std::fs::create_dir_all(&a).unwrap();
    std::fs::create_dir_all(&b).unwrap();
    std::fs::write(a.join("tela.png"), b"primeira").unwrap();
    std::fs::write(b.join("tela.png"), b"segunda").unwrap();
    let missing = bench.dir.path().join("sumiu.png");

    let (status, _) = bench.send("POST", "/api/mission/report", Some(json!({"session": "pm-1",
        "evidence": [{"path": a.join("tela.png")}, {"path": missing}]}))).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let dir = bench.dir.path().join("mission/evidence");
    assert_eq!(std::fs::read_dir(&dir).map_or(0, |d| d.count()), 0, "a cópia do primeiro sai com a falha do segundo");

    let (status, body) = bench.send("POST", "/api/mission/report", Some(json!({"session": "pm-1",
        "evidence": [{"path": a.join("tela.png")}, {"path": b.join("tela.png")}]}))).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let ids: Vec<String> = body["item"]["evidence"].as_array().unwrap().iter().map(|e| e["id"].as_str().unwrap().to_owned()).collect();
    assert_ne!(ids[0], ids[1]);
    for (id, expected) in ids.iter().zip([&b"primeira"[..], &b"segunda"[..]]) {
        let (_, bytes, _) = bench.raw("GET", &format!("/api/mission/evidence/{id}"), Some(TOKEN), Body::empty()).await;
        assert_eq!(bytes, expected);
    }
    let (status, body) = bench.send("POST", "/api/mission/report", Some(json!({"session": "outra", "evidence": [{"path": a.join("tela.png")}]}))).await;
    assert_eq!((status, body["detail"]["code"].clone()), (StatusCode::NOT_FOUND, json!("mission_session_not_linked")));
    assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 2, "sessão sem ticket não copia nada");
}

#[tokio::test(flavor = "multi_thread")]
async fn the_agent_going_back_before_a_gate_closes_it() {
    let bench = Bench::new();
    bench.link("pm-1").await;
    let report = |stage: &str| json!({"session": "pm-1", "stage": stage});
    bench.send("POST", "/api/mission/report", Some(report("validacao"))).await;
    bench.send("PATCH", ITEM, Some(json!({"action": "approve"}))).await;
    let (status, body) = bench.send("POST", "/api/mission/report", Some(report("execucao"))).await;
    assert_eq!((status, body["item"]["approved"].clone()), (StatusCode::OK, json!([])), "{body}");
    let (status, body) = bench.send("POST", "/api/mission/report", Some(report("entrega"))).await;
    assert_eq!((status, body["detail"]["code"].clone()), (StatusCode::CONFLICT, json!("mission_gate_pending")));
}
