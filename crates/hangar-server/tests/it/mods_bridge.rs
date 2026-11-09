use crate::fake;

use std::sync::Arc;

use fake::*;
use hangar_server::mods::bridge::{mint, mint_keyed, note_unknown_key};
use hangar_server::mods::model::*;
use hangar_server::mods::state::*;
use serde_json::{Value, json};

#[test]
fn mint_matches_python() {
    // python3 -c "import hmac,hashlib;print(hmac.new(b'dono-token',b'plugin:mods-s',hashlib.sha256).hexdigest()[:32])"
    assert_eq!(mint("dono-token", "mods-s"), "fe9b420d49e252b4c98ce09870cf7747");
    assert_eq!(mint("", "mods-s"), "1d50c42b88fb27d333798979117ec477", "sem token, o segredo é \"hangar\"");
    // python3 -c "import hmac,hashlib;print(hmac.new(b'dono-token',b'plugin:mods-s:k1',hashlib.sha256).hexdigest()[:32])"
    assert_eq!(mint_keyed("dono-token", "mods-s", "k1"), "k1.4547ef357a2c0c603e0a916d8764bb86");
}

/// O `reqwest` do crate não tem a função `json`: o corpo vai pronto, com o tipo.
async fn post(url: String, body: Value, owner: bool) -> reqwest::Response {
    let mut request = client().post(url).header("content-type", "application/json").body(body.to_string());
    if owner { request = request.header("authorization", format!("Bearer {OWNER}")); }
    request.send().await.unwrap()
}

async fn json_of(response: reqwest::Response) -> Value {
    serde_json::from_str(&response.text().await.unwrap()).unwrap()
}

/// O mod de um clique do app: o plugin do Hangar casa o press e manda a URL, como faz no `claude -p`
/// (Task 12) e fará na sessão com terminal da fase 3. Implementado direto no tipo local (A2): o `call`
/// só copia o endereço.
#[derive(Clone)]
struct PluginLike { server: Arc<std::sync::OnceLock<std::net::SocketAddr>> }
impl SurfaceLink for PluginLike {
    fn call(&self, _: ModsCall, _: std::time::Instant) -> CallFuture {
        let server = *self.server.get().unwrap();
        Box::pin(async move {
            let base = format!("http://{server}/api/plugin");
            // O processo nasceu `mods-s`, com a chave da sessão (`serve_mods` a abre com o processo `mods-s`).
            let token = mint_keyed(OWNER, "mods-s", "mods-s");
            let start = json_of(post(format!("{base}/press-start"), json!({"sessao": "mods-s", "token": token,
                "requestId": "vitrine-botoes", "element": "V45-url"}), false).await).await;
            assert_eq!(start["fromApp"], true);
            let again = json_of(post(format!("{base}/press-start"), json!({"sessao": "mods-s", "token": token,
                "requestId": "vitrine-botoes", "element": "V45-url"}), false).await).await;
            assert_eq!(again["fromApp"], false, "uma vez só");
            let opened = post(format!("{base}/opened"), json!({"sessao": "mods-s", "token": token,
                "attempt": start["attempt"], "url": "https://example.com/vitrine"}), false).await;
            assert_eq!(opened.status().as_u16(), 200);
            Ok(json!({"element": "V45-url"}))
        })
    }
}

async fn setup() -> (Arc<Fake>, std::net::SocketAddr, Mods, PluginLike) {
    let plugin = PluginLike { server: Arc::new(std::sync::OnceLock::new()) };
    let (python, server, mods) = serve_mods("mods-s", Arc::new(plugin.clone())).await;
    plugin.server.set(server).unwrap();
    (python, server, mods, plugin)
}

#[tokio::test]
async fn url_of_an_app_click_goes_back_in_the_press_answer() {
    let (_python, server, _mods, _plugin) = setup().await;
    let response = post(format!("http://{server}/api/sessions/mods-s/plugin/press"),
        json!({"site": "vitrine-botoes", "plugin": "vitrine", "key": "V45-url"}), true).await;
    assert_eq!(json_of(response).await, json!({"ok": true, "opened": "https://example.com/vitrine"}));
}

