//! O que o condutor tem de devolver para cada registro, sombra do Jev e eventos: `expected: None`
//! é recusa. Quantidade e profundidade grandes são montadas em laço, entrada e esperado.

pub struct Case {
    pub name: String,
    pub files: Vec<(&'static str, String)>,
    pub journal_bytes: Option<Vec<u8>>,
    pub expected: Option<String>,
}

struct Row {
    name: &'static str,
    journal: &'static str,
    shadow: &'static str,
    events: &'static str,
    expected: Option<&'static str>,
}

const EMPTY: Row = Row { name: "", journal: "", shadow: "", events: "", expected: None };
const TS: &str = "2026-09-25T13:00:00+00:00";
const NOTIFY: &str = "- 2026-09-25T13:00:00+00:00 · notify → arbiter: x\n";

fn built(name: &str, journal: String, shadow: String, events: String, expected: Option<String>) -> Case {
    Case {
        name: name.into(),
        files: vec![("registro.md", journal), ("jev-shadow.jsonl", shadow), ("eventos.jsonl", events)],
        journal_bytes: None,
        expected,
    }
}

fn feed(items: impl IntoIterator<Item = String>, truncated: bool) -> String {
    let items = items.into_iter().collect::<Vec<_>>().join(",");
    format!(r#"{{"feed":[{items}],"truncated":{truncated},"skipped":0}}"#)
}

fn jev(mode: &str, p: &str) -> String {
    format!(r#"{{"mode":{mode},"choice":null,"p":{p},"veto":{{}},"held":[],"would_drop":null,"error":null}}"#)
}

fn woke(id: u32, text: &str, jev: &str) -> String {
    format!(r#"{{"id":"r{id}","ts":"{TS}","kind":"woke","text":"{text}","task":null,"jev":{jev}}}"#)
}

fn event(text: &str) -> String {
    format!(r#"{{"id":"e1","ts":"{TS}","kind":"event","text":"{text}","task":null,"jev":null}}"#)
}

fn looped() -> Vec<Case> {
    let mut out = vec![
        built(
            "journal-tail",
            (0..305).map(|i| format!("- {TS} · aviso: {i}\n")).collect(),
            String::new(),
            String::new(),
            Some(feed(
                (5..305).map(|i| {
                    format!(r#"{{"id":"r{}","ts":"{TS}","kind":"notice","text":"{i}","task":null,"jev":null}}"#, i + 1)
                }),
                false,
            )),
        ),
        built(
            "shadow-tail",
            NOTIFY.into(),
            (0..305_u32).map(|i| format!(r#"{{"ts": "{TS}", "text": "x", "p": {:?}}}"#, f64::from(i) / 1000.0) + "\n").collect(),
            String::new(),
            Some(feed([woke(1, "x", &jev("null", "0.005"))], false)),
        ),
    ];
    let (long, short) = ("é".repeat(500), "é".repeat(499));
    out.push(built(
        "shadow-cut-codepoints",
        format!("- {TS} · notify → arbiter: {long}fim\n- {TS} · notify → arbiter: {short}fim\n"),
        format!("{{\"ts\": \"{TS}\", \"text\": \"{long}\"}}\n{{\"ts\": \"{TS}\", \"text\": \"{short}\"}}\n"),
        String::new(),
        Some(feed([woke(1, &format!("{long}fim"), &jev("null", "null")), woke(2, &format!("{short}fim"), "null")], false)),
    ));
    // O 501º evento fica de fora do feed, que marca `truncated`.
    for (name, count, truncated) in [("feed-cap500", 500, false), ("feed-truncated501", 501, true)] {
        out.push(built(
            name,
            String::new(),
            String::new(),
            (0..count).map(|i| format!("{{\"ts\": \"{TS}\", \"tipo\": \"entrega\", \"rodada\": {i}}}\n")).collect(),
            Some(feed(
                (0..500).map(|i| {
                    format!(r#"{{"id":"e{}","ts":"{TS}","kind":"event","text":"entrega r{i}","task":null,"jev":null}}"#, i + 1)
                }),
                truncated,
            )),
        ));
    }
    for depth in [200, 2000] {
        let value = format!("{}0{}", "[".repeat(depth), "]".repeat(depth));
        out.push(built(
            &format!("deep-event-text-{depth}"),
            String::new(),
            String::new(),
            format!("{{\"ts\":\"{TS}\",\"tipo\":\"entrega\",\"resultado\":{value}}}\n"),
            Some(feed([event(&format!("entrega resultado={value}"))], false)),
        ));
        out.push(built(
            &format!("deep-unused-field-{depth}"),
            String::new(),
            String::new(),
            format!("{{\"ts\":\"{TS}\",\"tipo\":\"entrega\",\"extra\":{value}}}\n"),
            Some(feed([event("entrega")], false)),
        ));
        out.push(built(
            &format!("deep-shadow-mode-{depth}"),
            NOTIFY.into(),
            format!("{{\"ts\":\"{TS}\",\"text\":\"x\",\"mode\":{value}}}\n"),
            String::new(),
            Some(feed([woke(1, "x", &jev(&value, "null"))], false)),
        ));
    }
    let digits = "9".repeat(4300);
    out.push(built(
        "integer-limit4300",
        format!("- {TS} · aviso: T{digits}\n"),
        String::new(),
        String::new(),
        Some(feed(
            [format!(r#"{{"id":"r1","ts":"{TS}","kind":"notice","text":"T{digits}","task":{digits},"jev":null}}"#)],
            false,
        )),
    ));
    // Um dígito além do limite de conversão: recusa.
    out.push(built("integer-limit4301", format!("- {TS} · aviso: T{digits}9\n"), String::new(), String::new(), None));
    out
}

pub fn cases() -> Vec<Case> {
    let mut out: Vec<Case> = ROWS
        .iter()
        .map(|row| built(row.name, row.journal.into(), row.shadow.into(), row.events.into(), row.expected.map(str::to_owned)))
        .collect();
    out.extend(looped());
    out.push(Case {
        name: "utf8-replacement".into(),
        files: vec![],
        journal_bytes: Some(BYTES.to_vec()),
        expected: Some(BYTES_EXPECTED.into()),
    });
    out
}

const BYTES: &[u8] = b"- 2026-09-25T13:00:00+00:00 \xc2\xb7 aviso: a\xc3\xa7\xc3\xa3o\x0a- 2026-09-25T13:00:00Z \xc2\xb7 aviso: \xff\x0a";
const BYTES_EXPECTED: &str = r#"{"feed":[{"id":"r1","ts":"2026-09-25T13:00:00+00:00","kind":"notice","text":"ação","task":null,"jev":null},{"id":"r2","ts":"2026-09-25T13:00:00Z","kind":"notice","text":"�","task":null,"jev":null}],"truncated":false,"skipped":0}"#;

const ROWS: &[Row] = &[
    Row { name: "empty",
        expected: Some(r#"{"feed":[],"truncated":false,"skipped":0}"#),
        ..EMPTY },
    Row { name: "prefixes",
        journal: r"- 2026-09-25T13:00:00+00:00 · notify → arbiter: T3 ação
- 2026-09-25T13:00:00+00:00 · (jev: no action) T3 ação
- 2026-09-25T13:00:00+00:00 · (regex: no action) T3 ação
- 2026-09-25T13:00:00+00:00 · aviso: T3 ação
- 2026-09-25T13:00:00+00:00 · alarm: T3 ação
- 2026-09-25T13:00:00+00:00 · notify FAILED: T3 ação
- 2026-09-25T13:00:00+00:00 · close failed: T3 ação
- 2026-09-25T13:00:00+00:00 · join failed: T3 ação
- 2026-09-25T13:00:00+00:00 · closed session: T3 ação
- 2026-09-25T13:00:00+00:00 · joined group: T3 ação
",
        expected: Some(r#"{"feed":[{"id":"r1","ts":"2026-09-25T13:00:00+00:00","kind":"woke","text":"T3 ação","task":3,"jev":null},{"id":"r2","ts":"2026-09-25T13:00:00+00:00","kind":"dropped","text":"T3 ação","task":3,"jev":null},{"id":"r3","ts":"2026-09-25T13:00:00+00:00","kind":"dropped","text":"T3 ação","task":3,"jev":null},{"id":"r4","ts":"2026-09-25T13:00:00+00:00","kind":"notice","text":"T3 ação","task":3,"jev":null},{"id":"r5","ts":"2026-09-25T13:00:00+00:00","kind":"alarm","text":"T3 ação","task":3,"jev":null},{"id":"r6","ts":"2026-09-25T13:00:00+00:00","kind":"alarm","text":"notify FAILED: T3 ação","task":3,"jev":null},{"id":"r7","ts":"2026-09-25T13:00:00+00:00","kind":"alarm","text":"close failed: T3 ação","task":3,"jev":null},{"id":"r8","ts":"2026-09-25T13:00:00+00:00","kind":"alarm","text":"join failed: T3 ação","task":3,"jev":null},{"id":"r9","ts":"2026-09-25T13:00:00+00:00","kind":"notice","text":"closed session: T3 ação","task":3,"jev":null},{"id":"r10","ts":"2026-09-25T13:00:00+00:00","kind":"notice","text":"joined group: T3 ação","task":3,"jev":null}],"truncated":false,"skipped":0}"#),
        ..EMPTY },
    Row { name: "ignored-prefixes",
        journal: r"- 2026-09-25T13:00:00+00:00 · orq init: arbitro
- 2026-09-25T13:00:00+00:00 · screen taken by ex1
",
        expected: Some(r#"{"feed":[],"truncated":false,"skipped":0}"#),
        ..EMPTY },
    Row { name: "journal-invalid",
        journal: "\n \t\n## solta\n- sem-data · texto\n- 2026-09-25T13:00:00+00:00 errado\n",
        expected: Some(r#"{"feed":[],"truncated":false,"skipped":3}"#),
        ..EMPTY },
    Row { name: "physical-ids-ties",
        journal: r"
- 2026-09-25T13:00:00+00:00 · notify → arbiter: T2 x
- 2026-09-25T13:00:00+00:00 · aviso: y
",
        events: r#"
{"ts": "2026-09-25T13:00:00+00:00", "tipo": "entrega", "task": 2, "rodada": 1}
"#,
        expected: Some(r#"{"feed":[{"id":"r2","ts":"2026-09-25T13:00:00+00:00","kind":"woke","text":"T2 x","task":2,"jev":null},{"id":"r3","ts":"2026-09-25T13:00:00+00:00","kind":"notice","text":"y","task":null,"jev":null},{"id":"e2","ts":"2026-09-25T13:00:00+00:00","kind":"event","text":"entrega T2 r1","task":2,"jev":null}],"truncated":false,"skipped":0}"#),
        ..EMPTY },
    Row { name: "all-event-types",
        events: r#"{"ts": "2026-09-25T13:00:00+00:00", "tipo": "entrega", "task": 3, "rodada": 2, "resultado": "aprova", "commit": "abc", "sessao": "rev", "executor": "ex", "par": "r", "de": "a", "para": "b", "motivo": "ação", "reincide": true}
{"ts": "2026-09-25T13:00:00+00:00", "tipo": "execucao_fim", "task": 3, "rodada": 2, "resultado": "aprova", "commit": "abc", "sessao": "rev", "executor": "ex", "par": "r", "de": "a", "para": "b", "motivo": "ação", "reincide": true}
{"ts": "2026-09-25T13:00:00+00:00", "tipo": "execucao_inicio", "task": 3, "rodada": 2, "resultado": "aprova", "commit": "abc", "sessao": "rev", "executor": "ex", "par": "r", "de": "a", "para": "b", "motivo": "ação", "reincide": true}
{"ts": "2026-09-25T13:00:00+00:00", "tipo": "sessao_trocada", "task": 3, "rodada": 2, "resultado": "aprova", "commit": "abc", "sessao": "rev", "executor": "ex", "par": "r", "de": "a", "para": "b", "motivo": "ação", "reincide": true}
{"ts": "2026-09-25T13:00:00+00:00", "tipo": "task_inicio", "task": 3, "rodada": 2, "resultado": "aprova", "commit": "abc", "sessao": "rev", "executor": "ex", "par": "r", "de": "a", "para": "b", "motivo": "ação", "reincide": true}
{"ts": "2026-09-25T13:00:00+00:00", "tipo": "veredito", "task": 3, "rodada": 2, "resultado": "aprova", "commit": "abc", "sessao": "rev", "executor": "ex", "par": "r", "de": "a", "para": "b", "motivo": "ação", "reincide": true}
"#,
        expected: Some(r#"{"feed":[{"id":"e1","ts":"2026-09-25T13:00:00+00:00","kind":"event","text":"entrega T3 r2 resultado=aprova commit=abc sessao=rev executor=ex par=r de=a para=b motivo=ação reincide","task":3,"jev":null},{"id":"e2","ts":"2026-09-25T13:00:00+00:00","kind":"event","text":"execucao_fim T3 r2 resultado=aprova commit=abc sessao=rev executor=ex par=r de=a para=b motivo=ação reincide","task":3,"jev":null},{"id":"e3","ts":"2026-09-25T13:00:00+00:00","kind":"event","text":"execucao_inicio T3 r2 resultado=aprova commit=abc sessao=rev executor=ex par=r de=a para=b motivo=ação reincide","task":3,"jev":null},{"id":"e4","ts":"2026-09-25T13:00:00+00:00","kind":"event","text":"sessao_trocada T3 r2 resultado=aprova commit=abc sessao=rev executor=ex par=r de=a para=b motivo=ação reincide","task":3,"jev":null},{"id":"e5","ts":"2026-09-25T13:00:00+00:00","kind":"event","text":"task_inicio T3 r2 resultado=aprova commit=abc sessao=rev executor=ex par=r de=a para=b motivo=ação reincide","task":3,"jev":null},{"id":"e6","ts":"2026-09-25T13:00:00+00:00","kind":"event","text":"veredito T3 r2 resultado=aprova commit=abc sessao=rev executor=ex par=r de=a para=b motivo=ação reincide","task":3,"jev":null}],"truncated":false,"skipped":0}"#),
        ..EMPTY },
    Row { name: "event-types-invalid",
        events: r#"
{torto
[]
null
true
{"ts": "2026-09-25T13:00:00+00:00", "tipo": "futuro"}
{"ts": "x", "tipo": "entrega"}
"#,
        expected: Some(r#"{"feed":[],"truncated":false,"skipped":6}"#),
        ..EMPTY },
    Row { name: "event-shapes",
        events: r#"{"ts": "2026-09-25T13:00:00+00:00", "tipo": "entrega", "task": true, "rodada": "2", "resultado": false, "commit": 0, "sessao": [], "executor": {}, "par": null, "de": "", "para": [1, null], "motivo": {"a": true}, "reincide": 1}
"#,
        expected: Some(r#"{"feed":[{"id":"e1","ts":"2026-09-25T13:00:00+00:00","kind":"event","text":"entrega resultado=False commit=0 sessao=[] executor={} para=[1, None] motivo={'a': True}","task":null,"jev":null}],"truncated":false,"skipped":0}"#),
        ..EMPTY },
    Row { name: "event-none-missing",
        events: r#"{"ts": "2026-09-25T13:00:00+00:00", "tipo": "entrega"}
{"ts": "2026-09-25T13:00:00+00:00", "tipo": "entrega", "task": null, "rodada": null}
"#,
        expected: Some(r#"{"feed":[{"id":"e1","ts":"2026-09-25T13:00:00+00:00","kind":"event","text":"entrega","task":null,"jev":null},{"id":"e2","ts":"2026-09-25T13:00:00+00:00","kind":"event","text":"entrega","task":null,"jev":null}],"truncated":false,"skipped":0}"#),
        ..EMPTY },
    Row { name: "event-big-integers",
        events: r#"{"ts": "2026-09-25T13:00:00+00:00", "tipo": "entrega", "task": 10000000000000000000000000000000000000000, "rodada": -10000000000000000000000000, "commit": 1000000000000000000000000000000}
"#,
        expected: Some(r#"{"feed":[{"id":"e1","ts":"2026-09-25T13:00:00+00:00","kind":"event","text":"entrega T10000000000000000000000000000000000000000 r-10000000000000000000000000 commit=1000000000000000000000000000000","task":10000000000000000000000000000000000000000,"jev":null}],"truncated":false,"skipped":0}"#),
        ..EMPTY },
    Row { name: "task-unicode",
        journal: r"- 2026-09-25T13:00:00+00:00 · aviso: T٣
- 2026-09-25T13:00:00+00:00 · aviso: Task ٤
- 2026-09-25T13:00:00+00:00 · aviso: T99999999999999999999999999
- 2026-09-25T13:00:00+00:00 · aviso: XT2 T5
- 2026-09-25T13:00:00+00:00 · aviso: T1 Task 8
- 2026-09-25T13:00:00+00:00 · aviso: Task 8 T1
- 2026-09-25T13:00:00+00:00 · aviso: éT4
- 2026-09-25T13:00:00+00:00 · aviso: T4é
- 2026-09-25T13:00:00+00:00 · aviso: T0
",
        expected: Some(r#"{"feed":[{"id":"r1","ts":"2026-09-25T13:00:00+00:00","kind":"notice","text":"T٣","task":3,"jev":null},{"id":"r2","ts":"2026-09-25T13:00:00+00:00","kind":"notice","text":"Task ٤","task":4,"jev":null},{"id":"r3","ts":"2026-09-25T13:00:00+00:00","kind":"notice","text":"T99999999999999999999999999","task":99999999999999999999999999,"jev":null},{"id":"r4","ts":"2026-09-25T13:00:00+00:00","kind":"notice","text":"XT2 T5","task":5,"jev":null},{"id":"r5","ts":"2026-09-25T13:00:00+00:00","kind":"notice","text":"T1 Task 8","task":1,"jev":null},{"id":"r6","ts":"2026-09-25T13:00:00+00:00","kind":"notice","text":"Task 8 T1","task":8,"jev":null},{"id":"r7","ts":"2026-09-25T13:00:00+00:00","kind":"notice","text":"éT4","task":null,"jev":null},{"id":"r8","ts":"2026-09-25T13:00:00+00:00","kind":"notice","text":"T4é","task":null,"jev":null},{"id":"r9","ts":"2026-09-25T13:00:00+00:00","kind":"notice","text":"T0","task":0,"jev":null}],"truncated":false,"skipped":0}"#),
        ..EMPTY },
    Row { name: "timestamps",
        events: r#"{"ts": "2026-09-25T13:00:00", "tipo": "entrega"}
{"ts": "2026-09-25", "tipo": "entrega"}
{"ts": "20260925T130000Z", "tipo": "entrega"}
{"ts": "2026-09-25X13:00:00,123456Z", "tipo": "entrega"}
{"ts": "2026-09-25T10:00:00-03:00", "tipo": "entrega"}
{"ts": "2026-09-25T13:00:00+00:00:00.5", "tipo": "entrega"}
{"ts": "2026-W39-5T13:00:00Z", "tipo": "entrega"}
"#,
        expected: Some(r#"{"feed":[{"id":"e4","ts":"2026-09-25X13:00:00,123456Z","kind":"event","text":"entrega","task":null,"jev":null},{"id":"e1","ts":"2026-09-25T13:00:00","kind":"event","text":"entrega","task":null,"jev":null},{"id":"e3","ts":"20260925T130000Z","kind":"event","text":"entrega","task":null,"jev":null},{"id":"e5","ts":"2026-09-25T10:00:00-03:00","kind":"event","text":"entrega","task":null,"jev":null},{"id":"e7","ts":"2026-W39-5T13:00:00Z","kind":"event","text":"entrega","task":null,"jev":null},{"id":"e6","ts":"2026-09-25T13:00:00+00:00:00.5","kind":"event","text":"entrega","task":null,"jev":null},{"id":"e2","ts":"2026-09-25","kind":"event","text":"entrega","task":null,"jev":null}],"truncated":false,"skipped":0}"#),
        ..EMPTY },
    Row { name: "calendar-edges",
        events: r#"{"ts": "0001-01-01T00:00:00", "tipo": "entrega"}
{"ts": "0001-01-01T00:00:00+14:00", "tipo": "entrega"}
{"ts": "9999-12-31T23:59:59", "tipo": "entrega"}
{"ts": "2026-09-25T24:00:00Z", "tipo": "entrega"}
"#,
        expected: Some(r#"{"feed":[{"id":"e4","ts":"2026-09-25T24:00:00Z","kind":"event","text":"entrega","task":null,"jev":null}],"truncated":false,"skipped":3}"#),
        ..EMPTY },
    Row { name: "shadow-shapes",
        journal: r"- 2026-09-25T13:00:00+00:00 · notify → arbiter: x
",
        shadow: r#"
[]
{}
{torto
{"ts": "2026-09-25T13:00:00+00:00", "text": 3}
{"ts": "x", "text": "x"}
{"ts": "2026-09-25T13:00:00+00:00", "text": "x"}
"#,
        expected: Some(r#"{"feed":[{"id":"r1","ts":"2026-09-25T13:00:00+00:00","kind":"woke","text":"x","task":null,"jev":{"mode":null,"choice":null,"p":null,"veto":{},"held":[],"would_drop":null,"error":null}}],"truncated":false,"skipped":5}"#),
        ..EMPTY },
    Row { name: "shadow-normalization",
        journal: r"- 2026-09-25T13:00:00+00:00 · notify → arbiter: tela ⏎ outra
",
        shadow: r#"{"ts": "2026-09-25T13:00:00+00:00", "text": "  tela\r\noutra\n"}
"#,
        expected: Some(r#"{"feed":[{"id":"r1","ts":"2026-09-25T13:00:00+00:00","kind":"woke","text":"tela ⏎ outra","task":null,"jev":{"mode":null,"choice":null,"p":null,"veto":{},"held":[],"would_drop":null,"error":null}}],"truncated":false,"skipped":0}"#),
        ..EMPTY },
    Row { name: "shadow-unicode-lines",
        journal: r"- 2026-09-25T13:00:00+00:00 · notify → arbiter: tela ⏎ outra
",
        shadow: "{\"ts\": \"2026-09-25T13:00:00+00:00\", \"text\": \"tela\\u000botra\"}\n{\"ts\": \"2026-09-25T13:00:00+00:00\", \"text\": \"tela\u{2028}outra\"}\n",
        expected: Some(r#"{"feed":[{"id":"r1","ts":"2026-09-25T13:00:00+00:00","kind":"woke","text":"tela ⏎ outra","task":null,"jev":null}],"truncated":false,"skipped":2}"#),
        ..EMPTY },
    Row { name: "shadow-first-once",
        journal: r"- 2026-09-25T13:00:00+00:00 · notify → arbiter: x
- 2026-09-25T13:00:00+00:00 · notify → arbiter: x
- 2026-09-25T13:00:00+00:00 · notify → arbiter: x
",
        shadow: r#"{"ts": "2026-09-25T13:00:00+00:00", "text": "x", "mode": "a"}
{"ts": "2026-09-25T13:00:00+00:00", "text": "x", "mode": "b"}
"#,
        expected: Some(r#"{"feed":[{"id":"r1","ts":"2026-09-25T13:00:00+00:00","kind":"woke","text":"x","task":null,"jev":{"mode":"a","choice":null,"p":null,"veto":{},"held":[],"would_drop":null,"error":null}},{"id":"r2","ts":"2026-09-25T13:00:00+00:00","kind":"woke","text":"x","task":null,"jev":{"mode":"b","choice":null,"p":null,"veto":{},"held":[],"would_drop":null,"error":null}},{"id":"r3","ts":"2026-09-25T13:00:00+00:00","kind":"woke","text":"x","task":null,"jev":null}],"truncated":false,"skipped":0}"#),
        ..EMPTY },
    Row { name: "shadow-window",
        journal: r"- 2026-09-25T13:00:00+00:00 · notify → arbiter: 0
- 2026-09-25T13:00:00+00:00 · notify → arbiter: 1
- 2026-09-25T13:00:00+00:00 · notify → arbiter: 2
- 2026-09-25T13:00:00+00:00 · notify → arbiter: 3
",
        shadow: r#"{"ts": "2026-09-25T13:00:10+00:00", "text": "0"}
{"ts": "2026-09-25T13:00:11+00:00", "text": "1"}
{"ts": "2026-09-25T13:00:00+00:00", "text": "2"}
{"ts": "2026-09-25T13:00:09+00:00", "text": "3"}
"#,
        expected: Some(r#"{"feed":[{"id":"r1","ts":"2026-09-25T13:00:00+00:00","kind":"woke","text":"0","task":null,"jev":{"mode":null,"choice":null,"p":null,"veto":{},"held":[],"would_drop":null,"error":null}},{"id":"r2","ts":"2026-09-25T13:00:00+00:00","kind":"woke","text":"1","task":null,"jev":null},{"id":"r3","ts":"2026-09-25T13:00:00+00:00","kind":"woke","text":"2","task":null,"jev":{"mode":null,"choice":null,"p":null,"veto":{},"held":[],"would_drop":null,"error":null}},{"id":"r4","ts":"2026-09-25T13:00:00+00:00","kind":"woke","text":"3","task":null,"jev":{"mode":null,"choice":null,"p":null,"veto":{},"held":[],"would_drop":null,"error":null}}],"truncated":false,"skipped":0}"#),
        ..EMPTY },
    Row { name: "shadow-negative-window",
        journal: r"- 2026-09-25T13:00:00+00:00 · notify → arbiter: x
",
        shadow: r#"{"ts": "2026-09-25T12:59:50+00:00", "text": "x"}
"#,
        expected: Some(r#"{"feed":[{"id":"r1","ts":"2026-09-25T13:00:00+00:00","kind":"woke","text":"x","task":null,"jev":{"mode":null,"choice":null,"p":null,"veto":{},"held":[],"would_drop":null,"error":null}}],"truncated":false,"skipped":0}"#),
        ..EMPTY },
    Row { name: "shadow-only-wake-drop",
        journal: r"- 2026-09-25T13:00:00+00:00 · aviso: x
- 2026-09-25T13:00:00+00:00 · notify → arbiter: x
- 2026-09-25T13:00:00+00:00 · (jev: no action) x
- 2026-09-25T13:00:00+00:00 · (regex: no action) x
",
        shadow: r#"{"ts": "2026-09-25T13:00:00+00:00", "text": "x"}
{"ts": "2026-09-25T13:00:00+00:00", "text": "x"}
{"ts": "2026-09-25T13:00:00+00:00", "text": "x"}
{"ts": "2026-09-25T13:00:00+00:00", "text": "x"}
"#,
        expected: Some(r#"{"feed":[{"id":"r1","ts":"2026-09-25T13:00:00+00:00","kind":"notice","text":"x","task":null,"jev":null},{"id":"r2","ts":"2026-09-25T13:00:00+00:00","kind":"woke","text":"x","task":null,"jev":{"mode":null,"choice":null,"p":null,"veto":{},"held":[],"would_drop":null,"error":null}},{"id":"r3","ts":"2026-09-25T13:00:00+00:00","kind":"dropped","text":"x","task":null,"jev":{"mode":null,"choice":null,"p":null,"veto":{},"held":[],"would_drop":null,"error":null}},{"id":"r4","ts":"2026-09-25T13:00:00+00:00","kind":"dropped","text":"x","task":null,"jev":{"mode":null,"choice":null,"p":null,"veto":{},"held":[],"would_drop":null,"error":null}}],"truncated":false,"skipped":0}"#),
        ..EMPTY },
    Row { name: "jev-numbers-order",
        journal: r"- 2026-09-25T13:00:00+00:00 · notify → arbiter: x
",
        shadow: r#"{"ts": "2026-09-25T13:00:00+00:00", "text": "x", "p": true, "veto": {"b": null, "a": 0.4, "c": 0.40001, "d": "0.9", "e": false, "f": NaN, "g": Infinity, "h": -Infinity}, "mode": {"x": 1}, "choice": ["act"], "would_drop": null, "error": 4}
"#,
        expected: Some(r#"{"feed":[{"id":"r1","ts":"2026-09-25T13:00:00+00:00","kind":"woke","text":"x","task":null,"jev":{"mode":{"x":1},"choice":["act"],"p":null,"veto":{"b":null,"a":0.4,"c":0.40001,"d":null,"e":null,"f":null,"g":null,"h":null},"held":["b","c","d","e","f","g","h"],"would_drop":null,"error":4}}],"truncated":false,"skipped":0}"#),
        ..EMPTY },
    Row { name: "jev-nonfinite",
        journal: r"- 2026-09-25T13:00:00+00:00 · notify → arbiter: x
",
        shadow: r#"{"ts": "2026-09-25T13:00:00+00:00", "text": "x", "p": NaN, "veto": {"a": Infinity}}
"#,
        expected: Some(r#"{"feed":[{"id":"r1","ts":"2026-09-25T13:00:00+00:00","kind":"woke","text":"x","task":null,"jev":{"mode":null,"choice":null,"p":null,"veto":{"a":null},"held":["a"],"would_drop":null,"error":null}}],"truncated":false,"skipped":0}"#),
        ..EMPTY },
    Row { name: "unicode-splitlines",
        journal: "- 2026-09-25T13:00:00+00:00 · aviso: a\r\n- 2026-09-25T13:00:00+00:00 · aviso: b\u{2028}",
        expected: Some(r#"{"feed":[{"id":"r1","ts":"2026-09-25T13:00:00+00:00","kind":"notice","text":"a","task":null,"jev":null},{"id":"r2","ts":"2026-09-25T13:00:00+00:00","kind":"notice","text":"b","task":null,"jev":null}],"truncated":false,"skipped":0}"#),
        ..EMPTY },
    Row { name: "task-unicode-boundaries",
        journal: "- 2026-09-25T13:00:00+00:00 · aviso: T2\u{200d}\n- 2026-09-25T13:00:00+00:00 · aviso: T3́\n- 2026-09-25T13:00:00+00:00 · aviso: T4²\n- 2026-09-25T13:00:00+00:00 · aviso: ²T5\n- 2026-09-25T13:00:00+00:00 · aviso: T6א\n- 2026-09-25T13:00:00+00:00 · aviso: T7ְ\n- 2026-09-25T13:00:00+00:00 · aviso: T8_\n- 2026-09-25T13:00:00+00:00 · aviso: _T9\n",
        expected: Some("{\"feed\":[{\"id\":\"r1\",\"ts\":\"2026-09-25T13:00:00+00:00\",\"kind\":\"notice\",\"text\":\"T2\u{200d}\",\"task\":2,\"jev\":null},{\"id\":\"r2\",\"ts\":\"2026-09-25T13:00:00+00:00\",\"kind\":\"notice\",\"text\":\"T3́\",\"task\":3,\"jev\":null},{\"id\":\"r3\",\"ts\":\"2026-09-25T13:00:00+00:00\",\"kind\":\"notice\",\"text\":\"T4²\",\"task\":null,\"jev\":null},{\"id\":\"r4\",\"ts\":\"2026-09-25T13:00:00+00:00\",\"kind\":\"notice\",\"text\":\"²T5\",\"task\":null,\"jev\":null},{\"id\":\"r5\",\"ts\":\"2026-09-25T13:00:00+00:00\",\"kind\":\"notice\",\"text\":\"T6א\",\"task\":null,\"jev\":null},{\"id\":\"r6\",\"ts\":\"2026-09-25T13:00:00+00:00\",\"kind\":\"notice\",\"text\":\"T7ְ\",\"task\":7,\"jev\":null},{\"id\":\"r7\",\"ts\":\"2026-09-25T13:00:00+00:00\",\"kind\":\"notice\",\"text\":\"T8_\",\"task\":null,\"jev\":null},{\"id\":\"r8\",\"ts\":\"2026-09-25T13:00:00+00:00\",\"kind\":\"notice\",\"text\":\"_T9\",\"task\":null,\"jev\":null}],\"truncated\":false,\"skipped\":0}"),
        ..EMPTY },
    Row { name: "nonfinite-string-values",
        journal: r"- 2026-09-25T13:00:00+00:00 · notify → arbiter: x
",
        shadow: "{\"ts\": \"2026-09-25T13:00:00+00:00\", \"text\": \"x\", \"mode\": \"\u{10ffff}orq-nonfinite:NaN\", \"p\": \"NaN\", \"veto\": {\"a\": \"NaN\"}}\n",
        expected: Some("{\"feed\":[{\"id\":\"r1\",\"ts\":\"2026-09-25T13:00:00+00:00\",\"kind\":\"woke\",\"text\":\"x\",\"task\":null,\"jev\":{\"mode\":\"\u{10ffff}orq-nonfinite:NaN\",\"choice\":null,\"p\":null,\"veto\":{\"a\":null},\"held\":[\"a\"],\"would_drop\":null,\"error\":null}}],\"truncated\":false,\"skipped\":0}"),
        ..EMPTY },
    Row { name: "event-nested-big-numbers",
        events: r#"{"ts": "2026-09-25T13:00:00+00:00", "tipo": "entrega", "resultado": [10000000000000000000000000000000000000000, {"a": 100000000000000000000000000000000000}], "commit": {"n": 10000000000000000000000000000000000000000}}
"#,
        expected: Some(r#"{"feed":[{"id":"e1","ts":"2026-09-25T13:00:00+00:00","kind":"event","text":"entrega resultado=[10000000000000000000000000000000000000000, {'a': 100000000000000000000000000000000000}] commit={'n': 10000000000000000000000000000000000000000}","task":null,"jev":null}],"truncated":false,"skipped":0}"#),
        ..EMPTY },
    Row { name: "high-year-order",
        events: r#"{"ts": "9998-12-31T23:59:59.000001Z", "tipo": "entrega", "rodada": 1}
{"ts": "9998-12-31T23:59:59.000002Z", "tipo": "entrega", "rodada": 2}
"#,
        expected: Some(r#"{"feed":[{"id":"e2","ts":"9998-12-31T23:59:59.000002Z","kind":"event","text":"entrega r2","task":null,"jev":null},{"id":"e1","ts":"9998-12-31T23:59:59.000001Z","kind":"event","text":"entrega r1","task":null,"jev":null}],"truncated":false,"skipped":0}"#),
        ..EMPTY },
    Row { name: "nonfinite-output-mode",
        journal: r"- 2026-09-25T13:00:00+00:00 · notify → arbiter: x
",
        shadow: r#"{"ts": "2026-09-25T13:00:00+00:00", "text": "x", "mode": NaN}
"#,
        expected: None,
        ..EMPTY },
    Row { name: "nonfinite-output-choice",
        journal: r"- 2026-09-25T13:00:00+00:00 · notify → arbiter: x
",
        shadow: r#"{"ts": "2026-09-25T13:00:00+00:00", "text": "x", "choice": NaN}
"#,
        expected: None,
        ..EMPTY },
    Row { name: "nonfinite-output-error",
        journal: r"- 2026-09-25T13:00:00+00:00 · notify → arbiter: x
",
        shadow: r#"{"ts": "2026-09-25T13:00:00+00:00", "text": "x", "error": NaN}
"#,
        expected: None,
        ..EMPTY },
    Row { name: "nonfinite-output-would_drop",
        journal: r"- 2026-09-25T13:00:00+00:00 · notify → arbiter: x
",
        shadow: r#"{"ts": "2026-09-25T13:00:00+00:00", "text": "x", "would_drop": NaN}
"#,
        expected: None,
        ..EMPTY },
    Row { name: "lone-surrogate-output",
        events: r#"{"ts":"2026-09-25T13:00:00+00:00","tipo":"entrega","resultado":"\ud800"}
"#,
        expected: None,
        ..EMPTY },
    Row { name: "surrogate-p-is-null",
        journal: r"- 2026-09-25T13:00:00+00:00 · notify → arbiter: x
",
        shadow: r#"{"ts":"2026-09-25T13:00:00+00:00","text":"x","p":"\ud800"}
"#,
        expected: Some(r#"{"feed":[{"id":"r1","ts":"2026-09-25T13:00:00+00:00","kind":"woke","text":"x","task":null,"jev":{"mode":null,"choice":null,"p":null,"veto":{},"held":[],"would_drop":null,"error":null}}],"truncated":false,"skipped":0}"#),
        ..EMPTY },
    Row { name: "iso-week-reduced",
        events: r#"{"ts": "2026-W39", "tipo": "entrega"}
{"ts": "2026W39", "tipo": "entrega"}
{"ts": "2026W395T13:00:00Z", "tipo": "entrega"}
{"ts": "2026-W39T13:00:00Z", "tipo": "entrega"}
"#,
        expected: Some(r#"{"feed":[{"id":"e3","ts":"2026W395T13:00:00Z","kind":"event","text":"entrega","task":null,"jev":null},{"id":"e4","ts":"2026-W39T13:00:00Z","kind":"event","text":"entrega","task":null,"jev":null},{"id":"e1","ts":"2026-W39","kind":"event","text":"entrega","task":null,"jev":null},{"id":"e2","ts":"2026W39","kind":"event","text":"entrega","task":null,"jev":null}],"truncated":false,"skipped":0}"#),
        ..EMPTY },
    Row { name: "calendar-interior-extreme-years",
        events: r#"{"ts": "0001-01-02T00:00:00", "tipo": "entrega"}
{"ts": "0001-06-15T13:00:00", "tipo": "entrega"}
{"ts": "9999-12-30T23:59:59", "tipo": "entrega"}
{"ts": "9999-06-15T13:00:00", "tipo": "entrega"}
"#,
        expected: Some(r#"{"feed":[{"id":"e3","ts":"9999-12-30T23:59:59","kind":"event","text":"entrega","task":null,"jev":null},{"id":"e4","ts":"9999-06-15T13:00:00","kind":"event","text":"entrega","task":null,"jev":null},{"id":"e2","ts":"0001-06-15T13:00:00","kind":"event","text":"entrega","task":null,"jev":null},{"id":"e1","ts":"0001-01-02T00:00:00","kind":"event","text":"entrega","task":null,"jev":null}],"truncated":false,"skipped":0}"#),
        ..EMPTY },
    Row { name: "nested-surrogate-string-repr",
        events: r#"{"ts":"2026-09-25T13:00:00+00:00","tipo":"entrega","resultado":["\ud800"],"commit":{"x":"a\udfff"}}
"#,
        expected: Some(r#"{"feed":[{"id":"e1","ts":"2026-09-25T13:00:00+00:00","kind":"event","text":"entrega resultado=['\\ud800'] commit={'x': 'a\\udfff'}","task":null,"jev":null}],"truncated":false,"skipped":0}"#),
        ..EMPTY },
    Row { name: "float-exponent-overflow-text",
        events: r#"{"ts":"2026-09-25T13:00:00+00:00","tipo":"entrega","resultado":1e400,"commit":[-1e400]}
"#,
        expected: Some(r#"{"feed":[{"id":"e1","ts":"2026-09-25T13:00:00+00:00","kind":"event","text":"entrega resultado=inf commit=[-inf]","task":null,"jev":null}],"truncated":false,"skipped":0}"#),
        ..EMPTY },
    Row { name: "float-exponent-overflow-output",
        journal: r"- 2026-09-25T13:00:00+00:00 · notify → arbiter: x
",
        shadow: r#"{"ts":"2026-09-25T13:00:00+00:00","text":"x","mode":1e400}
"#,
        expected: None,
        ..EMPTY },
    Row { name: "serde-magic-key-remains-object",
        events: r#"{"ts": "2026-09-25T13:00:00+00:00", "tipo": "entrega", "resultado": {"$serde_json::private::Number": "123"}, "commit": {"$serde_json::private::Number": "não é número"}}
"#,
        expected: Some(r#"{"feed":[{"id":"e1","ts":"2026-09-25T13:00:00+00:00","kind":"event","text":"entrega resultado={'$serde_json::private::Number': '123'} commit={'$serde_json::private::Number': 'não é número'}","task":null,"jev":null}],"truncated":false,"skipped":0}"#),
        ..EMPTY },
    Row { name: "serde-magic-key-raw-output",
        journal: r"- 2026-09-25T13:00:00+00:00 · notify → arbiter: x
",
        shadow: r#"{"ts": "2026-09-25T13:00:00+00:00", "text": "x", "mode": {"$serde_json::private::Number": "123"}}
"#,
        expected: Some(r#"{"feed":[{"id":"r1","ts":"2026-09-25T13:00:00+00:00","kind":"woke","text":"x","task":null,"jev":{"mode":{"$serde_json::private::Number":"123"},"choice":null,"p":null,"veto":{},"held":[],"would_drop":null,"error":null}}],"truncated":false,"skipped":0}"#),
        ..EMPTY },
    Row { name: "serde-raw-key-remains-object",
        events: r#"{"ts": "2026-09-25T13:00:00+00:00", "tipo": "entrega", "resultado": {"$serde_json::private::RawValue": "123"}}
"#,
        expected: Some(r#"{"feed":[{"id":"e1","ts":"2026-09-25T13:00:00+00:00","kind":"event","text":"entrega resultado={'$serde_json::private::RawValue': '123'}","task":null,"jev":null}],"truncated":false,"skipped":0}"#),
        ..EMPTY },
    Row { name: "serde-raw-key-raw-output",
        journal: r"- 2026-09-25T13:00:00+00:00 · notify → arbiter: x
",
        shadow: r#"{"ts": "2026-09-25T13:00:00+00:00", "text": "x", "mode": {"$serde_json::private::RawValue": "123"}}
"#,
        expected: Some(r#"{"feed":[{"id":"r1","ts":"2026-09-25T13:00:00+00:00","kind":"woke","text":"x","task":null,"jev":{"mode":{"$serde_json::private::RawValue":"123"},"choice":null,"p":null,"veto":{},"held":[],"would_drop":null,"error":null}}],"truncated":false,"skipped":0}"#),
        ..EMPTY },
    Row { name: "surrogate-timestamp-output",
        events: r#"{"ts":"2026-09-25\ud80013:00:00Z","tipo":"entrega"}
"#,
        expected: None,
        ..EMPTY },
    Row { name: "shadow-surrogate-timestamp",
        journal: r"- 2026-09-25T13:00:00+00:00 · notify → arbiter: x
",
        shadow: r#"{"ts":"2026-09-25\ud80013:00:00Z","text":"x","p":0.7}
"#,
        expected: Some(r#"{"feed":[{"id":"r1","ts":"2026-09-25T13:00:00+00:00","kind":"woke","text":"x","task":null,"jev":{"mode":null,"choice":null,"p":0.7,"veto":{},"held":[],"would_drop":null,"error":null}}],"truncated":false,"skipped":0}"#),
        ..EMPTY },
    Row { name: "journal-whitespace-class",
        journal: "- 2026-09-25\u{1f}13:00:00Z · aviso: x\n",
        expected: Some(r#"{"feed":[],"truncated":false,"skipped":1}"#),
        ..EMPTY },
    Row { name: "invalid-unquoted-event-key-NaN-tight",
        events: r#"{"tipo":"entrega","ts":"2026-09-25T13:00:00+00:00",NaN:0}
"#,
        expected: Some(r#"{"feed":[],"truncated":false,"skipped":1}"#),
        ..EMPTY },
    Row { name: "invalid-unquoted-shadow-key-NaN-tight",
        journal: r"- 2026-09-25T13:00:00+00:00 · notify → arbiter: x
",
        shadow: r#"{"ts":"2026-09-25T13:00:00+00:00","text":"x",NaN:0}
"#,
        expected: Some(r#"{"feed":[{"id":"r1","ts":"2026-09-25T13:00:00+00:00","kind":"woke","text":"x","task":null,"jev":null}],"truncated":false,"skipped":1}"#),
        ..EMPTY },
    Row { name: "invalid-unquoted-event-key-NaN-spaced",
        events: "{\"tipo\":\"entrega\",\"ts\":\"2026-09-25T13:00:00+00:00\",NaN \t :0}\n",
        expected: Some(r#"{"feed":[],"truncated":false,"skipped":1}"#),
        ..EMPTY },
    Row { name: "invalid-unquoted-shadow-key-NaN-spaced",
        journal: r"- 2026-09-25T13:00:00+00:00 · notify → arbiter: x
",
        shadow: "{\"ts\":\"2026-09-25T13:00:00+00:00\",\"text\":\"x\",NaN \t :0}\n",
        expected: Some(r#"{"feed":[{"id":"r1","ts":"2026-09-25T13:00:00+00:00","kind":"woke","text":"x","task":null,"jev":null}],"truncated":false,"skipped":1}"#),
        ..EMPTY },
    Row { name: "invalid-unquoted-event-key-Infinity-tight",
        events: r#"{"tipo":"entrega","ts":"2026-09-25T13:00:00+00:00",Infinity:0}
"#,
        expected: Some(r#"{"feed":[],"truncated":false,"skipped":1}"#),
        ..EMPTY },
    Row { name: "invalid-unquoted-shadow-key-Infinity-tight",
        journal: r"- 2026-09-25T13:00:00+00:00 · notify → arbiter: x
",
        shadow: r#"{"ts":"2026-09-25T13:00:00+00:00","text":"x",Infinity:0}
"#,
        expected: Some(r#"{"feed":[{"id":"r1","ts":"2026-09-25T13:00:00+00:00","kind":"woke","text":"x","task":null,"jev":null}],"truncated":false,"skipped":1}"#),
        ..EMPTY },
    Row { name: "invalid-unquoted-event-key-Infinity-spaced",
        events: "{\"tipo\":\"entrega\",\"ts\":\"2026-09-25T13:00:00+00:00\",Infinity \t :0}\n",
        expected: Some(r#"{"feed":[],"truncated":false,"skipped":1}"#),
        ..EMPTY },
    Row { name: "invalid-unquoted-shadow-key-Infinity-spaced",
        journal: r"- 2026-09-25T13:00:00+00:00 · notify → arbiter: x
",
        shadow: "{\"ts\":\"2026-09-25T13:00:00+00:00\",\"text\":\"x\",Infinity \t :0}\n",
        expected: Some(r#"{"feed":[{"id":"r1","ts":"2026-09-25T13:00:00+00:00","kind":"woke","text":"x","task":null,"jev":null}],"truncated":false,"skipped":1}"#),
        ..EMPTY },
    Row { name: "invalid-unquoted-event-key--Infinity-tight",
        events: r#"{"tipo":"entrega","ts":"2026-09-25T13:00:00+00:00",-Infinity:0}
"#,
        expected: Some(r#"{"feed":[],"truncated":false,"skipped":1}"#),
        ..EMPTY },
    Row { name: "invalid-unquoted-shadow-key--Infinity-tight",
        journal: r"- 2026-09-25T13:00:00+00:00 · notify → arbiter: x
",
        shadow: r#"{"ts":"2026-09-25T13:00:00+00:00","text":"x",-Infinity:0}
"#,
        expected: Some(r#"{"feed":[{"id":"r1","ts":"2026-09-25T13:00:00+00:00","kind":"woke","text":"x","task":null,"jev":null}],"truncated":false,"skipped":1}"#),
        ..EMPTY },
    Row { name: "invalid-unquoted-event-key--Infinity-spaced",
        events: "{\"tipo\":\"entrega\",\"ts\":\"2026-09-25T13:00:00+00:00\",-Infinity \t :0}\n",
        expected: Some(r#"{"feed":[],"truncated":false,"skipped":1}"#),
        ..EMPTY },
    Row { name: "invalid-unquoted-shadow-key--Infinity-spaced",
        journal: r"- 2026-09-25T13:00:00+00:00 · notify → arbiter: x
",
        shadow: "{\"ts\":\"2026-09-25T13:00:00+00:00\",\"text\":\"x\",-Infinity \t :0}\n",
        expected: Some(r#"{"feed":[{"id":"r1","ts":"2026-09-25T13:00:00+00:00","kind":"woke","text":"x","task":null,"jev":null}],"truncated":false,"skipped":1}"#),
        ..EMPTY },
    Row { name: "invalid-unquoted-event-key-1e400-tight",
        events: r#"{"tipo":"entrega","ts":"2026-09-25T13:00:00+00:00",1e400:0}
"#,
        expected: Some(r#"{"feed":[],"truncated":false,"skipped":1}"#),
        ..EMPTY },
    Row { name: "invalid-unquoted-shadow-key-1e400-tight",
        journal: r"- 2026-09-25T13:00:00+00:00 · notify → arbiter: x
",
        shadow: r#"{"ts":"2026-09-25T13:00:00+00:00","text":"x",1e400:0}
"#,
        expected: Some(r#"{"feed":[{"id":"r1","ts":"2026-09-25T13:00:00+00:00","kind":"woke","text":"x","task":null,"jev":null}],"truncated":false,"skipped":1}"#),
        ..EMPTY },
    Row { name: "invalid-unquoted-event-key-1e400-spaced",
        events: "{\"tipo\":\"entrega\",\"ts\":\"2026-09-25T13:00:00+00:00\",1e400 \t :0}\n",
        expected: Some(r#"{"feed":[],"truncated":false,"skipped":1}"#),
        ..EMPTY },
    Row { name: "invalid-unquoted-shadow-key-1e400-spaced",
        journal: r"- 2026-09-25T13:00:00+00:00 · notify → arbiter: x
",
        shadow: "{\"ts\":\"2026-09-25T13:00:00+00:00\",\"text\":\"x\",1e400 \t :0}\n",
        expected: Some(r#"{"feed":[{"id":"r1","ts":"2026-09-25T13:00:00+00:00","kind":"woke","text":"x","task":null,"jev":null}],"truncated":false,"skipped":1}"#),
        ..EMPTY },
    Row { name: "invalid-unquoted-event-key--1e400-tight",
        events: r#"{"tipo":"entrega","ts":"2026-09-25T13:00:00+00:00",-1e400:0}
"#,
        expected: Some(r#"{"feed":[],"truncated":false,"skipped":1}"#),
        ..EMPTY },
    Row { name: "invalid-unquoted-shadow-key--1e400-tight",
        journal: r"- 2026-09-25T13:00:00+00:00 · notify → arbiter: x
",
        shadow: r#"{"ts":"2026-09-25T13:00:00+00:00","text":"x",-1e400:0}
"#,
        expected: Some(r#"{"feed":[{"id":"r1","ts":"2026-09-25T13:00:00+00:00","kind":"woke","text":"x","task":null,"jev":null}],"truncated":false,"skipped":1}"#),
        ..EMPTY },
    Row { name: "invalid-unquoted-event-key--1e400-spaced",
        events: "{\"tipo\":\"entrega\",\"ts\":\"2026-09-25T13:00:00+00:00\",-1e400 \t :0}\n",
        expected: Some(r#"{"feed":[],"truncated":false,"skipped":1}"#),
        ..EMPTY },
    Row { name: "invalid-unquoted-shadow-key--1e400-spaced",
        journal: r"- 2026-09-25T13:00:00+00:00 · notify → arbiter: x
",
        shadow: "{\"ts\":\"2026-09-25T13:00:00+00:00\",\"text\":\"x\",-1e400 \t :0}\n",
        expected: Some(r#"{"feed":[{"id":"r1","ts":"2026-09-25T13:00:00+00:00","kind":"woke","text":"x","task":null,"jev":null}],"truncated":false,"skipped":1}"#),
        ..EMPTY },
    Row { name: "invalid-unquoted-event-key-18446744073709551616-tight",
        events: r#"{"tipo":"entrega","ts":"2026-09-25T13:00:00+00:00",18446744073709551616:0}
"#,
        expected: Some(r#"{"feed":[],"truncated":false,"skipped":1}"#),
        ..EMPTY },
    Row { name: "invalid-unquoted-shadow-key-18446744073709551616-tight",
        journal: r"- 2026-09-25T13:00:00+00:00 · notify → arbiter: x
",
        shadow: r#"{"ts":"2026-09-25T13:00:00+00:00","text":"x",18446744073709551616:0}
"#,
        expected: Some(r#"{"feed":[{"id":"r1","ts":"2026-09-25T13:00:00+00:00","kind":"woke","text":"x","task":null,"jev":null}],"truncated":false,"skipped":1}"#),
        ..EMPTY },
    Row { name: "invalid-unquoted-event-key-18446744073709551616-spaced",
        events: "{\"tipo\":\"entrega\",\"ts\":\"2026-09-25T13:00:00+00:00\",18446744073709551616 \t :0}\n",
        expected: Some(r#"{"feed":[],"truncated":false,"skipped":1}"#),
        ..EMPTY },
    Row { name: "invalid-unquoted-shadow-key-18446744073709551616-spaced",
        journal: r"- 2026-09-25T13:00:00+00:00 · notify → arbiter: x
",
        shadow: "{\"ts\":\"2026-09-25T13:00:00+00:00\",\"text\":\"x\",18446744073709551616 \t :0}\n",
        expected: Some(r#"{"feed":[{"id":"r1","ts":"2026-09-25T13:00:00+00:00","kind":"woke","text":"x","task":null,"jev":null}],"truncated":false,"skipped":1}"#),
        ..EMPTY },
    Row { name: "invalid-unquoted-event-key--9223372036854775809-tight",
        events: r#"{"tipo":"entrega","ts":"2026-09-25T13:00:00+00:00",-9223372036854775809:0}
"#,
        expected: Some(r#"{"feed":[],"truncated":false,"skipped":1}"#),
        ..EMPTY },
    Row { name: "invalid-unquoted-shadow-key--9223372036854775809-tight",
        journal: r"- 2026-09-25T13:00:00+00:00 · notify → arbiter: x
",
        shadow: r#"{"ts":"2026-09-25T13:00:00+00:00","text":"x",-9223372036854775809:0}
"#,
        expected: Some(r#"{"feed":[{"id":"r1","ts":"2026-09-25T13:00:00+00:00","kind":"woke","text":"x","task":null,"jev":null}],"truncated":false,"skipped":1}"#),
        ..EMPTY },
    Row { name: "invalid-unquoted-event-key--9223372036854775809-spaced",
        events: "{\"tipo\":\"entrega\",\"ts\":\"2026-09-25T13:00:00+00:00\",-9223372036854775809 \t :0}\n",
        expected: Some(r#"{"feed":[],"truncated":false,"skipped":1}"#),
        ..EMPTY },
    Row { name: "invalid-unquoted-shadow-key--9223372036854775809-spaced",
        journal: r"- 2026-09-25T13:00:00+00:00 · notify → arbiter: x
",
        shadow: "{\"ts\":\"2026-09-25T13:00:00+00:00\",\"text\":\"x\",-9223372036854775809 \t :0}\n",
        expected: Some(r#"{"feed":[{"id":"r1","ts":"2026-09-25T13:00:00+00:00","kind":"woke","text":"x","task":null,"jev":null}],"truncated":false,"skipped":1}"#),
        ..EMPTY },
    Row { name: "invalid-nested-unquoted-keys",
        events: r#"{"tipo":"entrega","ts":"2026-09-25T13:00:00+00:00","resultado":[{NaN : 0}]}
{"tipo":"entrega","ts":"2026-09-25T13:00:00+00:00","resultado":[{Infinity : 0}]}
{"tipo":"entrega","ts":"2026-09-25T13:00:00+00:00","resultado":[{-Infinity : 0}]}
{"tipo":"entrega","ts":"2026-09-25T13:00:00+00:00","resultado":[{1e400 : 0}]}
{"tipo":"entrega","ts":"2026-09-25T13:00:00+00:00","resultado":[{-1e400 : 0}]}
{"tipo":"entrega","ts":"2026-09-25T13:00:00+00:00","resultado":[{18446744073709551616 : 0}]}
{"tipo":"entrega","ts":"2026-09-25T13:00:00+00:00","resultado":[{-9223372036854775809 : 0}]}
"#,
        expected: Some(r#"{"feed":[],"truncated":false,"skipped":7}"#),
        ..EMPTY },
    Row { name: "invalid-shadow-veto-unquoted-keys",
        journal: r"- 2026-09-25T13:00:00+00:00 · notify → arbiter: x
",
        shadow: r#"{"ts":"2026-09-25T13:00:00+00:00","text":"x","veto":{NaN : 0.9}}
{"ts":"2026-09-25T13:00:00+00:00","text":"x","veto":{Infinity : 0.9}}
{"ts":"2026-09-25T13:00:00+00:00","text":"x","veto":{-Infinity : 0.9}}
{"ts":"2026-09-25T13:00:00+00:00","text":"x","veto":{1e400 : 0.9}}
{"ts":"2026-09-25T13:00:00+00:00","text":"x","veto":{-1e400 : 0.9}}
{"ts":"2026-09-25T13:00:00+00:00","text":"x","veto":{18446744073709551616 : 0.9}}
{"ts":"2026-09-25T13:00:00+00:00","text":"x","veto":{-9223372036854775809 : 0.9}}
"#,
        expected: Some(r#"{"feed":[{"id":"r1","ts":"2026-09-25T13:00:00+00:00","kind":"woke","text":"x","task":null,"jev":null}],"truncated":false,"skipped":7}"#),
        ..EMPTY },
    Row { name: "quoted-special-keys-and-values-stay-valid",
        events: r#"{"tipo":"entrega","ts":"2026-09-25T13:00:00+00:00","resultado":{"NaN":NaN,"Infinity":Infinity,"-Infinity":-Infinity,"18446744073709551616":18446744073709551616}}
"#,
        expected: Some(r#"{"feed":[{"id":"e1","ts":"2026-09-25T13:00:00+00:00","kind":"event","text":"entrega resultado={'NaN': nan, 'Infinity': inf, '-Infinity': -inf, '18446744073709551616': 18446744073709551616}","task":null,"jev":null}],"truncated":false,"skipped":0}"#),
        ..EMPTY },
    Row { name: "quoted-shadow-special-keys-stay-valid",
        journal: r"- 2026-09-25T13:00:00+00:00 · notify → arbiter: x
",
        shadow: r#"{"ts": "2026-09-25T13:00:00+00:00", "text": "x", "veto": {"NaN": 0.4, "Infinity": 0.5, "-Infinity": null, "18446744073709551616": 0.1}}
"#,
        expected: Some(r#"{"feed":[{"id":"r1","ts":"2026-09-25T13:00:00+00:00","kind":"woke","text":"x","task":null,"jev":{"mode":null,"choice":null,"p":null,"veto":{"NaN":0.4,"Infinity":0.5,"-Infinity":null,"18446744073709551616":0.1},"held":["Infinity","-Infinity"],"would_drop":null,"error":null}}],"truncated":false,"skipped":0}"#),
        ..EMPTY },
    Row { name: "type-list",
        events: r#"{"ts": "2026-09-25T13:00:00+00:00", "tipo": []}
"#,
        expected: None,
        ..EMPTY },
    Row { name: "num-overflow",
        journal: r"- 2026-09-25T13:00:00+00:00 · notify → arbiter: x
",
        shadow: r#"{"ts": "2026-09-25T13:00:00+00:00", "text": "x", "p": 10000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000}
"#,
        expected: None,
        ..EMPTY },
];
