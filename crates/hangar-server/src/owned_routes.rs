//! A tabela das rotas que o Rust atende sem repassar ao Python. Os testes a usam para provar que uma
//! rota é do Rust, e o teste daqui prende cada rota nova do roteador a ela.
use axum::http::Method;

/// O Rust atende este pedido do dono sem repassar? Mesma decisão do `routes::router`: as rotas
/// GET registradas lá, mais os `matches` de Git/arquivos e de worktrees que o `pass_any` consulta.
pub fn rust_route(method: &Method, path: &str) -> bool {
    if crate::auxiliary::owns_path(path) { return true; }
    let workflow = path.strip_prefix("/api/sessions/").and_then(|rest| rest.split_once('/'))
        .is_some_and(|(name, tail)| !name.is_empty() && (tail == "workflows" || tail.starts_with("workflows/")));
    // Essas famílias resolvem também os métodos recusados dentro do Rust.
    if crate::login_guests::owns_path(path) || crate::loops::http::owns_path(path)
        || crate::push::http::owns_path(path) || crate::machine_peers::owns_http_path(path)
        || crate::session_open::baton::matches(path) || crate::shortcuts::http::matches(path)
        || crate::projects_routes::matches(path)
        || crate::session_tools::owns_path(path)
        || crate::costs_routes::usage_path(path) || crate::diag::owns_path(path)
        || crate::plugins::matches(path) || workflow || hangar_desktop_theme::owns_path(path)
        || hangar_shortcut_transfer::owns_path(path)
        || matches!(path, "/api/config-sync/apply" | "/api/update-channel") { return true; }
    if *method == Method::GET && matches!(path, "/api/config-sync/manifest" | "/api/config-sync/bundle") { return true; }
    if crate::ui::matches(method, path) || crate::dictation::http::matches(method, path)
        || crate::speech::http::matches(method, path) { return true; }
    // O serviço MCP responde também aos métodos recusados, sem repasse ao Python.
    if matches!(path, "/mcp" | "/mcp/") { return true; }
    if *method == Method::GET && path == "/api/whoami" { return true; }
    if matches!(*method, Method::GET | Method::PUT) && matches!(path, "/api/auto-report" | "/api/session-defaults" | "/api/session-options" | "/api/tags" | "/api/notice-ignores" | "/api/tour-progress" | "/api/web-ui" | "/api/session-auto-hide" | "/api/telemetry") { return true; }
    if (*method == Method::GET && path == "/api/telemetry/owner") || (*method == Method::POST && path == "/api/telemetry") { return true; }
    if (matches!(*method, Method::GET | Method::PUT) && path == "/api/update-channel") { return true; }
    if (*method == Method::GET && path == "/api/atualizacao")
        || (*method == Method::POST && matches!(path, "/api/atualizacao/iniciar" | "/api/atualizacao/reiniciar" | "/api/atualizacao/dev-mode")) { return true; }
    if *method == Method::POST && path == "/api/push/expo/subscribe" { return true; }
    if crate::orq_conductor::matches(method, path) { return true; }
    if crate::orchestration::matches(method, path) || crate::provider_models::matches(method, path)
        || crate::omp_sync::matches(method, path) { return true; }
    if crate::prewarm::http::matches(method, path) { return true; }
    if crate::remote_browser::url::matches(method, path) { return true; }
    if crate::nav_requests::matches(method, path) { return true; }
    if crate::loops::http::matches(method, path) { return true; }
    if crate::push::http::matches(method, path) { return true; }
    if crate::session_media::is_media(method, path) { return true; }
    if crate::archive_history::matches(method, path) || crate::archive_list::matches(method, path) { return true; }
    if crate::accounts::http::matches(method, path) { return true; }
    // Ligação e credenciais de aparelho do Connect (`connect::routes`): nunca repassadas.
    if crate::connect::routes::matches(method, path) { return true; }
    // Senha do chaveiro (`keyring::unlock_route`): nunca repassada.
    if *method == Method::POST && path == "/api/keyring/unlock" { return true; }
    // Cofre de senhas (`vault::http`): nunca repassado.
    if crate::vault::http::matches(method, path) { return true; }
    // Integrações MCP (`integrations::http`): nunca repassado.
    if crate::integrations::http::matches(method, path) { return true; }
    // Relatório de diagnóstico (`diag_report::report_route`): só o dono, nunca repassado.
    if *method == Method::POST && path == "/api/diag/report" { return true; }
    // SideStore no aparelho do cabo (`device_install::routes`): nunca repassado.
    if *method == Method::GET && matches!(path, "/api/device-install/devices" | "/api/device-install/progress") { return true; }
    if *method == Method::POST && matches!(path, "/api/device-install/trust" | "/api/device-install/start" | "/api/device-install/code" | "/api/device-install/certs" | "/api/device-install/cancel") { return true; }
    if *method == Method::POST && matches!(path, "/api/update/accept" | "/api/update/arrived/seen") { return true; }
    if *method == Method::POST && path == "/api/ask-history" { return true; }
    // Code Mode já ligado no settings.json principal (`claude_defaults::code_mode`).
    if *method == Method::GET && path == "/api/claude/code-mode" { return true; }
    // Balanceamento de sessões (`placement`) e o interromper da filha pedido pelo pai de outra máquina.
    if (matches!(*method, Method::GET | Method::PUT) && path == "/api/placement/config")
        || (*method == Method::GET && path == "/api/placement/facts")
        || (*method == Method::POST && matches!(path, "/api/placement/suggest" | "/api/placement/clone")) { return true; }
    if *method == Method::POST && path.strip_prefix("/api/sessions/").and_then(|r| r.split_once('/')).is_some_and(|(_, t)| t == "linked-interrupt") { return true; }
    // `peer-address` saiu: o Rust responde qualquer método com o 404 de rota inexistente.
    if path.strip_prefix("/api/sessions/").and_then(|r| r.split_once('/')).is_some_and(|(_, t)| t == "peer-address") { return true; }
    if *method == Method::GET {
        if matches!(path, "/api/sessions" | "/api/sessions/events" | "/api/sessions/creation-progress" | "/api/costs" | "/api/cotacao" | "/api/uso" | "/api/status" | "/api/status/events" | "/api/presence" | "/api/update/notes" | "/api/update/arrived" | "/api/news" | "/api/manual"
            | "/api/voice" | "/api/voice/settings" | "/api/voice/memory" | "/api/jev/aux/cliproxy" | "/api/jev/delivery" | "/api/client-api"
            | "/api/search" | "/api/search/context" | "/api/port-tunnel" | "/api/browser-host") {
            return true;
        }
        let tail = path.strip_prefix("/api/sessions/").and_then(|r| r.split_once('/')).map(|(_, t)| t);
        // `term`: o painel e toda recusa do aperto de mão são do Rust, em todas as portas.
        if matches!(tail, Some("history" | "events" | "artifacts" | "cost" | "term" | "nav-remoto" | "models" | "limits" | "commands" | "codex-permissions" | "conta" | "pane" | "permission-modes")) || tail.is_some_and(|t| t.starts_with("pages/")) {
            return true;
        }
        if tail.is_some_and(|t| t == "subagents" || t.strip_prefix("subagents/").is_some_and(|id| !id.is_empty() && !id.contains('/'))) {
            return true;
        }
        if path.strip_prefix("/api/hangar-terminals/").is_some_and(|r| r.ends_with("/term")) {
            return true;
        }
    }
    if *method == Method::PUT && path == "/api/voice/settings" {
        return true;
    }
    if *method == Method::DELETE && path.starts_with("/api/voice/memory/") {
        return true;
    }
    // Interface dos mods: o Rust atende as sessões dele e repassa as outras.
    if *method == Method::POST {
        if matches!(path, "/api/sessions" | "/api/claude/defaults" | "/api/presence" | "/api/presence/heartbeat" | "/api/presence/app" | "/api/jev/aux/models" | "/api/mermaid") {
            return true;
        }
        let tail = path.strip_prefix("/api/sessions/").and_then(|r| r.split_once('/')).map(|(_, t)| t);
        if matches!(tail, Some("plugin/press" | "plugin/close" | "plugin/show" | "plugin/input")) || matches!(path.strip_prefix("/api/plugin/"),
            Some("press-start" | "opened" | "ui" | "toast" | "pressed" | "copied" | "focus-target" | "focused" | "scroll")) {
            return true;
        }
    }
    // Escritas Claude: o Rust decide por pedido; o que não é dele ele repassa.
    let tail = path.strip_prefix("/api/sessions/").and_then(|r| r.split_once('/')).map(|(_, t)| t);
    if *method == Method::POST && matches!(tail, Some("input" | "steer" | "interrupt" | "select" | "select/submit" | "answer" | "keys" | "term-input" | "browser-cmd")) {
        return true;
    }
    // Texto a um subagente em segundo plano: o plugin do Claude entrega pelo `/pull`.
    if *method == Method::POST && tail.and_then(|t| t.strip_prefix("subagents/")).and_then(|t| t.strip_suffix("/message"))
        .is_some_and(|id| !id.is_empty() && !id.contains('/')) {
        return true;
    }
    // Parar uma tarefa: o Rust responde por todo provedor, sem rota no Python.
    if *method == Method::POST && tail.and_then(|t| t.strip_prefix("tasks/")).and_then(|t| t.strip_suffix("/stop"))
        .is_some_and(|id| !id.is_empty() && !id.contains('/')) {
        return true;
    }
    // Parar o servidor de uma porta da sessão (`port_stop`): só o dono, sem rota no Python.
    if *method == Method::POST && tail.and_then(|t| t.strip_prefix("ports/")).and_then(|t| t.strip_suffix("/stop"))
        .is_some_and(|port| !port.is_empty() && !port.contains('/')) {
        return true;
    }
    // Rotas só do Codex, modos de permissão do Claude e as reaberturas (troca de modo, recarregar, conta,
    // conta ChatGPT fixa): o Rust atende as sessões dele.
    if *method == Method::POST && matches!(tail, Some("model" | "service-tier" | "codex/mode" | "codex/plan/implement" | "codex/terminate" | "question/skip" | "codex-permissions"
        | "permission-mode" | "modo-execucao" | "recarregar" | "conta" | "engine/model" | "engine/provider" | "model-effort" | "resume" | "reopen" | "dismiss" | "rename")) {
        return true;
    }
    // Sugestão de nome da sessão genérica (`dictation::rename_suggestion`): só o dono, nunca repassada.
    if matches!(*method, Method::POST | Method::DELETE) && tail == Some("rename-suggestion") {
        return true;
    }
    // Consertar o hook de um aviso do Claude (`notice_actions`): só o dono, nunca repassada.
    if *method == Method::POST && matches!(tail, Some("notice/fix" | "notice/fix/preview")) {
        return true;
    }
    // Ocultar da lista (`session_visibility`): só o dono, nunca repassada.
    if matches!(*method, Method::PUT | Method::DELETE) && tail == Some("hidden") {
        return true;
    }
    // Continuar sozinho depois de queda (`auto_continue`): interruptor e Cancelar, só do dono, nunca repassada.
    if matches!(*method, Method::PUT | Method::DELETE) && tail == Some("auto-continue") {
        return true;
    }
    // A pergunta lateral do Claude, com e sem terminal, e o histórico dela.
    if matches!(*method, Method::POST | Method::GET) && tail == Some("btw") {
        return true;
    }
    // Retomar do Arquivo: a abertura do Rust com a conversa que já existe.
    if *method == Method::POST && path.strip_prefix("/api/archive/").and_then(|r| r.strip_suffix("/resume"))
        .is_some_and(|r| r.split('/').count() == 2) {
        return true;
    }
    if *method == Method::GET && tail == Some("permission-modes") {
        return true;
    }
    // Cancelar a troca de agente que espera o turno acabar (`session_write::handoff`).
    if *method == Method::DELETE && tail == Some("conta") {
        return true;
    }
    // Encerrar (o Codex com terminal fecha aqui antes da limpeza do Python), broadcast e a ponte do plugin.
    if *method == Method::DELETE && path.strip_prefix("/api/sessions/").is_some_and(|name| !name.is_empty() && !name.contains('/')) {
        return true;
    }
    if *method == Method::POST && (path == "/api/broadcast" || matches!(path.strip_prefix("/api/plugin/"),
        Some("whoami" | "pull" | "ask" | "ask-fim" | "plan" | "filled" | "submitted" | "state" | "suggest" | "rate" | "agent" | "agent-sent"))) {
        return true;
    }
    if tail.is_some_and(|t| crate::session_write::control::queue_route(method, t).is_some()) {
        return true;
    }
    // Grupos (`groups::routes`): o convidado é recusado pelo porteiro, antes da rota.
    if matches!((method.as_str(), tail), ("POST", Some("pair" | "group-message" | "pair-remote" | "unpair-remote"))
        | ("DELETE", Some("pair")) | ("GET" | "PUT", Some("pair/contract"))) {
        return true;
    }
    (*method == Method::GET && (path == crate::claude_customizations::CATALOG_PATH || path == crate::commands::NEW_CHAT_PATH))
        || crate::plans::routes::matches(method, path) || crate::share_reach::matches(method, path) || share_route(method, path)
        || crate::workspace_routes::matches(method, path) || crate::worktree_routes::matches(method, path)
        || crate::reviews::matches(method, path)
        || crate::dream::routes::matches(method, path)
        || crate::computer_control::matches(method, path)
}

