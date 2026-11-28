//! O `hangar-server send` contra o `hangar-send` antigo (bash + Python), caso a caso.
//!
//! Dados (`contract/send_cases.json`): os 241 casos `send/*.json` capturados do script da base
//! 2ad8e335e (argv, ambiente, arquivos antes, saída, código e pedidos HTTP), com as rotas dos
//! servidores falsos, o livro de peers falso (`hangar-server peers|conta`) e o stub do tmux tirados de
//! `capture_r34.py` da mesma captura (fase python-fora-do-pacote). Placeholders: `$T` (pasta do
//! caso), `$HTTP_LOCAL`, `$HTTP_PEER_<ID>`, `$DEAD_PORT`.
//!
//! Comparados em todo caso não pulado: código, stdout, stderr e, por servidor, a lista de pedidos
//! (método, caminho cru, `Authorization`, presença do `Content-Type` e corpo como JSON). O que mudou
//! por decisão está em `overrides()`, com a decisão citada.
#![cfg(unix)]

mod common;

use std::collections::BTreeMap;
use std::path::Path;

use common::{Answer, FakeHost, FakeServer, Route, Reply};
use serde_json::{Value, json};

const CASES: &str = include_str!("contract/send_cases.json");

const NEW_USAGE: &str = "uso: hangar-send --new <nome> [cwd] [--engine <motor>] [--provider <claude|codex|pi|omp|kimi>] [--conta <nome>] [--model <id>] [--effort <nivel>] [--permissao <modo>] [--subagente <modelo>] [--service-tier <default|priority>] [--profile <nome>] [--headless|--terminal] [--jev|--sem-jev] [--code-mode|--sem-code-mode] [--read-only] [--inherit|--no-inherit] [--engine-account <id>] [--branch <nome> [--new-branch [--base <ref>]]] [--customizations <json>] [--prompt <texto>] [--remember-provider] [--link] [--forcar-conta] [--machine <id|auto>] [--resume <id da conversa>]\n";
const PAIR_USAGE: &str = "uso: hangar-send --pair <sessao|servidor::sessao> [tarefa...] [--substituir-tarefa] [--orq] | --pair --orq [tarefa...]\n";
const CONTRACT_USAGE: &str = "uso: hangar-send --contrato [--gravar <arquivo> [--mtime <valor da leitura>]]\n";
const UNREADABLE_SEND: &str = "— não dá para dizer se foi entregue; confira com hangar-send --list\n";

/// O que mudou no esperado de um caso, e por quê.
#[derive(Default)]
struct Override {
    why: &'static str,
    skip: Option<&'static str>,
    code: Option<u8>,
    stdout: Option<String>,
    stderr: Option<String>,
    /// Troca o `Authorization` de todos os pedidos esperados.
    auth: Option<&'static str>,
    /// Troca o caminho de um pedido esperado: (servidor, índice, caminho).
    paths: Vec<(&'static str, usize, &'static str)>,
    /// Pedidos esperados por inteiro (mesmo formato do arquivo); `{}` = nenhum.
    http: Option<Value>,
}

