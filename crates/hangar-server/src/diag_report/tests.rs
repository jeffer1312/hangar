use super::collect::{self, AccountFact, Diary, DiaryEntry, Sessions, Versions};
use super::*;
use chrono::{Local, TimeDelta, Utc};
use serde_json::{Value, json};
use std::io::Read;
use std::path::Path;

fn scrubber() -> Scrubber { Scrubber::new(Some("/home/fulano".into()), Some(r"C:\Users\Fulano\AppData\Local".into())) }

fn write(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

fn day(at: chrono::DateTime<Local>) -> String { at.format("%Y-%m-%d").to_string() }

// ---------- scrub ----------

#[test]
fn scrub_masks_every_rule_of_the_privacy_section() {
    let s = scrubber();
    let cases = [
        ("abriu /home/fulano/Projetos/x.rs", "abriu ~/Projetos/x.rs"),
        ("outra casa /home/beltrano/.claude", "outra casa ~/.claude"),
        (r"C:\Users\Fulano\.claude\x", r"~\.claude\x"),
        (r#"{"cwd":"C:\\Users\\Ciclana\\Projetos"}"#, r#"{"cwd":"~\\Projetos"}"#),
        (r"log em C:\Users\Fulano\AppData\Local\hangar", r"log em %LOCALAPPDATA%\hangar"),
        (r"log em D:\Users\Outra\AppData\Local\hangar", r"log em %LOCALAPPDATA%\hangar"),
        ("tmp /tmp/claude-1000/abc", "tmp <tmp-claude>/abc"),
        (concat!("chave sk-", "ant-api03-AbCdEf123456_xyz fim"), "chave sk-*** fim"),
        ("Authorization: Bearer abc.def-123", "Authorization: Bearer ***"),
        ("GET /x?a=1&token=s3cr3t&b=2", "GET /x?a=1&token=***&b=2"),
        ("ws://h/term?cp_token=zzz", "ws://h/term?cp_token=***"),
        (concat!("gh gh", "p_ABCDEFGHIJKLMNOP1234 ok"), "gh ghp_*** ok"),
        ("slack xoxb-123-456-abc ok", "slack xoxb-*** ok"),
        ("sha 0123456789abcdef0123456789abcdef01234567 ok", "sha *** ok"),
        ("b64 dGhpcyBpcyBhIHNlY3JldCB0b2tlbiB2YWx1ZSAxMjM0 ok", "b64 *** ok"),
        ("mail fulano.silva@exemplo.com.br ok", "mail f***@exemplo.com.br ok"),
        ("par nome.tail1a2b3c.ts.net:8443 ok", "par ***.ts.net:8443 ok"),
        ("ip 192.168.77.142 ok", "ip *.*.*.* ok"),
        ("clone https://fulano:s3nha@gitlab.example/x.git", "clone https://***@gitlab.example/x.git"),
        (r#"{"url":"postgres://app:pw@db.local:5432/x","to":"a@b.com"}"#, r#"{"url":"postgres://***@db.local:5432/x","to":"a***@b.com"}"#),
    ];
    for (raw, want) in cases { assert_eq!(s.scrub(raw), want, "entrada: {raw}"); }
}

#[test]
fn scrub_masks_key_values_headers_and_known_prefixes() {
    let s = scrubber();
    let cases = [
        ("GET /cb?access_token=abc123&refresh_token=def456&x=1", "GET /cb?access_token=***&refresh_token=***&x=1"),
        ("my_token=xyz api_key=k1 client_secret=s2 password=p3", "my_token=*** api_key=*** client_secret=*** password=***"),
        ("GET /oauth?code=4abc&state=1", "GET /oauth?code=***&state=1"),
        ("Authorization: Basic dXNlcjpwYXNz", "Authorization: Basic ***"),
        (r#"{"password": "hunter2", "Token":"x1"}"#, r#"{"password":"***", "Token":"***"}"#),
        (concat!("gitlab glp", "at-ABCDEFGHIJKLMNOPQRST ok"), "gitlab glpat-*** ok"),
        ("pat github_pat_11ABCDEF ok", "pat github_pat_*** ok"),
        (concat!("aws AK", "IAIOSFODNN7EXAMPLE ok"), "aws AKIA*** ok"),
        ("ts tskey-auth-k1 ok", "ts tskey-*** ok"),
        ("hf hf_abc ok", "hf hf_*** ok"),
        ("jwt eyJhbGciOi.eyJzdWIi.c2ln ok", "jwt eyJ*** ok"),
        ("sk curto sk-abc ok", "sk curto sk-*** ok"),
        ("b64 AbCd-1234_EfGh-5678_IjKl-9012_MnOp ok", "b64 *** ok"),
        (r"casa C:\Users\Maria Souza\AppData\Local\hangar\x", r"casa %LOCALAPPDATA%\hangar\x"),
        ("casa C:/Users/Maria Souza/Documents/x", "casa ~/Documents/x"),
        ("v6 fd7a:115c:a1e0::1 ok", "v6 *:* ok"),
        ("v6 [fd7a:115c:a1e0:ab12:4843:cd96:6258:b240]:8765", "v6 [*:*]:8765"),
    ];
    for (raw, want) in cases { assert_eq!(s.scrub(raw), want, "entrada: {raw}"); }
}

#[test]
fn scrub_keeps_versions_session_names_and_codes() {
    let s = scrubber();
    for text in ["hangar-server 0.1.0.8380", "projeto-web-pm19024-review-final", "projeto-web-pm19024-review-final-2026",
        "session_close_requested_by_owner", "rust.worktrees_inactivity_skipped", "17:31:16 WARN x code=permission_observe_failed",
        "POST /api/sessions/*/queue/*/steer 503 runtime_unhealthy"] {
        assert_eq!(s.scrub(text), text);
    }
}

#[test]
fn log_lines_are_read_up_to_four_kib() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("x.log");
    write(&path, &format!("{}\ncurta\r\nfim", "a".repeat(10_000)));
    let mut seen = Vec::new();
    collect::each_line(&path, |line| seen.push(line.to_owned()));
    assert_eq!(seen.iter().map(String::len).collect::<Vec<_>>(), [collect::LINE_MAX, 5, 3]);
}

#[test]
fn scrub_is_idempotent() {
    let s = scrubber();
    let raw = concat!("a /home/fulano/x Bearer abc sk-", "ant-api03-AbCdEf123456 x@y.com 10.0.0.1 h.ts.net https://u:p@h/x");
    let once = s.scrub(raw);
    assert_eq!(s.scrub(&once), once);
}

// ---------- hangar-server.log ----------

#[test]
fn server_line_keeps_only_listed_fields() {
    let s = scrubber();
    let since = Utc::now() - TimeDelta::hours(24);
    let ts = (Utc::now() - TimeDelta::minutes(5)).format("%Y-%m-%dT%H:%M:%S%.6fZ").to_string();
    let line = format!(r#"{ts}  WARN estado: permission.observe falhou session="sessao-a" code="permission_observe_failed" detail="permission_capture_stale""#);
    let out = collect::reduce_server_line(&line, since, &s).unwrap();
    assert!(out.ends_with(" WARN estado: permission.observe falhou code=permission_observe_failed session=sessao-a"), "{out}");
    assert!(!out.contains("detail") && !out.contains("permission_capture_stale"));

    let line = format!("{ts}  WARN runtime entrou em erro key=9d232527a8bd8aedc110caa59a939238 session=cx-b code=cano_closed reason=runtime indisponível; operação conservada no diário");
    let out = collect::reduce_server_line(&line, since, &s).unwrap();
    assert!(out.ends_with(" WARN runtime entrou em erro code=cano_closed session=cx-b reason=runtime indisponível; operação conservada no diário"), "{out}");
    assert!(!out.contains("9d232527"));

    let line = format!(r#"{ts} ERROR Python parado há ~1 min: o vigia pede para religar o serviço code="python_restart" unit=hangar-backend.service"#);
    let out = collect::reduce_server_line(&line, since, &s).unwrap();
    assert!(out.ends_with(" ERROR Python parado há ~1 min: o vigia pede para religar o serviço code=python_restart"), "{out}");

    let line = format!(r#"{ts}  WARN hangar_server::list::mux: lista falhou entries=["x"] attempt=2 code="mux_refused""#);
    let out = collect::reduce_server_line(&line, since, &s).unwrap();
    assert!(out.ends_with(" WARN lista falhou code=mux_refused attempt=2"), "{out}");
}

#[test]
fn server_line_outside_window_or_info_is_dropped() {
    let since = Utc::now() - TimeDelta::hours(24);
    let old = (Utc::now() - TimeDelta::hours(25)).format("%Y-%m-%dT%H:%M:%S%.6fZ");
    let new = (Utc::now() - TimeDelta::minutes(1)).format("%Y-%m-%dT%H:%M:%S%.6fZ");
    let s = scrubber();
    assert!(collect::reduce_server_line(&format!("{old}  WARN velho code=x"), since, &s).is_none());
    assert!(collect::reduce_server_line(&format!("{new}  INFO hangar-server de pé listen=0.0.0.0:8765"), since, &s).is_none());
    assert!(collect::reduce_server_line("linha solta de pânico", since, &s).is_none());
}

#[test]
fn server_line_scrubs_before_cutting_and_shortens_paths() {
    let s = scrubber();
    let since = Utc::now() - TimeDelta::hours(24);
    let ts = (Utc::now() - TimeDelta::minutes(5)).format("%Y-%m-%dT%H:%M:%S%.6fZ").to_string();
    // O segredo atravessa o limite de 120 do valor: cortado antes, sobrariam 19 caracteres que nenhuma regra pega.
    let hex = "0123456789abcdef0123456789abcdef01234567";
    let line = format!("{ts}  WARN x reason={} {hex}", "r".repeat(100));
    let out = collect::reduce_server_line(&line, since, &s).unwrap();
    assert!(!out.contains("0123456789abcdef0"), "{out}");
    let line = format!(r"{ts}  WARN falhou ao abrir /var/lib/hangar/x.json, /home/fulano/a/b.txt e C:\dados\y.txt via fd7a:115c:a1e0::1 code=io");
    let out = collect::reduce_server_line(&line, since, &s).unwrap();
    assert!(out.ends_with(r" WARN falhou ao abrir …/x.json, …/b.txt e …/y.txt via *:* code=io"), "{out}");
}

#[test]
fn server_log_reads_rotated_files_oldest_first() {
    let dir = tempfile::tempdir().unwrap();
    let ts = |m: i64| (Utc::now() - TimeDelta::minutes(m)).format("%Y-%m-%dT%H:%M:%S%.6fZ").to_string();
    write(&dir.path().join("privado/hangar-server.log.1"), &format!("{}  WARN primeiro code=a\n", ts(30)));
    write(&dir.path().join("privado/hangar-server.log"), &format!("{}  WARN segundo code=b\n", ts(10)));
    let lines = collect::read_server_log(dir.path(), Local::now(), &scrubber());
    assert_eq!(lines.len(), 2);
    assert!(lines[0].contains("primeiro") && lines[1].contains("segundo"));
}

// ---------- diário, Python, atualizações ----------

#[test]
fn diary_keeps_problems_and_restarts_of_the_last_day_only() {
    let dir = tempfile::tempdir().unwrap();
    let now = Local::now();
    let ts = |h: i64| (now - TimeDelta::hours(h)).to_rfc3339();
    let rows = [
        json!({"ts": ts(2), "evento": "api.servidor", "nivel": "ok", "codigo": "200"}),
        json!({"ts": ts(1), "evento": "lista.falhou", "nivel": "erro", "codigo": "x", "sessao": "s1", "motivo": "m", "detalhe": "segredo"}),
        json!({"ts": ts(3), "evento": "backend.inicio", "nivel": "ok", "backend": "0.1.0.8380"}),
        json!({"ts": ts(30), "evento": "antigo", "nivel": "erro"}),
    ];
    let text: String = rows.iter().map(|r| format!("{r}\n")).collect();
    write(&dir.path().join(format!("diario/uso-{}.jsonl", day(now))), &text);
    write(&dir.path().join(format!("diario/servidor-rust/uso-{}.jsonl", day(now))),
        &format!("{}\n", json!({"ts": ts(5), "evento": "rust.x", "nivel": "erro", "codigo": "c"})));
    write(&dir.path().join(format!("privado/uso-{}.jsonl", day(now))), &format!("{}\n", json!({"ts": ts(1), "evento": "privado", "nivel": "erro"})));
    let Diary { problems, restarts } = collect::read_diary(&collect::diary_files(dir.path()), now, &scrubber());
    let events: Vec<_> = problems.iter().map(|e| e.event.as_str()).collect();
    assert_eq!(events, ["rust.x", "lista.falhou"]);
    assert!(problems[1].compact().ends_with(" lista.falhou erro x s1 m"));
    assert!(!problems[1].compact().contains("segredo"));
    assert_eq!(restarts.len(), 1);
    assert!(restarts[0].restart_line().contains("backend.inicio ok - 0.1.0.8380"));
}

#[test]
fn python_log_is_only_counted() {
    let dir = tempfile::tempdir().unwrap();
    let now = Local::now();
    let at = |h: i64| (now - TimeDelta::hours(h)).format("%Y-%m-%d %H:%M:%S,123").to_string();
    write(&dir.path().join("privado/backend.log.1"), &format!("{} WARNING [app.connect] connect: segredo\n", at(2)));
    write(&dir.path().join("privado/backend.log"), &format!("{} ERROR [hangar.sse] x\n{} WARNING [hangar.sse] y\n{} INFO [hangar.sse] z\n{} ERROR [velho] w\n",
        at(1), at(1), at(1), at(30)));
    let counts = collect::python_counts(dir.path(), now, &scrubber());
    assert_eq!(counts.get("app.connect"), Some(&[1, 0]));
    assert_eq!(counts.get("hangar.sse"), Some(&[1, 1]));
    assert!(!counts.contains_key("velho"));
}

#[test]
fn updates_keep_the_last_ten_ends() {
    let dir = tempfile::tempdir().unwrap();
    let text: String = (0..12).map(|i| format!("2026-10-27T1{}:00:00.000000Z  INFO update: fim outcome=Updated {{ version: \"0.1.0.83{i:02}\" }}\n2026-10-27T10:00:00Z  INFO update: executor de pé\n", i % 10)).collect();
    write(&dir.path().join("privado/atualizacao-rust.log"), &text);
    let updates = collect::read_updates(dir.path(), &scrubber());
    assert_eq!(updates.len(), 10);
    assert!(updates[0].contains("0.1.0.8302") && updates[9].contains("0.1.0.8311"), "{updates:?}");
}

// ---------- diário inteiro ----------

fn gunzip(bytes: &[u8]) -> String {
    let mut out = String::new();
    flate2::read::MultiGzDecoder::new(bytes).read_to_string(&mut out).unwrap();
    out
}

#[test]
fn full_diary_is_scrubbed_and_in_date_order() {
    let dir = tempfile::tempdir().unwrap();
    write(&dir.path().join("diario/uso-2026-10-22.jsonl"), "{\"d\":2,\"cwd\":\"/home/fulano/x\"}\n");
    write(&dir.path().join("diario/servidor-rust/uso-2026-10-21.jsonl"), "{\"d\":1,\"ip\":\"10.1.2.3\"}\n\n");
    write(&dir.path().join("diario/servidor-rust/uso-2026-10-22.jsonl"), "{\"d\":3}\n");
    write(&dir.path().join("privado/uso-2026-10-22.jsonl"), "{\"privado\":1}\n");
    let s = scrubber();
    let text = gunzip(&collect::diary_gz(&collect::diary_files(dir.path()), &s, collect::DIARY_GZ_MAX));
    // Mesmo dia: os arquivos pela ordem do caminho (`servidor-rust/` antes de `uso-…`).
    assert_eq!(text, "{\"d\":1,\"ip\":\"*.*.*.*\"}\n{\"d\":3}\n{\"d\":2,\"cwd\":\"~/x\"}\n");
}

#[test]
fn full_diary_over_the_cap_drops_the_oldest_days() {
    let dir = tempfile::tempdir().unwrap();
    // Texto pouco compressível: cada dia vira um membro de vários KiB.
    let noise = |seed: u64| (0..400).map(|i| format!("{{\"n\":{}}}\n", (seed * 7919 + i * 104_729) % 1_000_003)).collect::<String>();
    for d in 1..=4u64 { write(&dir.path().join(format!("diario/uso-2026-10-2{d}.jsonl")), &noise(d)); }
    let s = scrubber();
    let paths = collect::diary_files(dir.path());
    let whole = collect::diary_gz(&paths, &s, collect::DIARY_GZ_MAX).len();
    let text = gunzip(&collect::diary_gz(&paths, &s, whole * 3 / 5));
    let first = text.lines().next().unwrap();
    let note: Value = serde_json::from_str(first).unwrap();
    assert_eq!(note["evento"], "diag.cortado");
    let dropped = note["dias_fora"].as_u64().unwrap();
    assert!((1..4).contains(&dropped));
    assert!(text.ends_with(&noise(4)), "o dia mais novo fica");
}

// ---------- montagem ----------

fn versions() -> Versions {
    Versions { server: "0.1.0".into(), package: "0.1.0.8380".into(), commit: "fe204bec6".into(), os: "Linux".into(),
        arch: "linux-x86_64".into(), claude: "2.1.0".into(), mux: "tmux 3.5".into(), offset: "-03:00".into() }
}

fn entry(at: chrono::DateTime<Local>, n: usize) -> DiaryEntry {
    DiaryEntry { at, event: format!("evento.n{n}"), level: "erro".into(), code: "codigo".into(), session: "s".into(), reason: String::new(), backend: String::new() }
}

fn facts(problems: Vec<DiaryEntry>, sessions: Vec<collect::SessionFact>, accounts: Vec<AccountFact>) -> Facts {
    Facts { now: Local::now(), versions: versions(), sessions: Sessions { list_error: None, facts: sessions }, accounts: Ok(accounts),
        diary: Diary { problems, restarts: Vec::new() }, server_log: vec!["10:00:00 WARN x code=y".into()],
        python: [("hangar.sse".to_owned(), [2, 1])].into(), updates: Vec::new() }
}

fn input() -> ClientInput {
    ClientInput { text: Some("travou a fila".into()), client: "native 0.1.0.8380 linux-x86_64".into(),
        failures: vec![ClientFailure { ts: 1_793_130_757.2, route: "POST /api/sessions/*/queue/*/steer".into(), status: 503, code: Some("runtime_unhealthy".into()) }], group: None }
}

fn report(body: &[u8]) -> (Value, String) {
    let value: Value = serde_json::from_slice(body).unwrap();
    let text = value["report"].as_str().unwrap().to_owned();
    (value, text)
}

#[test]
fn report_has_the_summary_first_and_the_sections_in_order() {
    let body = assemble(&facts(vec![entry(Local::now(), 1)], Vec::new(), Vec::new()), &input(), &scrubber(), BODY_MAX);
    let (value, text) = report(&body);
    assert_eq!((value["v"].as_u64(), value["kind"].as_str(), value["outcome"].as_str()), (Some(1), Some("diagnostico"), Some("aberto")));
    assert_eq!(value["app"], "hangar-server 0.1.0.8380");
    assert_eq!(value["os"], "linux-x86_64");
    assert!(value["code"].is_null() && value["agent"].is_null());
    let said = text.find("travou a fila").unwrap();
    let mut last = said;
    for section in ["## Cliente", "## Sessões", "## Contas", "## Versões", "## Reinícios e atualizações", "## hangar-server.log",
        "## Log do Python (contagem)", "## Diário (24 h)"] {
        let at = text.find(section).unwrap_or_else(|| panic!("falta {section}"));
        assert!(at > last, "{section} fora de ordem");
        last = at;
    }
    assert!(text[..text.find("## Cliente").unwrap()].contains("runtime_unhealthy"), "falha do cliente no resumo");
    assert!(text.contains("hangar.sse: 2 avisos, 1 erros"));
}

#[test]
fn oversized_diary_loses_only_its_oldest_lines() {
    let now = Local::now();
    let lines: Vec<_> = (0..2000).map(|n| entry(now - TimeDelta::seconds(4000 - n as i64), n)).collect();
    let body = assemble(&facts(lines, Vec::new(), Vec::new()), &input(), &scrubber(), 40 * 1024);
    assert!(body.len() <= 40 * 1024);
    let (_, text) = report(&body);
    let note = regex::Regex::new(r"\(diário cortado: (\d+) linhas mais antigas\)").unwrap();
    let dropped: usize = note.captures(&text).expect("nota do corte")[1].parse().unwrap();
    assert!(dropped > 0 && dropped < 2000);
    assert!(!text.contains(&format!("evento.n{} ", dropped - 1)), "a última cortada não aparece");
    assert!(text.contains(&format!("evento.n{} ", dropped)), "a primeira mantida aparece");
    assert!(text.contains("evento.n1999 "), "a mais nova fica");
    assert!(text.contains("## Log do Python"), "as outras seções ficam inteiras");
}

#[test]
fn huge_diary_still_fits_the_body_cap() {
    let now = Local::now();
    let lines: Vec<_> = (0..20_000).map(|n| entry(now, n)).collect();
    let body = assemble(&facts(lines, Vec::new(), Vec::new()), &input(), &scrubber(), BODY_MAX);
    assert!(body.len() <= BODY_MAX, "{}", body.len());
    assert!(report(&body).1.contains("evento.n19999 "));
}

#[test]
fn private_fields_never_reach_the_report() {
    let row: hangar_api::session::SessionRow = serde_json::from_value(json!({
        "name": "pm-1", "provider": "claude", "state": "awaiting_input", "problema": "runtime_unhealthy",
        "last_reply": "RESPOSTA_SECRETA", "question": "PERGUNTA_SECRETA", "options": ["OPCAO_SECRETA"],
        "label": "ROTULO_SECRETO", "ask_form": {"q": "FORM_SECRETO"}, "status_line": "STATUS_SECRETO",
        "cwd": "/home/fulano/projeto-secreto",
    })).unwrap();
    let s = scrubber();
    let mut session = collect::session_fact(&row, &s);
    session.runtime = Some(collect::runtime_fact(&json!({"generation": 3, "error": "queue_io",
        "view": {"input_stalled": "composer_busy", "alive": true, "conversation": "CONVERSA_SECRETA"}}), Some(2), &s));
    if let Some(runtime) = session.runtime.as_mut() {
        runtime.native_record = Some(false);
        runtime.plugin = Some(crate::plugin::PullStatus { open: false, last_seen_s: Some(14_400), reported: false, ended: false });
    }
    let account = collect::account_fact(&json!({
        "id": "claude:/home/fulano/.claude-x", "label": "fulano@exemplo.com", "provedor": "claude", "ativa": true,
        "estado": "lida", "janelas": [{"rotulo": "5h", "pct": 95.0, "reset_ts": 1_793_200_000.0, "por_modelo": false}],
        "credential_signature": [1, 2, 3], "reset_credits": "CREDITO_SECRETO", "token": "TOKEN_SECRETO",
        "refresh_expires_at": 1_793_150_000.0,
    }), &[("/home/fulano/.claude-x".to_owned(), 4_000_000_000.0)].into(), &s);
    let mut input = input();
    input.text = Some(concat!("veja /home/fulano/x com Bearer abc123 e sk-", "ant-api03-AAAABBBBCCCC1234").into());
    let body = assemble(&facts(vec![entry(Local::now(), 1)], vec![session], vec![account]), &input, &scrubber(), BODY_MAX);
    let raw = String::from_utf8(body).unwrap();
    for secret in ["RESPOSTA_SECRETA", "PERGUNTA_SECRETA", "OPCAO_SECRETA", "ROTULO_SECRETO", "FORM_SECRETO", "STATUS_SECRETO",
        "CONVERSA_SECRETA", "projeto-secreto", "CREDITO_SECRETO", "TOKEN_SECRETO", "/home/fulano", "fulano@", "abc123", "AAAABBBB"] {
        assert!(!raw.contains(secret), "{secret} vazou");
    }
    let (_, text) = report(raw.as_bytes());
    assert!(text.contains("pm-1 · claude · com terminal · awaiting_input · problema=runtime_unhealthy"));
    assert!(text.contains("erro=queue_io entrada_parada=composer_busy fila=2"));
    // Sem o registro nativo, o plugin parado há 4 h é o que explica a entrada em erro.
    assert!(text.contains("registro_nativo=não plugin: pull_aberto=false ultimo_pull=14400s conversa_nova=false fim=false"), "{text}");
    assert!(text.contains("f***@exemplo.com · claude · ativa · lida · 5h 95%"));
    assert!(text.contains("gasta até"));
}
