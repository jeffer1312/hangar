use hangar_server::costs::pricing::{Pricing, canonizar_provedor, custo};
use hangar_server::costs::py::{LocalTs, char_len, parse_obj, py_int};
use serde_json::{Value, json};
use std::path::Path;

fn contract() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../backend/tests/fixtures/contract")
}

fn rate_json(r: Option<hangar_server::costs::pricing::Rate>) -> Value {
    r.map_or(Value::Null, |r| json!({"input": r.input, "output": r.output, "cache_read": r.cache_read,
        "cache_write": r.cache_write, "provider": r.provider, "origin": r.origin, "cache_estimado": r.cache_estimado}))
}

#[test]
fn pricing_matches_python() {
    let p = Pricing::load(&contract().join("costs/pricing"));
    let rows: Value = serde_json::from_slice(&std::fs::read(contract().join("golden/costs_pricing.json")).unwrap()).unwrap();
    for row in rows.as_array().unwrap() {
        let m = row["model"].as_str().unwrap();
        assert_eq!(p.canonizar(m), row["canon"].as_str().unwrap(), "canon {m}");
        let r = p.rate_for(m);
        assert_eq!(rate_json(r.clone()), row["rate"], "rate {m}");
        assert_eq!(rate_json(r.as_ref().map(|r| p.rate_fast(r, m))), row["fast"], "fast {m}");
        assert_eq!(rate_json(r.as_ref().map(|r| p.rate_codex(r, m, true))), row["codex_long"], "codex {m}");
    }
}

#[test]
fn snapshot_is_used_without_cache_dir() {
    let dir = tempfile::tempdir().unwrap();
    let p = Pricing::load(&dir.path().join("missing"));
    assert_eq!(p.rate_for("claude-opus-5").unwrap().origin, "snapshot");
}

#[test]
fn py_int_follows_python_int_or_zero() {
    for (v, want) in [(json!(null), 0), (json!(false), 0), (json!(true), 1), (json!(7), 7), (json!(7.9), 7),
                      (json!(-2.5), -2), (json!(" 12 "), 12), (json!("-3"), -3), (json!("3.5"), 0), (json!(""), 0),
                      (json!([1]), 0), (json!({"a": 1}), 0), (json!(u64::MAX), i64::MAX),
                      (json!("9999999999999999999999999"), i64::MAX),
                      (json!("-9999999999999999999999999"), i64::MIN),
                      (json!("1_000"), 1000), (json!("１２"), 12), (json!("-١٢"), -12),
                      (json!("1__0"), 0), (json!("1_"), 0), (json!("_1"), 0),
                      (json!("9999999999999999999999999x"), 0)] {
        assert_eq!(py_int(Some(&v)), want, "{v}");
    }
    assert_eq!(py_int(None), 0);
}

#[test]
fn local_time_matches_python_isoformat() {
    let t = LocalTs::from_iso("2026-09-30T02:30:00.120Z").unwrap();
    assert_eq!(t.iso(), "2026-09-29T23:30:00.120000-03:00");
    assert_eq!(t.day(), "2026-09-29");
    let encoded = serde_json::to_value(t).unwrap();
    assert_eq!(serde_json::from_value::<LocalTs>(encoded).unwrap(), t);
    assert_eq!(LocalTs::from_iso("2026-09-30T10:00:00Z").unwrap().iso(), "2026-09-30T07:00:00-03:00");
    assert_eq!(LocalTs::from_millis_f64(1_790_935_200_123.4).iso(), "2026-10-02T07:00:00.123400-03:00");
    assert!(LocalTs::from_iso("ontem").is_none());
    for raw in ["2026-09-30T10:00:00.000001", "20260930 100000.000001"] {
        assert_eq!(LocalTs::from_iso(raw).unwrap().iso(), "2026-09-30T10:00:00.000001-03:00");
    }
    assert_eq!(LocalTs::from_iso("2026-09-30").unwrap().iso(), "2026-09-30T00:00:00-03:00");
    assert_eq!(LocalTs::from_iso("2026-09-30T10:00:00+01:30:00.000001").unwrap().iso(), "2026-09-30T05:29:59.999999-03:00");
    for (ms, micros) in [(0.0005, 0), (0.0015, 2), (-0.0005, 0), (-0.0015, -2),
                         (-9.9935, -9993), (-9.9905, -9991)] {
        assert_eq!(LocalTs::from_millis_f64(ms).0, micros);
    }
}

