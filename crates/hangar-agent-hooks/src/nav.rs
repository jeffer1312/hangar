//! `nav-hook` (UserPromptSubmit do Claude e do Codex): diz ao agente que esta sessão tem, ou pode ter,
//! o navegador embutido do Hangar. Sem o app desktop no ar (pid de `~/.hangar/nav/_srv.json` vivo), sem
//! saída. Navegador da sessão aberto: sempre diz a URL. Fechado: a dica só vai em mensagem de tela,
//! decidida pela regex e confirmada pelo Jev. Falha sai 0 sem saída, com uma linha no log do Hangar.
use crate::input;
use crate::{Context, Outcome};
use hangar_core::pycompat::{py, pyjson};
use regex::Regex;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::{Duration, SystemTime};
use unicode_normalization::UnicodeNormalization;

const LOG: &str = "nav_hook-falhas.log";
/// Notificação de subagente chega como prompt, mas ninguém a escreveu pedindo tela.
const NOT_A_REQUEST: [&str; 2] = ["<task-notification", "[SYSTEM NOTIFICATION"];
const JEV_URL: &str = "https://api.typesafe.ai/v1/systemone";
const JEV_TIMEOUT: Duration = Duration::from_secs(2);
const JEV_THRESHOLD: f64 = 0.5;
/// Depois de uma falha o Jev descansa: chave errada ou endereço fora fariam cada prompt pagar o prazo.
pub const JEV_PAUSE: Duration = Duration::from_secs(300);
const JEV_QUESTION: &str = "Will handling this message require opening, looking at, clicking or testing a web page or an app screen in a browser?";

pub fn handle(stdin: &[u8], context: &Context) -> Outcome {
    match hint(stdin, context) {
        Ok(Some(text)) => {
            let line = pyjson::dumps(
                &json!({"hookSpecificOutput": {"hookEventName": "UserPromptSubmit", "additionalContext": text}}),
                false,
            );
            Outcome { code: 0, stdout: format!("{line}\n").into_bytes(), stderr: Vec::new() }
        }
        Ok(None) => Outcome::default(),
        // Falha aberta, mas não calada: sem dica e com a linha no log.
        Err(reason) => {
            crate::log::record(context, LOG, &format!("main falhou ({reason})"));
            Outcome::default()
        }
    }
}

fn nav_dir(context: &Context) -> Option<PathBuf> {
    Some(context.home.as_ref()?.join(".hangar").join("nav"))
}

/// `Ok(None)` = sem dica (caso normal); `Err` = falha que o `main` do Python registrava.
fn hint(stdin: &[u8], context: &Context) -> Result<Option<String>, String> {
    let Some(nav) = nav_dir(context) else { return Ok(None) };
    // O arquivo sobrevive a um crash do app: só o pid vivo diz que ele está no ar.
    let Some(server) = std::fs::read(nav.join("_srv.json")).ok()
        .and_then(|bytes| input::parse(std::str::from_utf8(&bytes).ok()?)) else { return Ok(None) };
    let Some(server) = server.as_object() else { return Ok(None) };
    if !pid_alive(server.get("pid").unwrap_or(&Value::Null)) {
        return Ok(None);
    }
    let prompt = prompt(stdin);
    if notification(&prompt) {
        return Ok(None);
    }
    let url = match session_name(context) {
        Some(name) => browser_url(&nav, &name)?,
        None => None,
    };
    // Fechado, a dica só vai em mensagem de tela: em toda mensagem ela custava contexto a quem nunca
    // abre página (o árbitro de uma orquestração).
    if url.is_none() && !needs_browser(&prompt, context) {
        return Ok(None);
    }
    Ok(Some(text(url.as_deref(), context.var("HANGAR_JEV") == Some("on"))))
}

/// `isinstance(pid, int) and pid > 0` e vivo; `true` é o pid 1, como o `bool` do Python.
fn pid_alive(pid: &Value) -> bool {
    let pid = match pid {
        Value::Bool(flag) => i128::from(*flag),
        Value::Number(number) if !number.to_string().contains(['.', 'e', 'E']) => {
            let Some(pid) = number.as_i64() else { return false };
            i128::from(pid)
        }
        _ => return false,
    };
    // Nunca `kill` no Windows: lá ele é TerminateProcess e mataria o app a cada prompt.
    u32::try_from(pid).ok().filter(|pid| *pid > 0)
        .is_some_and(|pid| hangar_core::terminal_process::pid_alive(pid).unwrap_or(false))
}

