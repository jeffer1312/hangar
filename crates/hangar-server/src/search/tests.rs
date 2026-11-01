// crates/hangar-server/src/search/tests.rs
//! Índice da busca contra a captura do Python (`tests/fixtures/search/index_lifecycle.json`, gerada por
//! `capture.py` sobre `tests/fixtures/search/home`) e contra os casos de `test_transcript_index.py`.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, UNIX_EPOCH};

use proptest::prelude::*;
use rusqlite::Connection;
use serde_json::{Value, json};

use super::ingest::{FileState, absorb_line, internal_prefixes, masked_ino};
use super::*;

const T0: u64 = 1_793_000_000;
// Ordem do mais velho ao mais novo, como em `capture.py`.
const MTIMES: [&str; 14] = [
    ".codex/sessions/2026/08/30/rollout-2026-08-30T20-47-12-01a05511-e980-71b3-93a9-befda5704b20.jsonl",
    ".pi/agent/sessions/--home-jefferson-Projetos-hangar-.worktrees-contas-modelos--/2026-08-11T09-37-57-248Z_d337d6b8-0b3c-4bbe-8034-939d7c48f246.jsonl",
    ".omp/agent/sessions/-tmp-omp-teste/2026-09-03T21-35-10-766Z_01a06932-79ae-77e9-b95a-286b6e2e9288.jsonl",
    ".kimi-code/sessions/wd_hangar_5112ff7a84e0/session_ab697d05-66da-4b2a-8e80-3d769951e752/agents/main/wire.jsonl",
    ".codex-b/sessions/2026/09/23/rollout-2026-09-23T17-41-59-01a0d000-f6ac-74e3-b19b-98d42af1a712.jsonl",
    ".claude-b/projects/-tmp-cx-probe-work/f8f12de6-7941-4bd0-8f0c-02bc0b4b7f53.jsonl",
    ".claude-a/projects/-home-jefferson/42a7c450-7f7e-4fd7-84ed-ee699d11098d.jsonl",
    ".claude-b/projects/-home-jefferson-Projetos-hangar-mobile-deliveries-cad3e6fe-t32/db401d8f-fc08-463c-a3cc-4734fc5dc634.jsonl",
    ".claude-b/projects/-home-jefferson-Projetos-hangar/db401d8f-fc08-463c-a3cc-4734fc5dc634.jsonl",
    ".claude-b/projects/-home-jefferson-Projetos-hangar-computer-control-9c0b634f-t1/0cc60e6c-eafc-4915-9936-f0c7b81191d9.from-a.jsonl",
    ".claude-b/projects/-home-jefferson-Projetos-hangar/a5a5a5a5-0000-4000-8000-000000000001.jsonl",
    ".claude-a/projects/-home-jefferson-Projetos-hangar/75fea57d-2237-43fe-a669-bb1ed97c6eae.jsonl",
    ".claude-a/projects/-home-jefferson-Projetos-hangar/75fea57d-2237-43fe-a669-bb1ed97c6eae/subagents/agent-a23f817306f1d1b6a.jsonl",
    ".claude-a/projects/-home-jefferson-Projetos-hangar-computer-control-9c0b634f-t1/0cc60e6c-eafc-4915-9936-f0c7b81191d9.jsonl",
];
const RESERVE: &str = ".claude-a/projects/-home-jefferson/f2afaa93-d2aa-45db-9d68-b430c22deab2.jsonl";
const COMPACT_ID: &str = "1c3fcd4c-a0d5-5f03-978c-2be4a25ee107";

struct Env {
    _tmp: tempfile::TempDir,
    root: PathBuf,
    accounts: Arc<Mutex<Vec<Account>>>,
    /// O que o catálogo responde: `Ok`, `Err` (ilegível) ou pânico.
    catalog: Arc<Mutex<Catalog>>,
    reports: Arc<Mutex<Vec<(&'static str, String)>>>,
    index: Index,
}

#[derive(Clone, Copy, PartialEq)]
enum Catalog { Ok, Err, Panic }

impl Env {
    fn new(accounts: impl FnOnce(&Path) -> Vec<Account>, timing: Timing) -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().canonicalize().unwrap();
        let accounts = Arc::new(Mutex::new(accounts(&root)));
        let catalog = Arc::new(Mutex::new(Catalog::Ok));
        let reports = Arc::new(Mutex::new(Vec::new()));
        let (a, c, r) = (accounts.clone(), catalog.clone(), reports.clone());
        let index = Index::new(&root.join("idx"), Arc::new(move || match *c.lock().unwrap() {
                Catalog::Ok => Ok(a.lock().unwrap().clone()),
                Catalog::Err => Err("catálogo ilegível".into()),
                Catalog::Panic => panic!("catálogo em pânico"),
            }),
            Arc::new(move |code, detail| r.lock().unwrap().push((code, detail))), timing);
        Self { _tmp: tmp, root, accounts, catalog, reports, index }
    }

    fn reported(&self, code: &str) -> usize {
        self.reports.lock().unwrap().iter().filter(|(c, _)| *c == code).count()
    }

    fn watching(&self) -> bool { self.index.inner.watch_ok.load(std::sync::atomic::Ordering::Acquire) }

