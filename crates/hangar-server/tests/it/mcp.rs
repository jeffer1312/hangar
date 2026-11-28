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

struct Scene { addr: SocketAddr, home: PathBuf, sink: crate::sink::Sink, state: Arc<AppState>, _dir: tempfile::TempDir }

/// Sessões `s0` (pane `%0`) e `s1` (`%1`) com conversa; `beta` com os panes `%1` (repetido, como no
/// psmux) e `%7`; `sem-term` sem terminal, de chave `k123`; o Python falso atrás.
async fn scene() -> Scene { scene_with(false).await }

/// `groups`: o serviço de grupo do Rust sobre `home/.claude/.hangar-pair` (sem ele, grupo segue ao Python).
async fn scene_with(groups: bool) -> Scene { scene_full(groups, None).await }

async fn scene_full(groups: bool, peers: Option<hangar_server::groups::peers::PeerClient>) -> Scene { scene_built(groups, peers, false).await }

/// Grupos do Rust com `s0`, `s1` e `beta` abertos como Claude sem terminal: o recado chega ao cano falso.
async fn scene_owned() -> Scene { scene_built(true, None, true).await }

async fn scene_built(groups: bool, peers: Option<hangar_server::groups::peers::PeerClient>, owned: bool) -> Scene {
    scene_named(groups,peers,owned,"").await
}

async fn scene_named(groups: bool, peers: Option<hangar_server::groups::peers::PeerClient>, owned: bool, server: &str) -> Scene {
    scene_shared(groups, peers, owned, server, false).await
}

/// `share`: a loja de convites e de pares externos em `shares.json` e `.hangar-pair` da cena.
async fn scene_shared(groups: bool, peers: Option<hangar_server::groups::peers::PeerClient>, owned: bool, server: &str, share: bool) -> Scene {
    scene_share(groups, peers, owned, server, if share { Share::Store } else { Share::Absent }).await
}

/// Como a cena carrega os convites e os pares externos.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Share {
    /// Sem configuração de convites: não há convite nem par.
    Absent,
    /// Configuração de convites que não se carregou.
    Unavailable,
    Store,
    /// A loja aberta sobre um `external_pairs.json` torto.
    StoreWithBadPairs,
}

async fn scene_share(groups: bool, peers: Option<hangar_server::groups::peers::PeerClient>, owned: bool, server: &str, share: Share) -> Scene {
    scene_tuned(groups, peers, owned, server, share, |_| {}).await
}

/// `tune` ajusta o estado antes de o servidor subir (o cofre falso, por exemplo).
async fn scene_tuned(groups: bool, peers: Option<hangar_server::groups::peers::PeerClient>, owned: bool, server: &str, share: Share,
    tune: impl FnOnce(&mut AppState)) -> Scene {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    list_support::sessions(root, 2);
    let (w0, w1) = (root.join("w0"), root.join("w1"));
    let script = root.join("tmux");
    crate::write_executable(&script, format!(
        "#!/bin/sh\ncase \"$1:$4\" in list-panes:*CP_SESSION_KEY*) printf 's0\\tdddddddddddddddddddddddddddddddd\\ns1\\tfedcba9876543210fedcba9876543210\\nbeta\\tdddddddddddddddddddddddddddddddd\\n'; exit 0;; esac\n[ \"$1\" = show-environment ] && [ \"${{3#=}}\" = s1 ] && {{ printf 'CP_SESSION_KEY=fedcba9876543210fedcba9876543210\\n'; exit 0; }}\n[ \"$1\" = show-environment ] && {{ [ \"${{3#=}}\" = s0 ] || [ \"${{3#=}}\" = beta ]; }} && {{ printf 'CP_SESSION_KEY=dddddddddddddddddddddddddddddddd\\n'; exit 0; }}\n[ \"$1\" = list-panes ] || {{ printf 'ok\\n'; exit 0; }}\nprintf 's0\\t1\\t\\t{}\\t%%0\\t\\t\\t\\t0\\t0\\ns1\\t1\\t\\t{}\\t%%1\\t\\t\\t\\t0\\t0\\nbeta\\t1\\t\\t{}\\t%%1\\t\\t\\t\\t0\\t0\\nbeta\\t1\\t\\t{}\\t%%7\\t\\t\\t\\t0\\t1\\n'\n",
        w0.display(), w1.display(), w1.display(), w1.display()));
    let home = root.join("home");
    write_json(&home.join(".hangar/claude-headless/sem-term.json"), json!({"name": "sem-term", "key": "k123", "session_id": "s"}));
    let dirs = parse_dirs(&json!({"home": home, "claude": home.join(".claude"), "codex_home": home.join(".codex"),
        "pi_sessions": home.join(".pi/agent/sessions"), "omp_config": home.join(".omp"),
        "omp_agent": home.join(".omp/agent"), "kimi_home": home.join(".kimi-code")}).to_string());
    let mut state = AppState::new(fake::config("127.0.0.1"));
    state.server_id=server.to_owned();
    let sink = crate::sink::Sink::default();
    if owned {
        let runtime = Arc::new(crate::sink::registry().await);
        std::fs::create_dir_all(root.join("rt")).unwrap();
        for name in ["s0", "s1", "beta"] { sink.open(&runtime, &root.join("rt"), name).await; }
        assert!(state.state.runtime.set(runtime).is_ok());
    }
    let list = Arc::new(ListBridge::new(ListEnv { mux: Mux::with_program(&script, Duration::from_secs(5)),
        capture_program: script.into_os_string(), procs: Arc::new(hangar_server::list::procs::SystemProcs::default()), dirs },
        FactsClient::new()));
    state.list = list.clone();
    if let Some(runtime)=state.state.runtime.get() {list.set_runtime(runtime.clone());list.set_plugin_runtime(runtime.clone());}
    state.side.locate = Arc::new(hangar_server::locate::ListLocate { list });
    state.pages = Arc::new(hangar_server::pages::store::Store::new(root.join("paginas")));
    state.chromium = || None;
    state.mission = Arc::new(hangar_mission::Store::new(root.join("mission")));
    state.origins_home = home.clone();
    // Sem isto o WhatsApp nasceria na pasta pessoal de quem roda o teste e falaria com o serviço de verdade.
    assert!(state.whatsapp.set(hangar_whatsapp::WhatsApp::new(&home)).is_ok());
    // Os convidados com login da cena, nunca o `guests.json` de quem roda o teste.
    state.login_guests = Arc::new(hangar_server::login_guests::Service::new(hangar_server::login_guests::Settings {
        guests: root.join("guests.json"), vault: root.join("vault.json"), runtime_config: root.join("runtime-config.json"),
        sync_enabled: false, bootstrap: String::new(), secret: vec![1], secure: false, rate_max: 10.into(), rate_window: 300.into() }));
    if share == Share::StoreWithBadPairs {
        std::fs::create_dir_all(home.join(".claude/.hangar-pair")).unwrap();
        std::fs::write(home.join(".claude/.hangar-pair/external_pairs.json"), "{torto").unwrap();
    }
    state.share_unavailable = share == Share::Unavailable;
    if matches!(share, Share::Store | Share::StoreWithBadPairs) {
        state.share = Some(crate::share_pair_routes::share_state(&root.join("shares.json"), &home.join(".claude/.hangar-pair"),
            Arc::new(Mutex::new(1_000.0))));
    } else {
        state.share = None;
    }
    if let Some(peers) = peers { state.peers = Arc::new(peers); }
    if groups {
        use hangar_server::groups::{orq::OrqVault, service::GroupService, store::PairDir};
        let pair = PairDir::new(home.join(".claude/.hangar-pair"), root.join("arquivo"));
        state.groups = Some(Arc::new(GroupService::new(pair, Arc::new(OrqVault::from_state(&state)), if server.is_empty() {"casa"} else {server}.into())));
    }
    tune(&mut state);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let state=Arc::new(state);
    let app = router(state.clone()).into_make_service_with_connect_info::<SocketAddr>();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    // A lista dos rascunhos, como na subida: a resposta do dono a muda, e é a mudança que acorda a espera da tool.
    drop(hangar_server::whatsapp_outbox::spawn(state.clone()));
    Scene { addr, home, sink, state, _dir: dir }
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
}

#[tokio::test(flavor = "multi_thread")]
async fn catalog_is_the_python_one_with_the_whole_open_request() {
    let s = scene().await;
    let mut mcp = Mcp::open(s.addr, &[]).await;
    let listed = mcp.request("tools/list", json!({})).await["result"]["tools"].clone();
    // Tools que nasceram depois do Python (ele não tem mais MCP) ficam fora da comparação.
    let python: Vec<Value> = serde_json::from_str(PYTHON_CATALOG).unwrap();
    let names = |tools: &[Value]| tools.iter().map(|t| t["name"].as_str().unwrap().to_owned()).collect::<Vec<_>>();
    let known = names(&python);
    let all = names(listed.as_array().unwrap());
    for tool in ["request_secret", "vault_list", "vault_exec", "vault_file", "vault_fill", "mission",
                 "whatsapp_chats", "whatsapp_read", "whatsapp_search", "whatsapp_request_media", "whatsapp_draft"] {
        assert!(all.iter().any(|n| n == tool), "falta {tool} no catálogo");
    }
    let listed: Vec<Value> = listed.as_array().unwrap().iter()
        .filter(|t| t["name"].as_str().is_some_and(|n| known.iter().any(|k| k == n))).cloned().collect();
    assert_eq!(names(&listed), names(&python), "as 14 tools do Python, na mesma ordem");
    for (rust, python) in listed.iter().zip(&python) {
        // O texto destas mudou depois do Python (grupo entre máquinas; alvos e esperas do navegador; filhas que
        // ficam ao fechar o pai); os parâmetros continuam os do Python.
        if matches!(python["name"].as_str(), Some("sessions" | "group" | "pair" | "close_session" | "browser" | "browser_open")) {
            assert_eq!((&rust["inputSchema"], &rust["outputSchema"]), (&python["inputSchema"], &python["outputSchema"]), "{}", python["name"]);
            continue;
        }
        // `permitir_risco` e a frase da trava de risco vieram depois do Python; os outros parâmetros são os dele.
        if python["name"] == "browser_goal" {
            assert_eq!(rust["outputSchema"], python["outputSchema"]);
            assert_eq!(rust["inputSchema"]["required"], python["inputSchema"]["required"]);
            for (key, schema) in python["inputSchema"]["properties"].as_object().unwrap() {
                assert_eq!(&rust["inputSchema"]["properties"][key], schema, "parâmetro {key} do browser_goal igual ao Python");
            }
            assert!(rust["inputSchema"]["properties"]["permitir_risco"].is_object(), "falta permitir_risco no browser_goal");
            continue;
        }
        if python["name"] != "new_session" {
            assert_eq!(rust, python, "{} igual ao Python", python["name"]);
            continue;
        }
        // A herança do modo mudou depois do Python (o padrão salvo vem antes): só a abertura do texto segue igual.
        let (rust_text, python_text) = (rust["description"].as_str().unwrap(), python["description"].as_str().unwrap());
        // "nesta máquina" saiu com o `servidor::nome`.
        assert!(rust_text.starts_with(python_text.replacen(" nesta máquina", "", 1).split(" O que for omitido").next().unwrap()), "{rust_text}");
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
    assert_eq!(caller(&s, &[("x-hangar-key", renamed), ("x-hangar-pane", "%7")]).await, ok("s1", "chave"), "pane herdado de beta: a chave acha s1");
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
        "membros", "dona", "linked_by", "question", "orphaned_from"])), "membros com `servidor::sessao`, a máquina dona do grupo, quem abriu a filha, a pergunta dela e o pai que ela perdeu");
    assert_eq!(result["content"].as_array().unwrap().len(), rows.len(), "um bloco de texto por sessão, como o SDK do Python");
    let mut nobody = Mcp::open(s.addr, &[]).await;
    let rows = nobody.call("sessions", json!({})).await["structuredContent"]["result"].clone();
    assert!(rows.as_array().unwrap().iter().all(|r| r["voce"] == false));
}

