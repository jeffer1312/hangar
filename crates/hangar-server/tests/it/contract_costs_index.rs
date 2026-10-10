use crate::common;
use common::costs::*;
use hangar_server::costs::areas::AreaMap;
use hangar_server::costs::claude;
use hangar_server::costs::codex;
use hangar_server::costs::index::Index;
use hangar_server::costs::index::Fold;
use serde_json::json;
use serde_json::{Map, Value};
use std::path::Path;

fn sync_claude(ix: &Index, base: &Path, areas: &AreaMap) {
    let root = base.join("claude/projects");
    let files = hangar_server::costs::collect::list_files(&root, |n| n.ends_with(".jsonl"));
    ix.sync(&format!("claude:{}", root.display()), &files, &claude::new_fold(&root), claude::VERSION,
            areas.signature(), &|e| areas.area_lines(e), &progress()).unwrap();
}

fn claude_golden(g: &Value) -> Value {
    Value::Object(g.as_object().unwrap().iter().filter(|(k, _)| k.starts_with("claude/"))
        .map(|(k, v)| (k.clone(), v.clone())).collect::<Map<_, _>>())
}

fn sync_codex(ix: &Index, base: &Path, areas: &AreaMap) {
    // Como a varredura (`rollout_owners`): o rollout entra no índice pelo caminho canônico.
    let mut files: Vec<_> = hangar_server::costs::collect::list_files(&base.join("codex/sessions"),
        |n| n.starts_with("rollout-") && n.ends_with(".jsonl")).iter().map(|p| std::fs::canonicalize(p).unwrap()).collect();
    files.sort();
    ix.sync(&format!("codex:{}", base.join("codex").display()), &files, &codex::new_fold,
        codex::VERSION, areas.signature(), &|e| areas.area_lines(e), &progress()).unwrap();
}

fn codex_golden(g: &Value) -> Value {
    Value::Object(g.as_object().unwrap().iter().filter(|(k, _)| k.starts_with("codex/"))
        .map(|(k, v)| (k.clone(), v.clone())).collect::<Map<_, _>>())
}

#[test]
fn codex_index_matches_python() {
    let (_d, base) = fixtures_copy();
    let areas = AreaMap::load(Path::new("/nao/existe.json"));
    let ix = Index::open(&base.join("../idx")).unwrap();
    sync_codex(&ix, &base, &areas);
    assert_close(&dump(&ix, &base, "codex/"), &codex_golden(&golden_index()), "codex");
}

#[test]
fn codex_resumed_in_two_halves_matches_python() {
    let (_d, base) = fixtures_copy();
    let areas = AreaMap::load(Path::new("/nao/existe.json"));
    let ix = Index::open(&base.join("../idx")).unwrap();
    let rest = halve_all(&base);
    sync_codex(&ix, &base, &areas);
    for (p, tail) in rest { append(&p, &tail); }
    sync_codex(&ix, &base, &areas);
    assert_close(&dump(&ix, &base, "codex/"), &codex_golden(&golden_index()["__resumed__"]), "codex retomado");
}

#[test]
fn single_rollout_cost_reads_only_growth_and_keeps_existing_scope() {
    let (_d, base) = fixtures_copy();
    let areas = AreaMap::load(Path::new("/nao/existe.json"));
    let ix = Index::open(&base.join("../idx")).unwrap();
    let p = base.join("codex/sessions/2026/09/30/rollout-c1.jsonl");
    let rows = codex::session_rows(&ix, &p, &areas).unwrap();
    assert!(!rows.is_empty());
    sync_codex(&ix, &base, &areas);
    assert_eq!(codex::session_rows(&ix, &p, &areas).unwrap(), rows);
    assert_eq!(ix.read_costs(Some("codex:avulso"), None, None).unwrap().len(), 0);
    assert_eq!(rows, ix.read_costs(None, None, None).unwrap().into_iter()
        .filter(|r| r.session_id == rows[0].session_id).collect::<Vec<_>>());
    assert!(codex::session_rows(&ix, &base.join("missing.jsonl"), &areas).is_none());
}