#[tokio::test]
async fn bridge_checks_token_and_url_and_passes_other_sessions() {
    let (python, server, mods, _plugin) = setup().await;
    let base = format!("http://{server}/api/plugin");
    let wrong = post(format!("{base}/press-start"), json!({"sessao": "mods-s", "token": "x", "requestId": "a", "element": "b"}), false).await;
    assert_eq!(wrong.status().as_u16(), 403);
    let attempt = mods.begin_click("mods-s", "a", "vitrine", "b");
    let ftp = post(format!("{base}/opened"), json!({"sessao": "mods-s", "token": mint(OWNER, "mods-s"), "attempt": attempt, "url": "ftp://x"}), false).await;
    assert_eq!(ftp.status().as_u16(), 400);
    let late = post(format!("{base}/opened"), json!({"sessao": "mods-s", "token": mint(OWNER, "mods-s"), "attempt": "outra", "url": "https://x"}), false).await;
    assert_eq!(late.status().as_u16(), 409, "fora do clique o mod abre no servidor");
    // Sessão fora do Rust com token que não é o dela: 403 daqui, como o Python.
    let other = post(format!("{base}/press-start"), json!({"sessao": "com-terminal", "token": "t", "requestId": "a", "element": "b"}), false).await;
    assert_eq!(other.status().as_u16(), 403);
    assert_eq!(python.hits_to("/api/plugin/press-start"), 0);
}

#[tokio::test]
async fn opened_checks_token_case_size_and_passes_invalid_bodies() {
    let (python, server, mods, _plugin) = setup().await;
    let base = format!("http://{server}/api/plugin");
    let token = mint(OWNER, "mods-s");
    let attempt = mods.begin_click("mods-s", "a", "vitrine", "b");
    let wrong = post(format!("{base}/opened"), json!({"sessao": "mods-s", "token": "x", "attempt": attempt, "url": "https://x"}), false).await;
    assert_eq!(wrong.status().as_u16(), 403);
    let empty = post(format!("{base}/opened"), json!({"sessao": "mods-s", "token": token, "attempt": attempt, "url": ""}), false).await;
    assert_eq!(empty.status().as_u16(), 400, "URL vazia não tem limite no Pydantic: é o esquema que recusa");
    // Mais de 8192 bytes, menos de 8192 caracteres: o teto conta caracteres, como o Pydantic do Python.
    let wide = format!("https://{}", "á".repeat(4100));
    let accepted = post(format!("{base}/opened"), json!({"sessao": "mods-s", "token": token, "attempt": attempt, "url": wide}), false).await;
    assert_eq!(accepted.status().as_u16(), 200);
    let upper = post(format!("{base}/opened"), json!({"sessao": "mods-s", "token": token, "attempt": attempt, "url": "HTTPS://Example.com/x"}), false).await;
    assert_eq!(upper.status().as_u16(), 200, "o esquema não distingue caixa");
    // Corpo que não é o de uma rota da ponte, de sessão fora do Rust: 422 com código (D7).
    let broken = post(format!("{base}/opened"), json!({"sessao": "com-terminal"}), false).await;
    assert_eq!(broken.status().as_u16(), 422);
    assert_eq!(json_of(broken).await["detail"]["code"], "erro_corpo_invalido");
    assert_eq!(python.hits_to("/api/plugin/opened"), 0);
}

#[tokio::test]
async fn bridge_applies_the_python_limits_before_the_token() {
    let (_python, server, _mods, _plugin) = setup().await;
    let base = format!("http://{server}/api/plugin");
    let cases = [
        ("press-start", json!({"sessao": "mods-s", "token": "x", "requestId": "", "element": "b"})),
        ("press-start", json!({"sessao": "mods-s", "token": "x", "requestId": "é".repeat(65), "element": "b"})),
        ("press-start", json!({"sessao": "mods-s", "token": "x", "requestId": "a", "element": ""})),
        ("press-start", json!({"sessao": "mods-s", "token": "x", "requestId": "a", "element": "é".repeat(257)})),
        ("opened", json!({"sessao": "mods-s", "token": "x", "attempt": "", "url": "https://x"})),
        ("opened", json!({"sessao": "mods-s", "token": "x", "attempt": "é".repeat(65), "url": "https://x"})),
        ("opened", json!({"sessao": "mods-s", "token": "x", "attempt": "a", "url": format!("https://{}", "a".repeat(8192 - 8 + 1))})),
    ];
    for (route, body) in cases {
        let response = post(format!("{base}/{route}"), body.clone(), false).await;
        assert_eq!(response.status().as_u16(), 422, "{route} {body}");
    }
    // No limite exato, contado em caracteres e não em bytes, passa da validação e cai no token.
    let edge = post(format!("{base}/press-start"), json!({"sessao": "mods-s", "token": "x",
        "requestId": "é".repeat(64), "element": "é".repeat(256)}), false).await;
    assert_eq!(edge.status().as_u16(), 403);
}

