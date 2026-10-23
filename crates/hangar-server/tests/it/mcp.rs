//! Servidor MCP `hangar` em `/mcp/`, pelo transporte HTTP: o catálogo do Python, a identidade de quem
//! chama (`quem_chama.py`) e cada tool chamando a mesma rota que o `hangar-send`/`hangar-preview`.
use crate::{fake, list_support};

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use fake::{OWNER, SECRET, client};
use hangar_server::list::bridge::{ListBridge, ListEnv, parse_dirs};
use hangar_server::list::facts::FactsClient;
use hangar_server::list::mux::Mux;
use hangar_server::routes::{AppState, router};
use serde_json::{Value, json};

const PYTHON_CATALOG: &str = include_str!("fixtures/mcp_catalog_python.json");
const PYTHON_INSTRUCTIONS: &str = "Hangar: sessões vizinhas e o navegador embutido desta sessão. Mostrar algo que você montou ao usuário (desenho, mock de tela, comparação, gráfico, tabela, diagrama): html_render, sem esperar pedido; imagem por caminho só para print real de tela. Ver ou testar uma página: browser_open e depois browser (um verbo) ou browser_batch (vários já conhecidos); tela desconhecida com várias ações: browser_goal, quando a sessão tem o Jev. Falar com outra sessão: sessions, send, group, pair; abrir ou fechar: new_session, close_session.";

struct Scene { addr: SocketAddr, python: Arc<fake::Fake>, home: PathBuf, _dir: tempfile::TempDir }

/// Sessões `s0` (pane `%0`) e `s1` (`%1`) com conversa; `beta` com os panes `%1` (repetido, como no
/// psmux) e `%7`; `sem-term` sem terminal, de chave `k123`; o Python falso atrás.
async fn scene() -> Scene { scene_with(false).await }

/// `groups`: o serviço de grupo do Rust sobre `home/.claude/.hangar-pair` (sem ele, grupo segue ao Python).
async fn scene_with(groups: bool) -> Scene {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    list_support::sessions(root, 2);
    let (w0, w1) = (root.join("w0"), root.join("w1"));
    let script = root.join("tmux");
    crate::write_executable(&script, format!(
        "#!/bin/sh\n[ \"$1\" = show-environment ] && [ \"${{3#=}}\" = s1 ] && {{ printf 'CP_SESSION_KEY=fedcba9876543210fedcba9876543210\\n'; exit 0; }}\n[ \"$1\" = show-environment ] && {{ [ \"${{3#=}}\" = s0 ] || [ \"${{3#=}}\" = beta ]; }} && {{ printf 'CP_SESSION_KEY=dddddddddddddddddddddddddddddddd\\n'; exit 0; }}\n[ \"$1\" = list-panes ] || {{ printf 'ok\\n'; exit 0; }}\nprintf 's0\\t1\\t\\t{}\\t%%0\\t\\t\\t\\t0\\t0\\ns1\\t1\\t\\t{}\\t%%1\\t\\t\\t\\t0\\t0\\nbeta\\t1\\t\\t{}\\t%%1\\t\\t\\t\\t0\\t0\\nbeta\\t1\\t\\t{}\\t%%7\\t\\t\\t\\t0\\t1\\n'\n",
        w0.display(), w1.display(), w1.display(), w1.display()));
    let home = root.join("home");
    write_json(&home.join(".hangar/claude-headless/sem-term.json"), json!({"name": "sem-term", "key": "k123", "session_id": "s"}));
    let dirs = parse_dirs(&json!({"home": home, "claude": home.join(".claude"), "codex_home": home.join(".codex"),
        "pi_sessions": home.join(".pi/agent/sessions"), "omp_config": home.join(".omp"),
        "omp_agent": home.join(".omp/agent"), "kimi_home": home.join(".kimi-code")}).to_string());
    let (python, upstream) = fake::spawn_fake().await;
    let mut state = AppState::new(fake::config(upstream, "127.0.0.1"));
    let list = Arc::new(ListBridge::new(ListEnv { mux: Mux::with_program(&script, Duration::from_secs(5)),
        capture_program: script.into_os_string(), procs: Arc::new(hangar_server::list::procs::SystemProcs::default()), dirs },
        FactsClient::new(upstream, SECRET.into())));
    state.list = list.clone();
    state.side.locate = Arc::new(hangar_server::locate::ListLocate { list });
    state.pages = Arc::new(hangar_server::pages::store::Store::new(root.join("paginas")));
    state.chromium = || None;
    if groups {
        use hangar_server::groups::{orq::PythonOrq, service::GroupService, store::PairDir};
        let pair = PairDir::new(home.join(".claude/.hangar-pair"), root.join("arquivo"));
        state.groups = Some(Arc::new(GroupService::new(pair, Arc::new(PythonOrq::from_state(&state)), "casa".into())));
    }
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let app = router(Arc::new(state)).into_make_service_with_connect_info::<SocketAddr>();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    Scene { addr, python, home, _dir: dir }
}