    /// O HOME das fixtures com as duas contas Claude; `.claude-a` é a do servidor.
    fn fixtures(timing: Timing) -> Self {
        Self::new(|root| {
            let home = root.join("home");
            copy_dir(&PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/search/home"), &home);
            // O transcript do Pi não está nesta cópia das fixtures; só os Claude contam aqui.
            for (i, rel) in MTIMES.iter().enumerate().filter(|(_, rel)| home.join(rel).exists()) {
                set_mtime(&home.join(rel), T0 + 100 * (i as u64 + 1));
            }
            set_mtime(&home.join(RESERVE), T0 + 50);
            let dirs = [home.join(".claude-a"), home.join(".claude-b")].map(|p| p.to_string_lossy().into_owned());
            server_accounts(&dirs, &home.join(".claude-a"))
        }, timing)
    }

    /// Uma conta só, a do servidor, com `projects` vazio.
    fn small(timing: Timing) -> Self {
        Self::new(|root| {
            fs::create_dir_all(root.join("acct/projects")).unwrap();
            vec![Account { config_dir: None, dir: root.join("acct"), projects: root.join("acct/projects") }]
        }, timing)
    }

    fn home(&self) -> PathBuf { self.root.join("home") }

    fn write(&self, sid: &str, lines: &[String], append: bool) -> PathBuf {
        let dir = self.root.join("acct/projects/-home-u-proj");
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join(format!("{sid}.jsonl"));
        let mut f = fs::OpenOptions::new().create(true).append(append).write(true).truncate(!append).open(&path).unwrap();
        std::io::Write::write_all(&mut f, lines.concat().as_bytes()).unwrap();
        path
    }

    fn ids(&self, q: &str) -> Vec<String> {
        self.index.search(&terms(q), 50).unwrap().unwrap().into_iter().map(|r| r.event_id).collect()
    }

    fn db(&self) -> Connection { Connection::open(self.index.path()).unwrap() }

    fn count(&self, sql: &str) -> i64 { self.db().query_row(sql, [], |r| r.get(0)).unwrap() }

    /// As tabelas como `capture.py` as despeja, com o HOME trocado por `{HOME}`.
    fn dump(&self) -> Value {
        let c = self.db();
        let home = self.home().to_string_lossy().into_owned();
        let files: Vec<Value> = c.prepare("SELECT path, project, session_id, size, offset, lines, cwd, preview, head_done, internal FROM files ORDER BY path").unwrap()
            .query_map([], |r| Ok(json!({"path": r.get::<_, String>(0)?.replace(&home, "{HOME}"), "project": r.get::<_, String>(1)?,
                "session_id": r.get::<_, String>(2)?, "size": r.get::<_, i64>(3)?, "offset": r.get::<_, i64>(4)?, "lines": r.get::<_, i64>(5)?,
                "cwd": r.get::<_, Option<String>>(6)?, "preview": r.get::<_, String>(7)?, "head_done": r.get::<_, i64>(8)?,
                "internal": r.get::<_, Option<i64>>(9)?}))).unwrap().map(Result::unwrap).collect();
        let msg: Vec<Value> = c.prepare("SELECT f.path, m.event_id, m.role, m.ts, m.text FROM msg m JOIN files f ON f.id = m.file_id ORDER BY f.path, m.id").unwrap()
            .query_map([], |r| Ok(json!({"path": r.get::<_, String>(0)?.replace(&home, "{HOME}"), "event_id": r.get::<_, String>(1)?,
                "role": r.get::<_, String>(2)?, "ts": r.get::<_, Option<f64>>(3)?, "text": r.get::<_, String>(4)?}))).unwrap().map(Result::unwrap).collect();
        json!({"ready": self.index.ready(), "files": files, "msg": msg})
    }
}

fn copy_dir(from: &Path, to: &Path) {
    fs::create_dir_all(to).unwrap();
    for e in fs::read_dir(from).unwrap().flatten() {
        let (src, dst) = (e.path(), to.join(e.file_name()));
        if src.is_dir() { copy_dir(&src, &dst) } else { fs::copy(&src, &dst).unwrap(); }
    }
}

fn set_mtime(path: &Path, secs: u64) {
    fs::File::options().write(true).open(path).unwrap().set_modified(UNIX_EPOCH + Duration::from_secs(secs)).unwrap();
}

fn append(path: &Path, bytes: &[u8]) {
    std::io::Write::write_all(&mut fs::OpenOptions::new().append(true).open(path).unwrap(), bytes).unwrap();
}

fn terms(q: &str) -> Vec<String> { q.split_whitespace().map(str::to_owned).collect() }

/// A linha de `_linha` do teste Python: sem relógio, então o filtro de reescrita a mantém.
fn line(text: &str, uuid: &str, role: &str) -> String {
    let msg = if role == "user" { json!({"role": "user", "content": text}) }
        else { json!({"role": "assistant", "content": [{"type": "text", "text": text}]}) };
    format!("{}\n", json!({"type": role, "uuid": uuid, "cwd": "/home/u/proj", "message": msg}))
}

fn quiet() -> Timing { Timing { first_pass: Duration::ZERO, debounce: Duration::from_millis(100), stale: Duration::from_secs(3600) } }

fn wait_for(what: &str, mut ok: impl FnMut() -> bool) {
    let until = Instant::now() + Duration::from_secs(10);
    while !ok() {
        assert!(Instant::now() < until, "não aconteceu: {what}");
        std::thread::sleep(Duration::from_millis(50));
    }
}

const SID: &str = "11111111-1111-1111-1111-111111111111";
const SID2: &str = "22222222-2222-2222-2222-222222222222";

/// A captura de uma etapa com as diferenças aprovadas aplicadas.
fn expected(step: &str) -> Value {
    let raw = fs::read_to_string(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/search/index_lifecycle.json")).unwrap();
    let mut v: Value = serde_json::from_str::<Value>(&raw).unwrap()["data"][step].clone();
    let msgs = v["msg"].as_array_mut().unwrap();
    // M8: o `/compact` regravado pelo `--resume` sai pelo filtro de reescrita, como no histórico.
    msgs.retain(|m| m["event_id"] != COMPACT_ID);
    // M8 e o `held:` do histórico: o prompt barrado reenviado e a linha acrescentada duas vezes (mesmo
    // relógio e conteúdo) entram uma vez só por arquivo.
    let mut seen = std::collections::HashSet::new();
    msgs.retain(|m| seen.insert((m["path"].to_string(), m["event_id"].to_string())));
    v
}

#[test]
fn lifecycle_matches_python_capture() {
    // IDX-16, 26..31, 35..39; SRCH-30/31 (subagente e cópia `.from-` fora); M8.
    let env = Env::fixtures(quiet());
    let home = env.home();
    let check = |step: &str| {
        env.index.update().unwrap();
        assert_eq!(env.dump(), expected(step), "etapa {step}");
    };
    check("first_pass");
    let normal = home.join(MTIMES[13]);
    let sub = fs::read(home.join(MTIMES[12])).unwrap();
    append(&normal, &sub);
    set_mtime(&normal, T0 + 5000);
    check("append_line");
    append(&normal, &sub[..sub.len() / 2]);
    set_mtime(&normal, T0 + 5100);
    check("append_partial");
    append(&normal, &sub[sub.len() / 2..]);
    set_mtime(&normal, T0 + 5200);
    check("complete_partial");
    let tmp = normal.with_extension("tmp");
    fs::write(&tmp, fs::read(&normal).unwrap()).unwrap();
    fs::rename(&tmp, &normal).unwrap();
    set_mtime(&normal, T0 + 5300);
    check("inode_swap");
    let data = fs::read(&normal).unwrap();
    let first3: Vec<&[u8]> = data.split(|b| *b == b'\n').take(3).collect();
    let f = fs::OpenOptions::new().write(true).open(&normal).unwrap();
    f.set_len(0).unwrap();
    std::io::Write::write_all(&mut &f, &[first3.join(&b"\n"[..]), b"\n".to_vec()].concat()).unwrap();
    drop(f);
    set_mtime(&normal, T0 + 5400);
    check("shrink");
    fs::remove_file(home.join(MTIMES[6])).unwrap();
    check("delete");
    let dst = home.join(MTIMES[11].replace(".claude-a/", ".claude-b/"));
    fs::create_dir_all(dst.parent().unwrap()).unwrap();
    fs::rename(home.join(MTIMES[11]), &dst).unwrap();
    check("move_account");
    let b = home.join(".claude-b");
    let facts = env.index.file_facts(&dst.to_string_lossy()).unwrap().unwrap();
    assert_eq!((facts.config_dir.as_deref(), facts.account.as_str(), facts.provider.as_str()),
        (Some(&*b.to_string_lossy()), &*format!("claude:{}", b.to_string_lossy()), "claude"));
}

#[test]
fn indexed_ids_exist_in_merged_history() {
    // M8/F5: o id de todo trecho é um id do histórico do mesmo arquivo.
    let env = Env::fixtures(quiet());
    env.index.update().unwrap();
    let c = env.db();
    let paths: Vec<String> = c.prepare("SELECT path FROM files").unwrap().query_map([], |r| r.get(0)).unwrap().map(Result::unwrap).collect();
    assert_eq!(paths.len(), 8);
    for path in paths {
        let req = crate::transcript::HistoryRequest { provider: crate::transcript::Provider::Claude, jsonl: PathBuf::from(&path),
            queue: None, limit: None, tail_window: crate::transcript::TAIL_WINDOW };
        let mut history: Vec<String> = crate::transcript::merged_history(&req).unwrap().into_iter()
            .filter(|e| matches!(e.kind, hangar_api::chat::ChatKind::UserMsg | hangar_api::chat::ChatKind::AssistantMsg)
                && e.text.as_deref().is_some_and(|t| !t.is_empty()))
            .map(|e| e.id).collect();
        let mut indexed: Vec<String> = c.prepare("SELECT m.event_id FROM msg m JOIN files f ON f.id=m.file_id WHERE f.path=?").unwrap()
            .query_map([&path], |r| r.get(0)).unwrap().map(Result::unwrap).collect();
        history.sort();
        indexed.sort();
        if path.contains("42a7c450") {
            // IDX-36: a linha de mais de 40000 bytes só alimenta o cabeçalho.
            assert!(indexed.is_empty() && !history.is_empty(), "{path}");
        } else {
            assert_eq!(indexed, history, "{path}");
        }
    }
}

#[test]
fn rewrite_filter_continues_across_passes_and_restarts() {
    // M8 numa leitura incremental: a linha regravada que chega noutra passada (ou noutro processo)
    // sai como sairia na leitura inteira do histórico.
    let env = Env::small(quiet());
    let stamped = |uuid: &str, ts: &str| format!("{}\n", json!({"type": "user", "uuid": uuid, "timestamp": ts, "cwd": "/p",
        "message": {"role": "user", "content": "agulha regravada"}}));
    let f = env.write(SID, &[stamped("u1", "2026-10-13T22:39:12.030Z")], false);
    env.index.update().unwrap();
    append(&f, stamped("u2", "2026-10-13T22:37:58.473Z").as_bytes());
    env.index.update().unwrap();
    assert_eq!(env.ids("agulha"), ["u1"]);
    let again = Index::new(env.index.path().parent().unwrap(), env.index.inner.accounts.clone(), env.index.inner.report.clone(), quiet());
    append(&f, stamped("u3", "2026-10-13T22:37:00.000Z").as_bytes());
    again.update().unwrap();
    assert_eq!(env.ids("agulha"), ["u1"]);
}

#[test]
fn finds_substring_without_case_or_accent() {
    // IDX-18, SRCH-23: `test_acha_substring_no_meio_da_palavra` e `test_acha_por_prefixo_e_sem_acento`.
    let env = Env::small(quiet());
    env.write(SID, &[line("deu Error no TKT-40213abc", "u1", "user")], false);
    env.write(SID2, &[line("Configuração do SQLite3 na sessão", "u2", "user")], false);
    env.index.update().unwrap();
    for q in ["rror", "40213", "t-402"] { assert_eq!(env.ids(q), ["u1"], "{q}"); }
    for q in ["sqlite", "configuracao", "SESSAO", "sessão sqlite"] { assert_eq!(env.ids(q), ["u2"], "{q}"); }
    assert!(env.ids("sqlite inexistente").is_empty());
    let row = &env.index.search(&terms("sqlite"), 50).unwrap().unwrap()[0];
    assert_eq!((row.project.as_str(), row.session_id.as_str(), row.cwd.as_deref(), row.role.as_str(), row.provider.as_str(), row.config_dir.as_deref()),
        ("-home-u-proj", SID2, Some("/home/u/proj"), "user", "claude", None));
    assert_eq!(row.account, format!("claude:{}", env.root.join("acct").to_string_lossy()));
}

#[test]
fn word_table_tokens_are_whole_words_with_the_d28_boundary() {
    // D28: a tabela por palavra separa em `_`, `-` e nos símbolos ASCII, ignora acento e caixa, e
    // dígito fica na palavra; ela some e volta com a mensagem.
    let env = Env::small(quiet());
    env.write(SID, &[line("meu_tour-x", "u1", "user"), line("a+tour=b", "u2", "user"), line("estoura tour2", "u3", "user"),
        line("AÇÃO feita", "u4", "user")], false);
    env.index.update().unwrap();
    let words = |q: &str| env.count(&format!("SELECT count(*) FROM msg_words WHERE msg_words MATCH '\"{q}\"'"));
    assert_eq!((words("tour"), words("acao"), words("tour2"), words("estoura")), (2, 1, 1, 1));
    env.write(SID, &[line("outra", "u5", "user")], false);
    env.index.update().unwrap();
    assert_eq!(words("tour"), 0, "o gatilho de remoção tira a palavra");
}

#[test]
fn fts_syntax_is_literal_text() {
    // IDX-19, SRCH-22: `test_sintaxe_do_fts_nao_quebra_a_busca` e o texto como frase literal.
    let env = Env::small(quiet());
    env.write(SID, &[line("texto qualquer com a\"bc e near( junto", "u1", "user")], false);
    env.index.update().unwrap();
    for q in ["\"", "AND", "OR NOT", "foo*", "NEAR(a b)", "col:x", "^a", "(", "'"] {
        env.index.search(&terms(q), 50).unwrap().unwrap();
    }
    assert_eq!(env.ids("a\"bc"), ["u1"]);
    assert_eq!(env.ids("near("), ["u1"]);
    assert!(env.ids("texto OR").is_empty());
}

#[test]
fn short_terms_filter_folded_text() {
    // IDX-20/SRCH-20 pelo M3: termo curto responde pelo índice, sem acento nem caixa.
    let env = Env::small(quiet());
    env.write(SID, &[line("deu Error no TKT-40213abc", "u1", "user"), line("Ação feita", "u2", "assistant")], false);
    env.index.update().unwrap();
    assert_eq!(env.ids("tk 40213"), ["u1"]);
    assert_eq!(env.ids("40"), ["u1"]);
    assert_eq!(env.ids("aç"), ["u2"]);
    assert_eq!(env.ids("AC"), ["u2"]);
    assert!(env.ids("zz").is_empty());
    assert_eq!(fold("ÉÉ Ação"), "ee acao");
}

#[test]
fn not_ready_answers_none_and_empty_index_answers_empty() {
    // IDX-21 (antes da 1ª passada completa não há resposta do índice) e SRCH-21 (pronto e vazio = []).
    let env = Env::small(quiet());
    assert!(!env.index.ready());
    assert_eq!(env.index.search(&terms("agulha"), 50).unwrap(), None);
    env.index.update().unwrap();
    assert!(env.index.ready());
    assert_eq!(env.index.search(&terms("agulha"), 50).unwrap(), Some(vec![]));
    assert_eq!(env.index.search(&[], 50).unwrap(), None);
    // Reaberto do disco já pronto: `meta.built` fica gravado.
    let again = Index::new(env.index.path().parent().unwrap(), env.index.inner.accounts.clone(), env.index.inner.report.clone(), quiet());
    again.open_now().unwrap();
    assert!(again.ready());
}

#[test]
fn three_per_file_newest_file_first() {
    // IDX-22, SRCH-11: `test_tres_por_arquivo`, arquivo mais novo primeiro, teto `limit`.
    let env = Env::small(quiet());
    let lines: Vec<String> = (0..5).map(|i| line(&format!("agulha {i}"), &format!("u{i}"), "user")).collect();
    let old = env.write(SID, &lines, false);
    set_mtime(&old, T0);
    let new = env.write(SID2, &[line("agulha nova", "n1", "user")], false);
    set_mtime(&new, T0 + 10);
    env.index.update().unwrap();
    assert_eq!(env.ids("agulha"), ["n1", "u0", "u1", "u2"]);
    let two: Vec<String> = env.index.search(&terms("agulha"), 2).unwrap().unwrap().into_iter().map(|r| r.event_id).collect();
    assert_eq!(two, ["n1", "u0"]);
}

#[test]
fn only_newest_candidates_enter_the_cap() {
    // IDX-23, SRCH-25: o lote é das 2000 casadas mais novas; a conversa antiga fica fora dele.
    let env = Env::small(quiet());
    let old = env.write(SID, &[line("agulha velha", "old", "user")], false);
    set_mtime(&old, T0);
    let lines: Vec<String> = (0..2000).map(|i| line(&format!("agulha {i}"), &format!("n{i}"), "user")).collect();
    let new = env.write(SID2, &lines, false);
    set_mtime(&new, T0 + 10);
    env.index.update().unwrap();
    assert_eq!(env.ids("agulha"), ["n0", "n1", "n2"]);
    assert_eq!(env.ids("velha"), ["old"]);
}

#[test]
fn internal_conversations_stay_out() {
    // IDX-24, SRCH-06: a cópia do Perguntar nas fixtures some da busca; os prefixos são os do Python.
    assert_eq!(internal_prefixes(), [
        "Você responde onde um assunto apareceu nas conversas do usuá",
        "Você reescreve OBJETIVOS pra um loop autônomo de código. Boa",
        "Você recebeu abaixo um resumo automático de uma sessão de tr",
    ]);
    let env = Env::fixtures(quiet());
    env.index.update().unwrap();
    let rows = env.index.search(&terms("Revisor da Task 32"), 50).unwrap().unwrap();
    let sessions: std::collections::BTreeSet<&str> = rows.iter().map(|r| r.session_id.as_str()).collect();
    assert_eq!(sessions.into_iter().collect::<Vec<_>>(), ["db401d8f-fc08-463c-a3cc-4734fc5dc634"]);
    assert!(rows.iter().any(|r| r.event_id == "f94331cf-dc2c-4f1e-890e-b6a3d87836a0"));
}

#[test]
fn internal_is_decided_in_the_first_30_lines() {
    // IDX-25: 1ª user_msg depois da linha 30 não marca a conversa como interna.
    let env = Env::small(quiet());
    let prefix = &internal_prefixes()[0];
    let mut late: Vec<String> = (0..30).map(|i| line(&format!("resposta {i}"), &format!("a{i}"), "assistant")).collect();
    late.push(line(&format!("{prefix} agulha tardia"), "u1", "user"));
    env.write(SID, &late, false);
    env.write(SID2, &[line(&format!("  {prefix} agulha cedo"), "u2", "user")], false);
    env.index.update().unwrap();
    assert_eq!(env.ids("agulha"), ["u1"]);
}

#[test]
fn sqlite_failure_mid_file_leaves_no_partial_rows() {
    // IDX-33: `test_erro_de_sqlite_no_meio_nao_deixa_linhas_parciais`.
    let env = Env::small(quiet());
    env.index.update().unwrap();
    env.db().execute_batch("CREATE TRIGGER boom BEFORE UPDATE OF size ON files BEGIN SELECT RAISE(ABORT, 'disco cheio'); END;").unwrap();
    let lines: Vec<String> = (0..3).map(|i| line(&format!("agulha {i}"), &format!("u{i}"), "user")).collect();
    env.write(SID, &lines, false);
    assert!(env.index.update().is_err(), "erro do banco volta da passada");
    assert_eq!(env.count("SELECT count(*) FROM msg"), 0);
    env.db().execute_batch("DROP TRIGGER boom").unwrap();
    env.index.update().unwrap();
    assert_eq!(env.ids("agulha"), ["u0", "u1", "u2"]);
}

#[test]
fn unchanged_file_is_skipped_and_inode_is_masked() {
    // IDX-28 (mesma tupla, sem releitura) e IDX-32 (inode acima do INTEGER com sinal).
    assert_eq!(masked_ino(u64::MAX - 4), i64::MAX - 4);
    assert_eq!(masked_ino(42), 42);
    let env = Env::small(quiet());
    env.write(SID, &[line("agulha grande", "u1", "user")], false);
    env.index.update().unwrap();
    let before: (i64, i64) = env.db().query_row("SELECT ino, offset FROM files", [], |r| Ok((r.get(0)?, r.get(1)?))).unwrap();
    env.db().execute_batch("UPDATE files SET offset = offset + 1000").unwrap();
    env.index.update().unwrap();
    let after: (i64, i64) = env.db().query_row("SELECT ino, offset FROM files", [], |r| Ok((r.get(0)?, r.get(1)?))).unwrap();
    assert_eq!(after, (before.0, before.1 + 1000), "arquivo igual não é relido");
    assert_eq!(env.ids("agulha"), ["u1"]);
}

#[test]
fn incomplete_line_waits_for_the_next_pass() {
    // IDX-30: `test_append_incremental_so_linha_completa`.
    let env = Env::small(quiet());
    let f = env.write(SID, &[line("primeira agulha", "u1", "user")], false);
    env.index.update().unwrap();
    let partial = line("segunda agulha", "a1", "assistant");
    append(&f, &partial.as_bytes()[..partial.len() - 10]);
    env.index.update().unwrap();
    assert_eq!(env.ids("agulha"), ["u1"]);
    append(&f, &partial.as_bytes()[partial.len() - 10..]);
    env.index.update().unwrap();
    assert_eq!(env.ids("agulha"), ["u1", "a1"]);
}

#[test]
fn rewritten_or_deleted_file_is_reread_or_dropped() {
    // IDX-27, IDX-31: `test_arquivo_truncado_ou_trocado_e_relido`.
    let env = Env::small(quiet());
    let f = env.write(SID, &[line("velho texto", "u1", "user"), line("mais velho", "u2", "user")], false);
    env.index.update().unwrap();
    fs::write(&f, line("novo", "u3", "user")).unwrap();
    env.index.update().unwrap();
    assert!(env.ids("velho").is_empty());
    assert_eq!(env.ids("novo"), ["u3"]);
    fs::remove_file(&f).unwrap();
    env.index.update().unwrap();
    assert!(env.ids("novo").is_empty());
    assert_eq!(env.count("SELECT count(*) FROM files"), 0);
}

#[cfg(unix)]
#[test]
fn unreadable_file_waits_and_others_go_on() {
    // IDX-34: arquivo que não abre fica para a próxima passada, os outros entram.
    use std::os::unix::fs::PermissionsExt;
    let env = Env::small(quiet());
    let locked = env.write(SID, &[line("agulha trancada", "u1", "user")], false);
    env.write(SID2, &[line("agulha livre", "u2", "user")], false);
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o000)).unwrap();
    env.index.update().unwrap();
    assert_eq!(env.ids("agulha"), ["u2"]);
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o644)).unwrap();
    env.index.update().unwrap();
    let mut ids = env.ids("agulha");
    ids.sort();
    assert_eq!(ids, ["u1", "u2"]);
}

