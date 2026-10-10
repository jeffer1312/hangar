//! A lista montada no Rust contra as entradas e saídas gravadas do Python (`gen_list.py`).
use crate::common;

use std::path::Path;

use common::golden;
use hangar_api::session::SessionRow;
use hangar_server::list::reply::ReplyCache;
use serde_json::Value;

/// `sanitize_cwd` (registry.py:209) para a pasta do caso, que não termina em separador.
fn sanitize(path: &str) -> String {
    path.chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '-' }).collect()
}

/// Troca os marcadores das convenções do golden pela pasta do caso.
fn placed(v: &Value, root: &Path) -> Value {
    // Barra normal: a raiz entra em texto JSON, e a barra invertida do Windows viraria escape.
    let root = &root.to_str().expect("pasta temporária em UTF-8").replace('\\', "/");
    let text = serde_json::to_string(v).unwrap();
    let text = text.replace("⟦ROOT_SAN⟧", &sanitize(root)).replace("⟦ROOT⟧", root);
    serde_json::from_str(&text).unwrap()
}

/// Só o conteúdo dos arquivos importa aqui: o relógio do transcript chega pelo `last_activity`.
fn apply_fs(ops: &[Value]) {
    for op in ops {
        let path = Path::new(op["path"].as_str().unwrap());
        match op["op"].as_str().unwrap() {
            "write" => {
                std::fs::create_dir_all(path.parent().unwrap()).unwrap();
                std::fs::write(path, op["text"].as_str().unwrap()).unwrap();
            }
            "rm" => std::fs::remove_file(path).unwrap(),
            "mkdir" => std::fs::create_dir_all(path).unwrap(),
            "touch" => {}
            other => panic!("op de arquivo desconhecida: {other}"),
        }
    }
}

#[test]
fn last_reply_cases() {
    let g = golden("list_decorate.json");
    let mut seen = std::collections::BTreeSet::new();
    for case in g["cases"].as_array().unwrap() {
        let name = case["name"].as_str().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let mut cache = ReplyCache::default();
        for (i, tick) in case["ticks"].as_array().unwrap().iter().enumerate() {
            let tick = placed(tick, dir.path());
            apply_fs(tick["fs"].as_array().map_or(&[][..], Vec::as_slice));
            let want: Vec<SessionRow> = serde_json::from_value(tick["expected"]["rows"].clone()).unwrap();
            let mut rows = want.clone();
            for row in &mut rows {
                row.last_reply = Some("sobra do tique anterior".into());
                row.last_reply_at = Some(1.0);
            }
            let failures = cache.decorate(&mut rows, |_| None);
            assert!(failures.is_empty(), "{name} tique {i}: {failures:?}");
            for (got, want) in rows.iter().zip(&want) {
                assert_eq!(
                    (&got.last_reply, got.last_reply_at),
                    (&want.last_reply, want.last_reply_at),
                    "{name} tique {i}: {}",
                    want.name
                );
                if want.last_reply.is_some() {
                    seen.insert((want.provider.clone(), want.headless));
                }
            }
        }
    }
    // Claude com e sem terminal e Codex passaram por aqui com resposta de verdade.
    for kind in [("claude".to_string(), false), ("claude".to_string(), true), ("codex".to_string(), false)] {
        assert!(seen.contains(&kind), "golden sem resposta de {kind:?}");
    }
}

#[test]
fn last_reply_plain_text_like_python() {
    use hangar_server::list::reply::plain_text;
    // Saída do `archive._texto_simples` para a mesma entrada.
    let md = "## Título\n\n- **item** com [link](http://x) e `código`\n   > citação\n1. um\n\x1cfim";
    assert_eq!(plain_text(md), "Título item com link e código citação um fim");
}