fn write_json(path: &Path, value: Value) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, value.to_string()).unwrap();
}

/// Cliente MCP mínimo do transporte HTTP streamable: `initialize`, avisos e pedidos com resposta.
struct Mcp { addr: SocketAddr, headers: Vec<(String, String)>, session: Option<String>, next: u64 }

impl Mcp {
    async fn open(addr: SocketAddr, headers: &[(&str, &str)]) -> Mcp {
        let mut mcp = Mcp { addr, headers: headers.iter().map(|(k, v)| ((*k).to_owned(), (*v).to_owned())).collect(), session: None, next: 0 };
        let init = mcp.request("initialize", json!({"protocolVersion": "2025-06-18", "capabilities": {},
            "clientInfo": {"name": "teste", "version": "0"}})).await;
        // Tools que nasceram depois do Python entram no fim do texto dele.
        let instructions = init["result"]["instructions"].as_str().unwrap_or_default();
        assert!(instructions.starts_with(PYTHON_INSTRUCTIONS.trim_end_matches('.')), "{init}");
        let accepted = mcp.post(json!({"jsonrpc": "2.0", "method": "notifications/initialized"})).send().await.unwrap();
        assert_eq!(accepted.status(), 202);
        mcp
    }

    fn post(&self, body: Value) -> reqwest::RequestBuilder {
        let mut req = client().post(format!("http://{}/mcp/", self.addr)).bearer_auth(OWNER)
            .header("accept", "application/json, text/event-stream").header("content-type", "application/json")
            .header("mcp-protocol-version", "2025-06-18");
        if let Some(session) = &self.session { req = req.header("mcp-session-id", session); }
        for (k, v) in &self.headers { req = req.header(k, v); }
        req.body(body.to_string())
    }