#[test]
fn single_rollout_cost_with_non_canonical_path_shares_the_scan_entry() {
    // `..` reproduz em qualquer sistema o que o symlink do /var (macOS) e o separador misto
    // (Windows) fazem: um caminho que não é o canônico que a varredura grava.
    let (_d, base) = fixtures_copy();
    let areas = AreaMap::load(Path::new("/nao/existe.json"));
    let ix = Index::open(&base.join("../idx")).unwrap();
    let p = base.join("codex/sessions/2026/../2026/09/30/rollout-c1.jsonl");
    let rows = codex::session_rows(&ix, &p, &areas).unwrap();
    sync_codex(&ix, &base, &areas);
    assert_eq!(ix.read_costs(Some("codex:avulso"), None, None).unwrap().len(), 0);
    assert_eq!(rows, ix.read_costs(None, None, None).unwrap().into_iter()
        .filter(|r| r.session_id == rows[0].session_id).collect::<Vec<_>>());
}

fn codex_record(kind: &str, second: u32, payload: Value) -> Vec<u8> {
    let mut raw = serde_json::to_vec(&json!({"type":kind, "timestamp":format!("2026-10-01T12:00:{second:02}Z"), "payload":payload})).unwrap();
    raw.push(b'\n');
    raw
}

fn codex_start() -> codex::CodexFold {
    let mut fold = codex::new_fold(Path::new("rollout-synthetic.jsonl"));
    fold.line(&codex_record("session_meta", 0, json!({"id":"synthetic","model_provider":"openai-codex","source":"subagent"})));
    fold.line(&codex_record("turn_context", 1, json!({"turn_id":"turn","model":"gpt-5.6-sol","cwd":"/repo/synthetic"})));
    fold
}

#[test]
fn codex_turn_order_preserves_legacy_then_modern_insertion() {
    let mut fold = codex_start();
    fold.line(&codex_record("turn_context", 1, json!({"turn_id":"modern-first"})));
    fold.line(&codex_record("token_usage_record", 2, json!({"thread_id":"synthetic","usage":{"input_tokens":100}})));
    fold.line(&codex_record("turn_context", 3, json!({"turn_id":"legacy-first"})));
    fold.line(&codex_record("event_msg", 4, json!({"type":"token_count","info":{"total_token_usage":{"input_tokens":200}}})));
    let out = fold.close();
    assert_eq!(out.costs[0].ts.iso(), "2026-10-01T09:00:04-03:00");
    assert_eq!(out.costs[0].input, 300);
    let entries = out.areas.unwrap();
    assert_eq!(entries.turns[0].1[0].values[0], 200);
    assert_eq!(entries.turns[1].1[0].values[0], 100);
}

#[test]
fn codex_clamps_cache_and_marks_only_above_long_threshold() {
    let mut fold = codex_start();
    for (id, input, read, write, output) in [("threshold", 272000, 300000, 50, -2), ("long", 272001, 200000, 100000, 3)] {
        fold.line(&codex_record("token_usage_record", 2, json!({"thread_id":"synthetic","response_id":id,
            "usage":{"input_tokens":input,"cached_input_tokens":read,"cache_write_input_tokens":write,"output_tokens":output}})));
    }
    let saved = serde_json::to_vec(&fold).unwrap();
    let out = fold.close();
    assert_eq!(saved, serde_json::to_vec(&fold).unwrap());
    assert_eq!(out.costs, fold.close().costs);
    assert_eq!(out.costs.len(), 2);
    assert_eq!(out.costs[0].project, "desconhecido");
    assert_eq!(out.costs[0].provider, "openai");
    assert_eq!((out.costs[0].input, out.costs[0].cache_read, out.costs[0].cache_write, out.costs[0].output), (0, 272000, 0, 0));
    assert!(!out.costs[0].codex_long_context);
    assert_eq!((out.costs[1].input, out.costs[1].cache_read, out.costs[1].cache_write), (0, 200000, 72001));
    assert!(out.costs[1].codex_long_context);
    assert!(out.costs.iter().all(|r| r.subagente && r.account_id.is_none() && !r.fast && r.cache_write_1h == 0));
}

#[test]
fn codex_script_quoted_literal_rejects_escaped_newline_like_python() {
    let mut fold = codex_start();
    fold.line(&codex_record("response_item", 2, json!({"type":"custom_tool_call", "name":"exec", "input":"tools.exec_command({cmd: 'cat\\\nbackend/a.py'})"})));
    let out = fold.close();
    assert_eq!(out.usage.len(), 1);
    assert_eq!(out.usage[0].nome, "exec_command");
}

