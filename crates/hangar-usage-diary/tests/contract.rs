//! Contrato do diário de uso, com o relógio do servidor fixo em 2026-11-16 10:00 -03:00. O esperado
//! de cada caso é o que a rota antiga gravou e devolveu (captura de 2026-11-16, commit 1504c6c53).
use std::path::Path;

use chrono::{DateTime, FixedOffset};
use hangar_usage_diary::{WriteError, download, files, latest, latest_count, record_screen, summary};
use serde_json::{Value, json};

const DAY: &str = "uso-2026-11-16.jsonl";
const STAMP: &str = "2026-11-16T10:00:00.000-03:00";

fn now() -> DateTime<FixedOffset> {
    DateTime::parse_from_rfc3339("2026-11-16T10:00:00-03:00").unwrap()
}

fn post(base: &Path, body: Value) -> Result<usize, WriteError> {
    record_screen(base, body.to_string().as_bytes(), now())
}

fn written(base: &Path) -> String {
    std::fs::read_to_string(base.join(DAY)).unwrap_or_default()
}

fn rows(base: &Path) -> Vec<Value> {
    written(base).lines().map(|line| serde_json::from_str(line).unwrap()).collect()
}

/// D-01: corpo ilegível ou sem lista nunca é erro: `gravadas` 0 e nada no disco.
#[test]
fn d01_unreadable_or_listless_bodies_write_nothing() {
    let dir = tempfile::tempdir().unwrap();
    assert_eq!(record_screen(dir.path(), b"nao-json!", now()), Ok(0));
    assert_eq!(post(dir.path(), json!({"x": 1})), Ok(0));
    assert_eq!(post(dir.path(), json!({"eventos": {"evento": "a"}})), Ok(0));
    assert!(!dir.path().join(DAY).exists());
}

