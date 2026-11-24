//! Cliente do servidor do app desktop (`POST /cmd`), os mesmos verbos que o `hangar-preview` manda.
//! Endereço e token saem de `~/.hangar/nav/_srv.json`, que o app reescreve a cada subida: nada aqui
//! é guardado entre chamadas.
use std::path::Path;
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use zeroize::Zeroizing;

/// O app espera até 45 s pela resposta do navegador.
pub const VERB_TIMEOUT: Duration = Duration::from_secs(45);
pub const PAUSED: &str = "o usuario pausou o navegador; espere ele clicar em Retomar no painel e tente de novo";
const NOT_OPEN: &str = "o app desktop não está aberto nesta máquina";
const NO_HOST: &str = "nenhum app desktop ligado está com esta sessão aberta (o navegador dela fica no app que a mostrou por último): \
    abra a sessão no app desktop que você está usando e repita";

/// O app não atende: a frase volta como erro da tool.
#[derive(Debug, PartialEq, Eq)]
pub struct Unavailable(pub String);

pub(super) fn nav_dir(home: &Path) -> std::path::PathBuf { home.join(".hangar").join("nav") }

/// O sidecar do navegador da sessão (chave `<servidor>::<sessão>` ou só o nome). O do app nativo não
/// tem alvo CDP; aqui só a chave, a URL e o `ts` importam.
pub fn sidecar(home: &Path, name: &str) -> Option<Value> {
    let entries = std::fs::read_dir(nav_dir(home)).ok()?;
    entries.filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "json") && !p.file_name().is_some_and(|f| f.to_string_lossy().starts_with('_')))
        .filter_map(|p| serde_json::from_slice::<Value>(&std::fs::read(p).ok()?).ok())
        .find(|sc| sc["chave"].as_str().is_some_and(|k| k == name || k.ends_with(&format!("::{name}"))))
}

/// Endereço do navegador antes do `open`; `None` sem navegador.
pub fn url_before(home: &Path, name: &str) -> Option<String> {
    sidecar(home, name).map(|sc| sc["url"].as_str().unwrap_or_default().to_owned())
}

pub async fn verb(http: &reqwest::Client, home: &Path, name: &str, verb: &str, args: &[String], tab: Option<i64>, timeout: Duration)
    -> Result<String, Unavailable> {
    verb_on(http, home, sidecar(home, name).as_ref(), verb, args, tab, timeout, None).await
}

/// A URL da página do navegador da sessão. `erro:` do app (sessão sem navegador, página caída) também é `Unavailable`.
pub async fn page_url(http: &reqwest::Client, home: &Path, name: &str) -> Result<String, Unavailable> {
    let text = verb(http, home, name, "url", &[], None, VERB_TIMEOUT).await?;
    ok_value(&text).ok_or(Unavailable(text))
}

/// O texto depois de `ok:`, sem as aspas quando o app o manda como string JSON.
pub fn ok_value(text: &str) -> Option<String> {
    let value = text.strip_prefix("ok:")?.trim();
    Some(serde_json::from_str::<String>(value).unwrap_or_else(|_| value.to_owned()))
}

/// `fill-secret [ref, campo]` com o valor no campo `valor` do corpo, nunca nos `args`.
pub async fn fill_secret(http: &reqwest::Client, home: &Path, name: &str, args: &[String], value: &str) -> Result<String, Unavailable> {
    verb_on(http, home, sidecar(home, name).as_ref(), "fill-secret", args, None, VERB_TIMEOUT, Some(value)).await
}

/// Porta e token do app; `None` = sem arquivo ou incompleto. Pid morto no arquivo é recusa na hora: outra execução do
/// app pode tê-lo publicado e morrido.
fn server(home: &Path) -> Result<Option<(u64, String)>, Unavailable> {
    // Só a ausência é "sem app aqui": arquivo ilegível mandaria o verbo calado ao app de outra máquina.
    let raw = match std::fs::read(nav_dir(home).join("_srv.json")) {
        Ok(raw) => raw,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(unreadable(&e.to_string())),
    };
    let srv = serde_json::from_slice::<Value>(&raw).map_err(|e| unreadable(&e.to_string()))?;
    if !crate::orchestration::runs::pid_alive(&srv["pid"]) {
        return Err(Unavailable(format!("o servidor do navegador registrado em ~/.hangar/nav/_srv.json (pid {}) não está mais rodando; \
            abra o app desktop do Hangar; se ele já está aberto, retoma o registro em poucos segundos: tente de novo e, se continuar, feche e abra o app",
            srv["pid"])));
    }
    let port = srv["porta"].as_u64().or_else(|| srv["porta"].as_str().and_then(|p| p.parse().ok())).filter(|p| *p > 0);
    Ok(port.zip(srv["token"].as_str().filter(|t| !t.is_empty()).map(str::to_owned)))
}

fn unreadable(why: &str) -> Unavailable {
    Unavailable(format!("não deu para ler ~/.hangar/nav/_srv.json ({why}); feche e reabra o app desktop do Hangar"))
}