#[test]
fn codex_json_literal_keeps_lone_surrogate_as_one_character() {
    let mut fold = codex_start();
    fold.line(&codex_record("response_item", 2, json!({"type":"custom_tool_call","name":"exec","call_id":"script",
        "input":r#"tools.exec_command({cmd: "cat /h/skills/a\ud800/SKILL.md"})"#})));
    fold.line(&codex_record("response_item", 3, json!({"type":"custom_tool_call_output","call_id":"script","output":"é".repeat(100)})));
    let out = fold.close();
    let skill = out.usage.iter().find(|r| r.tipo == "skill").unwrap();
    assert_eq!(skill.nome.chars().count(), 2, "json.loads decodifica o escape solto");
    assert_eq!(skill.ctx_chars, 100);
}

#[test]
fn codex_long_context_effective_resume_and_single_growth_keep_scope() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    let (_d, base) = fixtures_copy();
    // Canônico como a varredura (`rollout_owners`): o custo avulso abaixo cai na mesma entrada.
    let path = std::fs::canonicalize(base.join("codex/sessions/2026/09/30/rollout-c3.jsonl")).unwrap();
    let areas = AreaMap::load(Path::new("/nao/existe.json"));
    let ix = Index::open(&base.join("../idx")).unwrap();
    let creations = AtomicUsize::new(0);
    let tracked = |p: &Path| { creations.fetch_add(1, Ordering::Relaxed); codex::new_fold(p) };
    let sync = || ix.sync("codex:synthetic", &[path.clone()], &tracked, codex::VERSION,
        areas.signature(), &|e| areas.area_lines(e), &progress()).unwrap();
    sync();
    append(&path, &codex_record("token_usage_record", 3, json!({"thread_id":"c3","turn_id":"long-turn","response_id":"long-second",
        "usage":{"input_tokens":400000,"cached_input_tokens":30000,"cache_write_input_tokens":6000,"output_tokens":300}})));
    append(&path, &codex_record("token_usage_record", 4, json!({"thread_id":"c3","turn_id":"long-turn","response_id":"short-third",
        "usage":{"input_tokens":100,"cached_input_tokens":10,"output_tokens":5}})));
    sync();
    assert_eq!(creations.load(Ordering::Relaxed), 1, "a cauda precisa acumular sobre o estado salvo");
    let rows = ix.read_costs(Some("codex:synthetic"), None, None).unwrap();
    assert_eq!(rows.len(), 2);
    assert_eq!((rows[0].input, rows[0].output, rows[0].cache_read, rows[0].cache_write), (639000, 500, 50000, 11000));
    assert_eq!((rows[1].input, rows[1].output), (90, 5));
    assert!(rows[0].codex_long_context && rows[0].subagente);
    assert_eq!(codex::session_rows(&ix, &path, &areas).unwrap(), rows);
    append(&path, &codex_record("token_usage_record", 5, json!({"thread_id":"c3","turn_id":"long-turn","response_id":"short-fourth",
        "usage":{"input_tokens":20,"output_tokens":2}})));
    let grown = codex::session_rows(&ix, &path, &areas).unwrap();
    assert_eq!(grown[1].input, 110);
    assert_eq!(ix.read_costs(Some("codex:synthetic"), None, None).unwrap(), grown);
    assert!(ix.read_costs(Some("codex:avulso"), None, None).unwrap().is_empty());
    let got = dump(&ix, &base, "codex/");
    let areas = &got["codex/sessions/2026/09/30/rollout-c3.jsonl"]["areas"];
    assert_eq!(areas[0][11], 639110);
    assert_eq!(areas[0][12], 507);
    assert_eq!(areas[0][13], 11000);
    assert_eq!(areas[0][14], 50010);
    assert_eq!(areas[0][21], 1);
}

#[test]
fn codex_legacy_counter_reset_counts_entire_response_and_other_thread_suppresses_turn() {
    let mut fold = codex_start();
    for (second, total) in [(2, json!({"input_tokens":100,"cached_input_tokens":20,"output_tokens":10})),
        (3, json!({"input_tokens":150,"cached_input_tokens":30,"output_tokens":15})),
        (4, json!({"input_tokens":150,"cached_input_tokens":30,"output_tokens":15})),
        (5, json!({"input_tokens":20,"cached_input_tokens":5,"output_tokens":3}))] {
        fold.line(&codex_record("event_msg", second, json!({"type":"token_count","info":{"total_token_usage":total}})));
    }
    let out = fold.close();
    assert_eq!((out.costs[0].input, out.costs[0].cache_read, out.costs[0].output), (135, 35, 18));
    let snapshot = serde_json::to_vec(&fold).unwrap();
    fold.line(&codex_record("token_usage_record", 6, json!({"thread_id":"parent","turn_id":"turn","usage":{"input_tokens":9999}})));
    assert!(fold.close().costs.is_empty(), "outra thread só abre o turno moderno vazio");
    let mut resumed: codex::CodexFold = serde_json::from_slice(&snapshot).unwrap();
    for input in [150, 9999] {
        resumed.line(&codex_record("token_usage_record", 7, json!({"thread_id":"synthetic","response_id":"new",
            "usage":{"input_tokens":input,"cached_input_tokens":30,"output_tokens":15}})));
    }
    let rows = resumed.close().costs;
    assert_eq!((rows[0].input, rows[0].cache_read, rows[0].output), (135, 35, 18));
}

#[test]
fn codex_scripts_unicode_window_outputs_agents_and_compaction_match_python() {
    let mut fold = codex_start();
    let input = format!("tools.apply_patch(\"*** Update File: backend/a.py\\n*** Add File: frontend/a.svelte\"); tools.view_image({{path:'a\\'b.png'}}); tools.exec_command({{/*{}*/ cmd: `cat /h/skills/database-x/SKILL.md`, workdir: \"/repo/synthetic\"}})", "é".repeat(3000));
    fold.line(&codex_record("response_item", 2, json!({"type":"custom_tool_call","name":"exec","call_id":"script","input":input})));
    fold.line(&codex_record("response_item", 3, json!({"type":"custom_tool_call_output","call_id":"script","output":[{"text":"é".repeat(302)},{"text":"a"}, "ignored"]})));
    fold.line(&codex_record("response_item", 4, json!({"type":"function_call","name":"spawn_agent","arguments":"{\"agent_type\":[\"worker\",true]}"})));
    fold.line(&codex_record("event_msg", 5, json!({"type":"token_count","info":{"total_token_usage":{"input_tokens":100},"last_token_usage":{"input_tokens":100,"cached_input_tokens":20}}})));
    fold.line(&codex_record("compacted", 6, json!({})));
    fold.line(&codex_record("event_msg", 7, json!({"type":"token_count","info":{"total_token_usage":{"input_tokens":120},"last_token_usage":{"input_tokens":20}}})));
    let out = fold.close();
    let skill = out.usage.iter().find(|r| r.tipo == "skill").unwrap();
    assert_eq!((skill.ctx_chars, skill.chamadas, skill.ocupados, skill.respostas, skill.ocupados_eq), (101, 1, 101, 1, 83));
    assert_eq!(skill.nome, "database-x");
    assert!(out.usage.iter().any(|r| r.tipo == "agente" && r.nome == "['worker', True]" && r.origem == "sozinho"));
    assert_eq!(out.usage.iter().find(|r| r.nome == "exec_command").unwrap().ctx_chars, 0);
    assert_eq!(out.usage.iter().find(|r| r.nome == "apply_patch").unwrap().ctx_chars, 101);
    assert!(out.usage.iter().any(|r| r.tipo == "imagem" && r.nome == "lida:view_image"));
    let entries = out.areas.unwrap();
    assert_eq!(entries.header.fonte.as_deref(), Some("codex"));
    let regs = &entries.turns[0].0;
    assert!(matches!(&regs[1], hangar_server::costs::areas::ToolReg::C { cwd, .. } if cwd == "/repo/synthetic"));
    assert!(matches!(&regs[0], hangar_server::costs::areas::ToolReg::P { paths, .. } if paths == &vec!["backend/a.py".to_string(),"frontend/a.svelte".to_string()]));
    assert_eq!(entries.turns[0].1[0].values, [100, 0, 0, 0, 0]);
    assert_eq!(entries.turns[0].1[1].values, [20, 0, 0, 0, 0]);
}

#[test]
fn codex_last_call_window_uses_characters_and_prior_call_runs_to_next_mark() {
    let mut fold = codex_start();
    for (second, input) in [
        (2, format!("tools.exec_command({{/*{}*/ cmd: 'cat backend/a.py'}})", "é".repeat(3900))),
        (3, format!("tools.exec_command({{/*{}*/ cmd: 'ignored backend/b.py'}})", "é".repeat(4000))),
        (4, format!("tools.exec_command({{/*{}*/ cmd: 'pwd'}}); tools.unknown()", "é".repeat(4100))),
    ] {
        fold.line(&codex_record("response_item", second, json!({"type":"custom_tool_call","name":"exec","input":input})));
    }
    let out = fold.close();
    assert_eq!(out.usage.iter().find(|r| r.nome == "exec_command").unwrap().chamadas, 3);
    let commands = out.usage.iter().filter(|r| r.tipo == "bash").map(|r| r.nome.as_str()).collect::<Vec<_>>();
    assert_eq!(commands, ["cat", "pwd"]);
}

#[test]
fn codex_area_signature_rebuild_keeps_costs_usage_and_saved_targets() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    let (_d, base) = fixtures_copy();
    let path = base.join("codex/sessions/2026/09/30/rollout-c1.jsonl");
    let ix = Index::open(&base.join("../idx")).unwrap();
    let areas = AreaMap::load(Path::new("/nao/existe.json"));
    let creations = AtomicUsize::new(0);
    let tracked = |p: &Path| { creations.fetch_add(1, Ordering::Relaxed); codex::new_fold(p) };
    ix.sync("synthetic", &[path.clone()], &tracked, codex::VERSION,
        areas.signature(), &|e| areas.area_lines(e), &progress()).unwrap();
    let before = dump(&ix, &base, "codex/");
    let mapping = base.join("areas.json");
    std::fs::write(&mapping, br#"{"padrao":[["changed",["*"]]]}"#).unwrap();
    let changed = AreaMap::load(&mapping);
    ix.sync("synthetic", &[path], &tracked, codex::VERSION,
        changed.signature(), &|e| changed.area_lines(e), &progress()).unwrap();
    assert_eq!(creations.load(Ordering::Relaxed), 1, "assinatura refaz alvos sem consumir o rollout");
    let after = dump(&ix, &base, "codex/");
    let name = "codex/sessions/2026/09/30/rollout-c1.jsonl";
    assert_eq!(before[name]["custo"], after[name]["custo"]);
    assert_eq!(before[name]["uso"], after[name]["uso"]);
    assert_eq!(after[name]["areas"].as_array().unwrap().len(), 1);
    assert_eq!(after[name]["areas"][0][4], "changed");
    assert_eq!(after[name]["areas"][0][11], 1000);
}

#[test]
fn codex_fork_metadata_context_and_counter_survive_snapshot() {
    let mut fold = codex_start();
    fold.line(&codex_record("session_meta", 0, json!({"id":"parent","cwd":"/parent","source":"cli","model_provider":"anthropic"})));
    fold.line(&serde_json::to_vec(&json!({"type":"turn_context","timestamp":"2026-09-01T12:00:00Z","payload":{"turn_id":"parent-turn","model":"gpt-5.5"}})).unwrap());
    fold.line(&codex_record("event_msg", 2, json!({"type":"token_count","info":{"total_token_usage":{"input_tokens":100}}})));
    fold.line(&codex_record("response_item", 3, json!({"type":"function_call","name":"Inherited"})));
    let snapshot = serde_json::to_vec(&fold).unwrap();
    assert!(fold.close().costs.is_empty());
    assert!(fold.close().usage.is_empty());
    let mut resumed: codex::CodexFold = serde_json::from_slice(&snapshot).unwrap();
    resumed.line(&codex_record("turn_context", 4, json!({"turn_id":"child-turn","model":"gpt-5.6-sol","cwd":"/child"})));
    resumed.line(&codex_record("event_msg", 5, json!({"type":"token_count","info":{"total_token_usage":{"input_tokens":130}}})));
    resumed.line(&codex_record("response_item", 6, json!({"type":"function_call","name":"Own"})));
    let out = resumed.close();
    assert_eq!(out.costs[0].input, 30);
    assert_eq!(out.costs[0].project, "desconhecido");
    assert_eq!(out.costs[0].provider, "openai");
    assert_eq!(out.costs[0].session_id, "synthetic");
    assert!(out.costs[0].subagente);
    assert_eq!(out.usage.len(), 1);
    assert_eq!(out.usage[0].nome, "Own");
    assert_eq!(out.usage[0].cwd, "/child");
    assert!(out.usage[0].subagente);
}

#[test]
fn claude_index_matches_python() {
    let (_d, base) = fixtures_copy();
    let areas = AreaMap::load(Path::new("/nao/existe.json"));
    let ix = Index::open(&base.join("../idx")).unwrap();
    sync_claude(&ix, &base, &areas);
    assert_close(&dump(&ix, &base, "claude/"), &claude_golden(&golden_index()), "claude");
}

#[test]
fn claude_resumed_in_two_halves_matches_python() {
    let (_d, base) = fixtures_copy();
    let areas = AreaMap::load(Path::new("/nao/existe.json"));
    let ix = Index::open(&base.join("../idx")).unwrap();
    let rest = halve_all(&base);
    sync_claude(&ix, &base, &areas);
    for (p, tail) in rest { append(&p, &tail); }
    sync_claude(&ix, &base, &areas);
    assert_close(&dump(&ix, &base, "claude/"), &claude_golden(&golden_index()["__resumed__"]), "claude retomado");
}

fn response(id: &str, input: i64) -> Vec<u8> {
    let mut raw = serde_json::to_vec(&json!({"type":"assistant", "timestamp":"2026-10-01T12:00:00Z", "cwd":"/repo/synthetic",
        "requestId":id, "message":{"id":id, "model":"claude-sonnet-5", "usage":{"input_tokens":input,"output_tokens":2},
        "content":[{"type":"tool_use","id":id,"name":"Read","input":{"file_path":"backend/a.py"}}]}})).unwrap();
    raw.push(b'\n');
    raw
}

#[test]
fn claude_effective_resume_preserves_parent_and_child_state() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    let d = tempfile::tempdir().unwrap();
    let root = d.path().join("costs/claude/projects");
    let parent = root.join("project/main.jsonl");
    let child = root.join("project/main/subagents/agent-a.jsonl");
    std::fs::create_dir_all(child.parent().unwrap()).unwrap();
    for path in [&parent, &child] { std::fs::write(path, response("first", 10)).unwrap(); }
    let areas = AreaMap::load(Path::new("/nao/existe.json"));
    let ix = Index::open(&d.path().join("idx")).unwrap();
    let creations = AtomicUsize::new(0);
    let factory = claude::new_fold(&root);
    let tracked = |p: &Path| { creations.fetch_add(1, Ordering::Relaxed); factory(p) };
    for round in 0..2 {
        if round == 1 { for path in [&parent, &child] { append(path, &response("second", 20)); } }
        ix.sync("synthetic", &[parent.clone(), child.clone()], &tracked, claude::VERSION,
            areas.signature(), &|e| areas.area_lines(e), &progress()).unwrap();
    }
    assert_eq!(creations.load(Ordering::Relaxed), 2, "a segunda passada precisa retomar o estado salvo");
    let got = dump(&ix, &d.path().join("costs"), "claude/");
    for (path, child) in [("claude/projects/project/main.jsonl", 0), ("claude/projects/project/main/subagents/agent-a.jsonl", 1)] {
        assert_eq!(got[path]["custo"][0][6], 30);
        assert_eq!(got[path]["custo"][0][10], child);
        assert_eq!(got[path]["uso"][0][8], 2);
        assert_eq!(got[path]["uso"][0][21], child);
        assert_eq!(got[path]["areas"][0][8], 2);
        assert_eq!(got[path]["areas"][0][11], 30);
        assert_eq!(got[path]["areas"][0][21], child);
    }
    let mapping = d.path().join("areas.json");
    std::fs::write(&mapping, br#"{"padrao":[["reclassified",["*.py"]]]}"#).unwrap();
    let changed = AreaMap::load(&mapping);
    ix.sync("synthetic", &[parent, child], &tracked, claude::VERSION,
        changed.signature(), &|e| changed.area_lines(e), &progress()).unwrap();
    assert_eq!(creations.load(Ordering::Relaxed), 2, "áreas novas não releem transcript");
    let updated = dump(&ix, &d.path().join("costs"), "claude/");
    for (path, old) in got.as_object().unwrap() {
        assert_eq!(updated[path]["custo"], old["custo"]);
        assert_eq!(updated[path]["uso"], old["uso"]);
        assert_eq!(updated[path]["areas"][0][4], "reclassified");
        assert_eq!(updated[path]["areas"][0][11], 30);
    }
}

#[test]
fn claude_snapshot_fragment_and_repeated_close_preserve_state() {
    let mut fold = claude::ClaudeFold::new("project/main".into(), false, false);
    fold.line(&response("first", 10));
    let saved = serde_json::to_vec(&fold).unwrap();
    let first = fold.close();
    assert_eq!(first.costs, fold.close().costs);
    assert_eq!(saved, serde_json::to_vec(&fold).unwrap(), "fechar não altera o snapshot");
    let mut resumed: claude::ClaudeFold = serde_json::from_slice(&saved).unwrap();
    resumed.line(&response("second", 20));
    assert_eq!(resumed.close().costs[0].input, 30);

    let d = tempfile::tempdir().unwrap();
    let root = d.path().join("costs/claude/projects");
    std::fs::create_dir_all(&root).unwrap();
    let path = root.join("fragment.jsonl");
    let mut raw = response("first", 10);
    let mut second = response("second", 20);
    second.pop();
    raw.extend(second);
    std::fs::write(&path, raw).unwrap();
    let ix = Index::open(&d.path().join("idx")).unwrap();
    let areas = AreaMap::load(Path::new("/nao/existe.json"));
    let sync = || ix.sync("fragment", &[path.clone()], &claude::new_fold(&root), claude::VERSION,
        areas.signature(), &|e| areas.area_lines(e), &progress()).unwrap();
    sync();
    assert_eq!(dump(&ix, &d.path().join("costs"), "claude/")["claude/projects/fragment.jsonl"]["custo"][0][6], 30);
    let mut tail = b"\n".to_vec();
    tail.extend(response("third", 40));
    append(&path, &tail);
    sync();
    let got = dump(&ix, &d.path().join("costs"), "claude/");
    assert_eq!(got["claude/projects/fragment.jsonl"]["custo"][0][6], 70);
    assert_eq!(got["claude/projects/fragment.jsonl"]["uso"][0][8], 3);
}

#[test]
fn claude_fallback_keeps_surrogates_and_invalid_bytes_but_rejects_non_objects() {
    let mut fold = claude::ClaudeFold::new(String::new(), false, false);
    fold.line(r#"{"type":"attachment","attachment":{"type":"surrogate","content":"a\ud800é"}}"#.as_bytes());
    let mut raw = br#"{"type":"attachment","attachment":{"type":"invalid","content":"a"#.to_vec();
    raw.push(0xff);
    raw.extend(r#"é"}}"#.as_bytes());
    fold.line(&raw);
    let before = serde_json::to_value(fold.close().usage).unwrap();
    assert_eq!(before[0]["ctx_chars"], 3);
    assert_eq!(before[1]["ctx_chars"], 3);
    // Serde aceita sequência como struct; o JSON Python exige objeto nesta entrada.
    fold.line(br#"["attachment",null,null,null,null,null,null,null,{"type":"array","content":"usage"},null,null]"#);
    fold.line(br#"null"#);
    assert_eq!(before, serde_json::to_value(fold.close().usage).unwrap());
}

#[test]
fn claude_ignored_model_discards_usage_tools_and_context_changes() {
    let mut fold = claude::ClaudeFold::new(String::new(), false, false);
    fold.line(&response("first", 10));
    fold.line(br#"{"type":"assistant","cwd":"/ignored","message":{"model":"  <synthetic>  ","usage":{"input_tokens":999},"content":[{"type":"tool_use","name":"Ignored","input":{}}]}}"#);
    fold.line(r#"{"type":"user","message":{"content":[{"type":"tool_result","content":"é"}]}}"#.as_bytes());
    let out = fold.close();
    assert_eq!(out.costs[0].input, 10);
    assert!(out.usage.iter().all(|r| r.nome != "Ignored" && r.cwd == "/repo/synthetic" && r.model == "claude-sonnet-5"));
}

#[test]
fn claude_typed_and_fallback_keep_the_same_fields() {
    let mut fast = claude::ClaudeFold::new(String::new(), false, false);
    let mut fallback = claude::ClaudeFold::new(String::new(), false, false);
    fast.line(r#"{"type":"attachment","cwd":"/repo/synthetic","rendered":null,"attachment":{"type":"hook_context","hookName":"Start","content":"[fixture] Texto sintético"}}"#.as_bytes());
    // Campo repetido recusa o struct tipado, mas json.loads mantém a última versão.
    fallback.line(r#"{"type":"ignored","type":"attachment","cwd":"/repo/synthetic","rendered":null,"attachment":{"type":"hook_context","hookName":"Start","content":"[fixture] Texto sintético"}}"#.as_bytes());
    assert_eq!(fast.close().usage, fallback.close().usage);
}

#[test]
fn claude_agent_type_uses_python_string_conversion() {
    let mut fold = claude::ClaudeFold::new(String::new(), false, false);
    let mut raw = response("agent", 1);
    let mut value: Value = serde_json::from_slice(&raw).unwrap();
    value["message"]["content"] = json!([{"type":"tool_use","name":"Agent","input":{"subagent_type":["worker",true]}}]);
    raw = serde_json::to_vec(&value).unwrap();
    fold.line(&raw);
    let rows = fold.close().usage;
    assert_eq!(rows.iter().find(|r| r.tipo == "agente").unwrap().nome, "['worker', True]");
}

#[test]
fn claude_factory_distinguishes_absolute_and_relative_subagent_markers() {
    let root = Path::new("/synthetic/subagents/projects");
    let mut fold = claude::new_fold(root)(&root.join("project/main.jsonl"));
    fold.line(&response("first", 1));
    let out = fold.close();
    assert_eq!(out.costs[0].session_id, "project/main");
    assert!(out.costs[0].subagente);
    assert!(!out.usage[0].subagente);
    assert_eq!(out.areas.unwrap().header.subagente, Some(false));
}

#[test]
fn claude_hook_label_accepts_only_string_or_list_content() {
    for (content, name, plugin) in [
        (json!({"text":"[fixture] Contexto"}), "hook_context:Start", ""),
        (json!("[fixture] Contexto"), "hook_context:Start · [fixture] Contexto", "fixture"),
        (json!([{}, "  ", {"text":"[fixture] Contexto\nSegunda linha"}]), "hook_context:Start · [fixture] Contexto", "fixture"),
    ] {
        let mut fold = claude::ClaudeFold::new(String::new(), false, false);
        let raw = serde_json::to_vec(&json!({"type":"attachment", "rendered":"conteúdo renderizado",
            "attachment":{"type":"hook_context", "hookName":"Start", "content":content}})).unwrap();
        fold.line(&raw);
        let out = fold.close();
        assert_eq!(out.usage.len(), 1);
        assert_eq!(out.usage[0].nome, name);
        assert_eq!(out.usage[0].plugin, plugin);
        assert_eq!(out.usage[0].ctx_chars, 20);
    }
}

#[test]
fn claude_hook_skill_search_still_accepts_object_content() {
    let mut fold = claude::ClaudeFold::new(String::new(), false, false);
    fold.line(&serde_json::to_vec(&json!({"type":"attachment", "rendered":"conteúdo renderizado",
        "attachment":{"type":"hook_context", "hookName":"Start",
        "content":{"text":"full content of your 'database-x' skill"}}})).unwrap());
    let out = fold.close();
    assert_eq!(out.usage.len(), 1);
    assert_eq!(out.usage[0].tipo, "skill");
    assert_eq!(out.usage[0].nome, "database-x");
    assert_eq!(out.usage[0].origem, "hook");
    assert_eq!(out.usage[0].ctx_chars, 20);
}