impl Override {
    fn code(mut self, code: u8) -> Self { self.code = Some(code); self }
    fn stdout(mut self, text: &str) -> Self { self.stdout = Some(text.into()); self }
    fn stderr(mut self, text: &str) -> Self { self.stderr = Some(text.into()); self }
    fn auth(mut self, auth: &'static str) -> Self { self.auth = Some(auth); self }
    fn path(mut self, server: &'static str, index: usize, path: &'static str) -> Self { self.paths.push((server, index, path)); self }
    fn http(mut self, http: Value) -> Self { self.http = Some(http); self }
    fn no_http(self) -> Self { self.http(json!({})) }
}

fn why(why: &'static str) -> Override {
    Override { why, ..Override::default() }
}

fn skip(why: &'static str, reason: &'static str) -> Override {
    Override { why, skip: Some(reason), ..Override::default() }
}

/// Os pedidos esperados de um caso sem a consulta a `/api/providers` e sem o `provider` que ela punha
/// no corpo: o provedor omitido passou a ser decidido pelo servidor (`resolve_open`).
fn without_provider_lookup(name: &str) -> Value {
    let cases: Vec<Value> = serde_json::from_str(CASES).unwrap();
    let case = cases.into_iter().find(|case| case["name"] == name).unwrap();
    let mut http = case["expect"]["http"].clone();
    for requests in http.as_object_mut().unwrap().values_mut() {
        let list = requests.as_array_mut().unwrap();
        list.retain(|request| request["path"] != "/api/providers");
        for request in list.iter_mut().filter(|request| request["path"] == "/api/sessions") {
            let mut body: Value = serde_json::from_str(request["body"].as_str().unwrap()).unwrap();
            body.as_object_mut().unwrap().remove("provider");
            request["body"] = Value::String(body.to_string());
        }
    }
    http
}

fn overrides() -> BTreeMap<&'static str, Override> {
    let lookup_gone = "padrão de abertura do servidor: o provedor omitido não é mais escolhido pelo cliente";
    let unreadable_send = |target: &str| format!("erro: resposta ilegível do envio para {target} {UNREADABLE_SEND}");
    let encoded = |path: &str| json!({"local": [{"method": "POST", "path": path, "auth": "Bearer tok-local", "json": true,
                                                  "body": "{\"text\": \"[de: cli] oi\", \"steer\": true}"}]});
    BTreeMap::from([
        // O binário é o próprio processo; achar o hangar-server é do atalho.
        ("env__no-binary-126", skip("D11", "coberto pelo teste do atalho")),
        ("env__rust-bin-invalid-126", skip("D11", "coberto pelo teste do atalho")),
        ("env__peers-path-fails", skip("D11", "coberto pelo teste do atalho")),
        // `.env` pelo leitor do Rust, falha com frase em vez do `grep`/`pipefail`.
        ("env__no-env-file", why("D14.8").code(2).stderr("erro: .env não encontrado: $T/inst/backend/.env\n")),
        ("env__no-env-file-whoami", why("D14.8").code(2).stderr("erro: .env não encontrado: $T/inst/backend/.env\n")),
        ("env__no-token-line", why("D14.8").code(1).stderr("erro: CP_AUTH_TOKEN ausente em $T/home/.hangar/config/.env\n")),
        ("env__export-token-not-read", why("D14.8").code(1).stderr("erro: CP_AUTH_TOKEN ausente em $T/home/.hangar/config/.env\n")),
        ("env__token-quoted-crlf", why("D14.8: aspas e \\r saem do token").auth("Bearer tok q")),
        ("env__token-duplicated", why("D14.8 / surpresa 18: vale a última linha").auth("Bearer dois")),
        // A ajuda documenta API_TIMEOUT e API_CONNECT_TIMEOUT.
        ("help__help", why("D14.7").stdout("<HELP>")),
        ("help__h", why("D14.7").stdout("<HELP>")),
        // `--list` com o servidor fora dá `erro:` e 2; resposta torta vira frase, sem traceback.
        ("list__local-down", why("D14.3").code(2).stderr("erro: backend inacessível em http://127.0.0.1:$DEAD_PORT (hangar rodando?)\n")),
        ("list__local-401", why("R3-D02").stderr("erro HTTP 401: {\"detail\": \"token\"}\n")),
        ("list__local-not-json", why("R3-D02").stderr("erro: resposta ilegível de /api/sessions\n")),
        ("list__session-missing-state", why("D14.3: campo faltando vira erro:").stderr("erro: resposta de /api/sessions sem o campo 'state'\n")),
        ("list__ext-missing-field", why("D14.3: campo faltando vira erro:").stderr("erro: resposta de /api/external-pairs sem o campo 'session'\n")),
        // O nome vai codificado na URL: `#`, `?`, `/` e espaço mandariam o pedido a outra rota.
        ("msg__target-raw-hash", why("D14.3").path("local", 0, "/api/sessions/a%23b/input")),
        ("msg__target-raw-slash", why("D14.3").path("local", 0, "/api/sessions/a%2Fb/input")),
        ("msg__target-raw-question", why("D14.3").path("local", 0, "/api/sessions/a%3Fb%3D1/input")),
        ("msg__target-raw-percent", why("D14.3").path("local", 0, "/api/sessions/a%252Fb/input")),
        ("msg__target-raw-space", why("D14.3 / surpresa 13").code(0).stderr("")
            .stdout("na fila -> a b (não orientado; entrega no próximo idle)\n").http(encoded("/api/sessions/a%20b/input"))),
        // Flag desconhecida no topo é recusada; senão viraria o nome do destino.
        ("msg__unknown-flag-is-target", why("D14.3").code(2).stdout("")
            .stderr("erro: opção desconhecida '--foo' (veja hangar-send --help)\n").no_http()),
        ("msg__no-target", why("D14.2: erro de uso sai com 2")
            .code(2).stderr("uso: hangar-send [--tmux] <sessao|servidor::sessao> <mensagem...>\n")),
        // Resposta ilegível do envio não vira "na fila".
        ("msg__response-not-json", why("R3-D05").code(1).stdout("").stderr(&unreadable_send("alvo"))),
        ("msg__response-empty", why("R3-D05").code(1).stdout("").stderr(&unreadable_send("alvo"))),
        ("msg__ext-pair-send-not-json", why("R3-D05").code(1).stdout("").stderr(&unreadable_send("ana::s1"))),
        // `a::b::c` recusado antes de qualquer pedido.
        ("msg__remote-three-parts", why("D14.3").code(2).stdout("")
            .stderr("erro: nome inválido 'a::b::c': use servidor::sessao (um '::' só)\n").no_http()),
        // Campo faltando ou JSON torto na resposta vira `erro:` com código 1.
        ("aceitar-par__missing-field", why("R3-D02").stderr("erro: resposta de /api/sessions/cli/pair-accept sem o campo 'session'\n")),
        ("group__missing-peers", why("R3-D02").stderr("erro: resposta de /api/sessions/eu/group-message sem o campo 'peers'\n")),
        ("group__warning-str-none-delivered", why("surpresa 16 corrigida").code(0).stderr("")
            .stdout("  ⚠ sem o aviso (máquina fora do ar): a\n  ⚠ só texto\n")),
        ("pair__orq-alone-not-json", why("R3-D02").stderr("erro: resposta ilegível de /api/sessions/eu/pair\n")),
        ("contrato__read-not-json", why("R3-D02").stderr("erro: resposta ilegível de /api/sessions/cli/pair/contract\n")),
        ("close__delete-response-not-json", why("surpresa 16 corrigida: resposta do DELETE ilegível é ignorada").stderr("")),
        ("new__providers-unreadable", why("R3-D02").stderr("erro: resposta ilegível de /api/providers\n")),
        ("new__response-not-json", why("R3-D02")
            .stderr("aviso: a criação foi confirmada, mas não consegui ler os avisos da resposta\n")),
        ("new__link-unreadable", why("R3-D02").stderr(
            "aviso: a criação foi confirmada, mas não consegui ler os avisos da resposta\n\
             erro: a sessão nova abriu, mas não consegui ler o resultado da ligação na resposta do servidor (confira com hangar-send --list)\n")),
        ("new__conta-auto-missing-label", why("surpresa 14 corrigida").code(2)
            .stderr("erro: resposta da cota ilegível em http://127.0.0.1:$HTTP_LOCAL — escolha uma conta: --conta <nome>\n")),
        ("new__resume-response-not-json", why("surpresa 15 e decisão 16: cai no nome pedido e avisa")
            .stderr("aviso: a retomada foi confirmada, mas não consegui ler a resposta (nome e pasta)\n")
            .stdout("sessão retomada: volta () na conversa abc — fala com ela via: hangar-send volta \"msg\"\n")),
        ("contrato__gravar-not-utf8", why("surpresa 11 corrigida: sem traceback").stderr("erro: '$T/work/c.md' não é texto UTF-8\n")),
        // Erro de uso sai com 2 e a frase `uso:`, sem o `script: line N:` do bash.
        ("new__no-name", why("D14.2").code(2).stderr(NEW_USAGE)),
        ("new__name-is-flag", why("--code-mode no uso").stderr(&format!("erro: nome de sessão não pode começar com '-' (esqueceu o nome?)\n{NEW_USAGE}"))),
        ("new__minimal-outside", why(lookup_gone).http(without_provider_lookup("new__minimal-outside"))
            .stdout("sessão criada: nova ($T/home) — fala com ela via: hangar-send nova \"msg\"\n")),
        ("new__providers-none-default", why(lookup_gone).http(without_provider_lookup("new__providers-none-default"))
            .stdout("sessão criada: nova ($T/home) — fala com ela via: hangar-send nova \"msg\"\n")),
        ("new__remote-default-provider-and-tilde", why(lookup_gone).http(without_provider_lookup("new__remote-default-provider-and-tilde"))
            .stdout("sessão criada: a::nova (~) — fala com ela via: hangar-send a::nova \"msg\"\n")),
        ("new__flags-before-cwd-terminal-sem-jev-no-inherit", why(lookup_gone).http(without_provider_lookup("new__flags-before-cwd-terminal-sem-jev-no-inherit"))
            .stdout("sessão criada: nova (/w) [jev: false] — fala com ela via: hangar-send nova \"msg\"\n")),
        ("new__providers-500", skip(lookup_gone, "o --new não consulta mais /api/providers")),
        ("new__providers-unreadable", skip(lookup_gone, "o --new não consulta mais /api/providers")),
        ("new__link-remote", skip("filha ligada entre máquinas", "servidor:: com --link abre pelo servidor daqui (open_fields.rs)")),
        ("new__value-missing-at-end", why("D14.2").code(2).stderr("uso: --model <id>\n")),
        ("pair__no-peer", why("D14.2").code(2).stderr(PAIR_USAGE)),
        ("close__no-target", why("D14.2").code(2).stderr("uso: hangar-send --close <sessao|servidor::sessao>\n")),
        ("contrato__gravar-no-value", why("D14.2").code(2).stderr(CONTRACT_USAGE)),
        // NaN no JSON é recusado com código, não reenviado.
        ("new__customizations-nan", why("D13.4").code(2).stdout("")
            .stderr("erro: --customizations precisa ser um objeto JSON\n").no_http()),
    ])
}