/// Há app desktop nesta máquina para o `/cmd` (`true`). Sem ele, os verbos vão pela rota `browser-cmd` ao app de quem
/// usa a sessão em outra máquina. Registro com pid morto e recente é o app daqui reiniciando: a recusa, não outra máquina.
pub fn local_app(home: &Path) -> Result<bool, Unavailable> {
    // Os apps apagam o registro ao fechar: pid morto com o arquivo presente é reinício ou queda, e a recusa é daqui.
    server(home).map(|found| found.is_some())
}

/// Quando (ms) o app desktop desta máquina mostrou a sessão por último (`_focus.json`, gravado por ele).
pub fn local_focus(home: &Path, name: &str) -> Option<u64> {
    let raw = std::fs::read(nav_dir(home).join("_focus.json")).ok()?;
    serde_json::from_slice::<Value>(&raw).ok()?[name].as_u64()
}

/// O corpo do `/cmd` para a rota `browser-cmd`, com o prazo que ela espera o app.
pub fn host_body(verb: &str, args: &[String], tab: Option<i64>, deadline: Duration) -> Value {
    let mut body = json!({"verbo": verb, "args": args, "deadline_ms": deadline.as_millis() as u64});
    if let Some(tab) = tab { body["aba"] = json!(tab); }
    body
}

/// A resposta da `browser-cmd` como a rota interna a devolve: texto do app (`{"detail": texto}`) ou a recusa.
pub fn host_answer(status: u16, body: &Value) -> Result<String, Unavailable> {
    // ponytail: texto que por acaso é JSON volta reserializado; só muda espaçamento, nunca o conteúdo.
    if (200..300).contains(&status) { return Ok(body["detail"].as_str().map_or_else(|| body.to_string(), str::to_owned)); }
    let detail = &body["detail"];
    Err(Unavailable(match (status, detail["code"].as_str()) {
        (409, Some("browser_host_none")) => NO_HOST.into(),
        (403, Some("browser_host_not_allowed")) => "o app desktop recusou usar o navegador dele".into(),
        _ => detail["msg"].as_str().or_else(|| detail.as_str()).map_or_else(|| format!("HTTP {status}"), str::to_owned),
    }))
}

/// `verb` com o sidecar já lido.
#[allow(clippy::too_many_arguments)]
async fn verb_on(http: &reqwest::Client, home: &Path, sc: Option<&Value>, verb: &str, args: &[String], tab: Option<i64>, timeout: Duration,
    secret: Option<&str>) -> Result<String, Unavailable> {
    let (port, token) = server(home)?.ok_or_else(|| Unavailable(NOT_OPEN.into()))?;
    let Some(key) = sc.and_then(|sc| sc["chave"].as_str()) else {
        return Err(Unavailable("esta sessão não tem navegador aberto".into()));
    };
    let mut body = json!({"chave": key, "verbo": verb, "args": args});
    if let Some(tab) = tab { body["aba"] = json!(tab); }
    let mut body = Zeroizing::new(body.to_string());
    if let Some(secret) = secret {
        // Fora do `Value`, que não zera o que guarda.
        body.pop();
        body.push_str(",\"valor\":");
        body.push_str(&Zeroizing::new(serde_json::to_string(secret).unwrap_or_default()));
        body.push('}');
    }
    // shortcut: a cópia que o reqwest leva do corpo não é zerada; trocar se o corpo passar a ficar vivo além do pedido.
    let answer = http.post(format!("http://127.0.0.1:{port}/cmd")).bearer_auth(token)
        .header("content-type", "application/json").body(body.as_bytes().to_vec()).timeout(timeout).send().await
        .map_err(|e| Unavailable(format!("o app desktop não respondeu: {e}")))?;
    // Resposta fora de 2xx também é a frase do app (`erro: …`), como no `hangar-preview`.
    answer.text().await.map_err(|e| Unavailable(format!("o app desktop não respondeu: {e}")))
}

/// O sidecar já é da página pedida: não havia navegador, o endereço mudou, ou a pedida é a de antes.
fn navigated(current: &str, before: Option<&str>, asked: &str) -> bool {
    let Some(before) = before.filter(|b| *b == current) else { return true };
    let asked = asked.trim();
    let asked = asked.strip_prefix("https://").or_else(|| asked.strip_prefix("http://")).unwrap_or(asked).trim_end_matches('/');
    !asked.is_empty() && before.contains(asked)
}