#[tokio::test]
async fn renamed_session_is_found_by_the_key_in_its_token_without_asking_python() {
    // Renomear fecha e reabre a sessão no Rust com o mesmo `claude -p`, que segue mandando o nome de
    // nascimento. A sessão reaberta sem nada guardado de antes é também a do servidor que reiniciou depois do
    // renomear: a chave no token basta, e o Python (lento aqui) nunca é perguntado.
    let (python, server, mods, plugin) = setup().await;
    python.set_info_delay(std::time::Duration::from_secs(20));
    mods.forget("mods-s", 1);
    mods.attach_process("renomeada", "mods-s", 2, Arc::new(plugin));
    assert_eq!(mods.bridge_session("mods-s", &mint_keyed(OWNER, "mods-s", "mods-s")).as_deref(), Some("renomeada"));
    let started = std::time::Instant::now();
    let response = post(format!("http://{server}/api/sessions/renomeada/plugin/press"),
        json!({"site": "vitrine-botoes", "plugin": "vitrine", "key": "V45-url"}), true).await;
    assert_eq!(json_of(response).await, json!({"ok": true, "opened": "https://example.com/vitrine"}));
    assert!(started.elapsed() < std::time::Duration::from_secs(5));
    assert_eq!(python.hits_to("/internal/sessions/mods-s/info"), 0);
}

#[tokio::test]
async fn two_processes_born_with_the_same_name_each_reach_only_their_session() {
    // Renomeada a sessão sem relançar o processo e criada outra com o nome antigo, cada processo tem o
    // token da própria chave: o antigo não toma o clique da sessão nova, e a nova é atendida.
    let (python, server, mods, plugin) = setup().await;
    mods.forget("mods-s", 1);
    mods.attach_process("renomeada", "mods-s", 2, Arc::new(plugin));
    mods.attach_process("mods-s", "processo-novo:1:2", 3, Arc::new(mods_support_free::Quiet));
    let attempt = mods.begin_click("mods-s", "a", "vitrine", "b");
    let base = format!("http://{server}/api/plugin");
    let old = mint_keyed(OWNER, "mods-s", "mods-s");
    let start = json_of(post(format!("{base}/press-start"), json!({"sessao": "mods-s", "token": old, "requestId": "a", "element": "b"}), false).await).await;
    assert_eq!(start["fromApp"], false, "o processo antigo cai na própria sessão, sem clique em aberto");
    let opened = post(format!("{base}/opened"), json!({"sessao": "mods-s", "token": old, "attempt": attempt, "url": "https://x"}), false).await;
    assert_eq!(opened.status().as_u16(), 409, "a URL do antigo não vai ao aparelho da sessão nova");
    // A janela do clique é de 1,5 s em relógio real: o processo novo recebe um clique aberto agora, e não
    // o que as chamadas do antigo já consumiram sob carga.
    let attempt = mods.begin_click("mods-s", "a", "vitrine", "b");
    let new = mint_keyed(OWNER, "mods-s", "processo-novo");
    let start = json_of(post(format!("{base}/press-start"), json!({"sessao": "mods-s", "token": new, "requestId": "a", "element": "b"}), false).await).await;
    assert_eq!((start["fromApp"].as_bool(), start["attempt"].as_str()), (Some(true), Some(attempt.as_str())));
    assert_eq!(python.hits_to("/api/plugin/press-start"), 0);
}

#[tokio::test]
async fn token_without_a_key_goes_by_the_current_name_and_unknown_keys_get_the_python_answer() {
    // O processo lançado antes da chave manda o token só do nome: vale para a sessão que tem o nome agora.
    let (python, server, mods, plugin) = setup().await;
    mods.forget("mods-s", 1);
    mods.attach_process("renomeada", "mods-s", 2, Arc::new(plugin));
    let base = format!("http://{server}/api/plugin");
    let not_from_app = json!({"fromApp": false, "attempt": null});
    let legacy = post(format!("{base}/press-start"), json!({"sessao": "mods-s", "token": mint(OWNER, "mods-s"), "requestId": "a", "element": "b"}), false).await;
    assert_eq!(json_of(legacy).await, not_from_app);
    let current = post(format!("{base}/press-start"), json!({"sessao": "renomeada", "token": mint(OWNER, "renomeada"), "requestId": "a", "element": "b"}), false).await;
    assert_eq!(current.status().as_u16(), 200);
    // Chave que nenhuma sessão do Rust tem: nunca é clique do app; HMAC errado com chave daqui é 403.
    let unknown = post(format!("{base}/press-start"), json!({"sessao": "x", "token": mint_keyed(OWNER, "x", "outra"), "requestId": "a", "element": "b"}), false).await;
    assert_eq!(json_of(unknown).await, not_from_app);
    let forged = post(format!("{base}/press-start"), json!({"sessao": "x", "token": "mods-s.00", "requestId": "a", "element": "b"}), false).await;
    assert_eq!(forged.status().as_u16(), 403);
    assert_eq!(python.hits_to("/api/plugin/press-start"), 0);
}