fn prompt(stdin: &[u8]) -> String {
    input::parse(&input::decode(stdin))
        .and_then(|event| event.as_object()?.get("prompt")?.as_str().map(str::to_owned))
        .unwrap_or_default()
}

fn notification(prompt: &str) -> bool {
    let head: String = py::lstrip(prompt).chars().take(400).collect();
    NOT_A_REQUEST.iter().any(|marker| head.contains(marker))
}

/// Sessão SEM terminal: a chave do sidecar (`CP_SESSION_KEY`) não muda no rename nem no `/clear`.
fn headless_name(context: &Context) -> Option<String> {
    let key = context.var("CP_SESSION_KEY")?;
    let dir = context.home.as_ref()?.join(".hangar").join("claude-headless");
    for entry in std::fs::read_dir(dir).ok()?.flatten() {
        let name = entry.file_name();
        if !name.to_string_lossy().ends_with(".json") {
            continue;
        }
        // Sidecar em escrita ou corrompido não pode esconder o certo.
        let Some(meta) = std::fs::read(entry.path()).ok()
            .and_then(|bytes| input::parse(std::str::from_utf8(&bytes).ok()?)) else { continue };
        let Some(meta) = meta.as_object() else { continue };
        if meta.get("key").and_then(Value::as_str) == Some(key)
            && let Some(name) = meta.get("name").filter(|name| hangar_core::pycompat::truthy(name))
        {
            return Some(py::py_str(name));
        }
    }
    None
}

fn session_name(context: &Context) -> Option<String> {
    if let Some(name) = headless_name(context) {
        return Some(name);
    }
    let pane = context.var("TMUX_PANE")?;
    let listing = tmux_panes(context)?;
    let found: Vec<&str> = listing.lines()
        .filter_map(|line| line.split_once('\t'))
        .filter(|(id, _)| *id == pane)
        .map(|(_, name)| name)
        .collect();
    match found.as_slice() {
        [name] => Some((*name).to_owned()),
        _ => None,
    }
}

/// `tmux list-panes -a -F "#{pane_id}\t#{session_name}"` com prazo de 1 s.
fn tmux_panes(context: &Context) -> Option<String> {
    use std::io::Read;
    use std::process::{Command, Stdio};
    let mut command = Command::new("tmux");
    command.args(["list-panes", "-a", "-F", "#{pane_id}\t#{session_name}"])
        .stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::null());
    if let Some(path) = context.vars.get("PATH") {
        command.env("PATH", path);
    }
    let mut child = command.spawn().ok()?;
    let mut stdout = child.stdout.take()?;
    let reader = std::thread::spawn(move || {
        let mut out = Vec::new();
        let _ = stdout.read_to_end(&mut out);
        out
    });
    let deadline = std::time::Instant::now() + Duration::from_secs(1);
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if std::time::Instant::now() < deadline => std::thread::sleep(Duration::from_millis(5)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    }
    reader.join().ok().map(|out| String::from_utf8_lossy(&out).into_owned())
}

/// Os `*.json` de `~/.hangar/nav` que não começam com `_` ou `.`; vale o de `chave` igual ao nome ou
/// terminada em `::<nome>` (sessão vista por outra máquina).
/// Pasta ilegível é falha do hook (no Python, a exceção chegava ao `main`): sem dica, com log.
fn browser_url(nav: &Path, name: &str) -> Result<Option<String>, String> {
    let suffix = format!("::{name}");
    let entries = std::fs::read_dir(nav).map_err(|error| format!("{:?}: {error}", error.kind()))?;
    for entry in entries.flatten() {
        let file = entry.file_name().to_string_lossy().into_owned();
        if !file.ends_with(".json") || file.starts_with(['_', '.']) {
            continue;
        }
        let Some(data) = std::fs::read(entry.path()).ok()
            .and_then(|bytes| input::parse(std::str::from_utf8(&bytes).ok()?)) else { continue };
        let Some(data) = data.as_object() else { continue };
        let Some(key) = data.get("chave").and_then(Value::as_str) else { continue };
        if key == name || key.ends_with(&suffix) {
            return Ok(Some(match data.get("url") {
                Some(url) if hangar_core::pycompat::truthy(url) => py::py_str(url),
                _ => String::new(),
            }));
        }
    }
    Ok(None)
}