#[test]
fn schema_version_change_rebuilds() {
    // IDX-13: `test_versao_do_esquema_diferente_reconstroi`.
    let env = Env::small(quiet());
    env.write(SID, &[line("agulha", "u1", "user")], false);
    env.index.update().unwrap();
    env.db().pragma_update(None, "user_version", 999).unwrap();
    let again = Index::new(env.index.path().parent().unwrap(), env.index.inner.accounts.clone(), env.index.inner.report.clone(), quiet());
    again.open_now().unwrap();
    assert!(!again.ready());
    assert_eq!(env.count("SELECT count(*) FROM files"), 0);
    assert_eq!(env.count("PRAGMA user_version"), db::SCHEMA_VERSION);
}

#[test]
fn corrupt_file_is_deleted_and_rebuilt() {
    // IDX-14/15: banco ilegível sai (com -wal e -shm) e volta vazio, não pronto.
    let env = Env::small(quiet());
    let path = env.index.path().to_owned();
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(&path, b"isto nao e um banco sqlite".repeat(100)).unwrap();
    fs::write(format!("{}-wal", path.display()), b"lixo").unwrap();
    env.index.open_now().unwrap();
    assert!(!env.index.ready());
    env.write(SID, &[line("agulha", "u1", "user")], false);
    env.index.update().unwrap();
    assert_eq!(env.ids("agulha"), ["u1"]);
}

#[test]
fn connections_use_the_python_pragmas_and_read_during_write() {
    // IDX-12 (PRAGMAs) e IDX-11 (leitura com escrita aberta noutra conexão).
    let env = Env::small(quiet());
    env.write(SID, &[line("agulha", "u1", "user")], false);
    env.index.update().unwrap();
    {
        let guard = env.index.inner.writer.lock().unwrap();
        let w = guard.as_ref().unwrap();
        let get = |p: &str| -> String { w.query_row(&format!("PRAGMA {p}"), [], |r| r.get::<_, rusqlite::types::Value>(0)).map(|v| format!("{v:?}")).unwrap() };
        assert_eq!(get("journal_mode"), "Text(\"wal\")");
        assert_eq!(get("synchronous"), "Integer(1)");
        assert_eq!(get("journal_size_limit"), "Integer(8388608)");
        assert_eq!(get("busy_timeout"), "Integer(10000)");
    }
    let writer = env.db();
    writer.execute_batch("BEGIN IMMEDIATE; INSERT INTO meta(k, v) VALUES ('x', 'y');").unwrap();
    assert_eq!(env.ids("agulha"), ["u1"]);
    writer.execute_batch("ROLLBACK").unwrap();
}

#[test]
fn one_writer_at_a_time() {
    // IDX-09: passadas simultâneas não se sobrepõem nem duplicam mensagens.
    let env = Env::small(quiet());
    let lines: Vec<String> = (0..200).map(|i| line(&format!("agulha {i}"), &format!("u{i}"), "user")).collect();
    env.write(SID, &lines, false);
    let threads: Vec<_> = (0..4).map(|_| { let i = env.index.clone(); std::thread::spawn(move || i.update().unwrap()) }).collect();
    threads.into_iter().for_each(|t| t.join().unwrap());
    assert_eq!(env.count("SELECT count(*) FROM msg"), 200);
}