/// Troca os placeholders do arquivo pelos valores deste caso.
struct Places {
    root: String,
    ports: Vec<(String, u16)>,
}

/// A porta fixa (`CP_PORT=<n>`, n ≥ 1024) que a captura gravou para o servidor desligado.
fn dead_port_literal(text: &str) -> Option<u16> {
    let rest = &text[text.find("CP_PORT=")? + "CP_PORT=".len()..];
    let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
    digits.parse::<u16>().ok().filter(|port| *port >= 1024)
}

impl Places {
    fn fill(&self, text: &str) -> String {
        // Alguns casos gravaram a porta morta como número fixo no `.env`; ela vira a porta morta do teste.
        let mut text = dead_port_literal(text).map_or_else(|| text.to_owned(), |port| text.replace(&format!("CP_PORT={port}\n"), "CP_PORT=$DEAD_PORT\n"));
        for (name, port) in &self.ports {
            text = text.replace(name, &port.to_string());
        }
        text.replace("$T", &self.root)
    }

    /// O inverso, na saída: pasta do caso e portas de volta aos nomes.
    fn normalize(&self, text: &str) -> String {
        let mut text = text.replace(&self.root, "$T");
        let marker = "127.0.0.1:";
        let mut out = String::new();
        while let Some(at) = text.find(marker) {
            out.push_str(&text[..at + marker.len()]);
            let rest = &text[at + marker.len()..];
            let digits = rest.bytes().take_while(u8::is_ascii_digit).count();
            let port = rest[..digits].parse::<u16>().ok();
            match port.and_then(|port| self.ports.iter().find(|(_, p)| *p == port)) {
                Some((name, _)) => out.push_str(name),
                None => out.push_str(&rest[..digits]),
            }
            text = rest[digits..].to_owned();
        }
        out.push_str(&text);
        out
    }
}