/// D-01, D-03, D-09: `{eventos:[…]}` e a lista crua; campo fora da lista sai; `ts` primeiro,
/// `nivel` na ordem dos campos e `origem` no fim, com os separadores do arquivo antigo.
#[test]
fn d02_d03_screen_lines_keep_the_old_file_format() {
    let dir = tempfile::tempdir().unwrap();
    let body = json!({"eventos": [{"evento": "app.abriu", "nivel": "ok", "cli": "abc", "seq": 1, "so": "Linux", "extra": "sai"}]});
    assert_eq!(post(dir.path(), body), Ok(1));
    assert_eq!(post(dir.path(), json!([{"evento": "a"}, {"evento": "b", "nivel": "erro"}])), Ok(2));
    assert_eq!(written(dir.path()), concat!(
        r#"{"ts": "2026-11-16T10:00:00.000-03:00", "evento": "app.abriu", "nivel": "ok", "cli": "abc", "seq": 1, "so": "Linux", "origem": "tela"}"#, "\n",
        r#"{"ts": "2026-11-16T10:00:00.000-03:00", "evento": "a", "nivel": "ok", "origem": "tela"}"#, "\n",
        r#"{"ts": "2026-11-16T10:00:00.000-03:00", "evento": "b", "nivel": "erro", "origem": "tela"}"#, "\n"));
}

/// D-03: conversa não entra por campo nenhum; a trava é a lista de campos, não varredura de texto.
#[test]
fn d03_conversation_fields_never_reach_the_file() {
    let dir = tempfile::tempdir().unwrap();
    let body = json!([{"evento": "msg.enviar", "prompt": "segredo", "resposta": "segredo", "conteudo": "segredo",
        "arquivo": "/home/eu/projeto/x.py", "api_key": "sk-segredo"}]);
    assert_eq!(post(dir.path(), body), Ok(1));
    let text = written(dir.path());
    assert!(!text.contains("segredo") && !text.contains("/home/eu"), "{text}");
}

/// D-02: o lote é cortado em 50 e a resposta conta as recebidas, inclusive as descartadas.
#[test]
fn d02_batch_is_cut_at_fifty_and_discarded_lines_still_count() {
    let dir = tempfile::tempdir().unwrap();
    let batch: Vec<Value> = (0..60).map(|i| json!({"evento": format!("e{i}"), "seq": i})).collect();
    assert_eq!(post(dir.path(), Value::Array(batch)), Ok(50));
    let rows = rows(dir.path());
    assert_eq!(rows.len(), 50);
    assert_eq!(rows[49]["evento"], "e49");
    let other = tempfile::tempdir().unwrap();
    let invalid = json!([null, 1, "s", [], {}, {"evento": ""}, {"evento": "   "}, {"evento": 5}, {"nivel": "erro"}, {"evento": "ok1"}]);
    assert_eq!(post(other.path(), invalid), Ok(10));
    assert_eq!(written(other.path()), format!("{{\"ts\": \"{STAMP}\", \"evento\": \"ok1\", \"nivel\": \"ok\", \"origem\": \"tela\"}}\n"));
}

/// D-04, D-06: booleano não passa por número nem o contrário; float não é inteiro; vazio sai.
#[test]
fn d04_types_are_checked_field_by_field() {
    let dir = tempfile::tempdir().unwrap();
    let line = json!([{"evento": "t", "ms": true, "seq": 1.5, "limpou": 1, "composer_legivel": true, "tela": 7,
        "codigo": null, "detalhe": "", "sessao": "   ", "errno": -3, "ms_x": 1}]);
    assert_eq!(post(dir.path(), line), Ok(1));
    assert_eq!(written(dir.path()), format!("{{\"ts\": \"{STAMP}\", \"evento\": \"t\", \"errno\": -3, \"composer_legivel\": true, \"nivel\": \"ok\", \"origem\": \"tela\"}}\n"));
}

/// D-05: texto aparado e cortado em 300 pontos de código (não bytes).
#[test]
fn d05_text_is_trimmed_and_cut_at_300_code_points() {
    let dir = tempfile::tempdir().unwrap();
    let body = json!([{"evento": "  abrir  ", "detalhe": "x".repeat(400), "pilha": "😀".repeat(400)}]);
    assert_eq!(post(dir.path(), body), Ok(1));
    let row = &rows(dir.path())[0];
    assert_eq!(row["evento"], "abrir");
    assert_eq!(row["detalhe"], "x".repeat(300));
    assert_eq!(row["pilha"], "😀".repeat(300));
}

/// D-07: nível fora de ok/aviso/erro vira "ok", na mesma posição.
#[test]
fn d07_unknown_level_becomes_ok() {
    let dir = tempfile::tempdir().unwrap();
    assert_eq!(post(dir.path(), json!([{"evento": "n", "nivel": "fatal"}, {"evento": "m", "nivel": "AVISO"}])), Ok(2));
    assert_eq!(rows(dir.path()).iter().map(|row| row["nivel"].clone()).collect::<Vec<_>>(), [json!("ok"), json!("ok")]);
}

/// D-08: o horário da tela vale no mesmo dia e até um minuto à frente; senão vale o do envio.
#[test]
fn d08_screen_time_rules() {
    let cases = [
        ("2026-11-16T09:15:30.123-03:00", "2026-11-16T09:15:30.123-03:00"),
        ("2026-11-15T23:59:00-03:00", STAMP),
        ("2026-11-16T10:00:50-03:00", "2026-11-16T10:00:50.000-03:00"),
        ("2026-11-16T10:01:30-03:00", STAMP),
        ("ontem", STAMP),
        ("2026-11-16T08:00:00", "2026-11-16T08:00:00.000-03:00"),
        ("2026-11-16T12:30:00Z", "2026-11-16T09:30:00.000-03:00"),
    ];
    for (sent, expected) in cases {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(post(dir.path(), json!([{"evento": "t", "ts": sent}])), Ok(1), "{sent}");
        assert_eq!(written(dir.path()), format!("{{\"ts\": \"{expected}\", \"evento\": \"t\", \"nivel\": \"ok\", \"origem\": \"tela\"}}\n"), "{sent}");
    }
}

/// D-05/D-09 com texto não ASCII: sai cru, sem escape, e o fim de linha do texto fica no JSON.
#[test]
fn unicode_is_written_raw() {
    let dir = tempfile::tempdir().unwrap();
    assert_eq!(post(dir.path(), json!([{"evento": "ação.😀", "detalhe": "olá\nmundo\t"}])), Ok(1));
    assert_eq!(written(dir.path()), format!("{{\"ts\": \"{STAMP}\", \"evento\": \"ação.😀\", \"detalhe\": \"olá\\nmundo\", \"nivel\": \"ok\", \"origem\": \"tela\"}}\n"));
}

/// Surrogate solto num campo guardado: a rota antiga respondia 500 e perdia o lote; agora o
/// caractere vira U+FFFD e a linha é gravada (correção decidida em 2026-11-16).
#[test]
fn lone_surrogate_is_replaced_and_written() {
    let dir = tempfile::tempdir().unwrap();
    assert_eq!(record_screen(dir.path(), br#"[{"evento": "x\ud800", "detalhe": "a\udc00b"}]"#, now()), Ok(1));
    assert_eq!(written(dir.path()), format!("{{\"ts\": \"{STAMP}\", \"evento\": \"x\u{FFFD}\", \"detalhe\": \"a\u{FFFD}b\", \"nivel\": \"ok\", \"origem\": \"tela\"}}\n"));
}

fn prefill(base: &Path, name: &str, size: usize) {
    std::fs::create_dir_all(base).unwrap();
    let line = "{\"ts\": \"2026-11-16T09:00:00.000-03:00\", \"evento\": \"fill\", \"nivel\": \"ok\", \"origem\": \"tela\"}\n";
    let text = line.repeat(size / line.len()) + &" ".repeat(size % line.len());
    std::fs::write(base.join(name), text).unwrap();
}

fn tail(base: &Path, size: usize) -> (u64, Vec<Value>) {
    let bytes = std::fs::read(base.join(DAY)).unwrap();
    let added = String::from_utf8(bytes[size..].to_vec()).unwrap();
    (bytes.len() as u64, added.lines().filter(|l| !l.trim().is_empty()).map(|l| serde_json::from_str(l).unwrap()).collect())
}

/// D-11: passado o teto do dia só falha entra; o teto rígido para tudo; cruzar cada um grava `diag.teto`.
#[test]
fn d11_day_and_hard_caps() {
    const MI: usize = 1024 * 1024;
    let teto = |what: &str, cap: usize| json!({"ts": STAMP, "evento": "diag.teto", "nivel": "aviso", "origem": "servidor",
        "etapa": what, "detalhe": format!("teto de {cap} bytes no dia")});
    let dir = tempfile::tempdir().unwrap();
    prefill(dir.path(), DAY, 4 * MI - 50);
    assert_eq!(post(dir.path(), json!([{"evento": "ok1", "detalhe": "z".repeat(100)}])), Ok(1));
    let (size, added) = tail(dir.path(), 4 * MI - 50);
    assert_eq!(size, 4194625);
    assert_eq!(added[1], teto("sucesso", 4 * MI));

    let dir = tempfile::tempdir().unwrap();
    prefill(dir.path(), DAY, 4 * MI + 10);
    assert_eq!(post(dir.path(), json!([{"evento": "ok1"}, {"evento": "w1", "nivel": "aviso"}])), Ok(2));
    let (size, added) = tail(dir.path(), 4 * MI + 10);
    assert_eq!((size, added), (4194406, vec![json!({"ts": STAMP, "evento": "w1", "nivel": "aviso", "origem": "tela"})]));

    let dir = tempfile::tempdir().unwrap();
    prefill(dir.path(), DAY, 4 * MI + 10);
    assert_eq!(post(dir.path(), json!([{"evento": "ok1"}, {"evento": "ok2"}])), Ok(2));
    assert_eq!(tail(dir.path(), 4 * MI + 10).0, 4194314);

    let dir = tempfile::tempdir().unwrap();
    prefill(dir.path(), DAY, 8 * MI - 50);
    assert_eq!(post(dir.path(), json!([{"evento": "e1", "nivel": "erro", "detalhe": "z".repeat(100)}])), Ok(1));
    let (size, added) = tail(dir.path(), 8 * MI - 50);
    assert_eq!(size, 8388927);
    assert_eq!(added[1], teto("tudo", 8 * MI));

    let dir = tempfile::tempdir().unwrap();
    prefill(dir.path(), DAY, 8 * MI + 10);
    assert_eq!(post(dir.path(), json!([{"evento": "e1", "nivel": "erro"}])), Ok(1));
    assert_eq!(tail(dir.path(), 8 * MI + 10).0, 8388618);
}

/// D-12: o append apaga os dias além dos sete guardados.
#[test]
fn d12_append_prunes_days_beyond_seven() {
    let dir = tempfile::tempdir().unwrap();
    for day in ["2026-11-08", "2026-11-09", "2026-11-10", "2026-11-15"] {
        prefill(dir.path(), &format!("uso-{day}.jsonl"), 100);
    }
    assert_eq!(post(dir.path(), json!([{"evento": "poda"}])), Ok(1));
    let names: Vec<String> = files(dir.path()).iter().map(|p| p.file_name().unwrap().to_string_lossy().into_owned()).collect();
    assert_eq!(names, ["uso-2026-11-10.jsonl", "uso-2026-11-15.jsonl", "uso-2026-11-16.jsonl"]);
}

/// D-13: lotes simultâneos não se misturam: toda linha sai inteira.
#[test]
fn d13_concurrent_batches_keep_whole_lines() {
    let dir = tempfile::tempdir().unwrap();
    std::thread::scope(|scope| {
        for writer in 0..8 {
            let base = dir.path();
            scope.spawn(move || {
                let batch: Vec<Value> = (0..50).map(|i| json!({"evento": format!("w{writer}-{i}"), "detalhe": "y".repeat(250)})).collect();
                assert_eq!(post(base, Value::Array(batch)), Ok(50));
            });
        }
    });
    assert_eq!(rows(dir.path()).len(), 400);
}

fn seed(base: &Path) {
    std::fs::create_dir_all(base.join("servidor-rust")).unwrap();
    std::fs::write(base.join("uso-2026-11-15.jsonl"), concat!(
        r#"{"ts": "2026-11-15T10:00:00.000-03:00", "evento": "y1", "seq": 1}"#, "\n",
        r#"{"ts": "2026-11-15T11:00:00.000-03:00", "evento": "y2"}"#, "\n", "nao json\n[1,2]\n")).unwrap();
    std::fs::write(base.join(DAY), concat!(
        r#"{"ts": "2026-11-16T09:00:00.000-03:00", "evento": "a", "seq": 2}"#, "\n",
        r#"{"ts": "2026-11-16T09:00:00.000-03:00", "evento": "b", "seq": 1}"#, "\n",
        r#"{"ts": "2026-11-16T08:00:00.000-03:00", "evento": "c"}"#, "\n", "nao json\n[1,2]\n")).unwrap();
    std::fs::write(base.join("servidor-rust").join(DAY), concat!(r#"{"ts": "2026-11-16T09:30:00.000-03:00", "evento": "rust1"}"#, "\n")).unwrap();
    std::fs::write(base.join("outro.jsonl"), "x\n").unwrap();
}

/// D-14, D-16: o resumo atravessa as subpastas; `dias` conta nomes distintos e `arquivos` repete.
#[test]
fn d14_d16_summary_walks_the_whole_diary() {
    let dir = tempfile::tempdir().unwrap();
    seed(dir.path());
    assert_eq!(summary(dir.path()), json!({"dias": 2, "bytes": 396,
        "arquivos": ["uso-2026-11-15.jsonl", "uso-2026-11-16.jsonl", "uso-2026-11-16.jsonl"], "dias_guardados": 7}));
    let empty = tempfile::tempdir().unwrap();
    assert_eq!(summary(&empty.path().join("diario")), json!({"dias": 0, "bytes": 0, "arquivos": [], "dias_guardados": 7}));
    assert_eq!(latest(&empty.path().join("diario"), 60), Vec::<Value>::new());
}

/// D-15: as mais novas por dia, `ts` e `seq` decrescentes, linha corrompida pulada, e cada arquivo
/// lido só até juntar N (por isso `ultimas=2` traz rust1 e b, não a e b).
#[test]
fn d15_latest_lines_follow_the_old_reading() {
    let dir = tempfile::tempdir().unwrap();
    seed(dir.path());
    let events = |count| latest(dir.path(), count).iter().map(|row| row["evento"].as_str().unwrap().to_owned()).collect::<Vec<_>>();
    assert_eq!(events(60), ["rust1", "a", "b", "c", "y2", "y1"]);
    assert_eq!(events(2), ["rust1", "b"]);
    assert_eq!(events(1), ["rust1"]);
    assert_eq!(latest(dir.path(), 60)[1], json!({"ts": "2026-11-16T09:00:00.000-03:00", "evento": "a", "seq": 2}));
}

/// D-15: `ultimas` padrão 60, entre 1 e 200; texto que não é inteiro é recusado com o valor recebido.
#[test]
fn d15_latest_count_parsing() {
    assert_eq!(latest_count(None), Ok(60));
    assert_eq!(latest_count(Some("ultimas=2")), Ok(2));
    assert_eq!(latest_count(Some("ultimas=0")), Ok(1));
    assert_eq!(latest_count(Some("ultimas=-5")), Ok(1));
    assert_eq!(latest_count(Some("ultimas=500")), Ok(200));
    assert_eq!(latest_count(Some("ultimas=abc")), Err("abc".into()));
    assert_eq!(latest_count(Some("ultimas=2.5")), Err("2.5".into()));
    assert_eq!(latest_count(Some("ultimas=")), Err(String::new()));
}

/// D-14, D-17: o download junta todos os dias (subpastas incluídas) depois do cabeçalho, do mais
/// antigo ao mais novo, e ignora quem não é `uso-AAAA-MM-DD.jsonl`.
#[test]
fn d17_download_concatenates_days_oldest_first() {
    let dir = tempfile::tempdir().unwrap();
    seed(dir.path());
    let body = download(dir.path(), "{\"evento\": \"diag.formato\"}");
    let lines: Vec<&str> = body.lines().collect();
    assert_eq!(lines[0], "{\"evento\": \"diag.formato\"}");
    assert_eq!(&lines[1..], [
        r#"{"ts": "2026-11-15T10:00:00.000-03:00", "evento": "y1", "seq": 1}"#,
        r#"{"ts": "2026-11-15T11:00:00.000-03:00", "evento": "y2"}"#, "nao json", "[1,2]",
        r#"{"ts": "2026-11-16T09:30:00.000-03:00", "evento": "rust1"}"#,
        r#"{"ts": "2026-11-16T09:00:00.000-03:00", "evento": "a", "seq": 2}"#,
        r#"{"ts": "2026-11-16T09:00:00.000-03:00", "evento": "b", "seq": 1}"#,
        r#"{"ts": "2026-11-16T08:00:00.000-03:00", "evento": "c"}"#, "nao json", "[1,2]",
    ]);
    let empty = tempfile::tempdir().unwrap();
    assert_eq!(download(empty.path(), "h"), "h\n");
}

proptest::proptest! {
    /// Corpo qualquer, de qualquer cliente: nunca pânico, e o que for gravado é uma linha JSON por evento.
    #[test]
    fn any_body_is_handled_without_panic(body in proptest::collection::vec(proptest::num::u8::ANY, 0..512)) {
        let dir = tempfile::tempdir().unwrap();
        let _ = record_screen(dir.path(), &body, now());
        for line in written(dir.path()).lines() {
            proptest::prop_assert!(serde_json::from_str::<Value>(line).is_ok());
        }
    }

    #[test]
    fn any_event_fields_are_handled_without_panic(event in ".{0,40}", ts in ".{0,40}", level in ".{0,10}", ms in proptest::num::i64::ANY) {
        let dir = tempfile::tempdir().unwrap();
        let _ = post(dir.path(), json!([{"evento": event, "ts": ts, "nivel": level, "ms": ms, "seq": ts}]));
        let _ = latest(dir.path(), 5);
    }
}