#[test]
fn creating_the_index_touches_no_disk() {
    // IDX-02: o `AppState` dos testes monta o índice sem abrir banco nem varrer transcripts.
    let env = Env::small(quiet());
    assert!(!env.index.path().parent().unwrap().exists());
    assert_eq!(env.index.search(&terms("agulha"), 50).unwrap(), None);
    assert!(env.index.heads().is_empty());
    assert_eq!(env.index.file_facts("/x").unwrap(), None);
    assert!(!env.index.path().parent().unwrap().exists());
    // IDX-03: arquivo próprio do Rust ao lado do índice de custos.
    let server = Index::server(crate::accounts::AccountService::new(crate::accounts::environment::AccountEnvironment::capture()));
    assert_eq!(server.path(), crate::costs::index::default_dir().join("busca-rust.sqlite3"));
}

#[test]
fn heads_and_file_facts_for_the_archive() {
    // IDX-42 e M4/F12: cabeçalho pronto, conta, se tem usuário e a última mensagem real.
    let env = Env::fixtures(quiet());
    env.index.update().unwrap();
    let home = env.home();
    let heads = env.index.heads();
    let normal = home.join(MTIMES[13]).to_string_lossy().into_owned();
    assert_eq!(heads[&normal], ("[painel: orquestrador 9c0b634f] Read /.claude/skills/orquestrar/references/executor.md — your role's page, plus the…".to_owned(),
        Some("/home/jefferson/Projetos/hangar-computer-control-9c0b634f-t1".to_owned())));
    assert_eq!(heads.len(), 7, "a reserva sem cabeçalho fica fora");
    assert!(!heads.contains_key(&*home.join(RESERVE).to_string_lossy()));
    let a = home.join(".claude-a").to_string_lossy().into_owned();
    let facts = |rel: &str| env.index.file_facts(&home.join(rel).to_string_lossy()).unwrap().unwrap();
    assert_eq!(facts(MTIMES[13]), FileFacts { has_user: true, provider: "claude".into(), config_dir: Some(a.clone()), account: format!("claude:{a}"),
        last_text: "O árbitro aceitou a correção do scripts/windows-test.sh e mandou incluir components = [\"clippy\"] no rust-toolchain.toml…".into() });
    let reserve = facts(RESERVE);
    assert_eq!((reserve.has_user, reserve.last_text.as_str()), (false, ""));
    assert_eq!(facts(MTIMES[6]).last_text, "tem como dedesativar esse mod do ponytail? eu nao queria isso ativo nao querof icar vendo esse botao ai");
    assert_eq!(facts(MTIMES[11]).last_text, "O ator do Rust chama POST /internal/runtime/policy no Python ao abrir e recebe 503. Vou ver por quê.");
    assert_eq!(env.index.file_facts("/nao/existe.jsonl").unwrap(), None);
}

#[test]
fn server_accounts_skip_repeats_and_add_the_server_last() {
    // `archive._contas`: sem repetir pelo caminho real; a do servidor por último se não veio.
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().canonicalize().unwrap();
    for d in ["a/projects", "b/projects", "srv/projects"] { fs::create_dir_all(root.join(d)).unwrap(); }
    #[cfg(unix)]
    std::os::unix::fs::symlink(root.join("a"), root.join("a-link")).unwrap();
    let s = |p: &str| root.join(p).to_string_lossy().into_owned();
    let mut dirs = vec![s("a"), s("b"), s("missing")];
    if cfg!(unix) { dirs.push(s("a-link")); }
    let got = server_accounts(&dirs, &root.join("srv"));
    assert_eq!(got, vec![
        Account { config_dir: Some(s("a")), dir: root.join("a"), projects: root.join("a/projects") },
        Account { config_dir: Some(s("b")), dir: root.join("b"), projects: root.join("b/projects") },
        // Conta ainda sem `projects` fica: a pasta nasce com a 1ª conversa.
        Account { config_dir: Some(s("missing")), dir: root.join("missing"), projects: root.join("missing/projects") },
        Account { config_dir: None, dir: root.join("srv"), projects: root.join("srv/projects") },
    ]);
    assert_eq!(server_accounts(&[s("a")], &root.join("a")).len(), 1);
}

#[test]
fn stale_search_starts_a_background_pass_without_waiting() {
    // IDX-10, SRCH-24: sem observador, a busca não espera; a conversa nova aparece numa busca seguinte.
    let env = Env::small(Timing { stale: Duration::ZERO, ..quiet() });
    let f = env.write(SID, &[line("agulha um", "u1", "user")], false);
    env.index.update().unwrap();
    append(&f, line("agulha dois", "u2", "user").as_bytes());
    let held = env.index.inner.writer.lock().unwrap();
    let t0 = Instant::now();
    assert_eq!(env.ids("agulha"), ["u1"]);
    assert!(t0.elapsed() < Duration::from_secs(1));
    drop(held);
    wait_for("passada disparada pela busca", || env.ids("agulha") == ["u1", "u2"]);
}

#[test]
fn producer_waits_first_pass_then_follows_writes() {
    // M1 (no lugar de IDX-05..07), IDX-01 e IDX-06 com prazo curto.
    let env = Env::small(Timing { first_pass: Duration::from_millis(400), ..quiet() });
    let f = env.write(SID, &[line("agulha um", "u1", "user")], false);
    let producer = env.index.start();
    assert!(producer.is_some());
    assert!(env.index.start().is_none(), "uma vez por processo");
    std::thread::sleep(Duration::from_millis(150));
    assert!(!env.index.ready(), "a 1ª passada espera o prazo");
    wait_for("1ª passada", || env.index.ready());
    assert_eq!(env.ids("agulha"), ["u1"]);
    wait_for("observador de pé", || env.index.inner.watch_ok.load(std::sync::atomic::Ordering::Acquire));
    append(&f, line("agulha dois", "u2", "user").as_bytes());
    wait_for("escrita indexada sem passada manual", || env.ids("agulha") == ["u1", "u2"]);
    env.write(SID2, &[line("agulha nova conversa", "n1", "user")], false);
    wait_for("conversa nova indexada", || env.ids("nova") == ["n1"]);
    let reports = env.reports.lock().unwrap().clone();
    assert!(reports.is_empty(), "{reports:?}");
}

#[test]
fn producer_rereads_the_catalog() {
    // Conta removida do catálogo sai; conta que volta entra e passa a ser vigiada.
    let env = Env::fixtures(Timing { first_pass: Duration::ZERO, ..quiet() });
    let home = env.home();
    let _producer = env.index.start().unwrap();
    wait_for("1ª passada", || env.index.ready());
    let b_files = || env.count("SELECT count(*) FROM files WHERE config_dir LIKE '%.claude-b'");
    assert_eq!(b_files(), 4);
    let all = env.accounts.lock().unwrap().clone();
    env.accounts.lock().unwrap().retain(|a| !a.dir.ends_with(".claude-b"));
    append(&home.join(MTIMES[13]), line("agulha gatilho", "g1", "user").as_bytes());
    wait_for("conta removida sai", || b_files() == 0);
    *env.accounts.lock().unwrap() = all;
    append(&home.join(MTIMES[13]), line("agulha gatilho 2", "g2", "user").as_bytes());
    wait_for("conta de volta entra", || b_files() == 4);
    append(&home.join(MTIMES[5]), line("zebraconta de volta", "b1", "user").as_bytes());
    wait_for("conta de volta é vigiada", || env.ids("zebraconta") == ["b1"]);
}

#[cfg(unix)]
#[test]
fn watcher_that_fails_is_reported_and_search_takes_over() {
    // M1: observador que não sobe vai ao diário e a busca dispara a passada (IDX-10).
    use std::os::unix::fs::PermissionsExt;
    let env = Env::small(Timing { first_pass: Duration::ZERO, stale: Duration::ZERO, ..quiet() });
    let blocked = env.root.join("blocked");
    fs::create_dir_all(blocked.join("projects")).unwrap();
    fs::set_permissions(blocked.join("projects"), fs::Permissions::from_mode(0o000)).unwrap();
    env.accounts.lock().unwrap().push(Account { config_dir: Some(blocked.to_string_lossy().into_owned()), dir: blocked.clone(), projects: blocked.join("projects") });
    let f = env.write(SID, &[line("agulha um", "u1", "user")], false);
    let _producer = env.index.start().unwrap();
    wait_for("1ª passada", || env.index.ready());
    wait_for("falha no diário", || env.reported("busca.observador_falhou") > 0);
    assert!(!env.watching());
    append(&f, line("agulha dois", "u2", "user").as_bytes());
    wait_for("busca dispara a passada", || env.ids("agulha") == ["u1", "u2"]);
    // Cada passada tenta de novo; o diário só recebe a mudança de estado.
    for _ in 0..5 {
        env.ids("agulha");
        std::thread::sleep(Duration::from_millis(150));
    }
    assert_eq!(env.reported("busca.observador_falhou"), 1);
    fs::set_permissions(blocked.join("projects"), fs::Permissions::from_mode(0o755)).unwrap();
}

#[test]
fn watcher_that_falls_is_reported() {
    let env = Env::small(Timing { first_pass: Duration::ZERO, ..quiet() });
    let _producer = env.index.start().unwrap();
    wait_for("observador de pé", || env.watching());
    env.index.wake_producer(watch::Wake::Failed("fila do inotify cheia".into()));
    wait_for("queda no diário", || env.reports.lock().unwrap().iter().any(|(c, d)| *c == "busca.observador_falhou" && d.contains("inotify")));
    assert!(!env.index.inner.watch_ok.load(std::sync::atomic::Ordering::Acquire));
}

#[test]
fn index_that_cannot_open_is_reported() {
    // IDX-04: banco que não abre vai ao diário (`busca.indice_indisponivel`); a busca segue sem ele.
    let env = Env::small(quiet());
    fs::write(env.root.join("idx"), b"arquivo no lugar da pasta").unwrap();
    env.index.start().unwrap().join();
    assert_eq!(env.reports.lock().unwrap()[0].0, "busca.indice_indisponivel");
    assert_eq!(env.index.search(&terms("agulha"), 50).unwrap(), None);
}

#[test]
fn account_without_projects_is_watched_until_projects_appears() {
    // Instalação nova: a conta do servidor ainda sem `projects` entra e é vigiada até a pasta nascer.
    let env = Env::new(|root| {
        fs::create_dir_all(root.join("acct")).unwrap();
        server_accounts(&[], &root.join("acct"))
    }, Timing { first_pass: Duration::ZERO, ..quiet() });
    let _producer = env.index.start().unwrap();
    wait_for("1ª passada", || env.index.ready());
    wait_for("pasta da conta vigiada", || env.watching());
    env.write(SID, &[line("agulha nascida", "u1", "user")], false);
    wait_for("conversa da pasta nova indexada", || env.ids("nascida") == ["u1"]);
}