fn placeholder(server: &str) -> String {
    format!("$HTTP_{}", server.replace('-', "_").to_uppercase())
}

fn text_of(value: &Value) -> Vec<u8> {
    use base64::Engine;
    match value {
        Value::String(text) => text.clone().into_bytes(),
        Value::Object(object) => object.get("b64").and_then(Value::as_str)
            .map(|b64| base64::engine::general_purpose::STANDARD.decode(b64).expect("b64 do caso"))
            .unwrap_or_default(),
        _ => Vec::new(),
    }
}

fn materialise(base: &Path, files: &serde_json::Map<String, Value>, places: &Places) {
    use std::os::unix::fs::PermissionsExt;
    let mut entries: Vec<_> = files.iter().collect();
    entries.sort_by(|a, b| a.0.cmp(b.0));
    for (relative, entry) in entries {
        let path = base.join(relative);
        let mode = entry.get("mode").and_then(Value::as_u64);
        match entry["type"].as_str() {
            Some("dir") => std::fs::create_dir_all(&path).unwrap(),
            Some("file") => {
                std::fs::create_dir_all(path.parent().unwrap()).unwrap();
                let bytes = match entry.get("text").and_then(Value::as_str) {
                    Some(text) => places.fill(text).into_bytes(),
                    None => text_of(entry),
                };
                std::fs::write(&path, bytes).unwrap();
            }
            Some("symlink") => {
                std::fs::create_dir_all(path.parent().unwrap()).unwrap();
                std::os::unix::fs::symlink(places.fill(entry["target"].as_str().unwrap()), &path).unwrap();
            }
            other => panic!("tipo de arquivo desconhecido no caso: {other:?}"),
        }
        if let Some(mode) = mode && entry["type"] != "symlink" {
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(mode as u32)).unwrap();
        }
    }
}