    async fn request(&mut self, method: &str, params: Value) -> Value {
        self.next += 1;
        let id = self.next;
        let resp = self.post(json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params})).send().await.unwrap();
        if let Some(session) = resp.headers().get("mcp-session-id") { self.session = Some(session.to_str().unwrap().to_owned()); }
        let text = resp.text().await.unwrap();
        text.lines().filter_map(|l| l.strip_prefix("data:")).filter_map(|d| serde_json::from_str::<Value>(d.trim()).ok())
            .chain(serde_json::from_str::<Value>(&text).ok())
            .find(|m| m["id"] == id).unwrap_or_else(|| panic!("sem resposta para {method}: {text}"))
    }

    async fn call(&mut self, tool: &str, args: Value) -> Value {
        let answer = self.request("tools/call", json!({"name": tool, "arguments": args})).await;
        assert!(answer["result"].is_object(), "{tool}: {answer}");
        answer["result"].clone()
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn stateless_lists_include_required_cache_metadata() {
    let s = scene().await;
    for (method, field) in [("tools/list", "tools"), ("resources/list", "resources"),
        ("resources/templates/list", "resourceTemplates"), ("prompts/list", "prompts")] {
        let resp = client().post(format!("http://{}/mcp/", s.addr)).bearer_auth(OWNER)
            .header("accept", "application/json, text/event-stream")
            .header("content-type", "application/json")
            .header("mcp-protocol-version", "2026-07-28").header("Mcp-Method", method)
            .body(json!({"jsonrpc": "2.0", "id": 2, "method": method, "params": {"_meta": {
                "io.modelcontextprotocol/protocolVersion": "2026-07-28",
                "io.modelcontextprotocol/clientCapabilities": {}}}}).to_string())
            .send().await.unwrap();
        assert_eq!(resp.status(), 200, "{method}");
        let text = resp.text().await.unwrap();
        let reply: Value = text.lines().filter_map(|l| l.strip_prefix("data:"))
            .filter_map(|d| serde_json::from_str(d.trim()).ok())
            .chain(serde_json::from_str(&text).ok())
            .find(|m: &Value| m["id"] == 2).unwrap_or_else(|| panic!("{method}: {text}"));
        let result = &reply["result"];
        assert_eq!(result["ttlMs"], 0, "{method}: {reply}");
        assert_eq!(result["cacheScope"], "private", "{method}: {reply}");
        assert_eq!(result["resultType"], "complete", "{method}: {reply}");
        let items = result[field].as_array().unwrap_or_else(|| panic!("{method}: {reply}"));
        if method == "tools/list" {
            assert!(items.iter().any(|t| t["name"] == "who_am_i"));
        } else {
            assert!(items.is_empty(), "{method}: {reply}");
        }
    }
    assert_eq!(s.python.hits_to("/mcp/"), 0);
}

fn failed(result: &Value) -> bool { result["isError"] == true }
fn first_text(result: &Value) -> String { result["content"][0]["text"].as_str().unwrap_or_default().to_owned() }

async fn caller(s: &Scene, headers: &[(&str, &str)]) -> Result<Value, String> {
    let mut mcp = Mcp::open(s.addr, headers).await;
    let result = mcp.call("who_am_i", json!({})).await;
    if failed(&result) { Err(first_text(&result)) } else { Ok(result["structuredContent"].clone()) }
}

#[tokio::test(flavor = "multi_thread")]
async fn bearer_and_this_machine_only() {
    let s = scene().await;
    let post = || client().post(format!("http://{}/mcp/", s.addr)).header("content-type", "application/json").body("{}");
    assert_eq!(post().send().await.unwrap().status(), 401);
    assert_eq!(post().bearer_auth("outro").send().await.unwrap().status(), 401);
    assert_eq!(post().bearer_auth(OWNER).header("x-forwarded-for", "10.0.0.5").send().await.unwrap().status(), 403);
    assert_eq!(s.python.hits_to("/mcp/"), 0, "o /mcp não segue ao Python");
}

#[tokio::test(flavor = "multi_thread")]
async fn catalog_is_the_python_one_with_the_whole_open_request() {
    let s = scene().await;
    let mut mcp = Mcp::open(s.addr, &[]).await;
    let listed = mcp.request("tools/list", json!({})).await["result"]["tools"].clone();
    // Tools que nasceram depois do Python (ele não tem mais MCP) ficam fora da comparação.
    let listed: Vec<Value> = listed.as_array().unwrap().iter().filter(|t| !matches!(t["name"].as_str(), Some("interrupt" | "contract"))).cloned().collect();
    let python: Vec<Value> = serde_json::from_str(PYTHON_CATALOG).unwrap();
    let names = |tools: &[Value]| tools.iter().map(|t| t["name"].as_str().unwrap().to_owned()).collect::<Vec<_>>();
    assert_eq!(names(&listed), names(&python), "as 14 tools do Python, na mesma ordem");
    for (rust, python) in listed.iter().zip(&python) {
        // O texto destas mudou com o grupo entre máquinas; os parâmetros continuam os do Python.
        if matches!(python["name"].as_str(), Some("sessions" | "group" | "pair")) {
            assert_eq!((&rust["inputSchema"], &rust["outputSchema"]), (&python["inputSchema"], &python["outputSchema"]), "{}", python["name"]);
            continue;
        }
        if python["name"] != "new_session" {
            assert_eq!(rust, python, "{} igual ao Python", python["name"]);
            continue;
        }
        // A herança do modo mudou depois do Python (o padrão salvo vem antes): só a abertura do texto segue igual.
        let (rust_text, python_text) = (rust["description"].as_str().unwrap(), python["description"].as_str().unwrap());
        assert!(rust_text.starts_with(python_text.split(" O que for omitido").next().unwrap()), "{rust_text}");
        assert!(rust_text.contains("padrão salvo no servidor"), "{rust_text}");
        assert_eq!(rust["outputSchema"], python["outputSchema"]);
        assert_eq!(rust["inputSchema"]["required"], python["inputSchema"]["required"]);
        for (key, schema) in python["inputSchema"]["properties"].as_object().unwrap() {
            assert_eq!(&rust["inputSchema"]["properties"][key], schema, "parâmetro {key} igual ao Python");
        }
        for key in ["initial_prompt", "engine_account", "omp_profile", "claude_customizations", "inherit", "remember_provider"] {
            assert!(rust["inputSchema"]["properties"][key].is_object(), "o pedido de abertura inteiro: falta {key}");
        }
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn identity_follows_quem_chama() {
    let s = scene().await;
    let ok = |name: &str, origin: &str| Ok(json!({"name": name, "origem": origin}));
    assert_eq!(caller(&s, &[("x-hangar-key", "k123"), ("x-hangar-pane", "%7")]).await, ok("sem-term", "chave"), "a chave vence o pane herdado");
    assert!(caller(&s, &[("x-hangar-key", "nada"), ("x-hangar-pane", "%7")]).await.is_err(), "chave sem sidecar não cai no pane");
    // Claude com terminal aberto pelo Rust: a chave mora no registro, e quem diz o nome é o pane.
    let key = "0123456789abcdef0123456789abcdef";
    assert!(caller(&s, &[("x-hangar-key", key), ("x-hangar-pane", "%7")]).await.is_err());
    write_json(&s.home.join(format!(".hangar/session-specs/{key}.json")), json!({}));
    assert_eq!(caller(&s, &[("x-hangar-key", key), ("x-hangar-pane", "%7")]).await, ok("beta", "pane"));
    // psmux repete o id do pane entre sessões (%1 em s1 e beta) e o rename envelhece CP_SESSION_NAME:
    // a chave no ambiente da sessão diz quem ela é agora.
    let renamed = "fedcba9876543210fedcba9876543210";
    write_json(&s.home.join(format!(".hangar/session-specs/{renamed}.json")), json!({}));
    assert_eq!(caller(&s, &[("x-hangar-key", renamed), ("x-hangar-pane", "%1"), ("x-hangar-session", "velho")]).await, ok("s1", "chave"));
    let inherited = "dddddddddddddddddddddddddddddddd";
    write_json(&s.home.join(format!(".hangar/session-specs/{inherited}.json")), json!({}));
    assert!(caller(&s, &[("x-hangar-key", inherited)]).await.is_err(), "chave em duas sessões não resolve");
    assert_eq!(caller(&s, &[("x-hangar-pane", "%7"), ("x-hangar-session", "s0")]).await, ok("beta", "pane"));
    assert_eq!(caller(&s, &[("x-hangar-pane", "%1"), ("x-hangar-session", "s0")]).await, ok("s0", "nome"), "pane repetido não resolve");
    assert_eq!(caller(&s, &[("x-hangar-session", "sem-term")]).await, ok("sem-term", "nome"));
    assert!(caller(&s, &[("x-hangar-session", "renomeada")]).await.is_err(), "nome de sessão morta não resolve");
    let none = caller(&s, &[]).await.unwrap_err();
    assert!(none.starts_with("sessao_desconhecida: nenhum cabeçalho"), "{none}");
    let voice = hangar_server::voice::machines::voice_signature(SECRET).unwrap();
    assert_eq!(caller(&s, &[("x-hangar-voice", &voice), ("x-hangar-pane", "%7")]).await,
        Ok(json!({"name": "voz", "origem": "voz", "kind": "voice", "label": "assistente de voz do Hangar"})));
    let forged = hangar_server::voice::machines::voice_signature("outro").unwrap();
    assert!(caller(&s, &[("x-hangar-voice", &forged), ("x-hangar-pane", "%7")]).await.unwrap_err().contains("assinatura da voz"));
}

#[tokio::test(flavor = "multi_thread")]
async fn old_names_still_answer_off_the_catalog() {
    let s = scene().await;
    let mut mcp = Mcp::open(s.addr, &[("x-hangar-pane", "%7")]).await;
    let listed = mcp.request("tools/list", json!({})).await.to_string();
    assert!(!listed.contains("\"quem_sou\"") && !listed.contains("\"nav_abrir\""));
    assert_eq!(mcp.call("quem_sou", json!({})).await["structuredContent"], json!({"name": "beta", "origem": "pane"}));
}

#[tokio::test(flavor = "multi_thread")]
async fn sessions_marks_the_caller() {
    let s = scene().await;
    let mut mcp = Mcp::open(s.addr, &[("x-hangar-pane", "%0")]).await;
    let result = mcp.call("sessions", json!({})).await;
    let rows = result["structuredContent"]["result"].as_array().unwrap().clone();
    let mine: Vec<_> = rows.iter().filter(|r| r["voce"] == true).map(|r| r["name"].clone()).collect();
    assert_eq!(mine, [json!("s0")]);
    assert!(rows.iter().all(|r| r.as_object().unwrap().keys().eq(["name", "state", "cwd", "provider", "headless", "voce", "grupo",
        "membros", "dona"])), "membros com `servidor::sessao` e a máquina dona do grupo");
    assert_eq!(result["content"].as_array().unwrap().len(), rows.len(), "um bloco de texto por sessão, como o SDK do Python");
    let mut nobody = Mcp::open(s.addr, &[]).await;
    let rows = nobody.call("sessions", json!({})).await["structuredContent"]["result"].clone();
    assert!(rows.as_array().unwrap().iter().all(|r| r["voce"] == false));
}

#[tokio::test(flavor = "multi_thread")]
async fn send_goes_through_the_input_route_as_the_caller() {
    let s = scene().await;
    let mut mcp = Mcp::open(s.addr, &[("x-hangar-pane", "%7")]).await;
    let result = mcp.call("send", json!({"alvo": "fora", "texto": "[de: beta] oi"})).await;
    assert!(!failed(&result), "{result}");
    assert_eq!(result["structuredContent"]["alvo"], "fora");
    assert_eq!(s.python.hits_to("/api/sessions/fora/input"), 1);
    assert_eq!(serde_json::from_slice::<Value>(&s.python.last_body()).unwrap(), json!({"text": "[de: beta] oi", "steer": true}),
        "o prefixo escrito pelo modelo não dobra");
    for alvo in ["beta"] {
        let refused = mcp.call("send", json!({"alvo": alvo, "texto": "oi", "tmux": true})).await;
        assert!(failed(&refused) && first_text(&refused).contains("é esta sessão"), "{refused}");
    }
    let remote = mcp.call("send", json!({"alvo": "srv2::outra", "texto": "oi"})).await;
    assert!(failed(&remote) && first_text(&remote).contains("CP_SERVER_ID ausente"), "{remote}");
    assert_eq!(s.python.hits_to("/api/sessions/fora/input"), 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn group_pair_unpair_call_the_routes_as_the_caller() {
    // Grupos são do Rust: nada disso segue ao Python.
    let s = scene_with(true).await;
    let pair = s.home.join(".claude/.hangar-pair");
    let mut mcp = Mcp::open(s.addr, &[("x-hangar-pane", "%7")]).await;
    let paired = mcp.call("pair", json!({"alvo": "s0", "tarefa": "t"})).await;
    assert!(!failed(&paired), "{paired}");
    assert!(pair.join("beta.json").exists() && pair.join("s0.json").exists());
    let notice = mcp.call("group", json!({"texto": "marco"})).await;
    assert!(!failed(&notice) && notice["structuredContent"]["peers"] == json!(["s0"]), "{notice}");
    assert!(!failed(&mcp.call("desparear", json!({})).await));
    assert!(!pair.join("beta.json").exists());
    for path in ["/api/sessions/beta/group-message", "/api/sessions/beta/pair"] {
        assert_eq!(s.python.hits_to(path), 0, "{path}");
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn new_session_opens_with_the_caller_as_creator_and_the_whole_request() {
    // Toda abertura é do Rust: o pedido chega à resolução (a pasta inexistente é recusada lá, antes de
    // qualquer efeito) e nada vai ao Python.
    let s = scene().await;
    let mut mcp = Mcp::open(s.addr, &[("x-hangar-pane", "%7")]).await;
    let result = mcp.call("new_session", json!({"nome": "nova", "cwd": "/nao/existe", "provider": "claude", "headless": true,
        "conta": "/home/x/.claude-outra", "initial_prompt": "comece", "inherit": false, "branch": ""})).await;
    assert!(failed(&result) && first_text(&result).contains("não existe"), "{result}");
    let refused = mcp.call("new_session", json!({"nome": "n2", "cwd": "/tmp", "provider": "pi", "headless": true})).await;
    assert!(failed(&refused) && first_text(&refused).contains("headless só vale"));
    let voice = hangar_server::voice::machines::voice_signature(SECRET).unwrap();
    let mut voz = Mcp::open(s.addr, &[("x-hangar-voice", &voice)]).await;
    let voiced = voz.call("new_session", json!({"nome": "n3", "cwd": "/nao/existe", "provider": "codex"})).await;
    assert!(failed(&voiced) && first_text(&voiced).contains("não existe"), "{voiced}");
    assert_eq!(s.python.hits_to("/api/sessions"), 0, "a abertura não passa pelo Python");
}

/// `resume` retoma uma conversa que existe no disco pela rota do Arquivo: a pasta vem da conversa;
/// o que a retomada não aceita é recusado, nunca largado calado.
#[tokio::test(flavor = "multi_thread")]
async fn new_session_with_resume_goes_through_the_archive_resume() {
    let s = scene().await;
    let mut mcp = Mcp::open(s.addr, &[("x-hangar-pane", "%7")]).await;
    let missing = mcp.call("new_session", json!({"nome": "volta", "cwd": "", "resume": "11111111-1111-4111-8111-111111111111",
        "headless": false})).await;
    assert!(failed(&missing) && first_text(&missing).contains("transcript"), "{missing}");
    let mixed = mcp.call("new_session", json!({"nome": "volta", "cwd": "", "resume": "11111111-1111-4111-8111-111111111111",
        "permissao": "plan"})).await;
    assert!(failed(&mixed) && first_text(&mixed).contains("permissao"), "{mixed}");
    assert_eq!(s.python.hits_to("/api/sessions"), 0);
}

#[tokio::test(flavor = "multi_thread")]
async fn close_session_refuses_itself_remote_and_unknown() {
    let s = scene().await;
    write_json(&s.home.join(".hangar/codex-sessions/outra.json"), json!({"name": "outra"}));
    let mut mcp = Mcp::open(s.addr, &[("x-hangar-pane", "%7")]).await;
    for (alvo, error) in [("beta", "é esta sessão"), ("srv::outra", "desta máquina"), ("sumiu", "não existe")] {
        let result = mcp.call("close_session", json!({"alvo": alvo})).await;
        assert!(failed(&result) && first_text(&result).contains(error), "{alvo}: {result}");
    }
    assert!(!failed(&mcp.call("close_session", json!({"alvo": "outra"})).await));
    assert_eq!(s.python.hits_to("/api/sessions/outra"), 1);
    let voice = hangar_server::voice::machines::voice_signature(SECRET).unwrap();
    let refused = Mcp::open(s.addr, &[("x-hangar-voice", &voice)]).await.call("close_session", json!({"alvo": "outra"})).await;
    assert!(failed(&refused) && first_text(&refused).contains("sim"), "fechar pela voz só com o sim falado");
}

/// `contract` lê e grava pela rota `/pair/contract` da sessão que chama; gravar exige o `mtime` lido.
#[tokio::test(flavor = "multi_thread")]
async fn contract_reads_and_writes_through_the_session_route() {
    let s = scene_with(true).await;
    for (name, peer) in [("beta", "s0"), ("s0", "beta")] {
        write_json(&s.home.join(format!(".claude/.hangar-pair/{name}.json")), json!({"peers": [peer], "task": "", "gid": "g1", "harness": {}}));
    }
    let mut mcp = Mcp::open(s.addr, &[("x-hangar-pane", "%7")]).await;
    let read = mcp.call("contract", json!({})).await;
    assert!(!failed(&read), "{read}");
    assert_eq!(read["structuredContent"]["content"], "");
    let written = mcp.call("contract", json!({"conteudo": "decisão A\n", "mtime": null})).await;
    assert!(!failed(&written), "{written}");
    let mtime = written["structuredContent"]["mtime"].clone();
    assert!(mtime.as_str().is_some_and(|m| !m.is_empty()), "{written}");
    assert_eq!(mcp.call("contract", json!({})).await["structuredContent"]["content"], "decisão A\n");
    let stale = mcp.call("contract", json!({"conteudo": "velho\n"})).await;
    assert!(failed(&stale) && first_text(&stale).starts_with("erro_contrato_mudou"), "{stale}");
    assert!(!failed(&mcp.call("contract", json!({"conteudo": "decisão B\n", "mtime": mtime})).await));
    let voice = hangar_server::voice::machines::voice_signature(SECRET).unwrap();
    let unnamed = Mcp::open(s.addr, &[("x-hangar-voice", &voice)]).await.call("contract", json!({})).await;
    assert!(failed(&unnamed) && first_text(&unnamed).contains("`sessao`"), "{unnamed}");
}

/// `interrupt` só passa para o pai ligado ainda no grupo da filha; passando, é a rota `/interrupt` de sempre.
#[tokio::test(flavor = "multi_thread")]
async fn interrupt_only_reaches_a_linked_child() {
    let s = scene_with(true).await;
    write_json(&s.home.join(".hangar/codex-sessions/outra.json"), json!({"name": "outra"}));
    let mut mcp = Mcp::open(s.addr, &[("x-hangar-pane", "%7")]).await;
    let refused = mcp.call("interrupt", json!({"alvo": "outra"})).await;
    assert!(failed(&refused) && first_text(&refused).starts_with("erro_interromper_nao_ligada"), "sem grupo: {refused}");
    let sidecar = s.home.join(".claude/.hangar-pair/outra.json");
    write_json(&sidecar, json!({"peers": ["s0"], "task": "", "gid": "g1", "harness": {}, "linked_by": "s0"}));
    let other = mcp.call("interrupt", json!({"alvo": "outra"})).await;
    assert!(failed(&other) && first_text(&other).starts_with("erro_interromper_nao_ligada"), "filha de outro pai: {other}");
    assert_eq!(s.python.hits_to("/api/sessions/outra/interrupt"), 0);
    write_json(&sidecar, json!({"peers": ["beta"], "task": "", "gid": "g1", "harness": {}, "linked_by": "beta"}));
    let done = mcp.call("interrupt", json!({"alvo": "outra"})).await;
    assert!(!failed(&done), "{done}");
    assert_eq!(s.python.hits_to("/api/sessions/outra/interrupt"), 1);
}

/// O servidor do app desktop (`POST /cmd`): anota cada pedido e responde por verbo.
async fn desktop(home: &Path, answer: fn(&str) -> String) -> Arc<Mutex<Vec<Value>>> {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let log = seen.clone();
    let app = axum::Router::new().route("/cmd", axum::routing::post(move |headers: axum::http::HeaderMap, body: String| {
        let log = log.clone();
        async move {
            assert_eq!(headers["authorization"], "Bearer t");
            let body: Value = serde_json::from_str(&body).unwrap();
            let reply = answer(body["verbo"].as_str().unwrap());
            log.lock().unwrap().push(body);
            reply
        }
    }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    write_json(&home.join(".hangar/nav/_srv.json"), json!({"porta": port, "token": "t"}));
    write_json(&home.join(".hangar/nav/casa-beta.json"), json!({"chave": "casa::beta", "url": "http://antes/", "ts": 0}));
    seen
}

#[tokio::test(flavor = "multi_thread")]
async fn browser_verbs_go_to_the_desktop_app() {
    let s = scene().await;
    let mut mcp = Mcp::open(s.addr, &[("x-hangar-pane", "%7")]).await;
    let closed = mcp.call("browser", json!({"verbo": "snapshot"})).await;
    assert!(failed(&closed) && first_text(&closed).contains("app desktop não está aberto"), "{closed}");
    let unknown = mcp.call("nav", json!({"verbo": "voar"})).await;
    assert!(failed(&unknown) && first_text(&unknown).contains("verbo desconhecido"));
    let seen = desktop(&s.home, |verb| match verb {
        "click" => "erro: ref @e9 nao existe".into(),
        "layout" => "layout: 1366x768".into(),
        "shot" => "shot".into(),
        other => format!("ok: {other}"),
    }).await;
    let layout = mcp.call("browser", json!({"verbo": "layout", "args": [1366, 768]})).await;
    assert_eq!((first_text(&layout), layout.get("structuredContent")), ("layout: 1366x768".to_owned(), None));
    assert_eq!(seen.lock().unwrap()[0], json!({"chave": "casa::beta", "verbo": "layout", "args": ["1366", "768"]}));
    assert!(!failed(&mcp.call("browser", json!({"verbo": "shot", "aba": 2})).await));
    let shot = seen.lock().unwrap()[1].clone();
    assert_eq!(shot["aba"], 2);
    let path = shot["args"][0].as_str().unwrap();
    assert!(path.starts_with(s.home.join(".hangar/nav/shots/beta").to_str().unwrap()) && path.ends_with(".png"), "{path}");
    let stopped = mcp.call("browser_batch", json!({"passos": [{"verbo": "snapshot"}, {"verbo": "click", "args": ["@e9"]}, {"verbo": "text"}]})).await;
    let text = first_text(&stopped);
    assert!(failed(&stopped) && text.contains("parou no passo 1 (click)") && text.contains("@e9") && text.contains("ok: snapshot"), "{text}");
    let batch = mcp.call("nav_lote", json!({"passos": [{"verbo": "snapshot"}, {"verbo": "text"}]})).await;
    assert_eq!(batch["structuredContent"], json!({"feitos": ["ok: snapshot", "ok: text"]}));
}

#[tokio::test(flavor = "multi_thread")]
async fn shot_comes_back_as_an_image() {
    let s = scene().await;
    let png = s.home.join("a.png");
    std::fs::write(&png, b"\x89PNG\r\n\x1a\n").unwrap();
    write_json(&s.home.join(".hangar/nav/casa-beta.json"), json!({"chave": "casa::beta", "url": "http://antes/", "ts": 0}));
    // O app responde com o caminho que ele gravou; aqui um PNG já existente.
    let answer = format!("ok: shot {}", png.display());
    let app = axum::Router::new().route("/cmd", axum::routing::post(move || { let a = answer.clone(); async move { a } }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    write_json(&s.home.join(".hangar/nav/_srv.json"), json!({"porta": port, "token": "t"}));
    let mut mcp = Mcp::open(s.addr, &[("x-hangar-pane", "%7")]).await;
    let result = mcp.call("browser", json!({"verbo": "shot"})).await;
    let kinds: Vec<_> = result["content"].as_array().unwrap().iter().map(|c| c["type"].clone()).collect();
    assert_eq!(kinds, [json!("text"), json!("image")]);
    assert_eq!(result["content"][1]["mimeType"], "image/png");
}

#[tokio::test(flavor = "multi_thread")]
async fn browser_open_asks_the_nav_route_and_waits_for_the_page() {
    let s = scene().await;
    let seen = desktop(&s.home, |verb| if verb == "url" { "ok: \"http://x/\"".into() } else { "ativo".into() }).await;
    let later = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis() as u64 + 60_000;
    write_json(&s.home.join(".hangar/nav/casa-beta.json"), json!({"chave": "casa::beta", "url": "http://x/", "ts": later}));
    let mut mcp = Mcp::open(s.addr, &[("x-hangar-pane", "%7")]).await;
    let result = mcp.call("browser_open", json!({"url": "http://x"})).await;
    assert_eq!(result["structuredContent"], json!({"ok": true, "url": "http://x/"}), "{result}");
    assert_eq!(s.python.hits_to("/api/sessions/beta/nav"), 1);
    assert_eq!(serde_json::from_slice::<Value>(&s.python.last_body()).unwrap(), json!({"url": "http://x"}));
    let verbs: Vec<_> = seen.lock().unwrap().iter().map(|b| b["verbo"].clone()).collect();
    assert_eq!(verbs, [json!("estado"), json!("url")]);
}

#[tokio::test(flavor = "multi_thread")]
async fn html_render_publishes_for_the_caller() {
    let s = scene().await;
    let mut mcp = Mcp::open(s.addr, &[("x-hangar-pane", "%0")]).await;
    for args in [json!({"title": "t"}), json!({"title": "t", "html": "<p></p>", "url": "http://localhost:3000/"})] {
        let result = mcp.call("html_render", args).await;
        assert!(failed(&result) && first_text(&result).starts_with("erro_pagina_html_ou_url"), "{result}");
    }
    let result = mcp.call("html_render", json!({"title": "Oi", "html": "<p>oi</p>"})).await;
    assert!(!failed(&result), "{result}");
    assert_eq!(result["structuredContent"]["hangar_page"]["title"], "Oi");
    let refused = mcp.call("html_render", json!({"title": "Oi", "html": "<p>oi</p>", "height": 5})).await;
    assert_eq!(first_text(&refused), "erro_pagina_invalida: height fica entre 80 e 2000");
}

#[tokio::test(flavor = "multi_thread")]
async fn voice_names_the_session_it_acts_for() {
    let s = scene_with(true).await;
    desktop(&s.home, |verb| format!("ok: {verb}")).await;
    let voice = hangar_server::voice::machines::voice_signature(SECRET).unwrap();
    let mut mcp = Mcp::open(s.addr, &[("x-hangar-voice", &voice)]).await;
    assert!(!failed(&mcp.call("send", json!({"alvo": "fora", "texto": "terminei"})).await));
    assert_eq!(serde_json::from_slice::<Value>(&s.python.last_body()).unwrap()["text"], "[de: voz] terminei");
    for (tool, args) in [("pair", json!({"alvo": "s0"})), ("group", json!({"texto": "x"})), ("unpair", json!({})),
        ("browser", json!({"verbo": "snapshot"})), ("browser_batch", json!({"passos": []})), ("html_render", json!({"title": "t", "html": "<p>x</p>"}))] {
        let result = mcp.call(tool, args.clone()).await;
        assert!(failed(&result) && first_text(&result).contains("`sessao`"), "{tool}: {result}");
        let mut args = args;
        args["sessao"] = json!(if tool == "html_render" { "s0" } else { "beta" });
        let done = mcp.call(tool, args).await;
        assert!(!failed(&done), "{tool}: {done}");
    }
    assert!(!s.home.join(".claude/.hangar-pair/beta.json").exists(), "a voz desfez o par de beta");
}

#[tokio::test(flavor = "multi_thread")]
async fn browser_goal_only_with_the_session_jev() {
    let s = scene().await;
    let mut mcp = Mcp::open(s.addr, &[("x-hangar-pane", "%7")]).await;
    let off = mcp.call("browser_goal", json!({"objetivo": "abre a agenda"})).await;
    assert!(failed(&off) && first_text(&off).contains("Jev não chegou ligado"), "{off}");
    let mut jev = Mcp::open(s.addr, &[("x-hangar-pane", "%7"), ("x-hangar-jev", "on")]).await;
    let unconfigured = jev.call("browser_check", json!({"estado": "lista com o item"})).await;
    assert!(failed(&unconfigured) && first_text(&unconfigured).contains("não está configurado"), "{unconfigured}");
}