#[test]
fn nothing_to_watch_is_not_a_watcher() {
    // Sem alvo vigiado, a busca continua disparando passada (observador não está de pé).
    let env = Env::new(|root| vec![Account { config_dir: None, dir: root.join("nada"), projects: root.join("nada/projects") }],
        Timing { first_pass: Duration::ZERO, ..quiet() });
    let _producer = env.index.start().unwrap();
    wait_for("1ª passada", || env.index.ready());
    std::thread::sleep(Duration::from_millis(300));
    assert!(!env.watching());
}

#[test]
fn new_catalog_account_is_noticed_when_read() {
    // Conta nova no catálogo não gera evento nas pastas vigiadas: a leitura percebe e pede a passada.
    let env = Env::small(Timing { first_pass: Duration::ZERO, ..quiet() });
    let _producer = env.index.start().unwrap();
    wait_for("observador de pé", || env.watching());
    let other = env.root.join("acct2");
    fs::create_dir_all(other.join("projects/-p")).unwrap();
    fs::write(other.join(format!("projects/-p/{SID2}.jsonl")), line("agulha conta nova", "n1", "user")).unwrap();
    env.accounts.lock().unwrap().push(Account { config_dir: Some(other.to_string_lossy().into_owned()), dir: other.clone(), projects: other.join("projects") });
    wait_for("conta nova indexada", || env.ids("nova") == ["n1"]);
}

#[cfg(unix)]
#[test]
fn listing_error_keeps_the_account_files() {
    // Falha momentânea ao listar a conta não apaga as conversas dela do índice.
    use std::os::unix::fs::PermissionsExt;
    let env = Env::fixtures(quiet());
    env.index.update().unwrap();
    let projects = env.home().join(".claude-b/projects");
    fs::set_permissions(&projects, fs::Permissions::from_mode(0o000)).unwrap();
    env.index.update().unwrap();
    let kept = env.count("SELECT count(*) FROM files WHERE config_dir LIKE '%.claude-b'");
    fs::set_permissions(&projects, fs::Permissions::from_mode(0o755)).unwrap();
    assert_eq!(kept, 4);
}

#[test]
fn unreadable_catalog_keeps_the_index() {
    let env = Env::fixtures(quiet());
    env.index.update().unwrap();
    *env.catalog.lock().unwrap() = Catalog::Err;
    assert!(env.index.update().is_err());
    assert_eq!(env.count("SELECT count(*) FROM files"), 8);
}

#[test]
fn pending_account_keeps_its_files() {
    // Conta em migração (`.hangar-account-pending`) some do catálogo por um tempo, não do índice.
    let env = Env::fixtures(quiet());
    env.index.update().unwrap();
    let b = env.home().join(".claude-b");
    let b_files = || env.count("SELECT count(*) FROM files WHERE config_dir LIKE '%.claude-b'");
    env.accounts.lock().unwrap().retain(|a| !a.dir.ends_with(".claude-b"));
    fs::write(b.join(crate::accounts::storage::PENDING), b"").unwrap();
    env.index.update().unwrap();
    assert_eq!(b_files(), 4);
    fs::remove_file(b.join(crate::accounts::storage::PENDING)).unwrap();
    env.index.update().unwrap();
    assert_eq!(b_files(), 0, "conta removida de verdade sai");
}

#[test]
fn watcher_is_up_before_the_first_pass() {
    // Escrita feita durante a 1ª passada precisa de um observador que já exista.
    let env = Env::small(Timing { first_pass: Duration::from_millis(500), ..quiet() });
    let _producer = env.index.start().unwrap();
    wait_for("banco aberto", || env.index.path().exists());
    let held = env.index.inner.writer.lock().unwrap();
    wait_for("observador de pé com a passada esperando", || env.watching());
    drop(held);
}

#[test]
fn same_size_and_mtime_skip_without_asking_the_file_id() {
    // Windows: o file ID só sai abrindo o arquivo; sem mudança de tamanho e relógio nem se pergunta.
    let windows = ingest::Stat { size: 10, mtime_ns: 5, ino: None };
    assert!(ingest::unchanged((10, 5, 77), &windows));
    assert!(!ingest::unchanged((11, 5, 77), &windows));
    assert!(!ingest::unchanged((10, 6, 77), &windows));
    assert!(!ingest::unchanged((10, 5, 77), &ingest::Stat { ino: Some(78), ..windows }));
    assert!(ingest::unchanged((10, 5, 77), &ingest::Stat { ino: Some(77), ..windows }));
}

#[test]
fn rescan_and_new_project_folder_wake_the_producer() {
    use notify::event::{CreateKind, EventKind, Flag};
    let tmp = tempfile::tempdir().unwrap();
    let folder = tmp.path().join("-home-u-novo");
    fs::create_dir(&folder).unwrap();
    let rescan = Ok(notify::Event::new(EventKind::Other).set_flag(Flag::Rescan));
    assert!(matches!(watch::wakes(&rescan), Some(watch::Wake::Changed)), "estouro da fila de eventos");
    let created = Ok(notify::Event::new(EventKind::Create(CreateKind::Folder)).add_path(folder));
    assert!(matches!(watch::wakes(&created), Some(watch::Wake::Changed)), "pasta de projeto nova");
    let other = Ok(notify::Event::new(EventKind::Create(CreateKind::File)).add_path(tmp.path().join("x.txt")));
    assert!(watch::wakes(&other).is_none());
}

#[test]
fn same_inode_rewritten_from_the_start_is_reread() {
    // `cp` por cima: mesmo inode, tamanho maior que o lido, conteúdo novo desde o 1º byte.
    let env = Env::small(quiet());
    let f = env.write(SID, &[line("velho conteúdo", "u1", "user")], false);
    env.index.update().unwrap();
    let fresh = [line("novo primeiro", "n1", "user"), line("novo segundo", "n2", "user")].concat();
    let file = fs::OpenOptions::new().write(true).open(&f).unwrap();
    file.set_len(0).unwrap();
    std::io::Write::write_all(&mut &file, fresh.as_bytes()).unwrap();
    drop(file);
    env.index.update().unwrap();
    assert!(env.ids("velho").is_empty());
    assert_eq!(env.ids("novo"), ["n1", "n2"]);
}

#[test]
fn dropping_the_producer_stops_it() {
    let env = Env::small(Timing { first_pass: Duration::ZERO, ..quiet() });
    let producer = env.index.start().unwrap();
    wait_for("observador de pé", || env.watching());
    drop(producer);
    assert!(!env.watching(), "parado, a busca volta a disparar passada");
}

#[test]
fn producer_panic_is_reported() {
    let env = Env::small(Timing { first_pass: Duration::ZERO, ..quiet() });
    let _producer = env.index.start().unwrap();
    wait_for("observador de pé", || env.watching());
    *env.catalog.lock().unwrap() = Catalog::Panic;
    env.write(SID, &[line("agulha", "u1", "user")], false);
    wait_for("queda do produtor no diário", || env.reported("busca.produtor_caiu") == 1);
    assert!(!env.watching());
}

#[test]
fn corrupt_rewrite_state_is_reported() {
    let env = Env::small(quiet());
    let f = env.write(SID, &[line("agulha um", "u1", "user")], false);
    env.index.update().unwrap();
    env.db().execute_batch("UPDATE files SET rewrite = 'lixo'").unwrap();
    append(&f, line("agulha dois", "u2", "user").as_bytes());
    env.index.update().unwrap();
    assert_eq!(env.reported("busca.filtro_ilegivel"), 1);
    assert_eq!(env.ids("agulha"), ["u1", "u2"]);
}

proptest! {
    #[test]
    fn absorb_line_never_panics(bytes in proptest::collection::vec(any::<u8>(), 0..512)) {
        let mut st = FileState::default();
        let mut raw = bytes;
        raw.push(b'\n');
        let _ = absorb_line(&mut st, &raw, &internal_prefixes());
        prop_assert_eq!(st.offset, raw.len() as i64);
    }

    #[test]
    fn absorb_truncated_json_never_panics(cut in 0usize..2000) {
        let full = line("Configuração com \"aspas\" e emoji 🚀", "u1", "user");
        let bytes = full.as_bytes();
        let mut raw = bytes[..cut.min(bytes.len() - 1)].to_vec();
        raw.push(b'\n');
        let mut st = FileState::default();
        let msgs = absorb_line(&mut st, &raw, &internal_prefixes());
        prop_assert!(msgs.len() <= 1);
    }
}

// R5: conversas de Codex (duas contas), Pi, omp e Kimi.

/// Chave de mentira que casa o provedor do Pi (`openai-codex`), o do omp (`opencode-go`) e o modelo
/// padrão do Kimi com a mesma conta, como as credenciais reais fazem.
const KIMI_CONFIG: &str = "default_model=\"apikey/k3\"\n[providers.apikey]\ntype=\"kimi\"\napi_key=\"sk-teste\"\nbase_url=\"https://example.invalid\"\n";
const PI_AUTH: &str = r#"{"openai-codex":{"key":"sk-teste"},"opencode-go":{"key":"sk-teste"}}"#;
const CODEX_A: &str = ".codex/sessions/2026/08/30/rollout-2026-08-30T20-47-12-01a05511-e980-71b3-93a9-befda5704b20.jsonl";
const CODEX_B: &str = ".codex-b/sessions/2026/09/23/rollout-2026-09-23T17-41-59-01a0d000-f6ac-74e3-b19b-98d42af1a712.jsonl";
const PI: &str = ".pi/agent/sessions/--home-jefferson-Projetos-hangar-.worktrees-contas-modelos--/2026-08-11T09-37-57-248Z_d337d6b8-0b3c-4bbe-8034-939d7c48f246.jsonl";

impl Env {
    /// As fixtures com os outros agentes ligados ao índice: o `{HOME}` do índice do Kimi trocado e as
    /// credenciais de mentira no lugar.
    fn agents(timing: Timing) -> Self {
        let env = Self::fixtures(timing);
        let home = env.home();
        let kimi_index = home.join(".kimi-code/session_index.jsonl");
        let text = fs::read_to_string(&kimi_index).unwrap().replace("{HOME}", &home.to_string_lossy());
        fs::write(&kimi_index, text).unwrap();
        fs::write(home.join(".kimi-code/config.toml"), KIMI_CONFIG).unwrap();
        fs::write(home.join(".pi/agent/auth.json"), PI_AUTH).unwrap();
        let path = |p: PathBuf| p.to_string_lossy().into_owned();
        let accounts = crate::accounts::environment::AccountEnvironment::from_map(std::collections::BTreeMap::from([
            ("HOME".to_owned(), path(home.clone())), ("USERPROFILE".to_owned(), path(home.clone())),
            ("CLAUDE_CONFIG_DIR".to_owned(), path(home.join(".claude-a"))), ("PATH".to_owned(), String::new()),
        ]));
        let index = env.index.clone().with_agents(accounts);
        Self { index, ..env }
    }