pub fn text(url: Option<&str>, jev: bool) -> String {
    // Com o Jev na sessão, várias ações numa tela desconhecida vão numa chamada só.
    let jev = if jev { " Várias ações numa tela que você não conhece: `browser_goal` (= `hangar-preview objetivo`)." } else { "" };
    match url.filter(|url| !url.is_empty()) {
        Some(url) => format!(
            "[hangar] Esta sessão tem o navegador embutido do Hangar aberto em {url}. Pra ver, ler, clicar, testar ou tirar print dessa página, use as tools `browser`/`browser_batch` do MCP hangar (sem elas: `hangar-preview` no shell; skill hangar-preview), não agent-browser nem ver-front.{jev}"
        ),
        None => format!(
            "[hangar] Pra ver, testar ou tirar print de uma página, use o navegador embutido do Hangar: tools `browser_open`/`browser`/`browser_batch` do MCP hangar (sem elas: `hangar-preview open <url>` no shell; skill hangar-preview). Não use agent-browser nem ver-front.{jev}"
        ),
    }
}

/// A regex decide se a mensagem tem cara de tarefa de tela; palavra comum de código (testa, ui, url,
/// login, interface) fica fora, senão a dica ia em "roda os testes".
fn screen_words() -> &'static Regex {
    static SCREEN: OnceLock<Regex> = OnceLock::new();
    SCREEN.get_or_init(|| {
        Regex::new(concat!(
            r"\b(pagina|paginas|tela|telas|navegador|browser|prints?|screenshot|captura de tela|clica|clicar|",
            r"clique|botao|botoes|layout|css|front|frontend|localhost|site|formulario|modal|preview|",
            r"teste visual)\b|\btestar? (o |a )?(login|cadastro|fluxo)\b|https?://",
        ))
        .unwrap()
    })
}

/// Minúsculas, NFD e sem as marcas de categoria `Mn` (só essas, como o `unicodedata` do Python).
fn without_accents(text: &str) -> String {
    static MN: OnceLock<Regex> = OnceLock::new();
    let decomposed: String = text.to_lowercase().nfd().collect();
    MN.get_or_init(|| Regex::new(r"\p{Mn}").unwrap()).replace_all(&decomposed, "").into_owned()
}

/// Mensagem de tela pela regex; o Jev (~0,5 s de rede) só barra o falso positivo, e resposta curta
/// ("A", "sim") nem chega a ele.
pub fn screen_message(prompt: &str) -> bool {
    !py::strip(prompt).is_empty() && !notification(prompt) && screen_words().is_match(&without_accents(prompt))
}

fn needs_browser(prompt: &str, context: &Context) -> bool {
    if !screen_message(prompt) {
        return false;
    }
    let words = prompt.split(py::is_space).filter(|word| !word.is_empty()).count();
    let jev = if words >= 4 { jev_says(prompt, context) } else { None };
    jev != Some(false)
}

fn pause_file(context: &Context) -> Option<PathBuf> {
    Some(nav_dir(context)?.join("_jev_pausa"))
}

