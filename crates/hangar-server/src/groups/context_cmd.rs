//! `hangar-server pair-context <pair_dir>`: o hook de SessionStart do Claude Code reinjeta o protocolo
//! do grupo depois de /clear, compactação e retomada (o prompt do `--pair` some; o sidecar fica). Só
//! disco, sem rede nem token: funciona com o servidor parado. Qualquer falha = sem saída e código 0,
//! porque o hook roda na abertura de toda sessão; a que não é "fora de grupo" vai ao diário do Hangar.
use std::path::Path;
use std::process::ExitCode;
use std::time::Duration;

use serde_json::{Map, Value, json};

use super::model::{Fed, Sidecar};
use super::store::file_stem;
use super::text::{self, ContractLine, Kind, OutsidePeer, TextArgs};
use crate::transcript::pyjson;

const TMUX_DEADLINE: Duration = Duration::from_secs(3);

pub fn run(args: &[String]) -> ExitCode {
    std::panic::set_hook(Box::new(|_| {}));
    let out = std::panic::catch_unwind(|| args.get(1).and_then(|dir| context(Path::new(dir))))
        .unwrap_or_else(|panic| {
            let what = panic.downcast_ref::<&str>().map(|s| (*s).to_owned()).or_else(|| panic.downcast_ref::<String>().cloned());
            failed("pair_context_panic", what.unwrap_or_default())
        });
    if let Some(out) = out {
        use std::io::Write;
        let _ = writeln!(std::io::stdout().lock(), "{out}");
    }
    ExitCode::SUCCESS
}

fn context(pair_dir: &Path) -> Option<String> {
    let text = protocol_text(pair_dir, "claude")?;
    // ASCII como o `json.dumps` do Python: o cmd do Windows não troca de página de código no meio.
    Some(pyjson::dumps(&json!({"hookSpecificOutput": {"hookEventName": "SessionStart", "additionalContext": text}}), false))
}

/// O protocolo do grupo desta sessão em texto, para o hook de abertura de um agente; `harness` é o agente que
/// o recebe (diz se ele tem o MCP). `None` fora de grupo ou em falha, já relatada.
pub fn protocol_text(pair_dir: &Path, harness: &str) -> Option<String> {
    let me = session_name()?;
    let path = pair_dir.join(format!("{}.json", file_stem(&me)));
    let raw = match std::fs::read_to_string(&path) {
        Ok(raw) => raw,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return None,
        Err(e) => return failed("pair_context_sidecar_unreadable", format!("{}: {e}", path.display())),
    };
    let Some(Value::Object(sidecar)) = pyjson::loads_lossless(&raw) else {
        return failed("pair_context_sidecar_unreadable", format!("{}: não é um objeto JSON", path.display()));
    };
    protocol(pair_dir, &me, &sidecar, harness)
}

fn protocol(pair_dir: &Path, me: &str, d: &Map<String, Value>, harness: &str) -> Option<String> {
    let task = d.get("task").and_then(Value::as_str).unwrap_or_default().to_owned();
    // O árbitro do orquestrar-auto nasce sozinho no grupo: vale sem peers.
    if d.get("orq") == Some(&Value::Bool(true)) {
        return Some(text::render(Kind::Orq, &TextArgs { task, ..Default::default() }));
    }
    if d.get("peers").is_some_and(|p| !p.is_array()) {
        return failed("pair_context_peers_invalid", "peers não é lista");
    }
    // `Sidecar::parse` lê também o formato 1:1 legado (`peer`), como o servidor.
    let Sidecar { peers, gid, .. } = Sidecar::parse(me, d)?;
    let outside = outside_of(pair_dir, me, &peers);
    let others: Vec<String> = peers.iter().filter(|p| !outside.iter().any(|o| &o.address == *p)).cloned().collect();
    // Filha ligada não recebe o protocolo (cobraria tokens do pai a cada /clear); o aviso de par de
    // fora é regra de segurança e fica, como fica quando não há membro desta máquina.
    let linked = d.get("linked_by").and_then(Value::as_str).is_some_and(|p| peers.iter().any(|x| x == p));
    if linked || others.is_empty() {
        let blocks: Vec<String> = outside.iter().map(|o| text::render(Kind::External, &TextArgs {
            me: me.to_owned(), peer: o.address.clone(), owner: o.owner.clone(), ..Default::default()
        })).collect();
        return (!blocks.is_empty()).then(|| blocks.join("\n"));
    }
    // Sem `gid` no arquivo vale o derivado (`legacy_gid`), o mesmo que a tool `contract` usa.
    let path = Some(pair_dir.join(format!("grupo-{gid}.md")));
    // `fed` torto vale como grupo de uma máquina, como no servidor; o diário guarda o porquê.
    let fed = match d.get("fed") {
        None | Some(Value::Null) => None,
        Some(f) => serde_json::from_value::<Fed>(f.clone()).map_or_else(|e| failed("pair_context_fed_invalid", e.to_string()), Some),
    };
    let across = fed.is_some();
    let contract = match fed {
        Some(fed) if fed.local => ContractLine { path, remote_owner: None },
        Some(fed) => ContractLine { path: None, remote_owner: Some(fed.owner) },
        // Par 1:1 antigo entre máquinas não sincroniza contrato.
        None if others.iter().any(|p| p.contains("::")) => ContractLine::default(),
        None => ContractLine { path, remote_owner: None },
    };
    let harness = [(me.to_owned(), harness.to_owned())].into();
    Some(text::render(Kind::Group, &TextArgs { me: me.to_owned(), others, task, harness, contract, outside, across, ..Default::default() }))
}