/// Espera o navegador que o `open` pediu: sidecar regravado depois do pedido, já na página pedida, e
/// `url` respondendo. Devolve a URL.
pub async fn wait_ready(http: &reqwest::Client, home: &Path, name: &str, since_ms: u64, before: Option<&str>, asked: &str)
    -> Result<String, Unavailable> {
    const LIMIT: Duration = Duration::from_secs(20);
    // O app aberto retoma um registro órfão a cada 3 s; passado isso, o pid morto não volta sozinho.
    const DEAD_GRACE: Duration = Duration::from_secs(5);
    let started = Instant::now();
    let deadline = started + LIMIT;
    let left = || deadline.saturating_duration_since(Instant::now()).clamp(Duration::from_millis(500), VERB_TIMEOUT);
    let (mut last, mut pause_checked) = (String::new(), false);
    while Instant::now() < deadline {
        // Sem arquivo (app subindo) ou lido pela metade, a espera segue.
        if let Err(dead) = server(home) && started.elapsed() >= DEAD_GRACE { return Err(dead); }
        let sc = sidecar(home, name);
        // Pausado pelo usuário o app não navega: dizer na hora em vez de esgotar o prazo.
        if sc.is_some() && !pause_checked {
            pause_checked = true;
            if verb_on(http, home, sc.as_ref(), "estado", &[], None, left(), None).await.is_ok_and(|t| t.trim() == "pausado") {
                return Err(Unavailable(PAUSED.into()));
            }
        }
        if let Some(sc) = &sc && sc["ts"].as_f64().unwrap_or(0.0) as u64 >= since_ms && navigated(sc["url"].as_str().unwrap_or_default(), before, asked) {
            let text = verb_on(http, home, Some(sc), "url", &[], None, left(), None).await.unwrap_or_else(|e| e.0);
            if let Some(value) = text.strip_prefix("ok:") {
                let value = value.trim();
                return Ok(match serde_json::from_str::<Value>(value) {
                    Ok(Value::String(url)) => url,
                    Ok(other) => other.to_string(),
                    Err(_) => value.to_owned(),
                });
            }
            last = text;
        }
        tokio::time::sleep(Duration::from_millis(150)).await;
    }
    let detail = if last.is_empty() { String::new() } else { format!(" ({last})") };
    Err(Unavailable(format!("o navegador não ficou pronto em 20s{detail}; o app desktop está aberto nesta máquina?")))
}

#[cfg(test)]
mod tests {
    use super::{Unavailable, host_answer, local_app, navigated};
    use serde_json::json;

    #[test]
    fn only_a_missing_registration_sends_the_verb_to_another_machine() {
        let home = tempfile::tempdir().unwrap();
        assert_eq!(local_app(home.path()), Ok(false), "sem arquivo: o verbo vai ao app de outra máquina");
        let nav = home.path().join(".hangar/nav");
        std::fs::create_dir_all(&nav).unwrap();
        std::fs::write(nav.join("_srv.json"), b"{not json").unwrap();
        assert!(local_app(home.path()).unwrap_err().0.contains("não deu para ler"), "corrompido: recusa daqui");
    }

    // Pid que sabidamente morreu só se consegue no Linux; no Windows a conferência não decide por um número inventado.
    #[cfg(target_os = "linux")]
    #[test]
    fn a_registration_with_a_dead_pid_is_the_local_app_restarting() {
        let home = tempfile::tempdir().unwrap();
        let nav = home.path().join(".hangar/nav");
        std::fs::create_dir_all(&nav).unwrap();
        let mut child = std::process::Command::new("true").spawn().unwrap();
        let dead = child.id();
        child.wait().unwrap();
        std::fs::write(nav.join("_srv.json"), json!({"porta": 1, "token": "t", "pid": dead}).to_string()).unwrap();
        assert!(local_app(home.path()).unwrap_err().0.contains("não está mais rodando"), "pid morto: o app daqui reiniciando");
    }

    #[test]
    fn host_answer_reads_the_app_text_and_says_why_it_did_not_run() {
        assert_eq!(host_answer(200, &json!({"detail": "ok: click @e1"})), Ok("ok: click @e1".into()));
        assert_eq!(host_answer(200, &json!(42)), Ok("42".into()));
        let none = json!({"detail": {"code": "browser_host_none", "msg": "x"}});
        assert!(host_answer(409, &none).unwrap_err().0.starts_with("o app desktop não está aberto nesta máquina e nenhum app de outra"));
        let denied = json!({"detail": {"code": "browser_host_not_allowed", "msg": "x"}});
        assert_eq!(host_answer(403, &denied), Err(Unavailable("o app desktop recusou usar o navegador dele".into())));
        let gone = json!({"detail": {"code": "browser_host_gone", "msg": "o app caiu; o shot pode ter sido executado"}});
        assert_eq!(host_answer(502, &gone).unwrap_err().0, "o app caiu; o shot pode ter sido executado");
    }

    #[test]
    fn navigated_follows_the_python_rule() {
        assert!(navigated("http://a/", None, "http://b"), "sem navegador antes, qualquer sidecar novo vale");
        assert!(navigated("http://b/", Some("http://a/"), "http://b"));
        assert!(navigated("http://a/x", Some("http://a/x"), "a/x/"), "a pedida é a mesma de antes");
        assert!(!navigated("http://a/", Some("http://a/"), "http://b"));
        assert!(!navigated("http://a/", Some("http://a/"), "  "));
    }
}