    /// Os ids que o histórico do Arquivo devolve para o arquivo (mensagens com texto, em ordem).
    fn history_ids(&self, rel: &str, provider: crate::session_open::AgentProvider) -> Vec<String> {
        fs::read(self.home().join(rel)).unwrap().split(|b| *b == b'\n')
            .flat_map(|line| crate::archive_history::agent_line(provider, line))
            .filter(crate::archive_history::is_message).map(|e| e.id).collect()
    }

    fn indexed_ids(&self, rel: &str) -> Vec<String> {
        let path = self.home().join(rel).to_string_lossy().into_owned();
        self.db().prepare("SELECT m.event_id FROM msg m JOIN files f ON f.id = m.file_id WHERE f.path = ? ORDER BY m.id").unwrap()
            .query_map([path], |r| r.get(0)).unwrap().map(Result::unwrap).collect()
    }
}

#[test]
fn agents_enter_the_index_with_provider_and_account() {
    // R5: rollouts Codex das duas contas, Pi, omp e o wire do Kimi entram com agente, conta, conta Codex
    // dona e a pasta do Claude do cwd; `config_dir` nulo.
    let env = Env::agents(quiet());
    env.index.update().unwrap();
    let home = env.home().to_string_lossy().into_owned();
    let rows: Vec<Value> = env.db().prepare("SELECT path, provider, project, session_id, cwd, config_dir, account, codex_account \
            FROM files WHERE provider <> 'claude' ORDER BY path").unwrap()
        .query_map([], |r| Ok(json!({"path": r.get::<_, String>(0)?.replace(&home, "{HOME}"), "provider": r.get::<_, String>(1)?,
            "project": r.get::<_, String>(2)?, "session_id": r.get::<_, String>(3)?, "cwd": r.get::<_, Option<String>>(4)?,
            "config_dir": r.get::<_, Option<String>>(5)?, "account": r.get::<_, Option<String>>(6)?.map(|a| a.replace(&home, "{HOME}")),
            "codex_account": r.get::<_, Option<String>>(7)?}))).unwrap().map(Result::unwrap).collect();
    // Ordem de bytes do caminho: `.codex-b/` vem antes de `.codex/`.
    assert_eq!(Value::Array(rows), json!([
        {"path": format!("{{HOME}}/{CODEX_B}"), "provider": "codex", "project": "-tmp-hangar-native-task4-fixture-jf",
         "session_id": "01a0d000-f6ac-74e3-b19b-98d42af1a712", "cwd": "/tmp/hangar-native-task4-fixture-jf", "config_dir": null,
         "account": "codex:{HOME}/.codex-b", "codex_account": "b"},
        {"path": format!("{{HOME}}/{CODEX_A}"), "provider": "codex", "project": "-home-jefferson-Projetos-hangar",
         "session_id": "01a05511-e980-71b3-93a9-befda5704b20", "cwd": "/home/jefferson/Projetos/hangar", "config_dir": null,
         "account": "codex:{HOME}/.codex", "codex_account": "default"},
        {"path": "{HOME}/.kimi-code/sessions/wd_hangar_5112ff7a84e0/session_ab697d05-66da-4b2a-8e80-3d769951e752/agents/main/wire.jsonl",
         "provider": "kimi", "project": "-home-jefferson-Projetos-hangar", "session_id": "session_ab697d05-66da-4b2a-8e80-3d769951e752",
         "cwd": "/home/jefferson/Projetos/hangar", "config_dir": null, "account": "kimi:apikey", "codex_account": null},
        {"path": format!("{{HOME}}/{}", MTIMES[2]), "provider": "omp",
         "project": "-tmp-claude-1000--home-jefferson-Projetos-hangar-ac1a7b0f-d0a7-4e66-8f05-f53ea5d4b4da-scratchpad-omp-teste",
         "session_id": "01a06932-79ae-77e9-b95a-286b6e2e9288",
         "cwd": "/tmp/claude-1000/-home-jefferson-Projetos-hangar/ac1a7b0f-d0a7-4e66-8f05-f53ea5d4b4da/scratchpad/omp-teste",
         "config_dir": null, "account": "kimi:apikey", "codex_account": null},
        {"path": format!("{{HOME}}/{PI}"), "provider": "pi", "project": "-home-jefferson-Projetos-hangar--worktrees-contas-modelos",
         "session_id": "d337d6b8-0b3c-4bbe-8034-939d7c48f246", "cwd": "/home/jefferson/Projetos/hangar/.worktrees/contas-modelos",
         "config_dir": null, "account": "kimi:apikey", "codex_account": null},
    ]));
    // As do Claude continuam como estavam.
    assert_eq!(env.count("SELECT count(*) FROM files WHERE provider = 'claude'"), 8);
}

#[test]
fn agent_ids_are_the_archive_history_ids() {
    // R5: cada trecho abre no ponto — o id indexado é o que o histórico do Arquivo devolve.
    use crate::session_open::AgentProvider;
    let env = Env::agents(quiet());
    env.index.update().unwrap();
    assert_eq!(env.ids("ancora"), ["de39cff4", "793d327e:1"]);
    assert_eq!(env.ids("responda apenas"), ["41c65634"]);
    assert_eq!(env.ids("posso ajudar"), ["24f7ca2b-aa6e-43e0-baf7-bb65bf16cd2e", "4ec91fcf-a1c1-432d-bf11-b89b275ab393"]);
    for (rel, provider) in [(CODEX_A, AgentProvider::Codex), (CODEX_B, AgentProvider::Codex), (PI, AgentProvider::Pi)] {
        let want = env.history_ids(rel, provider);
        assert!(!want.is_empty(), "{rel}");
        assert_eq!(env.indexed_ids(rel), want, "{rel}");
    }
    let rows = env.index.search(&terms("continued"), 50).unwrap().unwrap();
    assert_eq!(rows.iter().map(|r| (r.provider.as_str(), r.account.ends_with("/.codex-b"), r.codex_account.as_deref())).collect::<Vec<_>>(),
        [("codex", true, Some("b")), ("codex", true, Some("b"))]);
}

#[test]
fn agent_appended_line_enters_on_the_next_pass() {
    // R5: o rollout Codex lido do offset, como o transcript do Claude.
    let env = Env::agents(quiet());
    env.index.update().unwrap();
    let before = env.indexed_ids(CODEX_B).len();
    append(&env.home().join(CODEX_B), format!("{}\n", json!({"timestamp": "2026-09-23T21:05:00.000Z", "type": "response_item",
        "payload": {"type": "message", "role": "user", "content": [{"type": "input_text", "text": "zebracodex nova"}]}})).as_bytes());
    env.index.update().unwrap();
    assert_eq!(env.indexed_ids(CODEX_B).len(), before + 1);
    assert_eq!(env.indexed_ids(CODEX_B), env.history_ids(CODEX_B, crate::session_open::AgentProvider::Codex));
    assert_eq!(env.index.search(&terms("zebracodex"), 50).unwrap().unwrap().len(), 1);
}