/// Pares de fora entre os `peers`, pelo `external_pairs.json`. O par pode ter sido feito por outro
/// membro desta máquina. Registro ilegível: na dúvida, o primeiro `::` é de outra pessoa.
fn outside_of(pair_dir: &Path, me: &str, peers: &[String]) -> Vec<OutsidePeer> {
    let locals: Vec<&str> = std::iter::once(me).chain(peers.iter().map(String::as_str).filter(|p| !p.contains("::"))).collect();
    let registry = match std::fs::read_to_string(pair_dir.join("external_pairs.json")) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Vec::new(),
        read => read.ok().and_then(|raw| pyjson::loads_lossless(&raw)).and_then(|v| registered(&v, &locals)),
    };
    match registry {
        Some(known) => peers.iter().filter_map(|p| known.iter().find(|o| &o.address == p).cloned()).collect(),
        None => {
            let Some(blind) = peers.iter().find(|p| p.contains("::")) else { return Vec::new() };
            eprintln!("[hangar] external_pairs.json ilegível: par '{blind}' tratado como externo");
            vec![OutsidePeer { address: blind.clone(), owner: "desconhecido".to_owned() }]
        }
    }
}

/// Os pares feitos por sessões desta máquina; `None` = registro torto.
fn registered(registry: &Value, locals: &[&str]) -> Option<Vec<OutsidePeer>> {
    let mut found = Vec::new();
    for record in registry.as_array()? {
        let record = record.as_object()?;
        if !record.get("local_session").and_then(Value::as_str).is_some_and(|s| locals.contains(&s)) {
            continue;
        }
        let field = |k: &str| record.get(k).and_then(Value::as_str);
        found.push(OutsidePeer { address: format!("{}::{}", field("alias")?, field("peer_session")?), owner: field("peer_owner")?.to_owned() });
    }
    Some(found)
}

/// Mesma ordem do `me()` do hangar-send: sidecar sem terminal, pane, carimbo do nascimento.
fn session_name() -> Option<String> {
    if let Some(name) = headless_name() {
        return Some(name);
    }
    let env = |k: &str| std::env::var(k).ok().filter(|v| !v.is_empty());
    let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().ok()?;
    // Um prazo só para as duas perguntas ao tmux: a abertura espera no máximo isso.
    runtime.block_on(async { tokio::time::timeout(TMUX_DEADLINE, async {
        if let Some(pane) = env("TMUX_PANE") {
            let listed = tmux(&["list-panes", "-a", "-F", "#{pane_id}\t#{session_name}"]).await.map(|o| o.stdout).unwrap_or_default();
            let listed = String::from_utf8_lossy(&listed);
            // O psmux repete `%N` entre sessões: pane que aparece mais de uma vez não identifica ninguém.
            let found: Vec<&str> = listed.lines().filter_map(|l| l.split_once('\t')).filter(|(id, _)| *id == pane).map(|(_, n)| n).collect();
            if let [name] = found[..] {
                return Some(name.to_owned());
            }
        }
        let name = env("CP_SESSION_NAME")?;
        tmux(&["has-session", "-t", &format!("={name}")]).await.filter(|o| o.status.success()).map(|_| name)
    }).await.unwrap_or_else(|_| failed("pair_context_tmux_timeout", "tmux não respondeu no prazo")) })
}

/// Uma linha no diário do Hangar; `None` para quem chama seguir sem saída.
fn failed<T>(code: &str, detail: impl Into<String>) -> Option<T> {
    let line = json!({"evento": "grupo.contexto_sessao", "nivel": "erro", "codigo": code, "detalhe": detail.into()});
    crate::diag::record_blocking(crate::diag::diary_dir().as_deref(), &[line], "contexto do grupo");
    None
}

async fn tmux(args: &[&str]) -> Option<std::process::Output> {
    let mut command = crate::terminal_input::child_command("tmux");
    command.args(args).stdin(std::process::Stdio::null()).kill_on_drop(true);
    match command.output().await {
        Ok(output) => Some(output),
        Err(e) => failed("pair_context_tmux_failed", format!("tmux {}: {e}", args.first().copied().unwrap_or_default())),
    }
}

/// Sessão sem terminal: a chave do processo não muda no rename nem no /clear.
fn headless_name() -> Option<String> {
    let key = std::env::var("CP_SESSION_KEY").ok().filter(|k| !k.is_empty())?;
    let dir = std::env::home_dir()?.join(".hangar").join("claude-headless");
    std::fs::read_dir(dir).ok()?.flatten().map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "json"))
        // Sidecar alheio em escrita ou corrompido não esconde o certo.
        .filter_map(|p| serde_json::from_str::<Value>(&std::fs::read_to_string(p).ok()?).ok())
        .find_map(|meta| (meta.get("key")?.as_str()? == key).then(|| meta.get("name")?.as_str().filter(|n| !n.is_empty()).map(str::to_owned)).flatten())
}
