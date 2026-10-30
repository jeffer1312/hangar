//! `hangar-server pair-context` como processo, mais grupo com par de fora e linha do contrato em grupo
//! entre máquinas. O tmux falso é um script de shell, por isso só Unix.
#![cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::process::Command;
use std::time::{Duration, Instant};

use serde_json::{Map, Value, json};

#[path = "groups_context_cases.rs"]
mod cases;
use cases::*;

struct Run { rc: Option<i32>, stdout: String, elapsed: Duration, diary: String }

/// Pasta de grupo, HOME com `claude-headless`, PATH só com o tmux falso.
/// `tmux` = Some((list-panes, código do has-session)) ou um script inteiro em `Err`.
fn run(files: &Value, headless: &Value, env: &Value, tmux: Option<Result<(&str, i64), &str>>) -> Run {
    let root = tempfile::Builder::new().prefix("grp-t3-").tempdir().unwrap();
    let (pair, home, bin) = (root.path().join("pair"), root.path().join("home"), root.path().join("bin"));
    for d in [&pair, &home.join(".hangar/claude-headless"), &bin] { std::fs::create_dir_all(d).unwrap(); }
    for (dir, set) in [(&pair, files), (&home.join(".hangar/claude-headless"), headless)] {
        for (name, text) in set.as_object().unwrap() {
            // Array de bytes: conteúdo que não é UTF-8.
            let bytes: Vec<u8> = match text { Value::Array(b) => b.iter().map(|x| x.as_u64().unwrap() as u8).collect(), t => t.as_str().unwrap().into() };
            std::fs::write(dir.join(name), bytes).unwrap();
        }
    }
    let script = match tmux {
        None => None,
        Some(Ok((panes, rc))) => Some(format!("#!/bin/sh\nif [ \"$1\" = list-panes ]; then printf '%s' '{}'; exit 0; fi\n\
            if [ \"$1\" = has-session ]; then exit {rc}; fi\nexit 1\n", panes.replace('\'', "'\\''"))),
        Some(Err(body)) => Some(body.to_owned()),
    };
    if let Some(script) = script {
        let sh = bin.join("tmux");
        std::fs::write(&sh, script).unwrap();
        std::fs::set_permissions(&sh, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_hangar-server"));
    cmd.arg("pair-context").arg(&pair).env_clear().env("HOME", &home).env("PATH", &bin).env("LANG", "C.UTF-8");
    for (k, v) in env.as_object().unwrap() { cmd.env(k, v.as_str().unwrap()); }
    let started = Instant::now();
    let out = cmd.output().unwrap();
    let stdout = String::from_utf8(out.stdout).unwrap().replace(pair.to_str().unwrap(), "<pair>");
    let diary = std::fs::read_dir(home.join(".hangar/logs/diario/servidor-rust")).map(|d| d.flatten()
        .map(|e| std::fs::read_to_string(e.path()).unwrap()).collect()).unwrap_or_default();
    Run { rc: out.status.code(), stdout, elapsed: started.elapsed(), diary }
}

fn run_case(case: &Case) -> Run {
    let object = |set: &[(&str, &str)]| Value::Object(set.iter().map(|(k, v)| (k.to_string(), json!(v))).collect::<Map<_, _>>());
    run(&object(case.pair_dir), &object(case.headless), &object(case.env), case.tmux.map(Ok))
}

fn case(name: &str) -> &'static Case {
    CASES.iter().find(|c| c.name == name).unwrap()
}

/// `additionalContext` de uma saída do hook.
fn context(stdout: &str) -> String {
    let v: Value = serde_json::from_str(stdout).unwrap();
    v["hookSpecificOutput"]["additionalContext"].as_str().unwrap().to_owned()
}

/// Variantes com par de fora em que o texto de grupo vem antes do bloco externo.
const MIXED: [&str; 2] = ["external_registry_unreadable", "external_via_other_member"];
/// Sidecar sem `gid` (ou vazio): `stdout` da tabela não tem a linha do contrato.
const GID_DERIVED: [&str; 2] = ["gid_empty", "gid_missing"];

#[test]
fn pair_context_output_per_arrangement() {
    assert_eq!(CASES.len(), 32);
    let mut checked = 0;
    for c in CASES {
        if MIXED.contains(&c.name) { continue; }
        let got = run_case(c);
        assert_eq!(got.rc, Some(0), "{}", c.name);
        if GID_DERIVED.contains(&c.name) {
            // Sem `gid`, o contrato é o do gid derivado (a+b), o mesmo da tool `contract`.
            let gid = &sha1_smol::Sha1::from("a\nb").digest().to_string()[..8];
            let line = format!("- Decisões que o grupo consulta vão em <pair>/grupo-{gid}.md (criar se não existir, curto).\n");
            let ours = context(&got.stdout);
            assert!(ours.contains(&line), "{}: {ours}", c.name);
            assert_eq!(ours.replace(&line, ""), context(c.stdout), "{}", c.name);
        } else {
            assert_eq!(got.stdout, c.stdout, "{}", c.name);
        }
        checked += 1;
    }
    assert_eq!(checked, 30);
}

/// Membro local de grupo com par de fora recebe o texto de grupo (o mesmo de um grupo `a`+`b` sem
/// tarefa) seguido do bloco externo. Registro ilegível: só o stdout.
#[test]
fn mixed_group_reinjects_group_and_external_text() {
    let group = context(case("headless_key_unknown_falls_to_pane").stdout);
    for name in MIXED {
        let got = run_case(case(name));
        assert_eq!(got.rc, Some(0), "{name}");
        let external = context(case(name).stdout);
        assert_eq!(context(&got.stdout), format!("{group}\n{external}"), "{name}");
    }
}

fn pair_record(local: &str, alias: &str, session: &str, owner: &str) -> Value {
    json!({"share_id": format!("sh-{alias}"), "local_session": local, "alias": alias, "peer_owner": owner, "peer_session": session,
           "peer_address": format!("https://{alias}.example.ts.net:8443"), "peer_token": "tok", "created_at": 1.0})
}

fn pane_a() -> Value { json!({"TMUX_PANE": "%3"}) }
const PANES: Option<Result<(&str, i64), &str>> = Some(Ok(("%3\ta\n%4\tb\n", 1)));

/// Um bloco externo por par de fora, cada um uma vez.
#[test]
fn two_outside_pairs_get_one_block_each() {
    let files = json!({
        "a.json": json!({"peers": ["b", "ana::rev", "bia::x"], "task": "", "gid": "ab12cd34", "harness": {}}).to_string(),
        "external_pairs.json": json!([pair_record("a", "ana", "rev", "Ana"), pair_record("b", "bia", "x", "Bia")]).to_string(),
    });
    let text = context(&run(&files, &json!({}), &pane_a(), PANES).stdout);
    assert!(text.starts_with("[painel: grupo de trabalho] Você, 'a', está num grupo de trabalho."), "{text}");
    for (peer, owner) in [("ana::rev", "Ana"), ("bia::x", "Bia")] {
        assert_eq!(text.matches(&format!("está pareada com '{peer}', uma sessão da máquina '{owner}'")).count(), 1, "{text}");
        assert_eq!(text.matches(&format!("[de fora: {peer}]")).count(), 1, "{text}");
    }
}

/// Só par de fora entre os peers: nenhum membro daqui para citar no texto de grupo, fica o aviso externo
/// (a forma do `external_direct`). Filha ligada também fica só com ele.
#[test]
fn pair_context_without_local_others_prints_only_the_outside_warning() {
    let external = context(case("external_direct").stdout);
    let files = json!({
        "a.json": json!({"peers": ["pai", "ana::rev"], "task": "", "gid": "ab12cd34", "harness": {}, "linked_by": "pai"}).to_string(),
        "external_pairs.json": json!([pair_record("a", "ana", "rev", "Ana")]).to_string(),
    });
    assert_eq!(context(&run(&files, &json!({}), &pane_a(), PANES).stdout), external);
}

/// Sidecar 1:1 legado (`peer`, sem `peers`) reinjeta o mesmo texto do formato atual, como o servidor o lê.
#[test]
fn legacy_peer_sidecar_reinjects_like_peers() {
    let of = |sidecar: Value| run(&json!({"a.json": sidecar.to_string()}), &json!({}), &pane_a(), PANES).stdout;
    let current = of(json!({"peers": ["b"], "task": ""}));
    assert!(!current.is_empty());
    let legacy = of(json!({"peer": "b", "task": ""}));
    assert_eq!(legacy, current);
    // Sem `gid` no arquivo, o contrato é o do gid que o servidor deriva (`legacy_gid` de a+b).
    let gid = &sha1_smol::Sha1::from("a\nb").digest().to_string()[..8];
    assert!(context(&legacy).contains(&format!("<pair>/grupo-{gid}.md")), "{legacy}");
}

/// Grupo entre máquinas cita o caminho na dona e a dona pela tool `contract` nas outras.
#[test]
fn fed_contract_line_follows_the_owner() {
    for (local, line) in [(true, "- Decisões que o grupo consulta vão em <pair>/grupo-fe12.md (criar se não existir, curto)."),
                          (false, "- Decisões que o grupo consulta ficam com a máquina `casa`: leia e grave pela tool `contract` do MCP ou `hangar-send --contrato`.")] {
        let files = json!({"a.json": json!({"peers": ["casa::b"], "task": "", "gid": "fe12", "harness": {},
            "fed": {"owner": "casa", "local": local, "version": 3}}).to_string()});
        let text = context(&run(&files, &json!({}), &pane_a(), PANES).stdout);
        assert_eq!(text.lines().filter(|l| l.starts_with("- Decisões")).collect::<Vec<_>>(), [line], "local={local}");
    }
}

/// Nenhuma porta, token ou servidor no ambiente: a reinjeção sai só do disco.
#[test]
fn pair_context_works_without_server() {
    let got = run_case(case("group"));
    assert_eq!((got.rc, got.stdout.as_str()), (Some(0), case("group").stdout));
}

/// Pasta inexistente, sidecar torto, sem argumento, sem tmux ou tmux travado: código 0 e nada no stdout,
/// e o tmux travado (list-panes e has-session) não segura a abertura além do prazo único de 3 s.
#[test]
fn pair_context_never_fails_the_open() {
    let ok = json!({"a.json": json!({"peers": ["b"]}).to_string()});
    let stamp = json!({"TMUX_PANE": "%3", "CP_SESSION_NAME": "a"});
    for (label, files, tmux) in [
        ("sidecar torto", json!({"a.json": "{torto"}), PANES),
        ("sem tmux", ok.clone(), None),
        ("tmux travado", ok.clone(), Some(Err("#!/bin/sh\nexec /bin/sleep 30\n"))),
    ] {
        let got = run(&files, &json!({}), &stamp, tmux);
        assert_eq!((got.rc, got.stdout.as_str()), (Some(0), ""), "{label}");
        assert!(got.elapsed < Duration::from_secs(5), "{label}: {:?}", got.elapsed);
    }
    let missing = Command::new(env!("CARGO_BIN_EXE_hangar-server")).arg("pair-context").arg(PathBuf::from("/nonexistent/grp-t3"))
        .env_clear().env("CP_SESSION_NAME", "a").output().unwrap();
    assert_eq!((missing.status.code(), missing.stdout.as_slice()), (Some(0), &b""[..]));
    let bare = Command::new(env!("CARGO_BIN_EXE_hangar-server")).arg("pair-context").env_clear().output().unwrap();
    assert_eq!((bare.status.code(), bare.stdout.as_slice()), (Some(0), &b""[..]));
}

/// Falha que não é "fora de grupo" (sidecar ilegível, `peers` torto) vai ao diário do Hangar, e a
/// abertura continua: código 0 e nada no stdout. Sem sidecar é só sessão fora de grupo: diário vazio.
#[test]
fn pair_context_records_real_failures_in_the_diary() {
    for (label, files, code) in [
        ("não UTF-8", json!({"a.json": [0xff, 0xfe, b'{']}), "pair_context_sidecar_unreadable"),
        ("sidecar torto", json!({"a.json": "{torto"}), "pair_context_sidecar_unreadable"),
        ("peers torto", json!({"a.json": json!({"peers": "b"}).to_string()}), "pair_context_peers_invalid"),
    ] {
        let got = run(&files, &json!({}), &pane_a(), PANES);
        assert_eq!((got.rc, got.stdout.as_str()), (Some(0), ""), "{label}");
        let line: Value = serde_json::from_str(got.diary.lines().next().unwrap_or("null")).unwrap();
        assert_eq!((line["nivel"].as_str(), line["codigo"].as_str()), (Some("erro"), Some(code)), "{label}: {}", got.diary);
    }
    let alone = run(&json!({}), &json!({}), &pane_a(), PANES);
    assert_eq!((alone.rc, alone.stdout.as_str(), alone.diary.as_str()), (Some(0), "", ""));
    // Sem tmux para perguntar quem é a sessão, e `fed` torto (vale como grupo de uma máquina): diário, abertura segue.
    let no_tmux = run(&json!({"a.json": json!({"peers": ["b"]}).to_string()}), &json!({}), &pane_a(), None);
    assert_eq!((no_tmux.rc, no_tmux.stdout.as_str()), (Some(0), ""));
    assert!(no_tmux.diary.contains("pair_context_tmux_failed"), "{}", no_tmux.diary);
    let fed = run(&json!({"a.json": json!({"peers": ["b"], "gid": "g", "fed": "torto"}).to_string()}), &json!({}), &pane_a(), PANES);
    assert_eq!(fed.rc, Some(0));
    assert!(!fed.stdout.is_empty() && fed.diary.contains("pair_context_fed_invalid"), "{}", fed.diary);
}