#[test]
fn producer_follows_agent_roots_and_codex_accounts() {
    // R5: as raízes dos agentes entram no observador; conta Codex nova entra pela leitura do catálogo.
    let env = Env::agents(Timing { first_pass: Duration::ZERO, ..quiet() });
    let _producer = env.index.start().unwrap();
    wait_for("1ª passada", || env.index.ready());
    wait_for("observador de pé", || env.watching());
    let pi_line = json!({"type": "message", "id": "zz000001", "parentId": "ee91a905", "timestamp": "2026-08-11T09:50:00.000Z",
        "message": {"role": "user", "content": [{"type": "text", "text": "zebrapi chegou"}], "timestamp": 1786441800000_i64}});
    append(&env.home().join(PI), format!("{pi_line}\n").as_bytes());
    wait_for("linha do Pi indexada sem passada manual", || env.ids("zebrapi") == ["zz000001"]);
    let home = env.home().join(".codex-c");
    fs::create_dir_all(home.join("sessions/2026/10/31")).unwrap();
    let meta = json!({"timestamp": "2026-10-31T10:00:00.000Z", "type": "session_meta", "payload": {"id": "01a0f000-0000-7000-8000-000000000001", "cwd": "/tmp/conta-c"}});
    let user = json!({"timestamp": "2026-10-31T10:00:01.000Z", "type": "response_item",
        "payload": {"type": "message", "role": "user", "content": [{"type": "input_text", "text": "zebracontac"}]}});
    fs::write(home.join("sessions/2026/10/31/rollout-2026-10-31T10-00-00-01a0f000-0000-7000-8000-000000000001.jsonl"), format!("{meta}\n{user}\n")).unwrap();
    fs::write(home.join(".hangar-codex-conta"), r#"{"version": 1, "id": "c"}"#).unwrap();
    wait_for("conta Codex nova indexada", || env.index.search(&terms("zebracontac"), 50).unwrap().unwrap()
        .iter().any(|r| r.codex_account.as_deref() == Some("c")));
    // A linha ilegível de verdade da fixture do Claude (`f8f12de6…`, linha 4) é contada; mais nada.
    let reports = env.reports.lock().unwrap().clone();
    assert_eq!(reports, [("busca.linhas_ilegiveis", "1 linhas ilegíveis puladas em 1 arquivos".to_owned())]);
}

// Falhas que não podem ficar só no journal.

#[test]
fn ingest_failure_of_the_database_keeps_the_index_not_ready_and_reaches_the_diary_once() {
    // Disco cheio ou banco somente leitura na 1ª passada: o índice não fica pronto vazio (a busca
    // varreria `[]` para tudo), a passada devolve o erro e o diário recebe a queda uma vez.
    let env = Env::small(quiet());
    env.index.open_now().unwrap();
    env.db().execute_batch("CREATE TRIGGER falha BEFORE INSERT ON files BEGIN SELECT RAISE(ABORT, 'disco cheio'); END;").unwrap();
    env.write(SID, &[line("zebra listrada", "u1", "user")], false);
    assert!(env.index.update().is_err(), "a passada devolve o erro do banco");
    assert!(!env.index.ready(), "índice não fica pronto vazio");
    assert!(env.index.update().is_err());
    assert_eq!(env.reported("busca.ingestao_falhou"), 1, "a mesma queda vai ao diário uma vez");
    env.db().execute_batch("DROP TRIGGER falha").unwrap();
    env.index.update().unwrap();
    assert!(env.index.ready());
    assert_eq!(env.ids("zebra"), ["u1"]);
    // Voltou e caiu de novo: é outra transição.
    env.db().execute_batch("CREATE TRIGGER falha BEFORE INSERT ON msg BEGIN SELECT RAISE(ABORT, 'somente leitura'); END;").unwrap();
    env.write(SID2, &[line("outra zebra", "u2", "user")], false);
    assert!(env.index.update().is_err());
    assert_eq!(env.reported("busca.ingestao_falhou"), 2);
}

#[test]
fn illegible_lines_are_counted_per_pass_in_one_diary_line() {
    let env = Env::small(quiet());
    env.write(SID, &["lixo 1\n".into(), "lixo 2\n".into(), line("boa", "u1", "user"), "{\"quebrado\n".into()], false);
    env.write(SID2, &["nada disso\n".into(), "nem isto\n".into()], false);
    env.index.update().unwrap();
    let lines = |env: &Env| env.reports.lock().unwrap().iter().filter(|(c, _)| *c == "busca.linhas_ilegiveis").map(|(_, d)| d.clone()).collect::<Vec<_>>();
    let got = lines(&env);
    assert_eq!(got.len(), 1, "{got:?}");
    assert!(got[0].starts_with("5 "), "{got:?}");
    env.index.update().unwrap();
    assert_eq!(lines(&env).len(), 1, "passada sem linha nova não repete");
    append(&env.root.join(format!("acct/projects/-home-u-proj/{SID}.jsonl")), b"mais lixo\n");
    env.index.update().unwrap();
    let got = lines(&env);
    assert_eq!(got.len(), 2, "{got:?}");
    assert!(got[1].starts_with("1 "), "{got:?}");
}

#[test]
fn poisoned_last_pass_lock_does_not_bring_the_index_down() {
    let env = Env::small(Timing { stale: Duration::ZERO, ..quiet() });
    let index = env.index.clone();
    let _ = std::thread::spawn(move || {
        let _held = index.inner.last_pass.lock().unwrap();
        panic!("envenena a trava");
    }).join();
    env.index.update().unwrap();
    assert_eq!(env.index.search(&terms("nada"), 50).unwrap(), Some(vec![]));
}

#[cfg(unix)]
#[test]
fn unreadable_agent_folders_keep_their_conversations_and_reach_the_diary_on_the_transition() {
    // Erro de `read_dir` no Pi ou de leitura do `session_index.jsonl` do Kimi não é "nenhuma conversa":
    // a passada mantém o que o índice tem desses agentes e o diário recebe uma linha por agente.
    use std::os::unix::fs::PermissionsExt;
    let env = Env::agents(quiet());
    env.index.update().unwrap();
    let count = |provider: &str| env.count(&format!("SELECT count(*) FROM files WHERE provider = '{provider}'"));
    assert_eq!((count("pi"), count("kimi")), (1, 1));
    let (pi_root, kimi_index) = (env.home().join(".pi/agent/sessions"), env.home().join(".kimi-code/session_index.jsonl"));
    let mode = |p: &Path, m: u32| fs::set_permissions(p, fs::Permissions::from_mode(m)).unwrap();
    mode(&pi_root, 0o000);
    mode(&kimi_index, 0o000);
    if fs::read_dir(&pi_root).is_ok() { return mode(&pi_root, 0o755); } // root lê mesmo assim
    env.index.update().unwrap();
    env.index.update().unwrap();
    assert_eq!((count("pi"), count("kimi")), (1, 1), "agente ilegível não sai do índice");
    assert_eq!(env.reported("busca.agente_ilegivel"), 2, "uma linha por agente, não por passada");
    mode(&pi_root, 0o755);
    mode(&kimi_index, 0o644);
    env.index.update().unwrap();
    mode(&pi_root, 0o000);
    env.index.update().unwrap();
    mode(&pi_root, 0o755);
    assert_eq!(env.reported("busca.agente_ilegivel"), 3, "voltou e caiu de novo: outra linha");
    assert_eq!(count("pi"), 1);
}

#[test]
fn codex_rollout_with_an_unreadable_transfer_record_stays_and_the_others_go_on() {
    // `thread_phase` com erro não descarta a conversa (nem a apaga do índice) e não para as outras.
    let env = Env::agents(quiet());
    env.index.update().unwrap();
    let before = env.indexed_ids(CODEX_A);
    let home = env.home();
    let key = crate::list::local_transfers::thread_key(&home.join(".codex").to_string_lossy(), "01a05511-e980-71b3-93a9-befda5704b20");
    let indexes = home.join(".hangar/conversation-transfers/indexes");
    fs::create_dir_all(&indexes).unwrap();
    fs::write(indexes.join(format!("thread-{}.json", crate::list::local_transfers::sha256_hex(&key))), "{").unwrap();
    append(&home.join(CODEX_B), format!("{}\n", json!({"timestamp": "2026-09-23T21:05:00.000Z", "type": "response_item",
        "payload": {"type": "message", "role": "user", "content": [{"type": "input_text", "text": "zebracodex nova"}]}})).as_bytes());
    env.index.update().unwrap();
    env.index.update().unwrap();
    assert_eq!(env.indexed_ids(CODEX_A), before, "registro ilegível não apaga a conversa");
    assert_eq!(env.index.search(&terms("zebracodex"), 50).unwrap().unwrap().len(), 1, "o outro rollout segue");
    assert_eq!(env.reported("busca.agente_ilegivel"), 1);
}

#[cfg(unix)]
#[test]
fn a_scan_pool_that_cannot_start_is_reported_once() {
    let env = Env::small(quiet());
    let failed: Result<rayon::ThreadPool, String> = Err("sem threads".into());
    assert!(super::hits::scan_pool(&failed, &env.index).is_none());
    assert!(super::hits::scan_pool(&failed, &env.index).is_none());
    assert_eq!(env.reported("busca.pool_falhou"), 1, "a varredura ficaria sequencial sem nada no diário");
}

#[test]
fn the_scan_counts_every_eligible_conversation_like_the_index() {
    // O total do cabeçalho: as conversas não internas, também quando a varredura para antes de ler todas.
    let env = Env::small(quiet());
    for sid in [SID, SID2, "33333333-3333-3333-3333-333333333333"] { env.write(sid, &[line("zebra aqui", &format!("u{}", &sid[..1]), "user")], false); }
    let prefix = &internal_prefixes()[0];
    env.write("44444444-4444-4444-4444-444444444444", &[line(&format!("{prefix} zebra"), "i1", "user")], false);
    let counted = || super::hits::search_counted(&env.index, "zebra", &std::collections::HashMap::new(), 1).unwrap().1;
    let scanned = counted();
    env.index.update().unwrap();
    assert_eq!((scanned, counted()), (3, 3), "a varredura contava só o que leu antes de parar");
}

#[cfg(unix)]
#[test]
fn an_unreadable_transcript_that_comes_back_is_reported_again_when_it_falls() {
    use std::os::unix::fs::PermissionsExt;
    let env = Env::small(quiet());
    env.write(SID, &[line("zebra boa", "u1", "user")], false);
    let locked = env.write(SID2, &[line("zebra fechada", "f1", "user")], false);
    let mode = |m: u32| fs::set_permissions(&locked, fs::Permissions::from_mode(m)).unwrap();
    let search = || super::hits::search(&env.index, "zebra", &std::collections::HashMap::new(), 50).unwrap();
    mode(0o000);
    if fs::File::open(&locked).is_ok() { mode(0o644); return; } // root lê mesmo assim
    search();
    mode(0o644);
    search();
    mode(0o000);
    search();
    mode(0o644);
    assert_eq!(env.reported("busca.transcript_ilegivel"), 2, "voltou e caiu de novo: outra linha no diário");
}

#[cfg(unix)]
#[test]
fn scan_reports_skipped_files_and_folders_once_each() {
    // A varredura de antes do índice pronto pula arquivo grande, pasta sem permissão e transcript que
    // não abre; cada um vai ao diário uma vez, não a cada busca. Os tetos continuam.
    use std::os::unix::fs::PermissionsExt;
    let env = Env::small(quiet());
    let ok = env.write(SID, &[line("zebra boa", "u1", "user")], false);
    let projects = ok.parent().unwrap().parent().unwrap().to_owned();
    let big = projects.join("-home-u-grande").join(format!("{SID2}.jsonl"));
    fs::create_dir_all(big.parent().unwrap()).unwrap();
    fs::write(&big, line("zebra grande", "g1", "user")).unwrap();
    fs::File::options().write(true).open(&big).unwrap().set_len(65 * 1024 * 1024).unwrap();
    let locked_dir = projects.join("-home-u-trancada");
    fs::create_dir_all(&locked_dir).unwrap();
    fs::write(locked_dir.join("33333333-3333-3333-3333-333333333333.jsonl"), line("zebra trancada", "t1", "user")).unwrap();
    let locked_file = projects.join("-home-u-proj").join("44444444-4444-4444-4444-444444444444.jsonl");
    fs::write(&locked_file, line("zebra fechada", "f1", "user")).unwrap();
    let mode = |p: &Path, m: u32| fs::set_permissions(p, fs::Permissions::from_mode(m)).unwrap();
    mode(&locked_dir, 0o000);
    mode(&locked_file, 0o000);
    let readable = fs::read_dir(&locked_dir).is_ok(); // root lê mesmo assim
    let found = |q: &str| super::hits::search(&env.index, q, &std::collections::HashMap::new(), 50).unwrap()
        .into_iter().map(|h| h.event_id.unwrap()).collect::<Vec<_>>();
    let (first, second) = (found("zebra"), found("zebra"));
    mode(&locked_dir, 0o755);
    mode(&locked_file, 0o644);
    if readable { return; }
    assert_eq!((first, second), (vec!["u1".to_owned()], vec!["u1".to_owned()]));
    for code in ["busca.arquivo_grande", "busca.pasta_ilegivel", "busca.transcript_ilegivel"] {
        assert_eq!(env.reported(code), 1, "{code}: {:?}", env.reports.lock().unwrap());
    }
}

#[test]
fn scan_and_index_give_the_header_cwd() {
    // SRCH-42: conversa que trocou de pasta. O índice guarda o cwd do cabeçalho; a varredura de antes
    // do índice pronto usa a mesma regra, então os dois caminhos dão o mesmo trecho.
    let env = Env::small(quiet());
    let moved = format!("{}\n", json!({"type": "user", "uuid": "u2", "cwd": "/home/u/outra",
        "message": {"role": "user", "content": "zebra mudou"}}));
    env.write(SID, &[line("começo", "u1", "user"), moved], false);
    let cwd = || super::hits::search(&env.index, "zebra", &std::collections::HashMap::new(), 50).unwrap()
        .into_iter().map(|h| h.cwd).collect::<Vec<_>>();
    let scanned = cwd();
    env.index.update().unwrap();
    assert_eq!((scanned, cwd()), (vec![Some("/home/u/proj".to_owned())], vec![Some("/home/u/proj".to_owned())]));
}

#[test]
fn only_a_pass_over_one_second_is_logged_at_info() {
    // IDX-08: o limite entre a linha INFO e a DEBUG da passada.
    assert!(!super::slow_pass(Duration::from_millis(10)));
    assert!(!super::slow_pass(Duration::from_secs(1)));
    assert!(super::slow_pass(Duration::from_millis(1001)));
}

/// Medição da varredura de antes do índice pronto: `copies` cópias de cada transcript Claude das
/// fixtures, cada uma numa pasta de projeto, com o termo raro ausente de todas.
/// `SEARCH_BENCH_COPIES=600 cargo test --lib search::tests::scan_timing -- --ignored --nocapture`
#[test]
#[ignore = "medição"]
fn scan_timing_synthetic() {
    let copies: usize = std::env::var("SEARCH_BENCH_COPIES").ok().and_then(|v| v.parse().ok()).unwrap_or(300);
    let fixtures = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/search/home");
    let sources: Vec<PathBuf> = MTIMES.iter().chain([&RESERVE]).filter(|rel| rel.starts_with(".claude"))
        .map(|rel| fixtures.join(rel)).filter(|p| p.exists()).collect();
    let env = Env::small(quiet());
    let mut bytes = 0;
    for k in 0..copies {
        let dir = env.root.join(format!("acct/projects/-home-u-p{k}"));
        fs::create_dir_all(&dir).unwrap();
        for (j, src) in sources.iter().enumerate() {
            let dst = dir.join(format!("{k:08x}-0000-4000-8000-{j:012x}.jsonl"));
            bytes += fs::copy(src, &dst).unwrap();
            set_mtime(&dst, T0 + (k * sources.len() + j) as u64);
        }
    }
    for q in ["login-claude-navegador", "sessão", "sessao hangar"] {
        let started = Instant::now();
        let hits = super::hits::search(&env.index, q, &std::collections::HashMap::new(), 50).unwrap();
        eprintln!("bench {q:?}: {} arquivos, {} MB, {} trechos, {:.2?}", copies * sources.len(), bytes / 1_000_000, hits.len(), started.elapsed());
    }
}

/// A mesma medida sobre as contas reais da máquina, só leitura; o banco fica num temporário.
/// `SEARCH_BENCH_DIRS` restringe às contas Claude listadas (vírgula) e deixa os outros agentes de fora.
#[test]
#[ignore = "medição"]
fn scan_timing_real_home() {
    let tmp = tempfile::tempdir().unwrap();
    let env = crate::accounts::environment::AccountEnvironment::capture();
    let index = match std::env::var("SEARCH_BENCH_DIRS") {
        Ok(dirs) => {
            let dirs: Vec<String> = dirs.split(',').map(str::to_owned).collect();
            let server = PathBuf::from(&dirs[0]);
            Index::new(tmp.path(), Arc::new(move || Ok(server_accounts(&dirs, &server))), Arc::new(|_, _| {}), quiet())
        }
        Err(_) => Index::for_accounts(crate::accounts::AccountService::new(env), tmp.path(), Arc::new(|_, _| {}), quiet()),
    };
    for q in ["login-claude-navegador", "sessão"] {
        let started = Instant::now();
        let hits = super::hits::search(&index, q, &std::collections::HashMap::new(), 50).unwrap();
        eprintln!("bench real {q:?}: {} trechos, {:.2?}", hits.len(), started.elapsed());
    }
}

#[test]
fn fast_scan_returns_exactly_what_the_sequential_scan_did() {
    // A varredura com pré-filtro e arquivos em paralelo contra a de antes (sequencial, parser em toda
    // linha): mesmos trechos, na mesma ordem, com os mesmos tetos.
    let env = Env::agents(quiet());
    let escaped = env.home().join(".claude-b/projects/-home-u-escapes/33333333-3333-4333-8333-333333333333.jsonl");
    fs::create_dir_all(escaped.parent().unwrap()).unwrap();
    let ascii = |text: &str, uuid: &str| format!("{}\n", crate::transcript::pyjson::dumps(&json!({"type": "user", "uuid": uuid,
        "cwd": "/home/u/escapes", "message": {"role": "user", "content": text}}), false));
    fs::write(&escaped, [ascii("Configuração da Sessão do Hangar", "e1"), ascii("AÇÃO no login-claude", "e2"),
        ascii("log<system-reminder>lembrete</system-reminder>in partido", "e3")].concat()).unwrap();
    set_mtime(&escaped, T0 + 10_000);
    let accounts = env.index.accounts().unwrap();
    let live = std::collections::HashMap::new();
    let queries = ["sessão", "SESSAO", "sessao hangar", "configuracao", "ação", "acao login", "ç", "a", "de", "e", "hangar",
        "recado cx-livre", "\"sem", "kimi", "login", "login-claude-navegador", "Hangar Sessão ação", "zzz-nada"];
    let mut found = 0;
    for q in queries {
        let terms = super::hits::terms(q);
        for limit in [50, 7, 2] {
            let fast = super::hits::scan_with(&env.index, &accounts, &terms, &live, limit, true);
            let slow = super::hits::scan_with(&env.index, &accounts, &terms, &live, limit, false);
            assert_eq!(fast, slow, "q={q:?} limit={limit}");
            found += slow.0.len();
        }
    }
    assert!(found > 100, "as consultas acham trechos: {found}");
    let ids = |q: &str| super::hits::scan_with(&env.index, &accounts, &super::hits::terms(q), &live, 50, true).0
        .into_iter().filter_map(|h| h.event_id).filter(|id| id.starts_with('e') && id.len() == 2).collect::<Vec<_>>();
    assert_eq!((ids("configuracao"), ids("login"), ids("ação")), (vec!["e1".to_owned()], vec!["e2".to_owned(), "e3".to_owned()], vec!["e2".to_owned(), "e1".to_owned()]),
        "o `\\u` do JSON, a junção do `<system-reminder>` e o acento (D28: \"AÇÃO\" inteira antes de \"Configuração\")");
}

#[test]
fn corruption_with_the_index_open_rebuilds_on_the_next_pass() {
    // A conexão do escritor não fica quebrada: o arquivo corrompido sai e a passada seguinte refaz.
    let env = Env::small(quiet());
    env.write(SID, &[line("agulha um", "u1", "user")], false);
    env.index.update().unwrap();
    // Tudo no arquivo principal e o cache do escritor invalidado (outra escrita mudou o WAL); depois as
    // páginas além da primeira viram lixo no mesmo arquivo.
    let other = env.db();
    other.execute_batch("PRAGMA wal_checkpoint(TRUNCATE); INSERT INTO meta(k, v) VALUES ('t', '1'); PRAGMA wal_checkpoint(TRUNCATE);").unwrap();
    drop(other);
    let path = env.index.path().to_owned();
    let size = fs::metadata(&path).unwrap().len() as usize;
    let mut file = fs::OpenOptions::new().write(true).open(&path).unwrap();
    std::io::Seek::seek(&mut file, std::io::SeekFrom::Start(4096)).unwrap();
    std::io::Write::write_all(&mut file, &b"lixo".repeat(size / 4)).unwrap();
    drop(file);
    env.write(SID2, &[line("agulha dois", "u2", "user")], false);
    assert!(env.index.update().is_err(), "a passada vê a corrupção");
    assert!(!env.index.ready(), "até a passada refazer o índice, a busca vai à varredura");
    env.index.update().unwrap();
    let mut ids = env.ids("agulha");
    ids.sort();
    assert_eq!(ids, ["u1", "u2"]);
}

#[cfg(unix)]
#[test]
fn transcript_that_does_not_open_reaches_the_diary_once_per_fall() {
    use std::os::unix::fs::PermissionsExt;
    let env = Env::small(quiet());
    let locked = env.write(SID, &[line("agulha trancada", "u1", "user")], false);
    let mode = |m: u32| fs::set_permissions(&locked, fs::Permissions::from_mode(m)).unwrap();
    mode(0o000);
    if fs::File::open(&locked).is_ok() { mode(0o644); return; } // root lê mesmo assim
    env.index.update().unwrap();
    env.index.update().unwrap();
    assert_eq!(env.reported("busca.indice_transcript_ilegivel"), 1, "a mesma queda não se repete a cada passada");
    mode(0o644);
    env.index.update().unwrap();
    // Arquivo lido não volta a ser aberto: outra linha muda o stat.
    append(&locked, line("mais", "u2", "user").as_bytes());
    mode(0o000);
    env.index.update().unwrap();
    mode(0o644);
    assert_eq!(env.reported("busca.indice_transcript_ilegivel"), 2, "voltou e caiu de novo: outra linha no diário");
}

#[test]
fn a_huge_line_reaches_the_diary_once_and_counts_as_illegible() {
    let env = Env::small(quiet());
    let huge = format!("{}\n", json!({"type": "user", "uuid": "u0", "message": {"role": "user", "content": "x".repeat(ingest::HUGE_LINE_BYTES)}}));
    env.write(SID, &[huge, line("agulha", "u1", "user")], false);
    env.index.update().unwrap();
    env.index.update().unwrap();
    assert_eq!(env.ids("agulha"), ["u1"]);
    assert_eq!((env.reported("busca.linha_enorme"), env.reported("busca.linhas_ilegiveis")), (1, 1), "{:?}", env.reports.lock().unwrap());
}

#[test]
fn a_huge_line_that_comes_back_after_a_clean_read_is_reported_again() {
    let env = Env::small(quiet());
    let huge = || format!("{}\n", json!({"type": "user", "uuid": "u0", "message": {"role": "user", "content": "x".repeat(ingest::HUGE_LINE_BYTES)}}));
    let f = env.write(SID, &[huge(), line("agulha", "u1", "user")], false);
    env.index.update().unwrap();
    append(&f, line("limpa", "u2", "user").as_bytes());
    env.index.update().unwrap();
    append(&f, huge().as_bytes());
    env.index.update().unwrap();
    assert_eq!(env.reported("busca.linha_enorme"), 2, "a leitura sem linha enorme libera o registro");
}