#[tokio::test(flavor = "multi_thread")]
async fn send_goes_through_the_input_route_as_the_caller() {
    // A escrita é do Rust (sem registro ela é o `runtime_unavailable`): o recado chega ao cano de `s0`.
    let s = scene_owned().await;
    let mut mcp = Mcp::open(s.addr, &[("x-hangar-pane", "%7")]).await;
    let result = mcp.call("send", json!({"alvo": "s0", "texto": "[de: beta] oi"})).await;
    assert!(!failed(&result), "{result}");
    assert_eq!(result["structuredContent"]["alvo"], "s0");
    assert_eq!(s.sink.last("s0").as_deref(), Some("[de: beta] oi"), "o prefixo escrito pelo modelo não dobra");
    for alvo in ["beta"] {
        let refused = mcp.call("send", json!({"alvo": alvo, "texto": "oi", "tmux": true})).await;
        assert!(failed(&refused) && first_text(&refused).contains("é esta sessão"), "{refused}");
    }
    let remote = mcp.call("send", json!({"alvo": "srv2::outra", "texto": "oi"})).await;
    assert!(failed(&remote) && first_text(&remote).contains("CP_SERVER_ID ausente"), "{remote}");
    assert_eq!(s.sink.count("s0"), 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn group_pair_unpair_call_the_routes_as_the_caller() {
    // Grupos são do Rust: nada disso segue ao Python.
    let s = scene_owned().await;
    let pair = s.home.join(".claude/.hangar-pair");
    let mut mcp = Mcp::open(s.addr, &[("x-hangar-pane", "%7")]).await;
    let paired = mcp.call("pair", json!({"alvo": "s0", "tarefa": "t"})).await;
    assert!(!failed(&paired), "{paired}");
    assert!(pair.join("beta.json").exists() && pair.join("s0.json").exists());
    let notice = mcp.call("group", json!({"texto": "marco"})).await;
    assert!(!failed(&notice) && notice["structuredContent"]["peers"] == json!(["s0"]), "{notice}");
    assert!(!failed(&mcp.call("desparear", json!({})).await));
    assert!(!pair.join("beta.json").exists());
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
}

#[tokio::test(flavor = "multi_thread")]
async fn close_session_refuses_itself_unknown_server_and_unknown() {
    let s = scene().await;
    write_json(&s.home.join(".hangar/codex-sessions/outra.json"), json!({"name": "outra"}));
    let mut mcp = Mcp::open(s.addr, &[("x-hangar-pane", "%7")]).await;
    for (alvo, error) in [("beta", "é esta sessão"), ("srv::outra", "peers.json"), ("a::b::c", "um '::' só"), ("sumiu", "não existe")] {
        let result = mcp.call("close_session", json!({"alvo": alvo})).await;
        assert!(failed(&result) && first_text(&result).contains(error), "{alvo}: {result}");
    }
    // Sem o registro das sessões (servidor subindo) o fechamento falha com código, nunca vai ao Python.
    let closed = mcp.call("close_session", json!({"alvo": "outra"})).await;
    assert!(failed(&closed), "{closed}");
    let voice = hangar_server::voice::machines::voice_signature(SECRET).unwrap();
    let refused = Mcp::open(s.addr, &[("x-hangar-voice", &voice)]).await.call("close_session", json!({"alvo": "outra"})).await;
    assert!(failed(&refused) && first_text(&refused).contains("sim"), "fechar pela voz só com o sim falado");
}

/// `servidor::sessao` pelo ponto de entrada: fecha lá, com o token de lá e dizendo quem pediu.
#[tokio::test(flavor = "multi_thread")]
async fn close_session_reaches_the_peer() {
    use axum::http::{HeaderMap, HeaderValue, Uri, header::CONTENT_TYPE};
    use axum::routing::{delete, get};
    use hangar_server::groups::peers::{PeerBook, PeerClient};
    let seen: Arc<Mutex<Vec<(String, String, String)>>> = Arc::default();
    let sink = seen.clone();
    let app = axum::Router::new()
        .route("/api/sessions", get(|| async { ([(CONTENT_TYPE, "application/json")], json!([{"name": "filha"}]).to_string()) }))
        .route("/api/sessions/{name}", delete(move |h: HeaderMap, uri: Uri| {
            let sink = sink.clone();
            async move {
                let header = |name| h.get(name).and_then(|v: &HeaderValue| v.to_str().ok()).unwrap_or_default().to_owned();
                sink.lock().unwrap().push((header("authorization"), header("user-agent"), uri.path().to_owned()));
                ([(CONTENT_TYPE, "application/json")], json!({"ok": true}).to_string())
            }
        }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("peers.json");
    write_json(&file, json!({"lab": {"base_url": format!("http://{addr}"), "token": "tok-lab"}}));
    let s = scene_full(false, Some(PeerClient::new(PeerBook::new(Some(file))))).await;
    let mut mcp = Mcp::open(s.addr, &[("x-hangar-pane", "%7")]).await;
    let closed = mcp.call("close_session", json!({"alvo": "lab::filha"})).await;
    assert!(!failed(&closed), "{closed}");
    assert_eq!(closed["structuredContent"]["server"], "lab");
    let (auth, agent, path) = seen.lock().unwrap().pop().expect("DELETE lá");
    assert_eq!((auth.as_str(), path.as_str()), ("Bearer tok-lab", "/api/sessions/filha"));
    assert!(agent.contains("beta"), "{agent}");
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
    write_json(&sidecar, json!({"peers": ["beta"], "task": "", "gid": "g1", "harness": {}, "linked_by": "beta"}));
    // Ligada, passa a guarda e a rota responde no Rust (cena sem registro: `runtime_unavailable`), nunca o Python.
    let done = mcp.call("interrupt", json!({"alvo": "outra"})).await;
    assert!(!first_text(&done).starts_with("erro_interromper_nao_ligada"), "{done}");
}

/// `child_status` lê a resposta final da filha ligada em páginas de 8000 caracteres, pela rota do histórico; outra
/// sessão é recusada com `erro_nao_ligada`, e o `interrupt` segue com o código dele.
#[tokio::test(flavor = "multi_thread")]
async fn child_status_pages_the_final_answer_of_a_linked_child_only() {
    let s = scene_tuned(true, None, false, "", Share::Absent, |st| st.hangar_dir = st.origins_home.join(".hangar")).await;
    let pages = ["á".repeat(8000), "b".repeat(8000), "ç".repeat(4000)];
    let answer = pages.concat();
    // A conversa de `s0` (pasta `w0`) vira a da filha: pedido do pai e resposta de 20 000 caracteres.
    let cwd = s.home.parent().unwrap().join("w0");
    let proj = s.home.join(".claude/projects").join(hangar_workspace::worktrees::sanitize_cwd(cwd.to_str().unwrap()));
    let lines = [
        json!({"type": "user", "uuid": "u1", "timestamp": "2026-10-02T10:00:00.000Z",
            "message": {"role": "user", "content": "[de: beta] faz a busca"}}),
        json!({"type": "assistant", "uuid": "a1", "timestamp": "2026-10-02T10:00:05.000Z",
            "message": {"role": "assistant", "content": [{"type": "text", "text": answer}]}}),
    ];
    std::fs::write(proj.join("00000000-0000-0000-0000-000000000000.jsonl"), lines.iter().map(|l| format!("{l}\n")).collect::<String>()).unwrap();
    write_json(&s.home.join(".claude/.hangar-pair/s0.json"), json!({"peers": ["beta"], "task": "", "gid": "g1", "harness": {}, "linked_by": "beta"}));
    let mut mcp = Mcp::open(s.addr, &[("x-hangar-pane", "%7")]).await;
    for (offset, page, next) in [(0, &pages[0], json!(8000)), (8000, &pages[1], json!(16000)), (16000, &pages[2], Value::Null)] {
        let status = mcp.call("child_status", json!({"alvo": "s0", "offset": offset})).await;
        assert!(!failed(&status), "{status}");
        let got = &status["structuredContent"];
        assert_eq!(got["answer"]["text"].as_str(), Some(page.as_str()), "offset {offset}: {}", got["state"]);
        assert_eq!((&got["answer"]["offset"], &got["answer"]["total_chars"], &got["answer"]["next_offset"]), (&json!(offset), &json!(20000), &next));
        assert_eq!(got["returned"], false, "nada voltou sozinho ainda");
    }
    // A página seguinte pedida com a identidade da resposta passa; com a de outra resposta, o cursor expirou.
    let first = mcp.call("child_status", json!({"alvo": "s0"})).await;
    let key = first["structuredContent"]["answer"]["answer_key"].clone();
    assert!(key.as_str().is_some_and(|k| !k.is_empty()), "{first}");
    let same = mcp.call("child_status", json!({"alvo": "s0", "offset": 8000, "answer_key": key})).await;
    assert_eq!(same["structuredContent"]["answer"]["text"].as_str(), Some(pages[1].as_str()), "{same}");
    let moved = mcp.call("child_status", json!({"alvo": "s0", "offset": 8000, "answer_key": "outra"})).await;
    assert!(failed(&moved) && first_text(&moved).starts_with("child_status_cursor_expired"), "{moved}");
    let past = mcp.call("child_status", json!({"alvo": "s0", "offset": 30000})).await;
    let past = &past["structuredContent"]["answer"];
    assert_eq!((&past["text"], &past["next_offset"]), (&json!(""), &Value::Null), "além do fim: página vazia");
    let other = mcp.call("child_status", json!({"alvo": "s1"})).await;
    assert!(failed(&other) && first_text(&other).starts_with("erro_nao_ligada"), "{other}");
    let interrupt = mcp.call("interrupt", json!({"alvo": "s1"})).await;
    assert!(failed(&interrupt) && first_text(&interrupt).starts_with("erro_interromper_nao_ligada"), "{interrupt}");
}

/// O `/answer` falso atrás do MCP: anota cada `POST` (sessão e corpo) e responde o que o teste deixou em `reply`.
struct AnswerScene { s: Scene, mcp: SocketAddr, seen: Arc<Mutex<Vec<(String, Value)>>>, reply: Arc<Mutex<(u16, Value)>>, ids: [String; 3] }

/// `s0` é filha ligada de `beta` (pane `%7`), com pergunta, menu e permissão abertos ao mesmo tempo (IDs nessa
/// ordem); o MCP fala com um roteador que só tem o `/answer` falso.
async fn answer_scene() -> AnswerScene {
    use hangar_server::interactions::{InteractionKind, native_target};
    let s = scene_owned().await;
    let seen: Arc<Mutex<Vec<(String, Value)>>> = Arc::default();
    let reply = Arc::new(Mutex::new((200, json!({"ok": true, "fallback": false}))));
    let (log, answer) = (seen.clone(), reply.clone());
    let api = axum::Router::new().route("/api/sessions/{name}/answer", axum::routing::post(
        move |axum::extract::Path(name): axum::extract::Path<String>, body: String| {
            let (log, answer) = (log.clone(), answer.clone());
            async move {
                log.lock().unwrap().push((name, serde_json::from_str(&body).unwrap()));
                let (status, value) = answer.lock().unwrap().clone();
                (axum::http::StatusCode::from_u16(status).unwrap(), [("content-type", "application/json")], value.to_string())
            }
        }));
    let app = hangar_server::mcp::router(s.state.clone(), api).into_make_service_with_connect_info::<SocketAddr>();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let mcp = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    write_json(&s.home.join(".claude/.hangar-pair/s0.json"), json!({"peers": ["beta"], "task": "", "gid": "g1", "harness": {}, "linked_by": "beta"}));
    let binding = s.state.state.runtime.get().unwrap().interaction_binding("s0").await.unwrap();
    let store = &s.state.interactions;
    store.bind(&binding).unwrap();
    let shown = |kind: InteractionKind, request: &str| store
        .observe(&binding, &native_target(kind, &binding.conversation, &json!(request)), Some(json!({"pedido": request}))).unwrap().unwrap();
    let ids = [shown(InteractionKind::Question, "ask"), shown(InteractionKind::Menu, "menu"), shown(InteractionKind::Approval, "perm")];
    AnswerScene { s, mcp, seen, reply, ids }
}

/// `child_answer` só responde a pergunta da filha ligada, pela rota `/answer`; menu e permissão são do dono e
/// nem chegam à rota. ID trocado, voz e filha de outra sessão são recusados.
#[tokio::test(flavor = "multi_thread")]
async fn child_answer_answers_a_question_and_never_a_menu_or_permission() {
    let scene = answer_scene().await;
    let [question, menu, permission] = scene.ids.clone();
    let answers = json!([{"kind": "option", "indices": [0], "multi": false, "labels": ["sim"]}]);
    let mut mcp = Mcp::open(scene.mcp, &[("x-hangar-pane", "%7")]).await;
    let answered = mcp.call("child_answer", json!({"alvo": "s0", "interaction_id": question, "answers": answers})).await;
    assert!(!failed(&answered), "{answered}");
    assert_eq!(answered["structuredContent"]["ok"], true, "{answered}");
    assert_eq!(scene.seen.lock().unwrap().clone(), vec![("s0".to_owned(), json!({"answers": answers, "interaction_id": question}))]);
    for (id, what) in [(&menu, "menu"), (&permission, "permissão")] {
        let refused = mcp.call("child_answer", json!({"alvo": "s0", "interaction_id": id, "answers": answers})).await;
        assert!(failed(&refused) && first_text(&refused).starts_with("erro_permissao_do_dono"), "{what}: {refused}");
    }
    assert_eq!(scene.seen.lock().unwrap().len(), 1, "menu e permissão não chamam a rota");
    // ID que não está mais aberto (trocado, ou respondido pelo dono): a recusa é a da rota, repassada.
    *scene.reply.lock().unwrap() = (409, json!({"detail": {"code": "erro_interacao_desatualizada", "msg": "Essa pergunta mudou ou já foi encerrada."}}));
    let stale = mcp.call("child_answer", json!({"alvo": "s0", "interaction_id": "velho", "answers": answers})).await;
    assert!(failed(&stale) && first_text(&stale).starts_with("erro_interacao_desatualizada"), "{stale}");
    let other = mcp.call("child_answer", json!({"alvo": "s1", "interaction_id": question, "answers": answers})).await;
    assert!(failed(&other) && first_text(&other).starts_with("erro_nao_ligada"), "{other}");
    let voice = hangar_server::voice::machines::voice_signature(SECRET).unwrap();
    let spoken = Mcp::open(scene.mcp, &[("x-hangar-voice", &voice)]).await
        .call("child_answer", json!({"alvo": "s0", "interaction_id": question, "answers": answers})).await;
    assert!(failed(&spoken) && first_text(&spoken).contains("voz"), "{spoken}");
    assert_eq!(scene.seen.lock().unwrap().len(), 2, "só a pergunta e o ID velho chegaram à rota");
    drop(scene.s);
}

/// Codex com terminal: a rota escreve a resposta e diz 409 `erro_sem_confirmacao_resposta`, porque o Codex não conta
/// quem resolveu. A tool devolve "enviada, sem confirmação" como resultado, depois de um `POST` só.
#[tokio::test(flavor = "multi_thread")]
async fn child_answer_reports_an_unconfirmed_codex_answer_as_sent() {
    let scene = answer_scene().await;
    *scene.reply.lock().unwrap() = (409, json!({"detail": {"code": "erro_sem_confirmacao_resposta", "msg": "sem confirmação"}}));
    let mut mcp = Mcp::open(scene.mcp, &[("x-hangar-pane", "%7")]).await;
    let answers = json!([{"kind": "text", "value": "segue", "type_index": 2, "labels": ["segue"]}]);
    let sent = mcp.call("child_answer", json!({"alvo": "s0", "interaction_id": scene.ids[0], "answers": answers})).await;
    assert!(!failed(&sent), "{sent}");
    let got = &sent["structuredContent"];
    assert_eq!((&got["sent"], &got["confirmed"]), (&json!(true), &json!(false)), "{sent}");
    assert!(got["message"].as_str().is_some_and(|m| m.starts_with("enviada, sem confirmação") && m.contains("não tente de novo")), "{sent}");
    assert_eq!(scene.seen.lock().unwrap().len(), 1, "um POST só");
    drop(scene.s);
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
    assert!(failed(&closed) && first_text(&closed).contains("não está aberto nesta máquina"), "{closed}");
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
async fn browser_open_records_the_request_in_rust_and_waits_for_the_page() {
    let s = scene().await;
    let seen = desktop(&s.home, |verb| if verb == "url" { "ok: \"http://x/\"".into() } else { "ativo".into() }).await;
    // Fixo e depois de qualquer pedido: a página carregou depois dele.
    let later: u64 = 4_102_444_800_000;
    write_json(&s.home.join(".hangar/nav/casa-beta.json"), json!({"chave": "casa::beta", "url": "http://x/", "ts": later}));
    let mut mcp = Mcp::open(s.addr, &[("x-hangar-pane", "%7")]).await;
    let result = mcp.call("browser_open", json!({"url": "http://x"})).await;
    assert_eq!(result["structuredContent"], json!({"ok": true, "url": "http://x/"}), "{result}");
    // O arquivo dos pendentes leva a url.
    let file = s.home.join(".hangar/nav/_pendentes.json");
    fake::wait_until(move || std::fs::read(&file).ok()
        .and_then(|raw| serde_json::from_slice::<Value>(&raw).ok())
        .is_some_and(|v| v["beta"]["url"] == "http://x")).await;
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
    let s = scene_owned().await;
    desktop(&s.home, |verb| format!("ok: {verb}")).await;
    let voice = hangar_server::voice::machines::voice_signature(SECRET).unwrap();
    let mut mcp = Mcp::open(s.addr, &[("x-hangar-voice", &voice)]).await;
    assert!(!failed(&mcp.call("send", json!({"alvo": "s1", "texto": "terminei"})).await));
    assert_eq!(s.sink.last("s1").as_deref(), Some("[de: voz] terminei"));
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

/// Outra execução do app publicou o servidor dela e morreu: o arquivo aponta um pid sem processo.
#[cfg(target_os = "linux")]
#[tokio::test(flavor = "multi_thread")]
async fn browser_open_with_a_dead_desktop_server_fails_at_once() {
    let s = scene().await;
    let mut child = std::process::Command::new("true").spawn().unwrap();
    let dead = child.id();
    child.wait().unwrap();
    let port = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
    write_json(&s.home.join(".hangar/nav/_srv.json"), json!({"porta": port, "token": "t", "pid": dead}));
    write_json(&s.home.join(".hangar/nav/casa-beta.json"), json!({"chave": "casa::beta", "url": "http://antes/", "ts": 0}));
    let mut mcp = Mcp::open(s.addr, &[("x-hangar-pane", "%7")]).await;
    let result = tokio::time::timeout(Duration::from_secs(10), mcp.call("browser_open", json!({"url": "http://x"}))).await
        .expect("a tool esperou o servidor morto em vez de falhar logo");
    let text = first_text(&result);
    assert!(failed(&result) && text.contains(&format!("pid {dead}")) && text.contains("precisa abrir o app"), "{text}");
    let verb = mcp.call("browser", json!({"verbo": "snapshot"})).await;
    assert!(failed(&verb) && first_text(&verb).contains(&format!("pid {dead}")), "{verb}");
}

/// O app desktop de outra máquina com `session` em foco, pelo canal anfitrião: anota cada pedido e responde o texto de `answer`.
async fn remote_app(addr: SocketAddr, session: &str, answer: impl Fn(&Value) -> String + Send + 'static) -> Arc<Mutex<Vec<Value>>> {
    use futures_util::{SinkExt, StreamExt};
    use tokio_tungstenite::tungstenite::Message;
    let (mut ws, _) = tokio_tungstenite::connect_async(format!("ws://{addr}/api/browser-host?token={OWNER}")).await.expect("canal anfitrião");
    ws.send(Message::Text(json!({"type": "focus", "session": session, "caps": ["fill_secret"]}).to_string().into())).await.unwrap();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let log = seen.clone();
    tokio::spawn(async move {
        while let Some(Ok(message)) = ws.next().await {
            let Message::Text(text) = message else { continue };
            let asked: Value = serde_json::from_str(&text).unwrap();
            let reply = json!({"id": asked["id"], "ok": true, "text": answer(&asked)});
            log.lock().unwrap().push(asked);
            if ws.send(Message::Text(reply.to_string().into())).await.is_err() { break; }
        }
    });
    seen
}

/// O app daqui caiu (pid morto no `_srv.json`): sem ninguém lá, a recusa diz que ele reinicia; com o app de outra
/// máquina mostrando a sessão, mesmo sem uso registrado, o verbo vai a ele.
#[cfg(target_os = "linux")]
#[tokio::test(flavor = "multi_thread")]
async fn a_dead_desktop_app_here_hands_the_browser_to_the_app_that_shows_the_session() {
    let s = scene().await;
    let mut child = std::process::Command::new("true").spawn().unwrap();
    let dead = child.id();
    child.wait().unwrap();
    write_json(&s.home.join(".hangar/nav/_srv.json"), json!({"porta": 1, "token": "t", "pid": dead}));
    write_json(&s.home.join(".hangar/nav/casa-beta.json"), json!({"chave": "casa::beta", "url": "http://antes/", "ts": 0}));
    let mut mcp = Mcp::open(s.addr, &[("x-hangar-pane", "%7")]).await;
    let alone = mcp.call("browser", json!({"verbo": "url"})).await;
    assert!(failed(&alone) && first_text(&alone).contains(&format!("pid {dead}")), "{alone}");
    let asked = remote_app(s.addr, "beta", |_| "ok: \"http://localhost:3000/\"".into()).await;
    let state = s.state.clone();
    fake::wait_until(move || state.browser_host.has_host("beta")).await;
    let url = mcp.call("browser", json!({"verbo": "url"})).await;
    assert_eq!(first_text(&url), "ok: \"http://localhost:3000/\"", "{url}");
    let verbs: Vec<Value> = asked.lock().unwrap().iter().map(|a| a["verbo"].clone()).collect();
    assert_eq!(verbs, [json!("url")]);
}

/// O cofre da cena, com a chave na memória.
#[derive(Default)]
struct Keys(Mutex<Option<[u8; 32]>>);

impl hangar_server::vault::KeyStore for Keys {
    fn read(&self) -> futures_util::future::BoxFuture<'_, Result<Option<hangar_server::vault::Key>, hangar_server::vault::KeyError>> {
        let key = self.0.lock().unwrap().map(zeroize::Zeroizing::new);
        Box::pin(async move { Ok(key) })
    }
    fn write(&self, key: hangar_server::vault::Key) -> futures_util::future::BoxFuture<'_, Result<(), hangar_server::vault::KeyError>> {
        *self.0.lock().unwrap() = Some(*key);
        Box::pin(async { Ok(()) })
    }
}

/// Com a sessão no app de outra máquina, o `vault_fill` lê a página pelo canal e manda o valor por dentro do servidor:
/// o app recebe o valor e o domínio aprovado; a resposta, o `list` e a rota `browser-cmd` nunca o levam. Página que
/// trocou de domínio entre as duas leituras não recebe nada.
#[tokio::test(flavor = "multi_thread")]
async fn vault_fill_reaches_the_app_of_another_machine_inside_the_server() {
    const USER: &str = "usuario-do-cofre-9Qz";
    let s = scene_tuned(false, None, false, "", Share::Absent, |st| {
        st.hangar_dir = st.origins_home.join(".hangar");
        st.vault = Arc::new(hangar_server::vault::Vault::new(Arc::new(Keys::default())));
    }).await;
    s.state.vault.create(&s.state.hangar_dir).await.unwrap();
    s.state.vault.add(&s.state.hangar_dir, hangar_server::vault::NewEntry { name: "gh".into(), username: Some(USER.into()),
        password: zeroize::Zeroizing::new("senha-nao-usada".into()), url: Some("https://github.com".into()), notes: None,
        policy: hangar_server::vault::Policy::Ask }).await.unwrap();
    // As páginas que o `url` responde, uma por leitura; a última se repete.
    let pages = Arc::new(Mutex::new(vec!["https://github.com/login".to_owned()]));
    let shown = pages.clone();
    // O que o app responde ao preenchimento; `None` = deu certo.
    let fill_reply = Arc::new(Mutex::new(None::<String>));
    let replied = fill_reply.clone();
    let asked = remote_app(s.addr, "sem-term", move |a| match a["verbo"].as_str() {
        Some("url") => {
            let mut pages = shown.lock().unwrap();
            let page = if pages.len() > 1 { pages.remove(0) } else { pages[0].clone() };
            format!("ok: {}", Value::String(page))
        }
        Some("fill-secret") => replied.lock().unwrap().clone().unwrap_or_else(|| format!("ok: fill-secret {}", a["args"][0].as_str().unwrap_or_default())),
        _ => "ok".into(),
    }).await;
    let state = s.state.clone();
    fake::wait_until(move || state.browser_host.has_host("sem-term")).await;
    let mut mcp = Mcp::open(s.addr, &[("x-hangar-key", "k123")]).await;
    let fill = json!({"entry": "gh", "field": "username", "ref": "@e3", "reason": "login"});
    let filled = mcp.call("vault_fill", fill.clone()).await;
    assert!(!failed(&filled) && first_text(&filled) == "preenchido", "{filled}");
    assert!(!filled.to_string().contains(USER));
    let frames = asked.lock().unwrap().clone();
    let sent = frames.iter().find(|f| f["verbo"] == "fill-secret").expect("o app recebe o preenchimento");
    assert_eq!((sent["valor"].as_str(), sent["expected_host"].as_str()), (Some(USER), Some("github.com")));
    assert_eq!(sent["args"], json!(["@e3", "username"]));
    assert!(frames.iter().filter(|f| f["verbo"] != "fill-secret").all(|f| !f.to_string().contains(USER)), "o valor só vai no quadro do preenchimento");

    let route = |body: Value| client().post(format!("http://{}/api/sessions/sem-term/browser-cmd", s.addr)).bearer_auth(OWNER).json(&body).send();
    let list = route(json!({"verbo": "list"})).await.unwrap().text().await.unwrap();
    assert!(!list.contains(USER), "{list}");
    let refused = route(json!({"verbo": "fill-secret", "args": ["@e3", "username"], "valor": USER})).await.unwrap();
    assert_eq!(refused.status(), 422);
    let body = refused.text().await.unwrap();
    assert!(body.contains("browser_host_secret_refused") && !body.contains(USER), "{body}");

    // A página trocou de domínio entre a leitura de antes da aprovação e a de antes do envio.
    *pages.lock().unwrap() = vec!["https://github.com/login".into(), "https://evil.io/login".into()];
    let before = asked.lock().unwrap().len();
    let moved = mcp.call("vault_fill", fill).await;
    assert!(failed(&moved) && first_text(&moved).starts_with("erro_cofre_dominio_diferente:"), "{moved}");
    assert!(!moved.to_string().contains(USER));
    assert!(asked.lock().unwrap()[before..].iter().all(|f| f["verbo"] != "fill-secret"), "a página de outro domínio não recebe o valor");

    // O app cortou o preenchimento depois de digitar: a tool repete o que ele disse, nem "preenchido" nem falha limpa.
    *pages.lock().unwrap() = vec!["https://github.com/login".into()];
    let uncertain = "erro_cofre_incerto: fill-secret não confirmou em 6000 ms; o valor pode ter sido digitado (preso em Input.insertText)";
    *fill_reply.lock().unwrap() = Some(format!("erro: {uncertain}"));
    let cut = mcp.call("vault_fill", json!({"entry": "gh", "field": "username", "ref": "@e3", "reason": "login"})).await;
    assert!(failed(&cut) && first_text(&cut) == uncertain, "{cut}");
    assert!(!cut.to_string().contains(USER));
}

#[tokio::test(flavor = "multi_thread")]
async fn mission_reads_and_reports_as_the_linked_caller() {
    let s = scene_owned().await;
    let mut mcp = Mcp::open(s.addr, &[("x-hangar-pane", "%7")]).await;
    let unlinked = mcp.call("mission", json!({})).await;
    assert!(failed(&unlinked) && first_text(&unlinked).starts_with("mission_session_not_linked"), "{unlinked}");

    let linked = client().put(format!("http://{}/api/mission/items/help/PM-7", s.addr)).bearer_auth(OWNER)
        .json(&json!({"key": "PM-7", "title": "Tela", "url": "https://help.invalid/pm/PM-7", "session": "beta"})).send().await.unwrap();
    assert_eq!(linked.status(), 200);
    // O aviso do vínculo entra pela rota de entrada da sessão, numa tarefa à parte.
    tokio::time::timeout(Duration::from_secs(30), async { while s.sink.count("beta")==0 { tokio::task::yield_now().await } }).await
        .expect("o aviso do vínculo não chegou em 30 s");
    assert_eq!(s.sink.count("beta"),1,"a vida associada recebe o aviso pela entrada real");
    let read = mcp.call("mission", json!({})).await;
    assert!(!failed(&read), "{read}");
    assert_eq!(read["structuredContent"]["item"]["key"], "PM-7");
    assert_eq!(read["structuredContent"]["stages"][3]["gate"], true);

    let shot = s.home.join("tela.png");
    std::fs::write(&shot, b"png").unwrap();
    let reported = mcp.call("mission", json!({"etapa": "validacao", "nota": "pronto para conferir",
        "evidencias": [{"caminho": shot, "legenda": "botão novo"}]})).await;
    assert!(!failed(&reported), "{reported}");
    assert_eq!(reported["structuredContent"]["item"]["stage"], "validacao");
    assert_eq!(reported["structuredContent"]["item"]["evidence"][0]["by"], "beta");
    let gated = mcp.call("mission", json!({"etapa": "entrega"})).await;
    assert!(failed(&gated) && first_text(&gated).starts_with("mission_gate_pending"), "{gated}");
}

#[tokio::test(flavor="multi_thread")]
async fn mission_follows_a_federated_group_and_copies_remote_evidence_to_its_owner() {
    use hangar_server::groups::peers::{PeerClient,PeerBook};
    let links=tempfile::tempdir().unwrap();
    let (owner_book,worker_book)=(links.path().join("owner.json"),links.path().join("worker.json"));
    let owner=scene_named(true,Some(PeerClient::new(PeerBook::new(Some(owner_book.clone())))),false,"casa").await;
    let worker=scene_named(true,Some(PeerClient::new(PeerBook::new(Some(worker_book.clone())))),true,"lab").await;
    write_json(&owner_book,json!({"lab":{"base_url":format!("http://{}",worker.addr),"token":OWNER}}));
    write_json(&worker_book,json!({"casa":{"base_url":format!("http://{}",owner.addr),"token":OWNER}}));
    owner.state.mission.link_owner("help","T-1",hangar_mission::store::Link {key:"T-1".into(),title:"Trabalho em grupo".into(),url:"https://example.invalid/1".into(),session:"s0".into()},Some("dddddddddddddddddddddddddddddddd"),&[]).unwrap();
    let response=client().post(format!("http://{}/api/sessions/s0/pair",owner.addr)).bearer_auth(OWNER)
        .json(&json!({"peers":["lab::s1","lab::beta"],"task":"T-1"})).send().await.unwrap();
    let status=response.status();let body=response.text().await.unwrap();assert_eq!(status,200,"{body}");
    let mut mcp=Mcp::open(worker.addr,&[("x-hangar-session","s1")]).await;
    let read=mcp.call("mission",json!({})).await;
    assert!(!failed(&read),"{read}");
    assert_eq!(read["structuredContent"]["item"]["key"],"T-1");
    let source=worker.home.join("evidence.txt");
    std::fs::write(&source,"prova da outra máquina".as_bytes()).unwrap();
    let reported=mcp.call("mission",json!({"etapa":"execucao","nota":"Módulo concluído","evidencias":[{"caminho":source,"legenda":"Resultado remoto"}]})).await;
    assert!(!failed(&reported),"{reported}");
    let item=owner.state.mission.snapshot().unwrap().items.remove(0);
    assert_eq!(item.stage.as_deref(),Some("execucao"));
    assert_eq!(item.evidence[0].by,"lab::s1");
    let (file,_)=owner.state.mission.evidence_file(&item.evidence[0].id).unwrap();
    assert_eq!(std::fs::read(file).unwrap(),"prova da outra máquina".as_bytes());
    assert!(worker.state.mission.snapshot().unwrap().items.is_empty(),"o estado do ticket tem uma máquina dona");
    let response=client().delete(format!("http://{}/api/sessions/s0/pair",owner.addr)).bearer_auth(OWNER).send().await.unwrap();
    let status=response.status();let body=response.text().await.unwrap();assert_eq!(status,200,"{body}");
    let final_note=mcp.call("mission",json!({"nota":"Nota depois de sair a coordenadora"})).await;
    let life=worker.state.list.snapshot().await.unwrap().rows.iter().find(|row|row.name=="s1").and_then(|row|row.lifecycle_id.clone());
    let known=life.as_ref().map(|life|worker.state.mission.remote_owner("s1",life).unwrap());
    assert!(!failed(&final_note),"relato={final_note:?}; vida={life:?}; rota={known:?}");
}

#[tokio::test(flavor = "multi_thread")]
async fn the_router_does_not_keep_the_state_alive() {
    // O entregador de avisos do Mission Control mora no estado; prender o roteador nele seguraria o estado (e os
    // arquivos abertos) depois do fim do servidor.
    let state = Arc::new(AppState::new(fake::config("127.0.0.1")));
    let before = Arc::strong_count(&state);
    let app = router(state.clone());
    drop(app);
    assert_eq!(Arc::strong_count(&state), before);
}

// `GET /api/whoami`: a mesma identidade do `who_am_i`, pela rota HTTP do dono. O esperado vem da
// captura do Python (`quem_chama.py`, commit 1504c6c53).
const UNKNOWN_KEY: &str = "chave de sessão sem terminal não corresponde a nenhum sidecar";
const BAD_VOICE: &str = "assinatura da voz inválida (o hangar-server reiniciou ou não é a voz)";

async fn whoami(s: &Scene, token: Option<&str>, headers: &[(&str, &str)]) -> (u16, Value) {
    let mut req = client().get(format!("http://{}/api/whoami", s.addr));
    if let Some(token) = token { req = req.bearer_auth(token); }
    for (k, v) in headers { req = req.header(*k, *v); }
    let resp = req.send().await.unwrap();
    let status = resp.status().as_u16();
    (status, resp.json().await.unwrap_or(Value::Null))
}

fn found(name: &str, origin: &str) -> (u16, Value) { (200, json!({"name": name, "origem": origin})) }

fn unknown(msg: &str) -> (u16, Value) {
    (404, json!({"detail": {"code": "erro_sessao_desconhecida", "params": {}, "msg": msg}}))
}

#[tokio::test(flavor = "multi_thread")]
async fn whoami_w01_voice_signature_wins_and_a_bad_one_never_falls_through() {
    let s = scene().await;
    let voice = hangar_server::voice::machines::voice_signature(SECRET).unwrap();
    assert_eq!(whoami(&s, Some(OWNER), &[("x-hangar-voice", &voice), ("x-hangar-session", "s0")]).await, found("voz", "voz"));
    let forged = hangar_server::voice::machines::voice_signature("outro").unwrap();
    for bad in [forged.as_str(), "n1.dead", "semponto"] {
        assert_eq!(whoami(&s, Some(OWNER), &[("x-hangar-voice", bad), ("x-hangar-session", "s0")]).await, unknown(BAD_VOICE), "{bad}");
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn whoami_w02_key_of_a_sidecar_beats_the_inherited_pane() {
    let s = scene().await;
    write_json(&s.home.join(".hangar/codex-sessions/cx-1.json"), json!({"name": "cx-1", "key": "cccccccccccccccccccccccccccccccc"}));
    assert_eq!(whoami(&s, Some(OWNER), &[("x-hangar-key", "k123"), ("x-hangar-pane", "%7")]).await, found("sem-term", "chave"));
    assert_eq!(whoami(&s, Some(OWNER), &[("x-hangar-key", "cccccccccccccccccccccccccccccccc")]).await, found("cx-1", "chave"));
}

#[tokio::test(flavor = "multi_thread")]
async fn whoami_w03_key_without_sidecar_or_record_is_404_and_never_the_pane() {
    let s = scene().await;
    assert_eq!(whoami(&s, Some(OWNER), &[("x-hangar-key", "nada"), ("x-hangar-pane", "%7")]).await, unknown(UNKNOWN_KEY));
    // O registro existe em minúsculas; a chave em maiúsculas não o acha (a captura do Python).
    let key = "eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee";
    write_json(&s.home.join(format!(".hangar/session-specs/{key}.json")), json!({}));
    assert_eq!(whoami(&s, Some(OWNER), &[("x-hangar-key", &key.to_uppercase()), ("x-hangar-pane", "%7")]).await, unknown(UNKNOWN_KEY));
}

#[tokio::test(flavor = "multi_thread")]
async fn whoami_w04_key_of_a_terminal_session_goes_on_to_the_pane() {
    let s = scene().await;
    let key = "0123456789abcdef0123456789abcdef";
    write_json(&s.home.join(format!(".hangar/session-specs/{key}.json")), json!({}));
    assert_eq!(whoami(&s, Some(OWNER), &[("x-hangar-key", key), ("x-hangar-pane", "%7")]).await, found("beta", "pane"));
    // Diferença aceita (decisão 14): a chave no ambiente da sessão resolve antes do pane repetido.
    let renamed = "fedcba9876543210fedcba9876543210";
    write_json(&s.home.join(format!(".hangar/session-specs/{renamed}.json")), json!({}));
    assert_eq!(whoami(&s, Some(OWNER), &[("x-hangar-key", renamed), ("x-hangar-pane", "%1")]).await, found("s1", "chave"));
}

#[tokio::test(flavor = "multi_thread")]
async fn whoami_w05_only_a_unique_pane_resolves() {
    let s = scene().await;
    assert_eq!(whoami(&s, Some(OWNER), &[("x-hangar-pane", "%7")]).await, found("beta", "pane"));
    assert_eq!(whoami(&s, Some(OWNER), &[("x-hangar-pane", "%1"), ("x-hangar-session", "s0")]).await, found("s0", "nome"));
    assert_eq!(whoami(&s, Some(OWNER), &[("x-hangar-pane", "%1")]).await, unknown(hangar_server::mcp::identity::HINT));
}

#[tokio::test(flavor = "multi_thread")]
async fn whoami_w06_name_only_of_a_live_session() {
    let s = scene().await;
    assert_eq!(whoami(&s, Some(OWNER), &[("x-hangar-session", "sem-term")]).await, found("sem-term", "nome"));
    assert_eq!(whoami(&s, Some(OWNER), &[("x-hangar-session", "s1")]).await, found("s1", "nome"));
    for dead in ["zeta", "ção"] {
        assert_eq!(whoami(&s, Some(OWNER), &[("x-hangar-session", dead)]).await, unknown(hangar_server::mcp::identity::HINT), "{dead}");
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn whoami_w07_nothing_resolved_is_the_404_with_the_hint() {
    let s = scene().await;
    assert_eq!(whoami(&s, Some(OWNER), &[]).await, unknown(hangar_server::mcp::identity::HINT));
}

#[tokio::test(flavor = "multi_thread")]
async fn whoami_w08_blank_headers_are_absent_and_values_are_trimmed() {
    let s = scene().await;
    assert_eq!(whoami(&s, Some(OWNER), &[("x-hangar-key", "   "), ("x-hangar-session", "s0")]).await, found("s0", "nome"));
    assert_eq!(whoami(&s, Some(OWNER), &[("X-Hangar-Session", "  s0  ")]).await, found("s0", "nome"));
}

#[tokio::test(flavor = "multi_thread")]
async fn whoami_w09_only_who_has_access_and_never_forwarded() {
    let s = scene().await;
    let refused = json!({"detail": {"code": "erro_nao_autorizado", "params": {}, "msg": "unauthorized"}});
    for token in [None, Some("outro")] {
        assert_eq!(whoami(&s, token, &[("x-hangar-session", "s0")]).await, (401, refused.clone()), "{token:?}");
    }
    // Cabeçalho de convidado forjado por quem não é loopback não vale como convidado.
    let (status, _) = whoami(&s, None, &[("x-hangar-guest", "eyJzIjoge319.00"), ("x-hangar-session", "s0")]).await;
    assert_eq!(status, 401);
    let by_query = client().get(format!("http://{}/api/whoami?token={OWNER}", s.addr)).header("x-hangar-session", "s0").send().await.unwrap();
    assert_eq!(by_query.status(), 200);
    let by_cookie = client().get(format!("http://{}/api/whoami", s.addr)).header("cookie", format!("cp_token={OWNER}"))
        .header("x-hangar-session", "s0").send().await.unwrap();
    assert_eq!(by_cookie.status(), 200);
}

#[tokio::test(flavor = "multi_thread")]
async fn whoami_w10_a_mux_that_does_not_answer_is_no_pane_not_a_500() {
    let s = scene().await;
    crate::write_executable(&s._dir.path().join("tmux"), "#!/bin/sh\nexit 1\n");
    assert_eq!(whoami(&s, Some(OWNER), &[("x-hangar-pane", "%7")]).await, unknown(hangar_server::mcp::identity::HINT));
    assert_eq!(whoami(&s, Some(OWNER), &[("x-hangar-pane", "%7"), ("x-hangar-session", "sem-term")]).await, found("sem-term", "nome"));
}

/// W-09: `x-hangar-guest` não vale nada na 8765; quem chega com ele e sem token é recusado como estranho.
#[tokio::test(flavor = "multi_thread")]
async fn whoami_w09_a_signed_guest_header_alone_is_refused() {
    let s = scene().await;
    // O formato que o Rust assinava para o Python (`<payload>.<hmac>`): ninguém mais o honra.
    let header = "eyJzIjp7InMwIjoic2hhcmUifSwiZXhwIjo0MTAyNDQ0ODAwfQ.00";
    let refused = json!({"detail": {"code": "erro_nao_autorizado", "params": {}, "msg": "unauthorized"}});
    assert_eq!(whoami(&s, None, &[("x-hangar-guest", header), ("x-hangar-session", "s0")]).await, (401, refused));
    let (status, _) = whoami(&s, None, &[("x-hangar-guest", header), ("x-forwarded-for", "203.0.113.5"),
        ("x-hangar-session", "s0")]).await;
    assert_eq!(status, 401);
}

/// O `whoami` com o convidado já decidido (convite ou login), como o porteiro o entrega ao roteador.
async fn whoami_as_guest(s: &Scene, sessions: &[(&str, hangar_core::guest::Kind)], session: &str) -> (u16, Value) {
    use tower::ServiceExt;
    let mut request = axum::http::Request::builder().uri("/api/whoami").header("x-hangar-session", session)
        .body(axum::body::Body::empty()).unwrap();
    request.extensions_mut().insert(axum::extract::ConnectInfo::<SocketAddr>("127.0.0.1:1234".parse().unwrap()));
    request.extensions_mut().insert(hangar_core::guest::Caller::Guest(Arc::new(hangar_core::guest::GuestGrant {
        token: "convidado".into(), token_hash: "convidado-hash".into(), pair_roots: vec![], invite: true,
        sessions: sessions.iter().map(|(name, kind)| ((*name).to_owned(), *kind)).collect() })));
    let response = router(s.state.clone()).oneshot(request).await.unwrap();
    let status = response.status().as_u16();
    let body = axum::body::to_bytes(response.into_body(), 1 << 20).await.unwrap();
    (status, serde_json::from_slice(&body).unwrap_or(Value::Null))
}

/// W-09: o convidado só fica sabendo do nome das sessões que alcança; a viva fora do convite leva o
/// mesmo 404 da que não existe, sem revelar que existe.
#[tokio::test(flavor = "multi_thread")]
async fn whoami_w09_a_guest_learns_only_the_sessions_it_reaches() {
    use hangar_core::guest::Kind;
    let s = scene().await;
    let hint = hangar_server::mcp::identity::HINT;
    assert_eq!(whoami_as_guest(&s, &[("s1", Kind::Share)], "s0").await, unknown(hint), "viva, mas fora do convite");
    assert_eq!(whoami_as_guest(&s, &[("s1", Kind::Share)], "zeta").await, unknown(hint), "inexistente: a mesma resposta");
    assert_eq!(whoami_as_guest(&s, &[("s0", Kind::Share)], "s0").await, found("s0", "nome"));
    assert_eq!(whoami_as_guest(&s, &[("s0", Kind::Pair)], "s0").await, found("s0", "nome"), "o par lê");
}

/// Com o serviço desligado, a rota responde `whatsapp_off` e a tool devolve o código e a frase dela.
#[tokio::test(flavor = "multi_thread")]
async fn whatsapp_tools_return_the_route_error_with_its_code() {
    let s = scene_shared(false, None, false, "", true).await;
    let mut mcp = Mcp::open(s.addr, &[("x-hangar-pane", "%0")]).await;
    for (tool, args) in [("whatsapp_chats", json!({})), ("whatsapp_read", json!({"chat": "c@s.whatsapp.net"})),
                         ("whatsapp_search", json!({"query": "oi"})), ("whatsapp_request_media", json!({"chat": "c@s.whatsapp.net"}))] {
        let result = mcp.call(tool, args).await;
        let text = first_text(&result);
        assert!(failed(&result) && text.starts_with("whatsapp_off:") && text.contains("Configurações > WhatsApp"), "{tool}: {result}");
    }
}

/// Sem configuração de convites não há convite nem par: a leitura chega à rota (aqui, `whatsapp_off`).
#[tokio::test(flavor = "multi_thread")]
async fn without_share_configuration_whatsapp_reaches_the_route() {
    let s = scene_share(false, None, false, "", Share::Absent).await;
    assert!(s.state.share.is_none());
    let mut mcp = Mcp::open(s.addr, &[("x-hangar-pane", "%0")]).await;
    let result = mcp.call("whatsapp_chats", json!({})).await;
    assert!(failed(&result) && first_text(&result).starts_with("whatsapp_off:"), "{result}");
}

/// Configuração de convites que não se carregou na subida: ninguém sabe se a sessão é compartilhada, e a
/// leitura é recusada antes da rota, em vez de deixar passar.
#[tokio::test(flavor = "multi_thread")]
async fn with_the_share_configuration_unloaded_whatsapp_is_refused() {
    let s = scene_share(false, None, false, "", Share::Unavailable).await;
    assert!(s.state.share.is_none());
    let mut mcp = Mcp::open(s.addr, &[("x-hangar-pane", "%0")]).await;
    let result = mcp.call("whatsapp_chats", json!({})).await;
    let text = first_text(&result);
    assert!(failed(&result) && text.starts_with("whatsapp_shared_check_failed:")
        && text.contains("estado de compartilhamento indisponível"), "{result}");
}

/// `external_pairs.json` torto na subida vira lista vazia: a lista não prova que não há par de fora, e a
/// leitura é recusada.
#[tokio::test(flavor = "multi_thread")]
async fn with_unreadable_external_pairs_whatsapp_is_refused() {
    let s = scene_share(false, None, false, "", Share::StoreWithBadPairs).await;
    let mut mcp = Mcp::open(s.addr, &[("x-hangar-pane", "%0")]).await;
    let result = mcp.call("whatsapp_chats", json!({})).await;
    let text = first_text(&result);
    assert!(failed(&result) && text.starts_with("whatsapp_shared_check_failed:") && text.contains("pares externos ilegível"), "{result}");
}

/// Sidecar com `fed` torto não vale como grupo de uma máquina: o registro da dona não seria lido, e a leitura
/// é recusada. Sem `fed`, o grupo de uma máquina segue até a rota.
#[tokio::test(flavor = "multi_thread")]
async fn a_group_with_an_unreadable_fed_does_not_read_whatsapp() {
    let s = scene_shared(true, None, false, "", true).await;
    let sidecar = s.home.join(".claude/.hangar-pair/sem-term.json");
    write_json(&sidecar, json!({"peers": ["s1"], "task": "", "gid": "g1", "harness": {}, "fed": {"owner": 7}}));
    let mut mcp = Mcp::open(s.addr, &[("x-hangar-key", "k123")]).await;
    let result = mcp.call("whatsapp_chats", json!({})).await;
    let text = first_text(&result);
    assert!(failed(&result) && text.starts_with("whatsapp_shared_check_failed:") && text.contains("grupo da sessão ilegível"), "{result}");

    write_json(&sidecar, json!({"peers": ["s1"], "task": "", "gid": "g1", "harness": {}}));
    let result = mcp.call("whatsapp_chats", json!({})).await;
    assert!(failed(&result) && first_text(&result).starts_with("whatsapp_off:"), "grupo de uma máquina: {result}");
}

/// Convite de convidado vivo na sessão, ou par de fora no grupo dela: recusa antes da rota. A sessão sem
/// nada disso passa e chega à rota (aqui, `whatsapp_off`).
#[tokio::test(flavor = "multi_thread")]
async fn a_shared_session_does_not_read_whatsapp() {
    let s = scene_shared(true, None, false, "", true).await;
    let share = s.state.share.clone().unwrap();
    share.store.create("s0", None, hangar_server::share::store::Kind::Share, false).unwrap();
    let mut invited = Mcp::open(s.addr, &[("x-hangar-pane", "%0")]).await;
    let result = invited.call("whatsapp_read", json!({"chat": "c@s.whatsapp.net"})).await;
    assert!(failed(&result) && first_text(&result).starts_with("whatsapp_shared_session:"), "{result}");

    share.pairs.add(hangar_server::share::pairs::ExternalPair { share_id: "sh1".into(), local_session: "outra".into(),
        alias: "ana".into(), peer_owner: "Ana".into(), peer_session: "dela".into(), peer_address: "https://ana.tail.ts.net:8443".into(),
        peer_token: "t".repeat(24), created_at: 1.0 }).unwrap();
    write_json(&s.home.join(".claude/.hangar-pair/sem-term.json"), json!({"peers": ["ana::dela"], "task": "", "gid": "g1", "harness": {}}));
    let mut grouped = Mcp::open(s.addr, &[("x-hangar-key", "k123")]).await;
    let result = grouped.call("whatsapp_chats", json!({})).await;
    assert!(failed(&result) && first_text(&result).starts_with("whatsapp_shared_session:"), "par de fora no grupo: {result}");

    std::fs::remove_file(s.home.join(".claude/.hangar-pair/sem-term.json")).unwrap();
    let result = grouped.call("whatsapp_chats", json!({})).await;
    assert!(failed(&result) && first_text(&result).starts_with("whatsapp_off:"), "sem convidado nem par, a rota responde: {result}");
}

/// O par do grupo nesta máquina recebe o que a sessão lê: convite nele, ou convidado com login que o vê,
/// recusa a leitura de quem está pareada com ele.
#[tokio::test(flavor = "multi_thread")]
async fn a_session_paired_with_a_shared_one_does_not_read_whatsapp() {
    let s = scene_shared(true, None, false, "", true).await;
    write_json(&s.home.join(".claude/.hangar-pair/s0.json"), json!({"peers": ["s1"], "task": "", "gid": "g1", "harness": {}}));
    let mut mcp = Mcp::open(s.addr, &[("x-hangar-pane", "%0")]).await;
    let result = mcp.call("whatsapp_chats", json!({})).await;
    assert!(failed(&result) && first_text(&result).starts_with("whatsapp_off:"), "par sem convite nem convidado: {result}");

    let share = s.state.share.clone().unwrap();
    let (invite, _) = share.store.create("s1", None, hangar_server::share::store::Kind::Share, false).unwrap();
    let result = mcp.call("whatsapp_chats", json!({})).await;
    assert!(failed(&result) && first_text(&result).starts_with("whatsapp_shared_session:"), "convite no par: {result}");
    share.store.revoke(&invite.id).unwrap();

    let guests = s.state.login_guests.store.clone();
    guests.create("Ana", &s.home.to_string_lossy(), false, true, "ana".into(), "token-ana".into()).unwrap();
    guests.claim("s1", "ana", None).unwrap();
    let result = mcp.call("whatsapp_chats", json!({})).await;
    assert!(failed(&result) && first_text(&result).starts_with("whatsapp_shared_session:"), "convidado com login vê o par: {result}");
}

/// Convidado com login que vê as sessões do dono recusa; o que só tem a raiz cobrindo a pasta da sessão, sem
/// vê-las, não alcança a conversa e deixa passar até a rota.
#[tokio::test(flavor = "multi_thread")]
async fn a_session_a_login_guest_sees_does_not_read_whatsapp() {
    let s = scene_shared(false, None, false, "", true).await;
    let guests = s.state.login_guests.store.clone();
    let root = s.home.to_string_lossy().into_owned();
    guests.create("Ana", &root, false, true, "ana".into(), "token-ana".into()).unwrap();
    let mut mcp = Mcp::open(s.addr, &[("x-hangar-pane", "%0")]).await;
    let result = mcp.call("whatsapp_chats", json!({})).await;
    assert!(failed(&result) && first_text(&result).starts_with("whatsapp_off:"), "raiz sem ver as sessões do dono: {result}");

    guests.create("Bia", &root, true, true, "bia".into(), "token-bia".into()).unwrap();
    let result = mcp.call("whatsapp_chats", json!({})).await;
    assert!(failed(&result) && first_text(&result).starts_with("whatsapp_shared_session:"), "vê as sessões do dono: {result}");
}

/// O C5 de mentira do `hangar-whatsapp` na pasta `~/.hangar/whatsapp` da cena: assina as respostas como o binário e
/// guarda os rascunhos. `outcome` são os campos que o envio aprovado ganha quando a tool relê o rascunho; `calls` anda a
/// cada pedido recebido, o sinal que os testes esperam.
struct FakeWhatsApp { drafts: Arc<Mutex<Vec<Value>>>, seen: Arc<Mutex<Vec<String>>>, calls: Arc<tokio::sync::watch::Sender<u64>>, hold: Hold }

/// A criação da conversa `lento` fica presa até `release`, como a cópia de um anexo grande; `held` avisa que chegou.
#[derive(Clone, Default)]
struct Hold { held: Arc<tokio::sync::Notify>, release: Arc<tokio::sync::Notify> }

const WA_KEY: [u8; 32] = [9; 32];

async fn fake_whatsapp(home: &Path, api: u32, outcome: Value) -> FakeWhatsApp {
    let data = home.join(".hangar/whatsapp");
    std::fs::create_dir_all(&data).unwrap();
    std::fs::write(data.join("control.key"), WA_KEY.iter().map(|b| format!("{b:02x}")).collect::<String>()).unwrap();
    let fake = FakeWhatsApp { drafts: Arc::default(), seen: Arc::default(), calls: Arc::new(tokio::sync::watch::channel(0).0), hold: Hold::default() };
    let (drafts, seen, calls, hold) = (fake.drafts.clone(), fake.seen.clone(), fake.calls.clone(), fake.hold.clone());
    let app = axum::Router::new().fallback(move |request: axum::extract::Request| {
        let (drafts, seen, calls, outcome, hold) = (drafts.clone(), seen.clone(), calls.clone(), outcome.clone(), hold.clone());
        async move {
            let response = fake_c5(request, &drafts, &seen, &outcome, &hold).await;
            calls.send_modify(|n| *n += 1);
            response
        }
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    std::fs::write(data.join("control.json"), json!({"version": 1, "api": api, "port": port, "pid": std::process::id()}).to_string()).unwrap();
    fake
}

async fn fake_c5(request: axum::extract::Request, drafts: &Mutex<Vec<Value>>, seen: &Mutex<Vec<String>>, outcome: &Value, hold: &Hold)
    -> axum::response::Response {
    use axum::response::IntoResponse;
    let method = request.method().as_str().to_owned();
    let uri = request.uri().path_and_query().map_or("/", |p| p.as_str()).to_owned();
    let ts = request.headers().get(hangar_whatsapp::control::SIGNATURE_HEADER).and_then(|v| v.to_str().ok())
        .and_then(|h| h.split_once('.')).map(|(ts, _)| ts.to_owned()).unwrap_or_default();
    let body = axum::body::to_bytes(request.into_body(), usize::MAX).await.unwrap();
    seen.lock().unwrap().push(format!("{method} {uri}"));
    if method == "POST" && uri == "/v1/outbox" && serde_json::from_slice::<Value>(&body).is_ok_and(|b| b["chat"] == "lento") {
        hold.held.notify_one();
        hold.release.notified().await;
    }
    let (status, out) = c5_reply(&method, &uri, &body, drafts, outcome);
    let bytes = out.to_string().into_bytes();
    let signature = hangar_whatsapp::control::sign_response(&WA_KEY, &ts, status, &bytes);
    (axum::http::StatusCode::from_u16(status).unwrap(), [(hangar_whatsapp::control::SIGNATURE_HEADER, signature)], bytes).into_response()
}

/// Expiração fixa, longe de hoje: a espera da tool decide igual em qualquer dia. A conversa `vencido` nasce com o prazo
/// já passado, como um rascunho que o dono deixou expirar. A criação fica depois de qualquer subida, também fixa: a de
/// antes da subida seria recusada pela lista dos pendentes como sobra de outra vida.
fn c5_reply(method: &str, uri: &str, body: &[u8], drafts: &Mutex<Vec<Value>>, outcome: &Value) -> (u16, Value) {
    let error = |status: u16, code: &str| (status, json!({"detail": {"code": code, "msg": format!("recusado: {code}")}}));
    let mut drafts = drafts.lock().unwrap();
    let (path, query) = uri.split_once('?').unwrap_or((uri, ""));
    if path == "/v1/outbox" {
        return match (method, query) {
            ("POST", _) => {
                let asked: Value = serde_json::from_slice(body).unwrap_or(Value::Null);
                let expires = if asked["chat"] == "vencido" { "2000-01-01T00:10:00Z" } else { "2099-01-01T00:10:00Z" };
                let draft = json!({"id": format!("d{}", drafts.len() + 1), "origin": asked["origin"], "chat": asked["chat"],
                    "text": asked["text"], "state": "pending", "created_at": "2099-01-01T00:00:00Z",
                    "expires_at": expires, "answered_at": null,
                    "message_id": "", "error": "", "asked": asked});
                drafts.push(draft.clone());
                (201, draft)
            }
            ("GET", "state=pending") => (200, json!({"drafts": drafts.iter().filter(|d| d["state"] == "pending").collect::<Vec<_>>()})),
            _ => error(400, "bad_request"),
        };
    }
    let rest = path.strip_prefix("/v1/outbox/").unwrap_or_default();
    let (id, action) = rest.split_once('/').unwrap_or((rest, ""));
    let Some(draft) = drafts.iter_mut().find(|d| d["id"] == id) else { return error(404, "not_found") };
    let expired = draft["chat"] == "vencido";
    match (method, action) {
        ("GET", "") => {
            if draft["state"] == "sending" && let Some(fields) = outcome.as_object() {
                for (key, value) in fields { draft[key] = value.clone(); }
            }
            (200, draft.clone())
        }
        ("POST", "send" | "send-edited" | "decline") if draft["state"] != "pending" => error(409, "answered"),
        ("POST", "send" | "send-edited" | "decline") if expired => {
            draft["state"] = json!("expired");
            error(409, "expired")
        }
        ("POST", "send") => {
            draft["state"] = json!("sending");
            draft["answered_at"] = json!("2099-01-01T00:00:00Z");
            (202, draft.clone())
        }
        // O C7: o texto que o dono editou sai no lugar do pedido pela sessão, que fica em `text`.
        ("POST", "send-edited") => {
            let text = serde_json::from_slice::<Value>(body).ok().and_then(|b| b["text"].as_str().map(str::to_owned)).unwrap_or_default();
            draft["state"] = json!("sending");
            draft["answered_at"] = json!("2099-01-01T00:00:00Z");
            let edited = text != draft["text"].as_str().unwrap_or_default();
            draft["edited"] = json!(edited);
            draft["sent_text"] = json!(text);
            (202, draft.clone())
        }
        ("POST", "decline") => {
            draft["state"] = json!("declined");
            (200, draft.clone())
        }
        _ => error(404, "bad_request"),
    }
}

fn declined(fake: &FakeWhatsApp, id: &str) -> bool {
    fake.drafts.lock().unwrap().iter().any(|d| d["id"] == id && d["state"] == "declined")
}

/// Espera a condição a cada pedido que o serviço falso recebe: a tool roda noutra tarefa, presa na espera do dono.
async fn until<T>(fake: &FakeWhatsApp, mut probe: impl FnMut() -> Option<T>) -> T {
    let mut calls = fake.calls.subscribe();
    loop {
        if let Some(value) = probe() { return value; }
        // Teto generoso: só estoura se a tool travar.
        tokio::time::timeout(Duration::from_secs(10), calls.changed()).await
            .expect("o serviço falso não recebeu pedido em 10 s").unwrap();
    }
}

/// O cartão do rascunho apareceu: a lista dos pendentes, que o dono vê, já o tem.
async fn on_card(s: &Scene, id: &str) {
    let mut feed = s.state.whatsapp.get().unwrap().outbox().feed();
    let shown = |feed: &Option<Arc<hangar_whatsapp::outbox::Feed>>| feed.as_ref().is_some_and(|f| f.drafts.iter().any(|d| d["id"] == id));
    tokio::time::timeout(Duration::from_secs(10), feed.wait_for(shown)).await.expect("o cartão não apareceu em 10 s").unwrap();
}

/// O dono responde no cartão: a mesma rota que o app chama.
async fn owner_answers(s: &Scene, id: &str, action: &str) -> u16 {
    client().post(format!("http://{}/api/whatsapp/outbox/{id}/{action}", s.addr)).bearer_auth(OWNER)
        .header("content-type", "application/json").body(json!({"via": "desktop"}).to_string()).send().await.unwrap().status().as_u16()
}

/// A tool numa tarefa própria, com a sessão `s0` (pane `%0`), e o id do rascunho que ela criou.
async fn draft_in_flight(s: &Scene, fake: &FakeWhatsApp, chat: &str) -> (tokio::task::JoinHandle<Value>, String) {
    let mut mcp = Mcp::open(s.addr, &[("x-hangar-pane", "%0")]).await;
    let args = json!({"chat": chat, "text": "segue o arquivo", "file": "/tmp/a.zip"});
    let task = tokio::spawn(async move { mcp.call("whatsapp_draft", args).await });
    let id = until(fake, || fake.drafts.lock().unwrap().first().and_then(|d| d["id"].as_str().map(str::to_owned))).await;
    (task, id)
}

/// O rascunho leva a sessão como origem e a pasta dela; a tool espera o "Enviar" do dono e devolve o envio.
#[tokio::test(flavor = "multi_thread")]
async fn whatsapp_draft_waits_for_the_owner_and_returns_the_sent_message() {
    let s = scene_shared(false, None, false, "", true).await;
    let fake = fake_whatsapp(&s.home, 2, json!({"state": "sent", "message_id": "3EB0AA"})).await;
    let (task, id) = draft_in_flight(&s, &fake, "5564999").await;
    let asked = fake.drafts.lock().unwrap()[0]["asked"].clone();
    assert_eq!((&asked["chat"], &asked["text"], &asked["file"], &asked["origin"]),
        (&json!("5564999"), &json!("segue o arquivo"), &json!("/tmp/a.zip"), &json!("local::s0")), "{asked}");
    assert!(asked["origin_cwd"].as_str().is_some_and(|cwd| cwd.ends_with("w0")), "a pasta da sessão: {asked}");
    assert!(!task.is_finished(), "a tool espera o dono");
    on_card(&s, &id).await;
    assert_eq!(owner_answers(&s, &id, "send").await, 202);
    let result = task.await.unwrap();
    assert!(!failed(&result), "{result}");
    assert_eq!(result["structuredContent"], json!({"status": "sent", "id": id, "message_id": "3EB0AA"}));
}

/// Binário `api` 3: o cartão oferece a edição, e o dono envia outro texto. A tool devolve o que saiu (`sent_text`) e a
/// nota de que ele editou, para a sessão não dizer que saiu o texto dela.
#[tokio::test(flavor = "multi_thread")]
async fn whatsapp_draft_says_the_owner_edited_the_text() {
    let s = scene_shared(false, None, false, "", true).await;
    let fake = fake_whatsapp(&s.home, 3, json!({"state": "sent", "message_id": "3EB0AB"})).await;
    let (task, id) = draft_in_flight(&s, &fake, "5564999").await;
    on_card(&s, &id).await;
    assert!(s.state.whatsapp.get().unwrap().outbox().feed().borrow().as_ref().is_some_and(|f| f.edit), "o cartão oferece Editar");
    let status = client().post(format!("http://{}/api/whatsapp/outbox/{id}/send-edited", s.addr)).bearer_auth(OWNER)
        .header("content-type", "application/json").body(json!({"via": "desktop", "text": "segue o zip"}).to_string())
        .send().await.unwrap().status().as_u16();
    assert_eq!(status, 202);
    let result = task.await.unwrap();
    assert!(!failed(&result), "{result}");
    assert_eq!(result["structuredContent"], json!({"status": "sent", "id": id, "message_id": "3EB0AB", "edited": true,
        "sent_text": "segue o zip", "note": "o dono editou o texto antes de enviar; o aprovado foi sent_text"}));
}

/// Recusado pelo dono, falho sem confirmação e expirado: estado final com o id e o aviso de não repetir.
#[tokio::test(flavor = "multi_thread")]
async fn whatsapp_draft_returns_declined_failed_and_expired() {
    for (case, outcome, chat, action) in [
        ("declined", json!({}), "5564999", Some("decline")),
        ("failed", json!({"state": "failed", "error": "unconfirmed"}), "5564999", Some("send")),
        ("expired", json!({}), "vencido", None),
    ] {
        let s = scene_shared(false, None, false, "", true).await;
        let fake = fake_whatsapp(&s.home, 2, outcome).await;
        let (task, id) = draft_in_flight(&s, &fake, chat).await;
        if let Some(action) = action {
            on_card(&s, &id).await;
            owner_answers(&s, &id, action).await;
        }
        let result = task.await.unwrap();
        let out = &result["structuredContent"];
        assert_eq!((&out["status"], &out["id"]), (&json!(case), &json!(id)), "{case}: {result}");
        assert!(out["note"].as_str().is_some_and(|n| n.contains(hangar_whatsapp::outbox::DONT_REPEAT)), "{case}: {result}");
        if case == "failed" {
            assert_eq!(out["error"], json!("unconfirmed"));
            assert!(out["note"].as_str().unwrap().contains("pode ter saído"), "{result}");
        }
        if case == "expired" {
            assert!(fake.seen.lock().unwrap().iter().any(|r| r == &format!("POST /v1/outbox/{id}/decline")), "a tool recusa o vencido");
        }
    }
}

/// Depois de um `wipe` o binário não conhece mais o rascunho: a tool para, com o id, sem saber se saiu.
#[tokio::test(flavor = "multi_thread")]
async fn whatsapp_draft_after_a_wipe_is_final() {
    let s = scene_shared(false, None, false, "", true).await;
    let fake = fake_whatsapp(&s.home, 2, json!({})).await;
    let (task, id) = draft_in_flight(&s, &fake, "5564999").await;
    on_card(&s, &id).await;
    fake.drafts.lock().unwrap().clear();
    // O wipe some com a lista inteira: a releitura dela é o que acorda a espera.
    s.state.whatsapp.get().unwrap().outbox().poke();
    let result = task.await.unwrap();
    let out = &result["structuredContent"];
    assert_eq!((&out["status"], &out["id"], &out["error"]), (&json!("failed"), &json!(id), &json!("not_found")), "{result}");
}

/// Sessão compartilhada e binário sem envio recusam antes de o pedido chegar ao serviço.
#[tokio::test(flavor = "multi_thread")]
async fn whatsapp_draft_refuses_a_shared_session_and_a_binary_without_send() {
    let s = scene_shared(false, None, false, "", true).await;
    let fake = fake_whatsapp(&s.home, 1, json!({})).await;
    let mut mcp = Mcp::open(s.addr, &[("x-hangar-pane", "%0")]).await;
    let result = mcp.call("whatsapp_draft", json!({"chat": "5564999", "text": "oi"})).await;
    assert!(failed(&result) && first_text(&result).starts_with("whatsapp_send_unsupported:"), "{result}");
    assert!(first_text(&result).contains("atualize o hangar-whatsapp"), "{result}");

    let share = s.state.share.clone().unwrap();
    share.store.create("s0", None, hangar_server::share::store::Kind::Share, false).unwrap();
    let result = mcp.call("whatsapp_draft", json!({"chat": "5564999", "text": "oi"})).await;
    assert!(failed(&result) && first_text(&result).starts_with("whatsapp_shared_session:"), "{result}");
    assert!(fake.seen.lock().unwrap().iter().all(|r| !r.contains("/v1/outbox")), "nada chegou ao serviço");
}

/// Cancelada a chamada, o rascunho pendente é recusado no binário: largar a espera não bastaria, ele mora lá.
/// Já aprovado, o envio segue e nada é recusado.
#[tokio::test(flavor = "multi_thread")]
async fn a_cancelled_whatsapp_draft_is_declined_unless_already_approved() {
    for approved in [false, true] {
        let s = scene_shared(false, None, false, "", true).await;
        let fake = fake_whatsapp(&s.home, 2, json!({"state": "sending"})).await;
        let mut mcp = Mcp::open(s.addr, &[("x-hangar-pane", "%0")]).await;
        let canceller = Mcp { addr: mcp.addr, headers: mcp.headers.clone(), session: mcp.session.clone(), next: 0 };
        let request = mcp.next + 1;
        let task = tokio::spawn(async move { mcp.request("tools/call", json!({"name": "whatsapp_draft",
            "arguments": {"chat": "5564999", "text": "oi"}})).await });
        let id = until(&fake, || fake.drafts.lock().unwrap().first().and_then(|d| d["id"].as_str().map(str::to_owned))).await;
        let reads = |fake: &FakeWhatsApp| fake.seen.lock().unwrap().iter().filter(|r| **r == format!("GET /v1/outbox/{id}")).count();
        if approved {
            on_card(&s, &id).await;
            assert_eq!(owner_answers(&s, &id, "send").await, 202);
            // A segunda leitura depois do envio só sai quando a tool já tratou a primeira, que viu o envio aprovado.
            let before = reads(&fake);
            until(&fake, || (reads(&fake) > before + 1).then_some(())).await;
        }
        let decline = format!("POST /v1/outbox/{id}/decline");
        if approved {
            // A ausência se prova por um rascunho de controle, pendente, na mesma sessão MCP e cancelado depois: o
            // cancelamento dos dois passa em ordem pelo mesmo laço, e a recusa do controle chega pelo mesmo caminho.
            let mut other = Mcp { addr: canceller.addr, headers: canceller.headers.clone(), session: canceller.session.clone(), next: 99 };
            let control = tokio::spawn(async move { other.request("tools/call", json!({"name": "whatsapp_draft",
                "arguments": {"chat": "5564998", "text": "controle"}})).await });
            let control_id = until(&fake, || fake.drafts.lock().unwrap().get(1).and_then(|d| d["id"].as_str().map(str::to_owned))).await;
            for target in [request, 100] {
                let cancel = canceller.post(json!({"jsonrpc": "2.0", "method": "notifications/cancelled",
                    "params": {"requestId": target, "reason": "teste"}})).send().await.unwrap();
                assert_eq!(cancel.status(), 202);
            }
            until(&fake, || declined(&fake, &control_id).then_some(())).await;
            control.abort();
            assert!(!fake.seen.lock().unwrap().contains(&decline), "aprovado não é recusado");
            assert_eq!(fake.drafts.lock().unwrap()[0]["state"], json!("sending"));
        } else {
            let cancel = canceller.post(json!({"jsonrpc": "2.0", "method": "notifications/cancelled",
                "params": {"requestId": request, "reason": "teste"}})).send().await.unwrap();
            assert_eq!(cancel.status(), 202);
            until(&fake, || declined(&fake, &id).then_some(())).await;
            assert!(fake.seen.lock().unwrap().contains(&decline));
        }
        task.abort();
    }
}

/// Cancelada com a criação ainda correndo (a cópia de um anexo grande), a tool não chega a guardar o rascunho: a
/// criação termina sozinha e recusa o que nasceu, senão ele ficaria aprovável por 10 min sem ninguém esperando.
#[tokio::test(flavor = "multi_thread")]
async fn a_draft_born_after_the_call_was_cancelled_is_declined() {
    let s = scene_shared(false, None, false, "", true).await;
    let fake = fake_whatsapp(&s.home, 2, json!({})).await;
    let mut mcp = Mcp::open(s.addr, &[("x-hangar-pane", "%0")]).await;
    let canceller = Mcp { addr: mcp.addr, headers: mcp.headers.clone(), session: mcp.session.clone(), next: 0 };
    let request = mcp.next + 1;
    let task = tokio::spawn(async move { mcp.request("tools/call", json!({"name": "whatsapp_draft",
        "arguments": {"chat": "lento", "text": "oi"}})).await });
    tokio::time::timeout(Duration::from_secs(10), fake.hold.held.notified()).await.expect("a criação não chegou ao serviço em 10 s");
    let cancel = canceller.post(json!({"jsonrpc": "2.0", "method": "notifications/cancelled",
        "params": {"requestId": request, "reason": "teste"}})).send().await.unwrap();
    assert_eq!(cancel.status(), 202);
    assert!(fake.drafts.lock().unwrap().is_empty(), "a criação ainda está presa");
    // O cancelamento chega à tool antes da volta da criação, que ainda faz dois saltos de HTTP: sem a tarefa própria,
    // a criação morre com a chamada e o rascunho não nasce, ou nasce sem ninguém para recusá-lo.
    fake.hold.release.notify_one();
    let id = until(&fake, || fake.drafts.lock().unwrap().first().and_then(|d| d["id"].as_str().map(str::to_owned))).await;
    // O pedido entra em `seen` antes da recusa gravar o estado: espera o estado, que vale também se já veio antes.
    until(&fake, || declined(&fake, &id).then_some(())).await;
    assert!(fake.seen.lock().unwrap().contains(&format!("POST /v1/outbox/{id}/decline")));
    task.abort();
}

/// `machine` abre naquela máquina: o pedido vai com o id dela fixado (lá não se escolhe de novo), sem a
/// criadora, que lá não existe, e a resposta dá o `target` com a máquina.
#[tokio::test(flavor = "multi_thread")]
async fn placement_machine_opens_there_with_its_id_fixed_and_answers_the_target() {
    use axum::http::header::CONTENT_TYPE;
    use axum::routing::{get, post};
    use hangar_server::groups::peers::{PeerBook, PeerClient};
    let seen: Arc<Mutex<Vec<Value>>> = Arc::default();
    let sink = seen.clone();
    // Lá o `CP_SERVER_ID` não é a chave do `peers.json` daqui: o pedido leva o id de lá, o `target`, a chave.
    let facts = json!({"server_id": "lab-id", "version": "1", "capable": true, "mem_total": 0, "mem_available": 0, "working": 0,
        "cpu_pct": null, "repos": [], "clone_dir": null}).to_string();
    let app = axum::Router::new()
        .route("/api/placement/facts", get(move || { let facts = facts.clone(); async move { ([(CONTENT_TYPE, "application/json")], facts) } }))
        .route("/api/sessions", post(move |body: axum::body::Bytes| {
            let sink = sink.clone();
            async move {
                sink.lock().unwrap().push(serde_json::from_slice(&body).unwrap());
                ([(CONTENT_TYPE, "application/json")], json!({"name": "filha", "cwd": "/home/lab", "provider": "claude"}).to_string())
            }
        }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("peers.json");
    write_json(&file, json!({"lab": {"base_url": format!("http://{addr}"), "token": "tok-lab"}}));
    let s = scene_tuned(false, Some(PeerClient::new(PeerBook::new(Some(file)))), false, "", Share::Absent, isolated_accounts).await;
    let mut mcp = Mcp::open(s.addr, &[("x-hangar-pane", "%7")]).await;
    // A pasta pessoal daqui vira a de lá.
    let result = mcp.call("new_session", json!({"nome": "filha", "cwd": s.home, "machine": "lab", "provider": "claude", "inherit": false})).await;
    assert!(!failed(&result), "{result}");
    assert_eq!(result["structuredContent"]["target"], "lab::filha", "{result}");
    let sent = seen.lock().unwrap().clone();
    assert_eq!(sent.len(), 1, "{sent:?}");
    assert_eq!((&sent[0]["machine"], &sent[0]["cwd"], &sent[0]["provider"]), (&json!("lab-id"), &json!("~"), &json!("claude")), "{}", sent[0]);
    assert!(sent[0].get("creator").is_none() && sent[0].get("inherit").is_none(), "lá a criadora não existe: {}", sent[0]);
    // O id que ela informa (o que a sugestão e as mensagens citam) também abre lá, pela chave daqui.
    let by_id = mcp.call("new_session", json!({"nome": "filha2", "cwd": s.home, "machine": "lab-id", "provider": "claude", "inherit": false})).await;
    assert!(!failed(&by_id), "{by_id}");
    assert_eq!(by_id["structuredContent"]["target"], "lab::filha", "{by_id}");
    assert_eq!(seen.lock().unwrap().get(1).map(|b| b["machine"].clone()), Some(json!("lab-id")));
    // Com a criadora (`beta`, a do pane), só ela é lida aqui: o nome em uso nesta máquina (o tmux falso diz que todo
    // nome existe) é assunto de lá e não recusa a abertura.
    let inherited = mcp.call("new_session", json!({"nome": "s0", "cwd": s.home, "machine": "lab", "provider": "claude"})).await;
    assert!(!failed(&inherited), "{inherited}");
    assert_eq!(seen.lock().unwrap().get(2).map(|b| b["name"].clone()), Some(json!("s0")));
    let both = mcp.call("new_session", json!({"nome": "lab::x", "machine": "lab"})).await;
    assert!(failed(&both) && first_text(&both).contains("não os dois"), "{both}");
    // Nem chave do `peers.json` nem id informado: recusa clara, nada aberto.
    let unknown = mcp.call("new_session", json!({"nome": "outra", "cwd": s.home, "machine": "nada", "provider": "claude", "inherit": false})).await;
    assert!(failed(&unknown) && first_text(&unknown).starts_with("placement_machine_unknown:"), "{unknown}");
    assert_eq!(seen.lock().unwrap().len(), 3);
}

/// A pasta fica reservada para a outra máquina só quando lá confirmou a abertura: a recusada não prende a próxima.
#[tokio::test(flavor = "multi_thread")]
async fn placement_reserves_the_folder_only_after_the_other_machine_opens() {
    use axum::http::{StatusCode, header::CONTENT_TYPE};
    use axum::routing::{get, post};
    use hangar_server::groups::peers::{PeerBook, PeerClient};
    use std::sync::atomic::{AtomicBool, Ordering};
    let facts = json!({"server_id": "lab-id", "version": "1", "capable": true, "mem_total": 0, "mem_available": 0, "working": 0,
        "cpu_pct": null, "repos": [{"project": "git:github.com/a/b", "root": "/lab/b"}], "clone_dir": null}).to_string();
    let refuse = Arc::new(AtomicBool::new(true));
    let gate = refuse.clone();
    let app = axum::Router::new()
        .route("/api/placement/facts", get(move || { let facts = facts.clone(); async move { ([(CONTENT_TYPE, "application/json")], facts) } }))
        .route("/api/sessions", post(move || {
            let refused = gate.swap(false, Ordering::SeqCst);
            async move {
                if refused { return (StatusCode::CONFLICT, [(CONTENT_TYPE, "application/json")], json!({"detail": {"code": "erro_x", "msg": "não"}}).to_string()) }
                (StatusCode::OK, [(CONTENT_TYPE, "application/json")], json!({"name": "filha", "cwd": "/lab/b", "provider": "claude"}).to_string())
            }
        }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("peers.json");
    write_json(&file, json!({"lab": {"base_url": format!("http://{addr}"), "token": "tok-lab"}}));
    let s = scene_tuned(false, Some(PeerClient::new(PeerBook::new(Some(file)))), false, "", Share::Absent, isolated_accounts).await;
    let repo = s.home.join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    for args in [&["init", "-q", "-b", "main"][..], &["remote", "add", "origin", "https://github.com/a/b"],
                 &["-c", "user.name=t", "-c", "user.email=t@t", "commit", "-q", "--allow-empty", "-m", "um"]] {
        let out = std::process::Command::new("git").arg("-C").arg(&repo).args(args)
            .env("GIT_CONFIG_GLOBAL", "/dev/null").env("GIT_CONFIG_NOSYSTEM", "1").output().unwrap();
        assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
    }
    let root = repo.to_string_lossy().into_owned();
    let mut mcp = Mcp::open(s.addr, &[("x-hangar-pane", "%7")]).await;
    let open = json!({"nome": "filha", "cwd": root, "machine": "lab", "provider": "claude", "inherit": false});
    let refused = mcp.call("new_session", open.clone()).await;
    assert!(failed(&refused), "{refused}");
    assert_eq!(s.state.placement_reservations.lookup(&root), None, "lá recusou: a pasta não fica presa a lá");
    let opened = mcp.call("new_session", open).await;
    assert!(!failed(&opened), "{opened}");
    assert_eq!(s.state.placement_reservations.lookup(&root).as_deref(), Some("lab-id"));
}

/// As contas e as regras do balanceamento na pasta pessoal da cena, nunca na de quem roda o teste.
fn isolated_accounts(state: &mut AppState) {
    use hangar_server::accounts::{AccountService, environment::AccountEnvironment};
    let home = state.origins_home.display().to_string();
    state.accounts = AccountService::new(AccountEnvironment::from_map([("HOME".into(), home.clone()), ("USERPROFILE".into(), home)].into()));
}

/// A sugestão da tela escolhe sem abrir: a pasta pessoal fica nesta máquina pela carga, e a pasta sem git, por
/// existir só aqui, com o motivo em texto.
#[tokio::test(flavor = "multi_thread")]
async fn placement_suggest_chooses_without_opening() {
    let s = scene_tuned(false, None, false, "", Share::Absent, isolated_accounts).await;
    // Limites folgados: a carga é a da máquina que roda o teste.
    write_json(&s.home.join(".hangar/placement.json"), json!({"limits": {"mem_free_pct": 0, "working": 1000, "cpu_pct": 100}}));
    std::fs::create_dir_all(s.home.join("plain")).unwrap();
    let url = format!("http://{}/api/placement/suggest", s.addr);
    let suggest = |cwd: PathBuf| { let url = url.clone(); async move {
        let resp = client().post(&url).bearer_auth(OWNER).json(&json!({"cwd": cwd})).send().await.unwrap();
        assert_eq!(resp.status(), 200);
        resp.json::<Value>().await.unwrap()
    } };
    let body = suggest(s.home.clone()).await;
    assert_eq!(body["server"], json!(s.state.current_server_id()), "{body}");
    // Processador acima de 100% da carga média também tira a local; sem outra máquina, fica nela do mesmo jeito.
    assert!(matches!(body["reason"]["code"].as_str(), Some("local_within_limits" | "no_candidate")), "{body}");
    assert!(body["text"].as_str().is_some_and(|t| !t.is_empty()), "{body}");
    let plain = suggest(s.home.join("plain")).await;
    assert_eq!((&plain["server"], &plain["reason"]["code"]), (&json!(s.state.current_server_id()), &json!("plain_dir")), "{plain}");
    let bad = client().post(&url).bearer_auth(OWNER).body("{}").send().await.unwrap();
    assert_eq!(bad.status(), 422);
    let anonymous = client().post(&url).json(&json!({"cwd": "/tmp"})).send().await.unwrap();
    assert!(matches!(anonymous.status().as_u16(), 401 | 403), "só o dono: {}", anonymous.status());
}