/// R1.5-9: a ponte do plugin de sessão sem entrada no Rust responde como o Python (contract/r1-mods
/// `bridge_*`), sem repassar nada.
#[tokio::test]
async fn bridge_without_a_rust_session_answers_like_python() {
    let (python, server, _mods, _plugin) = setup().await;
    let base = format!("http://{server}/api/plugin");
    let token = mint(OWNER, "s1");
    let wrong = "k-unknown.00000000000000000000000000000000";
    let clicked = json!({"detail": "clique do app já respondido"});
    let only_http = json!({"detail": "só http(s)"});
    let press = |token: &str| json!({"sessao": "s1", "token": token, "requestId": "req-1", "element": "btn-a"});
    let copied = |token: &str| json!({"sessao": "s1", "token": token, "attempt": "a1", "text": "copiado"});
    let opened = |token: &str, url: &str| json!({"sessao": "s1", "token": token, "attempt": "a1", "url": url});
    let cases: Vec<(&str, Value, bool, u16, Option<Value>)> = vec![
        ("pressed", press(&token), false, 200, Some(json!({"ok": true}))),
        ("pressed", press(&token), true, 200, Some(json!({"ok": true}))),
        ("pressed", press(wrong), false, 403, None),
        ("pressed", json!({"sessao": "s2", "token": &token, "requestId": "r", "element": "e"}), false, 403, None),
        ("press-start", press(&token), false, 200, Some(json!({"fromApp": false, "attempt": null}))),
        ("press-start", press(wrong), false, 403, None),
        ("copied", copied(&token), false, 409, Some(clicked.clone())),
        ("copied", copied(wrong), false, 403, None),
        ("opened", opened(&token, "http://example.com/x"), false, 409, Some(clicked.clone())),
        ("opened", opened(&token, "HTTPS://example.com"), false, 409, Some(clicked)),
        ("opened", opened(&token, "ftp://example.com/f"), false, 400, Some(only_http.clone())),
        ("opened", opened(&token, "javascript:alert(1)"), false, 400, Some(only_http.clone())),
        ("opened", opened(&token, "/x"), false, 400, Some(only_http)),
        ("opened", opened(wrong, "http://x"), false, 403, None),
        ("opened", opened(wrong, "ftp://x"), false, 403, None),
    ];
    for (route, body, owner, status, expected) in cases {
        let response = post(format!("{base}/{route}"), body.clone(), owner).await;
        assert_eq!(response.status().as_u16(), status, "{route} {body}");
        if let Some(expected) = expected { assert_eq!(json_of(response).await, expected, "{route} {body}"); }
    }
    // D7: corpo inválido é 422 com código.
    for (route, body) in [("pressed", json!({"sessao": "s1", "token": &token, "requestId": "r"})),
        ("copied", json!({"sessao": "s1", "token": &token, "attempt": "a".repeat(65), "text": "x"}))] {
        let response = post(format!("{base}/{route}"), body, false).await;
        assert_eq!(response.status().as_u16(), 422, "{route}");
        assert_eq!(json_of(response).await["detail"]["code"], "erro_corpo_invalido", "{route}");
    }
    // O Python não tem estas rotas: 404 do FastAPI, com ou sem o bearer.
    for route in ["focus-target", "focused", "scroll"] {
        for owner in [false, true] {
            let response = post(format!("{base}/{route}"), json!({"sessao": "s1", "token": "k1.9f4b61638d8f41ffacc16dc205251311",
                "id": "x", "seq": 1, "offset": 0}), owner).await;
            assert_eq!(response.status().as_u16(), 404, "{route}");
            assert_eq!(json_of(response).await, json!({"detail": "Not Found"}), "{route}");
        }
    }
    // Chave sem sessão: registrada uma vez só, por mais que o plugin volte; a rota já a registrou.
    let keyed = mint_keyed(OWNER, "s1", "k-unknown-once");
    for _ in 0..3 {
        assert_eq!(json_of(post(format!("{base}/pressed"), press(&keyed), false).await).await, json!({"ok": true}));
    }
    assert!(!note_unknown_key(&keyed, "s1"), "a chave foi registrada pela rota");
    assert!(note_unknown_key("k-nunca-vista.00", "s1"));
    assert!(!note_unknown_key("k-nunca-vista.00", "s1"), "uma vez só");
    assert!(!note_unknown_key("sem-chave", "s1"), "token só do nome não tem chave a registrar");
    for route in ["pressed", "press-start", "copied", "opened", "focus-target", "focused", "scroll"] {
        assert_eq!(python.hits_to(&format!("/api/plugin/{route}")), 0, "{route}");
    }
}

/// Superfície que não é chamada nos testes da ponte.
mod mods_support_free {
    pub struct Quiet;
    impl hangar_server::mods::state::SurfaceLink for Quiet {
        fn call(&self, _: hangar_server::mods::model::ModsCall, _: std::time::Instant) -> hangar_server::mods::state::CallFuture {
            Box::pin(async { Err(hangar_server::mods::model::missing()) })
        }
    }
}