/// Convites e par externo do dono (`share::owner_routes`, `share::pair_routes`).
fn share_route(method: &Method, path: &str) -> bool {
    let tail = path.strip_prefix("/api/sessions/").and_then(|r| r.split_once('/')).map(|(_, t)| t);
    match method.as_str() {
        "GET" => matches!(path, "/api/share/prereqs" | "/api/external-pairs") || tail == Some("share"),
        "POST" => path == "/api/external-pairs/send" || matches!(tail, Some("share" | "pair-invite" | "pair-accept" | "pair-access")),
        "DELETE" => tail.is_some_and(|t| t == "share" || t.strip_prefix("share/").is_some_and(|id| !id.is_empty() && !id.contains('/')))
            || path.strip_prefix("/api/external-pairs/").is_some_and(|alias| !alias.is_empty() && !alias.contains('/')),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_table_follows_the_router() {
        let get = |p| rust_route(&Method::GET, p);
        assert!(get("/api/sessions/a/history") && get("/api/sessions") && get("/api/uso"));
        assert!(get("/api/sessions/a/git/log") && get("/api/worktrees"), "Git e worktrees vêm dos matches");
        assert!(get("/api/sessions/a/pages/p1") && get("/api/sessions/a/pages/p1/shot") && !get("/api/sessions/a/pages"));
        assert!(rust_route(&Method::POST, "/api/mermaid") && !get("/api/mermaid"));
        assert!(get("/api/status") && get("/api/status/events"), "a tela Status é só do Rust");
        assert!(rust_route(&Method::POST, "/api/sessions/a/input") && rust_route(&Method::DELETE, "/api/sessions/a/queue/e1")
            && !rust_route(&Method::POST, "/api/sessions/a/queue/e1") && rust_route(&Method::POST, "/api/sessions/a/queue/e1/steer")
            && rust_route(&Method::PATCH, "/api/sessions/a/queue/e1") && get("/api/cotas") && rust_route(&Method::POST, "/api/worktrees/create"));
        assert!(rust_route(&Method::POST, "/api/sessions/a/pair") && rust_route(&Method::DELETE, "/api/sessions/a/pair")
            && get("/api/sessions/a/pair/contract") && rust_route(&Method::PUT, "/api/sessions/a/pair/contract")
            && rust_route(&Method::POST, "/api/sessions/a/unpair-remote")
            && !get("/api/sessions/a/pair") && rust_route(&Method::POST, "/api/sessions/a/pair-invite"));
        assert!(rust_route(&Method::POST, "/api/sessions/a/share") && rust_route(&Method::DELETE, "/api/sessions/a/share/s1")
            && get("/api/share/prereqs") && rust_route(&Method::POST, "/api/share/reachability")
            && !rust_route(&Method::DELETE, "/api/sessions/a/share/s1/x"));
        // O corte: relançar e recarregar (Claude e Codex), trocar de conta, modos do Claude, encerrar, broadcast e
        // a ponte do plugin são do Rust.
        let post = |p: &str| rust_route(&Method::POST, p);
        assert!(post("/api/sessions/a/modo-execucao") && post("/api/sessions/a/recarregar") && post("/api/sessions/a/conta") && get("/api/sessions/a/conta"));
        assert!(post("/api/sessions/a/permission-mode")
            && get("/api/sessions/a/permission-modes") && rust_route(&Method::DELETE, "/api/sessions/a") && post("/api/broadcast"));
        assert!(post("/api/sessions/a/subagents/x1/message") && !post("/api/sessions/a/subagents//message") && !get("/api/sessions/a/subagents/x1/message"));
        assert!(post("/api/sessions/a/transcribe") && post("/api/sessions/a/bash-output") && get("/api/sessions/a/tool-progress/toolu_x")
            && get("/api/sessions/a/transcript-image/u/0") && !get("/api/sessions/a/bash-output"));
        assert!(["whoami", "pull", "ask", "ask-fim", "filled", "submitted", "state", "suggest", "rate", "agent", "agent-sent"].iter().all(|e| post(&format!("/api/plugin/{e}"))));
        assert!(!rust_route(&Method::DELETE, "/api/sessions") && !post("/api/plugin/outra"));
        assert!(get("/api/sessions/a/plan") && get("/api/sessions/a/plans") && get("/api/sessions/a/plan-preview")
            && get("/api/claude/customizations") && rust_route(&Method::POST, "/api/sessions/a/plan-step")
            && !rust_route(&Method::POST, "/api/sessions/a/plan") && !get("/api/sessions/a/plan-step"));
        assert!(!get("/docs") && get("/"));
        assert!(!get("/api/migration/status"), "a tela da migração saiu com a rota");
    }

    /// Rota nova no `routes::router` sem entrar em `rust_route` faria a tabela dizer "Python" para o
    /// que o Rust atende. O roteador não se deixa listar; o texto dele, sim.
    #[test]
    fn every_route_registered_in_the_router_is_known_to_the_table() {
        // O checkout do Windows pode trazer CRLF; o fim do roteador é procurado por "\n}\n".
        let source = include_str!("routes.rs").replace("\r\n", "\n");
        // As rotas de sessão moram em `session_routes`, montadas pelas duas portas; o resto, em `public_routes`.
        let fn_body = |name: &str| {
            let body = &source[source.find(name).unwrap()..];
            body[..body.find("\n}\n").unwrap()].to_owned()
        };
        let body = fn_body("fn public_routes(") + "\n" + &fn_body("fn session_routes<");
        let body = body.as_str();
        let (mut seen, mut api) = (0, 0);
        for line in body.lines().map(str::trim).filter(|l| l.starts_with(".route(\"")) {
            seen += 1;
            let path = line.split('"').nth(1).unwrap().replace("{name}", "x");
            if !path.starts_with("/api/") { continue; }
            // Uma rota pode juntar métodos (`get(…).post(…)`): cada um tem de estar na tabela.
            let mut methods:Vec<Method> = [("get(",Method::GET),("post(",Method::POST),("delete(",Method::DELETE),("put(",Method::PUT),("patch(",Method::PATCH)]
                .into_iter().filter(|(call,_)| line.contains(&format!("::{call}")) || line.contains(&format!(" {call}")) || line.contains(&format!(".{call}")) || line.contains(&format!("({call}")))
                .map(|(_,method)| method).collect();
            if line.contains("::any(") {
                methods = vec![Method::GET, Method::POST, Method::PUT, Method::PATCH,
                    Method::DELETE, Method::HEAD, Method::OPTIONS, Method::TRACE, Method::CONNECT];
            }
            assert!(!methods.is_empty(), "método que o leitor não conhece: {line}");
            for method in methods { assert!(rust_route(&method, &path), "rota do roteador fora da tabela: {method} {path}"); }
            api += 1;
        }
        // Rota quebrada em várias linhas escaparia do filtro acima: a contagem bruta a denuncia.
        assert_eq!(seen, body.matches(".route(").count(), "há `.route(` fora de uma linha `.route(\"…\"`");
        assert!(api >= 10, "o leitor do roteador não achou as rotas ({api})");
        // Montadas por `connect::routes` e `share::pair_routes`, fora do texto lido acima.
        for (method, path) in [(Method::POST, "/api/connect/redeem"), (Method::POST, "/api/connect/redeem/status"), (Method::GET, "/api/connect/devices"),
            (Method::POST, "/api/connect/devices/self"), (Method::POST, "/api/connect/devices/x/approve"), (Method::DELETE, "/api/connect/devices/x"),
            (Method::GET, "/api/connect"), (Method::DELETE, "/api/connect"), (Method::POST, "/api/connect/link"),
            (Method::GET, "/api/connect/peers"), (Method::GET, "/api/connect/machines"), (Method::POST, "/api/connect/peers/x/link"),
            (Method::GET, "/api/connect/preview"), (Method::POST, "/api/connect/preview"), (Method::DELETE, "/api/connect/preview")] {
            assert!(rust_route(&method, path), "{method} {path}");
        }
        assert!(!rust_route(&Method::PUT, "/api/connect"), "o código de ligação colado saiu");
        assert!(rust_route(&Method::POST, "/api/sessions/a/pair-access"));
        assert!(rust_route(&Method::DELETE, "/api/external-pairs/ana"));
        assert!(!rust_route(&Method::DELETE, "/api/external-pairs/a/b"));
    }

    #[test]
    fn mcp_and_speech_routes_belong_to_rust() {
        for path in ["/mcp", "/mcp/"] {
            for method in [Method::GET, Method::POST, Method::DELETE, Method::HEAD, Method::PUT] {
                assert!(rust_route(&method, path), "MCP já pertence ao Rust: {method} {path}");
            }
        }
        assert!(!rust_route(&Method::POST, "/mcp/unknown"));
        for (method, path) in [
            (Method::GET, "/"), (Method::HEAD, "/assets/app.js"),
            (Method::POST, "/api/dictation/transcribe"), (Method::POST, "/api/ditado/relimpar"),
            (Method::GET, "/api/dictation/models"), (Method::GET, "/api/transcription/providers/status"),
            (Method::POST, "/api/transcription/providers/x/test"),
        ] {
            assert!(rust_route(&method, path), "rota migrada ainda marcada como Python: {method} {path}");
        }
        for (method, path) in [
            (Method::GET, "/api/voice"), (Method::POST, "/api/tts"), (Method::POST, "/api/tts/narrar"),
            (Method::GET, "/api/tts/voices"), (Method::GET, "/api/tts/saldo"), (Method::GET, "/api/tts/audio/x"),
        ] {
            assert!(rust_route(&method, path), "rota de voz fora do Rust: {method} {path}");
        }
    }

    #[test]
    fn integrations_routes_belong_to_rust() {
        for (method, path) in [
            (Method::GET, "/api/integrations/catalog"),
            (Method::GET, "/api/integrations"),
            (Method::POST, "/api/integrations"),
            (Method::PATCH, "/api/integrations/github"),
            (Method::DELETE, "/api/integrations/github"),
            (Method::POST, "/api/integrations/github/oauth/start"),
            (Method::POST, "/api/integrations/github/oauth/open-on-desktop"),
            (Method::GET, "/api/integrations/oauth/callback"),
            (Method::POST, "/api/integrations/github/test"),
            (Method::GET, "/api/integrations/github/tools"),
            (Method::POST, "/api/integrations/approvals/req-1"),
            (Method::DELETE, "/api/integrations/approvals/req-1"),
        ] {
            assert!(rust_route(&method, path), "rota de integrações fora do Rust: {method} {path}");
        }
        assert!(!rust_route(&Method::GET, "/api/integrations/oauth/other"));
        assert!(!rust_route(&Method::GET, "/api/integrations/approvals/req-1"));
        assert!(!rust_route(&Method::POST, "/api/integrations/catalog"));
    }
}