#[test]
fn last_reply_cache_by_transcript_and_mtime() {
    let dir = tempfile::tempdir().unwrap();
    let jsonl = dir.path().join("s.jsonl");
    let line = |text: &str| {
        serde_json::json!({"type": "assistant", "timestamp": "2027-01-15T08:00:05.000Z",
            "message": {"role": "assistant", "content": [{"type": "text", "text": text}]}})
        .to_string()
            + "\n"
    };
    std::fs::write(&jsonl, line(&"ç".repeat(200))).unwrap();
    let mut row: SessionRow = serde_json::from_value(serde_json::json!({
        "name": "s", "jsonl": jsonl.to_str().unwrap(), "last_activity": 10.0})).unwrap();
    let mut cache = ReplyCache::default();
    assert!(cache.decorate(std::slice::from_mut(&mut row), |_| None).is_empty());
    // O corte de 160 é por caractere, como o fatiamento do Python.
    assert_eq!(row.last_reply.as_deref(), Some("ç".repeat(160).as_str()));

    // Mesmo transcript e mtime: não relê.
    std::fs::write(&jsonl, line("nova")).unwrap();
    assert!(cache.decorate(std::slice::from_mut(&mut row), |_| None).is_empty());
    assert_eq!(row.last_reply.as_deref(), Some("ç".repeat(160).as_str()));

    // mtime novo: relê.
    row.last_activity = Some(11.0);
    assert!(cache.decorate(std::slice::from_mut(&mut row), |_| None).is_empty());
    assert_eq!(row.last_reply.as_deref(), Some("nova"));

    // Trabalhando: sem resposta, e a linha de provedor que o Rust não lê fica como veio.
    row.state = "working".into();
    let mut pi: SessionRow = serde_json::from_value(serde_json::json!({
        "name": "p", "provider": "pi", "jsonl": "/x", "last_reply": "do python"})).unwrap();
    assert!(cache.decorate(std::slice::from_mut(&mut row), |_| None).is_empty());
    assert!(cache.decorate(std::slice::from_mut(&mut pi), |_| None).is_empty());
    assert_eq!((row.last_reply, row.last_reply_at), (None, None));
    assert_eq!(pi.last_reply.as_deref(), Some("do python"));
}

#[test]
fn last_reply_read_failure_surfaces_without_cache() {
    let dir = tempfile::tempdir().unwrap();
    // Um diretório no lugar do transcript: abrir dá certo, ler não.
    let mut row: SessionRow = serde_json::from_value(serde_json::json!({
        "name": "s", "jsonl": dir.path().to_str().unwrap(), "last_activity": 10.0,
        "last_reply": "velha", "last_reply_at": 1.0})).unwrap();
    let mut cache = ReplyCache::default();
    let failures = cache.decorate(std::slice::from_mut(&mut row), |_| None);
    assert_eq!(failures.iter().map(|f| f.0.as_str()).collect::<Vec<_>>(), ["s"]);
    assert_eq!((row.last_reply.as_deref(), row.last_reply_at), (None, None));
    // Sem cache: o próximo tique tenta de novo.
    assert_eq!(cache.decorate(std::slice::from_mut(&mut row), |_| None).len(), 1);
}

#[test]
fn last_reply_forget_and_rename() {
    let dir = tempfile::tempdir().unwrap();
    let jsonl = dir.path().join("s.jsonl");
    let line = |text: &str| {
        serde_json::json!({"type": "assistant", "message": {"role": "assistant",
            "content": [{"type": "text", "text": text}]}})
        .to_string()
            + "\n"
    };
    std::fs::write(&jsonl, line("antes")).unwrap();
    let row = |name: &str| -> SessionRow {
        serde_json::from_value(serde_json::json!({
            "name": name, "jsonl": jsonl.to_str().unwrap(), "last_activity": 10.0})).unwrap()
    };
    let mut cache = ReplyCache::default();
    let mut a = row("a");
    assert!(cache.decorate(std::slice::from_mut(&mut a), |_| None).is_empty());
    // Sem relógio na linha, `last_reply_at` cai no `last_activity`.
    assert_eq!((a.last_reply.as_deref(), a.last_reply_at), (Some("antes"), Some(10.0)));
    std::fs::write(&jsonl, line("depois")).unwrap();

    cache.rename("a", "b");
    let mut b = row("b");
    assert!(cache.decorate(std::slice::from_mut(&mut b), |_| None).is_empty());
    assert_eq!(b.last_reply.as_deref(), Some("antes"));

    cache.forget("b");
    assert!(cache.decorate(std::slice::from_mut(&mut b), |_| None).is_empty());
    assert_eq!(b.last_reply.as_deref(), Some("depois"));
}