#[test]
fn rate_text_numbers_accept_python_float_syntax() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("models.dev.json"),
        r#"{"modelos":{"numeric":{"input":" 1_0.5 ","output":"２","cache_read":"٠.١","cache_write":false}}}"#).unwrap();
    let p = Pricing::load(dir.path());
    let rate = p.rate_for("numeric").unwrap();
    assert_eq!(rate.input, 10.5);
    assert_eq!(rate.output, 2.0);
    assert_eq!(rate.cache_read, 0.1);
    assert_eq!(rate.cache_write, 0.0);
    assert!(!rate.cache_estimado);
}

#[test]
fn local_time_accepts_python_iso_week_dates() {
    for (raw, expected) in [
        ("2026-W40-5", "2026-10-02T00:00:00-03:00"),
        ("2026W405", "2026-10-02T00:00:00-03:00"),
        ("2026-W40", "2026-09-28T00:00:00-03:00"),
        ("2026W40", "2026-09-28T00:00:00-03:00"),
        ("2026-W40-5T10:00:00+00:00", "2026-10-02T07:00:00-03:00"),
        ("2026W405T10:00:00Z", "2026-10-02T07:00:00-03:00"),
        ("2026W405T10:00:00", "2026-10-02T10:00:00-03:00"),
        ("2026-W40T10:00:00.000001", "2026-09-28T10:00:00.000001-03:00"),
        ("2026W40T10:00:00.000001-03:00", "2026-09-28T10:00:00.000001-03:00"),
        ("2020-W53-7", "2021-01-03T00:00:00-03:00"),
        ("2020W537", "2021-01-03T00:00:00-03:00"),
        ("0001-W01-1", "0001-01-01T00:00:00-03:00"),
    ] {
        assert_eq!(LocalTs::from_iso(raw).map(|t| t.iso()).as_deref(), Some(expected), "{raw}");
    }
    for raw in ["2021-W53-1", "2026-W00-1", "2026-W54-1", "2026-W40-0", "2026-W40-8",
                "2026W400", "2026W408", "2026-W401", "2026W40-1", "9999-W52-7"] {
        assert!(LocalTs::from_iso(raw).is_none(), "{raw}");
    }
}

#[test]
fn number_whitespace_matches_python_conversions() {
    let dir = tempfile::tempdir().unwrap();
    let mut models = serde_json::Map::new();
    for code in 0x1c..=0x1f {
        let control = char::from_u32(code).unwrap();
        let raw = format!("{control}12{control}");
        assert_eq!(py_int(Some(&json!(raw))), 0, "controle {code:#x}");
        models.insert(format!("invalid-{code}"), json!({"input": raw, "output": 2}));
    }
    for (index, raw) in ["\t12\r\n", "\u{a0}12\u{a0}", "\u{2003}１２\u{2003}", "\u{202f}1_2\u{202f}"].into_iter().enumerate() {
        assert_eq!(py_int(Some(&json!(raw))), 12, "{raw:?}");
        models.insert(format!("valid-{index}"), json!({"input": raw, "output": 2}));
    }
    std::fs::write(dir.path().join("models.dev.json"), serde_json::to_vec(&json!({"modelos": models})).unwrap()).unwrap();
    let p = Pricing::load(dir.path());
    for code in 0x1c..=0x1f {
        assert!(p.rate_for(&format!("invalid-{code}")).is_none(), "controle float {code:#x}");
    }
    for index in 0..4 {
        assert_eq!(p.rate_for(&format!("valid-{index}")).unwrap().input, 12.0);
    }
}