/// `Some` pelo Jev; `None` sem chave, em pausa ou quando ele não responde (vale a regex).
fn jev_says(prompt: &str, context: &Context) -> Option<bool> {
    let key = context.var("TYPESAFE_API_KEY")?;
    let pause = pause_file(context)?;
    if let Ok(modified) = std::fs::metadata(&pause).and_then(|meta| meta.modified()) {
        // Pausa no futuro (relógio voltou) também vale.
        if SystemTime::now().duration_since(modified).map_or(true, |age| age < JEV_PAUSE) {
            return None;
        }
    }
    let tail: String = {
        let chars: Vec<char> = prompt.chars().collect();
        chars[chars.len().saturating_sub(4000)..].iter().collect()
    };
    // Endereço e modelo chegam resolvidos pelo servidor no ambiente da sessão.
    let endpoint = context.var("JEV_ENDPOINT").unwrap_or(JEV_URL);
    let body = pyjson::dumps(&json!({
        "model": context.var("JEV_MODEL").unwrap_or("jev-latest"),
        "state": tail,
        "questions": {"precisa": {"type": "noul", "instructions": JEV_QUESTION}},
    }), false);
    match ask_jev(endpoint, key, body.into_bytes()) {
        Ok(score) => Some(score >= JEV_THRESHOLD),
        Err(reason) => {
            crate::log::record(context, LOG, &format!("jev falhou ({reason}); regex por {}s", JEV_PAUSE.as_secs()));
            let written = std::fs::create_dir_all(pause.parent()?)
                .and_then(|()| std::fs::File::create(&pause))
                .and_then(|file| file.set_modified(SystemTime::now()));
            if let Err(error) = written {
                crate::log::record(context, LOG, &format!("pausa do jev nao gravada ({error}); cada prompt vai tentar de novo"));
            }
            None
        }
    }
}

/// O pedido ao Jev, com o código da falha (`<tipo>: <detalhe>`) para o log.
fn ask_jev(endpoint: &str, key: &str, body: Vec<u8>) -> Result<f64, String> {
    let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build()
        .map_err(|error| format!("RuntimeError: {error}"))?;
    runtime.block_on(async {
        // Prazo por operação (conectar, cada leitura), como o `timeout` do urllib; não há teto total.
        let client = hangar_core::net::http_client().connect_timeout(JEV_TIMEOUT).read_timeout(JEV_TIMEOUT).build()
            .map_err(|error| format!("URLError: {error}"))?;
        let response = client.post(endpoint)
            .header("authorization", format!("Bearer {key}"))
            .header("content-type", "application/json")
            .body(body)
            .send().await
            .map_err(|error| if error.is_timeout() { "TimeoutError: timed out".to_owned() } else { format!("URLError: {error}") })?;
        let status = response.status();
        if !status.is_success() {
            return Err(format!("HTTPError: HTTP Error {}", status.as_u16()));
        }
        let bytes = response.bytes().await
            .map_err(|error| if error.is_timeout() { "TimeoutError: timed out".to_owned() } else { format!("URLError: {error}") })?;
        let answer = std::str::from_utf8(&bytes).ok().and_then(input::parse)
            .ok_or_else(|| "JSONDecodeError: resposta não é JSON".to_owned())?;
        let score = &answer["answers"]["precisa"]["noul"];
        match score {
            Value::Number(number) => number.as_f64().ok_or_else(|| "ValueError: noul".to_owned()),
            Value::Bool(flag) => Ok(f64::from(u8::from(*flag))),
            Value::String(text) => py::strip(text).parse::<f64>().map_err(|_| "ValueError: noul".to_owned()),
            Value::Null if answer.pointer("/answers/precisa/noul").is_none() => Err("KeyError: noul".to_owned()),
            _ => Err("TypeError: noul".to_owned()),
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn common_code_words_are_not_screen_messages() {
        for prompt in ["roda os testes", "testa a interface do adapter", "muda a url do endpoint", "corrige o login do backend"] {
            assert!(!screen_message(prompt), "{prompt}");
        }
        for prompt in ["abre a página de configurações", "o botão não aparece", "testa o login", "veja http://localhost:3000"] {
            assert!(screen_message(prompt), "{prompt}");
        }
    }

    /// Só `Mn` sai; marca espaçada (`Mc`) e envolvente (`Me`) ficam, para casar a mesma regex de antes.
    #[test]
    fn only_nonspacing_marks_are_stripped() {
        assert_eq!(without_accents("Página"), "pagina");
        assert_eq!(without_accents("\u{0915}\u{093E}"), "\u{0915}\u{093E}", "Mc fica");
        assert_eq!(without_accents("a\u{20DD}"), "a\u{20DD}", "Me fica");
    }

    #[test]
    fn subagent_notifications_never_count() {
        assert!(!screen_message("<task-notification>\n abre a tela do navegador"));
        assert!(!screen_message("   [SYSTEM NOTIFICATION] página localhost"));
    }
}