fn routes_of(value: &Value) -> Vec<Route> {
    value.as_array().unwrap().iter().map(|route| Route {
        method: route["method"].as_str().unwrap().into(),
        prefix: route["prefix"].as_str().unwrap().into(),
        replies: route["responses"].as_array().unwrap().iter().map(|reply| Reply {
            status: reply["status"].as_u64().unwrap() as u16,
            body: reply["body"].as_str().unwrap().into(),
            delay: reply.get("delay").and_then(Value::as_f64).unwrap_or(0.0),
        }).collect(),
    }).collect()
}

fn table(value: &Value, places: &Places) -> BTreeMap<String, Answer> {
    value.as_object().unwrap().iter()
        .map(|(key, answer)| (key.clone(), Answer::from_json(answer, &|text: &str| places.fill(text))))
        .collect()
}

/// Corpo como JSON quando dá; vazio é igual a vazio.
fn same_body(expected: &[u8], actual: &[u8]) -> bool {
    if expected.is_empty() || actual.is_empty() {
        return expected.is_empty() == actual.is_empty();
    }
    match (serde_json::from_slice::<Value>(expected), serde_json::from_slice::<Value>(actual)) {
        (Ok(a), Ok(b)) => a == b,
        _ => expected == actual,
    }
}

fn run_case(case: &Value, change: Option<&Override>) -> Vec<String> {
    let name = case["name"].as_str().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let base = dir.path().to_path_buf();
    let servers: BTreeMap<String, FakeServer> = case["routes"].as_object().unwrap().iter()
        .map(|(server, routes)| (server.clone(), FakeServer::start(routes_of(routes))))
        .collect();
    let mut ports: Vec<(String, u16)> = servers.iter().map(|(server, fake)| (placeholder(server), fake.port)).collect();
    ports.push(("$DEAD_PORT".into(), common::dead_port()));
    // O placeholder mais longo primeiro: `$HTTP_PEER_AB` antes de `$HTTP_PEER_A`.
    ports.sort_by(|a, b| b.0.len().cmp(&a.0.len()));
    let places = Places { root: base.to_string_lossy().into_owned(), ports };
    std::fs::create_dir_all(base.join("work")).unwrap();
    std::fs::create_dir_all(base.join("home")).unwrap();
    materialise(&base, case["files"].as_object().unwrap(), &places);

    let server = case["server"].as_object().map(|_| table(&case["server"], &places));
    let host = FakeHost::new(server, table(&case["tmux"], &places));
    let args: Vec<String> = case["args"].as_array().unwrap().iter().map(|arg| places.fill(arg.as_str().unwrap())).collect();
    let mut vars: BTreeMap<String, String> = case["env"].as_object().unwrap().iter()
        .map(|(key, value)| (key.clone(), places.fill(value.as_str().unwrap())))
        .collect();
    // O script rodava com o cwd em $T/work; aqui o processo é o do teste.
    vars.insert("PWD".into(), base.join("work").to_string_lossy().into_owned());
    let root = base.join(case["inst"].as_str().unwrap());
    let outcome = common::invoke(&args, Some(root), &base.join("home"), vars, &host);

    let expect = &case["expect"];
    let mut problems = Vec::new();
    let expected_code = change.and_then(|c| c.code).unwrap_or(expect["code"].as_u64().unwrap() as u8);
    if outcome.code != expected_code {
        problems.push(format!("código {} (esperado {expected_code})", outcome.code));
    }
    let help = |text: String| if text == "<HELP>" { hangar_send::HELP.to_owned() } else { text };
    let expected_out = change.and_then(|c| c.stdout.clone()).map(help)
        .unwrap_or_else(|| String::from_utf8_lossy(&text_of(&expect["stdout"])).into_owned());
    let expected_err = change.and_then(|c| c.stderr.clone())
        .unwrap_or_else(|| String::from_utf8_lossy(&text_of(&expect["stderr"])).into_owned());
    let (stdout, stderr) = (places.normalize(&outcome.stdout), places.normalize(&outcome.stderr));
    if stdout != expected_out {
        problems.push(format!("stdout {stdout:?}\n   esperado {expected_out:?}"));
    }
    if stderr != expected_err {
        problems.push(format!("stderr {stderr:?}\n   esperado {expected_err:?}"));
    }

    let mut wanted = change.and_then(|c| c.http.clone()).unwrap_or_else(|| expect["http"].clone());
    if let Some(change) = change {
        for (server, index, path) in &change.paths {
            wanted[*server][*index]["path"] = json!(path);
        }
        if let Some(auth) = change.auth {
            for requests in wanted.as_object_mut().unwrap().values_mut() {
                for request in requests.as_array_mut().unwrap() {
                    request["auth"] = json!(auth);
                }
            }
        }
    }
    // Balanceamento: o `--new` sem servidor:: pede a escolha da máquina ao servidor daqui.
    if let Some(requests) = wanted.get_mut("local").and_then(Value::as_array_mut) {
        for request in requests.iter_mut().filter(|request| request["method"] == "POST" && request["path"] == "/api/sessions") {
            let mut body: Value = serde_json::from_str(request["body"].as_str().unwrap()).unwrap();
            body["machine"] = json!("auto");
            request["body"] = Value::String(body.to_string());
        }
    }
    let empty = Vec::new();
    let mut names: Vec<&String> = servers.keys().collect();
    names.extend(wanted.as_object().unwrap().keys().filter(|key| !servers.contains_key(*key)));
    for server in names {
        let actual = servers.get(server).map(FakeServer::requests).unwrap_or_default();
        let expected = wanted.get(server).and_then(Value::as_array).unwrap_or(&empty);
        if actual.len() != expected.len() {
            let seen: Vec<String> = actual.iter().map(|r| format!("{} {}", r.method, r.path)).collect();
            problems.push(format!("{server}: {} pedidos {seen:?} (esperados {})", actual.len(), expected.len()));
            continue;
        }
        for (index, (got, want)) in actual.iter().zip(expected).enumerate() {
            let auth = got.header("authorization").map(|auth| places.normalize(auth));
            let checks = [
                (got.method == want["method"].as_str().unwrap(), "método"),
                (places.normalize(&got.path) == want["path"].as_str().unwrap(), "caminho"),
                (auth.as_deref() == want["auth"].as_str(), "Authorization"),
                (got.header("content-type").is_some() == want["json"].as_bool().unwrap(), "Content-Type"),
                (same_body(&places.fill(&String::from_utf8_lossy(&text_of(&want["body"]))).into_bytes(), &got.body), "corpo"),
            ];
            for (ok, what) in checks {
                if !ok {
                    problems.push(format!("{server}[{index}] {what}: {} {} {:?} {}  esperado {want}",
                        got.method, got.path, auth, String::from_utf8_lossy(&got.body)));
                }
            }
        }
    }
    problems.into_iter().map(|problem| format!("{name}: {problem}")).collect()
}

#[test]
fn every_captured_case_matches_or_has_a_decision() {
    let cases: Vec<Value> = serde_json::from_str(CASES).expect("send_cases.json");
    let changes = overrides();
    let known: Vec<&str> = cases.iter().map(|case| case["name"].as_str().unwrap()).collect();
    for name in changes.keys() {
        assert!(known.contains(name), "override sem caso: {name}");
    }
    let (mut failures, mut compared, mut skipped) = (Vec::new(), 0, 0);
    for case in &cases {
        let change = changes.get(case["name"].as_str().unwrap());
        if change.is_some_and(|c| c.skip.is_some()) {
            skipped += 1;
            continue;
        }
        assert!(change.is_none_or(|c| !c.why.is_empty()));
        compared += 1;
        failures.extend(run_case(case, change));
    }
    assert!(failures.is_empty(), "{} divergências em {compared} casos ({skipped} pulados):\n{}", failures.len(), failures.join("\n"));
    assert_eq!(compared + skipped, 241);
}