#[test]
fn iso_week_separators_follow_python_disambiguation() {
    for (raw, expected) in [
        ("2026W40810:00", "2026-09-28T10:00:00-03:00"),
        ("2026W40810", "2026-09-28T10:00:00-03:00"),
        ("2026W40810:00Z", "2026-09-28T07:00:00-03:00"),
        ("2026W405810:00", "2026-10-02T10:00:00-03:00"),
        ("2026W4010000", "2026-09-28T00:00:00-03:00"),
        ("2026W405100000", "2026-09-28T10:00:00-03:00"),
        ("2026-W40-1000", "2026-09-28T10:00:00-03:00"),
        ("2026-W40-10:00", "2026-09-28T10:00:00-03:00"),
        ("2026W40水10:00", "2026-09-28T10:00:00-03:00"),
        ("2026W405水10:00", "2026-10-02T10:00:00-03:00"),
        ("2026-W40水10:00", "2026-09-28T10:00:00-03:00"),
        ("2026-W40-5水10:00", "2026-10-02T10:00:00-03:00"),
    ] {
        assert_eq!(LocalTs::from_iso(raw).map(|t| t.iso()).as_deref(), Some(expected), "{raw}");
    }
    for raw in ["2026W408", "2026W4010", "2026-W40-", "2026-W40-510:00"] {
        assert!(LocalTs::from_iso(raw).is_none(), "{raw}");
    }
}

#[test]
fn line_objects_preserve_python_character_counts() {
    let obj = parse_obj(b" \t{\"text\":\"a\\ud800\\ud83d\\ude00\"}\r\n").unwrap();
    assert_eq!(char_len(obj["text"].as_str().unwrap()), 3);
    assert_eq!(char_len("á水𐀀"), 3);
    assert_eq!(parse_obj(b"{\"text\":\"\xff\"}").unwrap()["text"], "�");
    assert_eq!(parse_obj(b"\x0b{\"x\":1}\x0b").unwrap()["x"], 1);
    assert!(parse_obj("\u{a0}{\"x\":1}\u{a0}".as_bytes()).is_none());
    for raw in [b"".as_slice(), b"[]", b"null", b"{\"x\":"] {
        assert!(parse_obj(raw).is_none());
    }
}

#[test]
fn pricing_reload_invalidates_both_memos_and_keeps_alias_order() {
    let dir = tempfile::tempdir().unwrap();
    let catalog = dir.path().join("models.dev.json");
    let overrides = dir.path().join("overrides.json");
    std::fs::write(&catalog, r#"{"modelos":{"MiXeD":{"input":"1","output":"2","provider":"openai"},"mixed":{"input":9,"output":9}}}"#).unwrap();
    let mut p = Pricing::load(dir.path());
    assert_eq!(p.canonizar("MIXED"), "MiXeD");
    let initial = p.rate_for("MIXED").unwrap();
    assert_eq!(initial.cache_write, 0.0);
    assert!(initial.cache_estimado);
    assert_eq!(p.provider_for("MIXED").as_deref(), Some("openai"));
    assert_eq!(custo(&initial, 1_000_000, 2_000_000, 3_000_000, 4_000_000), [1.0, 4.0, 0.0, 4.0]);
    let generation = p.generation();
    assert!(!p.reload_if_changed());
    std::fs::write(&overrides, r#"{"MiXeD":{"input":"4","output":"8","provider":"custom"}}"#).unwrap();
    assert!(p.reload_if_changed());
    assert_eq!(p.generation(), generation + 1);
    assert_eq!(p.rate_for("MIXED").unwrap().input, 4.0);
    assert_eq!(p.rate_for("MIXED").unwrap().origin, "override");
    std::fs::write(&catalog, r#"{"modelos":{"mixed":{"input":3,"output":6,"provider":"openai","cache_read":0.3}}}"#).unwrap();
    // No Windows mudar a data exige o arquivo aberto para escrita; só leitura dá "Access is denied".
    std::fs::OpenOptions::new().write(true).open(&catalog).unwrap().set_modified(std::time::SystemTime::UNIX_EPOCH).unwrap();
    assert!(p.reload_if_changed());
    assert_eq!(p.canonizar("MIXED"), "mixed");
    assert_eq!(p.rate_for("MIXED").unwrap().input, 3.0);
    std::fs::remove_file(&catalog).unwrap();
    assert!(p.reload_if_changed());
    assert_eq!(p.rate_for("claude-opus-5").unwrap().origin, "snapshot");
    assert_eq!(canonizar_provedor(" openai-codex "), "openai");
    assert_eq!(canonizar_provedor("moonshot"), "moonshotai");
    assert_eq!(canonizar_provedor("anthropic:abc"), "anthropic:abc");
}

// ── R1.6: download das tarifas (contract/r1-pricing) ─────────────────────────────────────────────

mod refresh {
    use hangar_server::costs::pricing_refresh::{PROVIDERS, PricingRefresh, excerpt, slim, versioned_text, write_snapshot};
    use serde_json::{Value, json};
    use std::path::{Path, PathBuf};
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, SystemTime, UNIX_EPOCH};

    fn fixtures() -> PathBuf { Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/it/fixtures/pricing") }
    fn recorte() -> Vec<u8> { std::fs::read(fixtures().join("models_dev_recorte.json")).unwrap() }
    /// `~/.claude/.hangar-pricing/models.dev.json` que o Python gravou a partir do recorte (bg_no_cache_200).
    fn python_cache() -> String { std::fs::read_to_string(fixtures().join("models_dev_cache.json")).unwrap() }
    /// O cache antigo dos casos `stale`, com mtime 2026-01-01.
    const SEEDED: &str = r#"{"modelos": {"seeded-model": {"provider": "anthropic", "input": 1, "output": 2, "cache_read": null, "cache_write": null}}}"#;
    const JAN_1_2026: u64 = 1_767_225_600;

    #[derive(Clone, Copy)]
    enum Mode { Strict, Body(&'static str), Status(u16) }

    /// models.dev falso em loopback; `Strict` imita o real: 403 sem o UA do Hangar, 304 com `If-Modified-Since`.
    struct Fake { url: String, seen: Arc<Mutex<Vec<(Option<String>, Option<String>)>>> }

    async fn fake(mode: Mode) -> Fake {
        let seen: Arc<Mutex<Vec<(Option<String>, Option<String>)>>> = Arc::default();
        let log = seen.clone();
        let body = recorte();
        let app = axum::Router::new().route("/api.json", axum::routing::get(move |headers: axum::http::HeaderMap| {
            let (log, body) = (log.clone(), body.clone());
            async move {
                let header = |name: &str| headers.get(name).and_then(|v| v.to_str().ok()).map(str::to_owned);
                let (agent, since) = (header("user-agent"), header("if-modified-since"));
                log.lock().unwrap().push((agent.clone(), since.clone()));
                let status = |code: u16| axum::http::StatusCode::from_u16(code).unwrap();
                match mode {
                    Mode::Strict if !agent.as_deref().is_some_and(|a| a.starts_with("hangar/")) => (status(403), Vec::new()),
                    Mode::Strict if since.is_some() => (status(304), Vec::new()),
                    Mode::Strict => (status(200), body),
                    Mode::Body(text) => (status(200), text.as_bytes().to_vec()),
                    Mode::Status(code) => (status(code), b"{}".to_vec()),
                }
            }
        }));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/api.json", listener.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        Fake { url, seen }
    }

    #[derive(Clone, Default)]
    struct Log(Arc<Mutex<Vec<u8>>>);
    impl std::io::Write for Log {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> { self.0.lock().unwrap().extend_from_slice(bytes); Ok(bytes.len()) }
        fn flush(&mut self) -> std::io::Result<()> { Ok(()) }
    }
    impl Log {
        fn text(&self) -> String { String::from_utf8_lossy(&self.0.lock().unwrap()).into_owned() }
    }

    struct Run { dir: tempfile::TempDir, fake: Fake, log: Log }
    impl Run {
        fn cache(&self) -> PathBuf { self.dir.path().join("models.dev.json") }
        fn text(&self) -> Option<String> { std::fs::read_to_string(self.cache()).ok() }
        fn mtime(&self) -> Option<SystemTime> { std::fs::metadata(self.cache()).and_then(|m| m.modified()).ok() }
        fn requests(&self) -> Vec<(Option<String>, Option<String>)> { self.fake.seen.lock().unwrap().clone() }
    }

    /// `seed`: cache antigo (2026-01-01) ou recém-gravado; `times`: chamadas no mesmo processo.
    async fn run(mode: Mode, seed: Option<bool>, times: usize) -> Run {
        let fake = fake(mode).await;
        let dir = tempfile::tempdir().unwrap();
        if let Some(stale) = seed {
            let path = dir.path().join("models.dev.json");
            std::fs::write(&path, SEEDED).unwrap();
            if stale {
                std::fs::OpenOptions::new().write(true).open(&path).unwrap().set_modified(UNIX_EPOCH + Duration::from_secs(JAN_1_2026)).unwrap();
            }
        }
        let log = Log::default();
        let writer = log.clone();
        let _guard = tracing::subscriber::set_default(tracing_subscriber::fmt().with_ansi(false).with_writer(move || writer.clone()).finish());
        let refresh = PricingRefresh::new(dir.path().to_owned(), fake.url.clone());
        for _ in 0..times { refresh.run().await; }
        Run { dir, fake, log }
    }

    fn ua() -> Option<String> { Some("hangar/pricing".into()) }
    fn jan_1() -> Option<String> { Some("Thu, 01 Jan 2026 00:00:00 GMT".into()) }

    #[tokio::test]
    async fn first_download_writes_the_slim_catalog_like_python() {
        let r = run(Mode::Strict, None, 1).await;
        assert_eq!(r.requests(), [(ua(), None)]);
        assert_eq!(r.text().unwrap(), python_cache(), "mesmo arquivo, byte a byte");
        #[cfg(unix)]
        { use std::os::unix::fs::PermissionsExt; assert_eq!(std::fs::metadata(r.cache()).unwrap().permissions().mode() & 0o777, 0o644); }
        assert_eq!(r.log.text().matches("tarifas: 80 modelos atualizados do models.dev").count(), 1, "{}", r.log.text());
    }

    #[tokio::test]
    async fn fresh_cache_asks_nothing() {
        let r = run(Mode::Strict, Some(false), 1).await;
        assert!(r.requests().is_empty());
        assert_eq!(r.text().as_deref(), Some(SEEDED));
    }

    #[tokio::test]
    async fn stale_cache_sends_its_mtime_and_304_only_touches_it() {
        let r = run(Mode::Strict, Some(true), 1).await;
        assert_eq!(r.requests(), [(ua(), jan_1())]);
        assert_eq!(r.text().as_deref(), Some(SEEDED));
        assert!(r.mtime().unwrap().duration_since(SystemTime::now() - Duration::from_secs(60)).is_ok(), "o mtime virou agora");
        assert!(!r.log.text().contains("tarifas"), "{}", r.log.text());
    }

    #[tokio::test]
    async fn stale_cache_is_replaced_by_a_new_catalog() {
        let r = run(Mode::Body(std::str::from_utf8(&recorte()).unwrap().to_owned().leak()), Some(true), 1).await;
        assert_eq!(r.requests(), [(ua(), jan_1())]);
        assert_eq!(r.text().unwrap(), python_cache());
    }

    #[tokio::test]
    async fn broken_or_unknown_answers_keep_the_cache() {
        let cases = [
            (Mode::Body("{garbage"), "falha ao baixar models.dev"),
            (Mode::Body("[1, 2]"), "formato do models.dev não reconhecido"),
            (Mode::Body(r#"{"anthropic": {"models": [1]}}"#), "formato do models.dev não reconhecido"),
            (Mode::Status(403), "models.dev respondeu 403"),
            (Mode::Status(500), "models.dev respondeu 500"),
        ];
        for (mode, warning) in cases {
            let r = run(mode, Some(true), 1).await;
            assert_eq!(r.text().as_deref(), Some(SEEDED), "{warning}");
            assert_eq!(r.mtime(), Some(UNIX_EPOCH + Duration::from_secs(JAN_1_2026)), "{warning}");
            assert!(r.log.text().contains("WARN") && r.log.text().contains(warning), "{}", r.log.text());
            let r = run(mode, None, 1).await;
            assert_eq!(r.text(), None, "{warning}");
        }
    }

    /// D13: JSON válido sem modelo conhecido (`{}`, `{"anthropic": 5}`) conserva a tabela anterior e avisa;
    /// o Python gravava um catálogo vazio por cima.
    #[tokio::test]
    async fn valid_answer_without_known_models_keeps_the_previous_table() {
        for body in ["{}", r#"{"anthropic": 5}"#] {
            let r = run(Mode::Body(body), Some(true), 1).await;
            assert_eq!(r.text().as_deref(), Some(SEEDED), "{body}");
            assert_eq!(r.mtime(), Some(UNIX_EPOCH + Duration::from_secs(JAN_1_2026)), "{body}");
            assert!(r.log.text().contains("models.dev sem modelo conhecido; tabela anterior mantida"), "{}", r.log.text());
            assert_eq!(run(Mode::Body(body), None, 1).await.text(), None, "{body}: sem cache, fica o snapshot");
        }
    }

    #[tokio::test]
    async fn connection_refused_warns_and_writes_nothing() {
        let closed = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/api.json", closed.local_addr().unwrap());
        drop(closed);
        let dir = tempfile::tempdir().unwrap();
        let log = Log::default();
        let writer = log.clone();
        let _guard = tracing::subscriber::set_default(tracing_subscriber::fmt().with_ansi(false).with_writer(move || writer.clone()).finish());
        PricingRefresh::new(dir.path().to_owned(), url).run().await;
        assert!(log.text().contains("tarifas: falha ao baixar models.dev"), "{}", log.text());
        assert!(!dir.path().join("models.dev.json").exists());
    }

    #[tokio::test]
    async fn one_attempt_per_day_after_failure_or_success() {
        assert_eq!(run(Mode::Status(500), None, 2).await.requests().len(), 1);
        let r = run(Mode::Body(std::str::from_utf8(&recorte()).unwrap().to_owned().leak()), None, 2).await;
        assert_eq!(r.requests().len(), 1);
        assert!(r.text().is_some());
    }

    #[test]
    fn only_the_exact_one_turns_the_download_off() {
        use std::ffi::OsStr;
        assert!(PricingRefresh::offline_value(Some(OsStr::new("1"))));
        for value in [None, Some("true"), Some("0"), Some(" 1"), Some("")] {
            assert!(!PricingRefresh::offline_value(value.map(OsStr::new)), "{value:?}");
        }
    }

    /// slim_of_fixture: 80 entradas, chaves `id` e `provedor/id`, só os provedores de primeira mão.
    #[test]
    fn slim_of_the_fixture_matches_python() {
        let raw: Value = serde_json::from_slice(&recorte()).unwrap();
        let expected: Value = serde_json::from_str(&python_cache()).unwrap();
        let models = slim(&raw).unwrap();
        assert_eq!(models.len(), 80);
        assert_eq!(models["kimi-k3"]["provider"], "moonshotai");
        assert!(!models.contains_key("k3"), "modelo de revenda com preço zero não entra");
        assert_eq!(Value::Object(models), expected["modelos"]);
        assert_eq!(PROVIDERS, ["anthropic", "openai", "moonshotai", "zhipuai", "deepseek", "google", "xai", "mistral"]);
    }

    #[test]
    fn slim_follows_the_python_rules() {
        let raw = json!({
            "anthropic": {"models": {
                "sem-input": {"cost": {"output": 5}},
                "sem-output": {"cost": {"input": 5}},
                "zero": {"cost": {"input": 0, "output": 1}},
                "texto": {"cost": {"input": " 1_0 ", "output": true, "cache_read": 0}},
                "lista": [1],
                "cost-lista": {"cost": [1]},
            }},
            "revenda": {"models": {"texto": {"cost": {"input": 0, "output": 0}}}},
            "openai": 5,
            "google": {"models": null},
        });
        assert_eq!(Value::Object(slim(&raw).unwrap()), json!({
            "texto": {"provider": "anthropic", "input": 10.0, "output": 1.0, "cache_read": 0.0, "cache_write": null},
            "anthropic/texto": {"provider": "anthropic", "input": 10.0, "output": 1.0, "cache_read": 0.0, "cache_write": null},
        }));
        for bad in [json!([]), json!(null), json!({"openai": {"models": "x"}}), json!({"openai": {"models": {"m": {"cost": {"input": "abc", "output": 1}}}}})] {
            assert!(slim(&bad).is_err(), "{bad}");
        }
        // Uma entrada ruim não derruba o catálogo inteiro, e a ordem é a de chegada.
        let mixed = json!({"openai": {"models": {"text-embedding-9": {"cost": {"input": 0.02}}, "gpt-bom": {"cost": {"input": 1, "output": 2}}}}});
        assert_eq!(slim(&mixed).unwrap().keys().collect::<Vec<_>>(), ["gpt-bom", "openai/gpt-bom"]);
    }

    /// O cache guarda cada preço de cache como veio: o `cache_read` de quem só não publica `cache_write`
    /// continua valendo na releitura.
    #[tokio::test]
    async fn downloaded_cache_keeps_cache_read_when_only_cache_write_is_missing() {
        let body = r#"{"deepseek": {"models": {"deepseek-v4-flash": {"cost": {"input": 0.14, "output": 0.28, "cache_read": 0.0028, "cache_write": null}}}}}"#;
        let r = run(Mode::Body(body), None, 1).await;
        let cached: Value = serde_json::from_str(&r.text().unwrap()).unwrap();
        assert_eq!(cached["modelos"]["deepseek-v4-flash"]["cache_read"], json!(0.0028));
        assert_eq!(cached["modelos"]["deepseek-v4-flash"]["cache_write"], Value::Null);
        let rate = hangar_server::costs::pricing::Pricing::load(r.dir.path()).rate_for("deepseek-v4-flash").unwrap();
        assert_eq!((rate.cache_read, rate.cache_write, rate.origin.as_str()), (0.0028, 0.14, "models.dev"));
    }

    /// fetch_script_200: o snapshot e o recorte saem como o script gravava; o recorte do recorte é ele mesmo.
    #[tokio::test]
    async fn snapshot_writes_the_versioned_files_like_the_script() {
        let fake = fake(Mode::Strict).await;
        let root = repo_root();
        let lines = write_snapshot(root.path(), &fake.url).await.unwrap();
        assert_eq!(fake.seen.lock().unwrap().clone(), [(Some("hangar/fetch_model_prices".to_owned()), None)]);
        let snapshot = std::fs::read_to_string(root.path().join("backend/app/pricing_data.json")).unwrap();
        let python = std::fs::read_to_string(fixtures().join("pricing_data_snapshot.json")).unwrap();
        let generated = |text: &str| text.lines().nth(1).unwrap().to_owned();
        assert!(generated(&snapshot).starts_with(" \"gerado_em\": \"") && generated(&snapshot).ends_with("+00:00\","), "{}", generated(&snapshot));
        assert_eq!(snapshot.replace(&generated(&snapshot), ""), python.replace(&generated(&python), ""));
        let written = std::fs::read(root.path().join("crates/hangar-server/tests/it/fixtures/pricing/models_dev_recorte.json")).unwrap();
        assert_eq!(written, recorte(), "o recorte do recorte sai byte a byte igual");
        assert_eq!(lines[0], format!("80 entradas -> {}", root.path().join("backend/app/pricing_data.json").display()));
        // A versão do Python vira o mesmo texto pelo leitor do snapshot.
        assert_eq!(versioned_text(&serde_json::from_str(&python).unwrap()), python);
    }

    #[tokio::test]
    async fn snapshot_failure_writes_nothing() {
        // Catálogo sem modelo conhecido também é recusado (D13): o snapshot bom fica.
        for mode in [Mode::Status(403), Mode::Body("{garbage"), Mode::Body("{}"), Mode::Body(r#"{"anthropic": 5}"#)] {
            let fake = fake(mode).await;
            let root = repo_root();
            assert!(write_snapshot(root.path(), &fake.url).await.is_err());
            assert_eq!(files_under(root.path()), ["backend/app", "crates/hangar-server/tests/it/fixtures/pricing"].map(String::from));
        }
    }

    /// Fora da raiz do repositório nada é criado nem baixado.
    #[tokio::test]
    async fn snapshot_needs_the_repository_folders() {
        let fake = fake(Mode::Strict).await;
        let root = tempfile::tempdir().unwrap();
        let error = write_snapshot(root.path(), &fake.url).await.unwrap_err();
        assert!(error.contains("backend/app"), "{error}");
        assert!(std::fs::read_dir(root.path()).unwrap().next().is_none(), "nenhuma pasta criada");
        assert!(fake.seen.lock().unwrap().is_empty(), "nada baixado");
    }

    /// Resposta acima do teto é recusada antes de ser juntada, e o cache fica.
    #[tokio::test]
    async fn oversized_answer_keeps_the_cache() {
        let fake = fake(Mode::Strict).await;
        let dir = tempfile::tempdir().unwrap();
        let log = Log::default();
        let writer = log.clone();
        let _guard = tracing::subscriber::set_default(tracing_subscriber::fmt().with_ansi(false).with_writer(move || writer.clone()).finish());
        PricingRefresh::new(dir.path().to_owned(), fake.url.clone()).with_max_body(1024).run().await;
        assert_eq!(fake.seen.lock().unwrap().len(), 1);
        assert!(!dir.path().join("models.dev.json").exists());
        assert!(log.text().contains("resposta maior que 1024 bytes"), "{}", log.text());
    }

    /// A subida não baixa nada com `CP_PRICING_OFFLINE=1`; sem ele, baixa uma vez.
    #[tokio::test]
    async fn boot_task_follows_the_offline_switch() {
        let fake = fake(Mode::Strict).await;
        let dir = tempfile::tempdir().unwrap();
        assert!(PricingRefresh::start(dir.path().to_owned(), &fake.url, true).is_none());
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert!(fake.seen.lock().unwrap().is_empty());
        PricingRefresh::start(dir.path().to_owned(), &fake.url, false).unwrap().await.unwrap();
        assert_eq!(fake.seen.lock().unwrap().len(), 1);
        assert_eq!(std::fs::read_to_string(dir.path().join("models.dev.json")).unwrap(), python_cache());
    }

    /// Raiz com as duas pastas que o snapshot regrava, vazias.
    fn repo_root() -> tempfile::TempDir {
        let root = tempfile::tempdir().unwrap();
        for dir in ["backend/app", "crates/hangar-server/tests/it/fixtures/pricing"] { std::fs::create_dir_all(root.path().join(dir)).unwrap(); }
        root
    }

    /// Caminhos relativos de tudo sob `root` que é pasta folha ou arquivo, em ordem.
    fn files_under(root: &Path) -> Vec<String> {
        fn walk(dir: &Path, root: &Path, out: &mut Vec<String>) {
            let mut entries: Vec<_> = std::fs::read_dir(dir).unwrap().map(|e| e.unwrap().path()).collect();
            if entries.is_empty() { out.push(dir.strip_prefix(root).unwrap().display().to_string()); }
            entries.sort();
            for path in entries {
                if path.is_dir() { walk(&path, root, out) } else { out.push(path.strip_prefix(root).unwrap().display().to_string()); }
            }
        }
        let mut out = Vec::new();
        walk(root, root, &mut out);
        out
    }

    proptest::proptest! {
        /// O catálogo vem da rede: nada nele derruba o leitor nem o recorte.
        #[test]
        fn any_catalog_is_read_without_panicking(text in "(?s).{0,200}") {
            if let Ok(raw) = serde_json::from_str::<Value>(&text) {
                let _ = slim(&raw);
                let _ = excerpt(&raw);
            }
        }

        #[test]
        fn any_catalog_shape_is_read_without_panicking(input in proptest::option::of(proptest::prelude::any::<f64>()), text in ".{0,10}", flag in proptest::prelude::any::<bool>()) {
            let raw = json!({"anthropic": {"models": {"m": {"cost": {"input": input, "output": text, "cache_read": flag}}}},
                "revenda": {"models": {"m": {"cost": {"input": flag}}}}});
            let _ = slim(&raw);
            let _ = excerpt(&raw);
        }
    }
}
