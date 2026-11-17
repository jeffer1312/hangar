//! Comportamentos dos hooks que a captura não cobre sozinha: concorrência da prévia, a árvore de
//! processos real, as correções por decisão (R1-D06/D07) e a entrada hostil sem pânico.
use hangar_agent_hooks::{Context, askq, nav, preview, state, tree};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

fn context(root: &Path) -> Context {
    let mut vars = BTreeMap::new();
    vars.insert("CLAUDE_CONFIG_DIR".to_owned(), root.join("cfg").to_string_lossy().into_owned());
    Context { home: Some(root.join("home")), vars }
}

fn event(value: Value) -> Vec<u8> {
    serde_json::to_vec(&value).unwrap()
}

struct NoClaude;
impl tree::Processes for NoClaude {
    fn argv(&self, _: u32) -> Option<Vec<String>> { Some(vec!["codex".into()]) }
    fn parent(&self, _: u32) -> Option<u32> { None }
}

/// H-17/H-21 (R1-S07): 8 deltas da mesma mensagem ao mesmo tempo, em ordens embaralhadas, 20 vezes:
/// o texto final tem todos os pedaços em ordem, e ao lado do sidecar só fica a trava.
#[test]
fn h17_h21_concurrent_deltas_keep_every_part_in_order() {
    let root = tempfile::tempdir().unwrap();
    let ctx = context(root.path());
    fs::create_dir_all(root.path().join("cfg")).unwrap();
    let sidecar = root.path().join("cfg/.hangar-preview/abc.json");
    for rep in 0..20u64 {
        let _ = fs::remove_file(&sidecar);
        let mut order: Vec<usize> = (0..8).collect();
        // Embaralhamento determinístico por repetição.
        order.sort_by_key(|i| (*i as u64 * 7919 + rep * 104_729) % 8 + if rep % 2 == 0 { 0 } else { 8 - *i as u64 % 3 });
        std::thread::scope(|scope| {
            for i in order {
                let ctx = &ctx;
                scope.spawn(move || {
                    let out = preview::handle(&event(json!({"hook_event_name": "MessageDisplay", "session_id": "s",
                        "transcript_path": "/p/abc.jsonl", "message_id": "msg_01", "index": i, "delta": format!("[{i}]")})), ctx);
                    assert_eq!(out.code, 0);
                    assert!(out.stderr.is_empty(), "{}", String::from_utf8_lossy(&out.stderr));
                });
            }
        });
        let data: Value = serde_json::from_slice(&fs::read(&sidecar).unwrap()).unwrap();
        assert_eq!(data["text"], "[0][1][2][3][4][5][6][7]", "rep {rep}");
        assert_eq!(data["parts"].as_object().unwrap().len(), 8);
        let mut others: Vec<_> = fs::read_dir(sidecar.parent().unwrap()).unwrap().flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned()).filter(|n| n != "abc.json").collect();
        others.sort();
        assert_eq!(others, vec!["abc.lock".to_owned()], "R1-S08");
    }
}

/// R1-D08 (H-19): depois do `Stop`, o índice 0 atrasado da MESMA mensagem não republica a prévia; o
/// índice 0 de uma mensagem nova e o de mensagem sem id (não há como provar que é velho) entram.
#[test]
fn d08_a_late_index_zero_of_the_closed_message_is_dropped() {
    let root = tempfile::tempdir().unwrap();
    let ctx = context(root.path());
    let sidecar = root.path().join("cfg/.hangar-preview/abc.json");
    let display = |message: Value, index: u64, delta: &str| event(json!({"hook_event_name": "MessageDisplay",
        "transcript_path": "/p/abc.jsonl", "message_id": message, "index": index, "delta": delta}));
    let stop = event(json!({"hook_event_name": "Stop", "transcript_path": "/p/abc.jsonl"}));
    preview::handle(&display(json!("msg_01"), 0, "A"), &ctx);
    preview::handle(&stop, &ctx);
    preview::handle(&display(json!("msg_01"), 0, "A"), &ctx);
    let data: Value = serde_json::from_slice(&fs::read(&sidecar).unwrap()).unwrap();
    assert_eq!((data["text"].as_str(), data["closed"].as_str()), (Some(""), Some("msg_01")), "o Python republicava \"A\"");
    preview::handle(&display(json!("msg_02"), 0, "B"), &ctx);
    let data: Value = serde_json::from_slice(&fs::read(&sidecar).unwrap()).unwrap();
    assert_eq!(data["text"], "B");
    preview::handle(&stop, &ctx);
    preview::handle(&display(Value::Null, 0, "C"), &ctx);
    let data: Value = serde_json::from_slice(&fs::read(&sidecar).unwrap()).unwrap();
    assert_eq!(data["text"], "C");
}

/// R1-D07 (H-04/H-85): o temporário leva o pid. Um `<sid>.json.tmp` que não dá para abrir (o nome fixo
/// do Python, aqui uma pasta) derrubava o marcador; agora não o atrapalha, e nenhuma sobra fica.
#[test]
fn d07_the_marker_temporary_never_collides_with_the_fixed_name() {
    let root = tempfile::tempdir().unwrap();
    let ctx = context(root.path());
    let dir = root.path().join("cfg/.hangar-state");
    fs::create_dir_all(dir.join("s1.json.tmp")).unwrap();
    state::handle(&event(json!({"hook_event_name": "Stop", "session_id": "s1"})), &ctx, &NoClaude, Some(10));
    let data: Value = serde_json::from_slice(&fs::read(dir.join("s1.json")).unwrap()).unwrap();
    assert_eq!(data["state"], "idle");
    let left: Vec<_> = fs::read_dir(&dir).unwrap().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
    assert_eq!(left.len(), 2, "{left:?}");
}

/// R1-D06 (H-25/H-44): nenhum `session_id` hostil grava fora da pasta do marcador.
#[test]
fn d06_hostile_session_ids_stay_inside_the_marker_folders() {
    for sid in ["../fora", "a/../../fora", "/abs/fora", "x/"] {
        let root = tempfile::tempdir().unwrap();
        let ctx = context(root.path());
        state::handle(&event(json!({"hook_event_name": "UserPromptSubmit", "session_id": sid, "prompt": "oi"})), &ctx, &NoClaude, None);
        askq::handle(&event(json!({"hook_event_name": "PreToolUse", "session_id": sid, "tool_name": "AskUserQuestion"})), &ctx);
        let mut pending = vec![root.path().to_path_buf()];
        while let Some(dir) = pending.pop() {
            for entry in fs::read_dir(&dir).unwrap().flatten() {
                let path = entry.path();
                if path.is_dir() {
                    pending.push(path);
                    continue;
                }
                let parent = path.parent().unwrap().file_name().unwrap().to_string_lossy().into_owned();
                assert!(parent.starts_with(".hangar-"), "{sid}: {}", path.display());
            }
        }
    }
}

/// H-07/H-09 na árvore real (`/proc`): o processo com argv[0] `claude` acima do pid dá o pid dele e o
/// `--session-id` do cmdline; um `sh` cujo comando cita "claude" não conta.
#[cfg(target_os = "linux")]
#[test]
fn h07_h09_the_real_process_tree_finds_the_claude_by_argv0() {
    use tree::Processes;
    let uuid = "aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee";
    let mut claude = std::process::Command::new("bash")
        .args(["-c", &format!("exec -a claude bash -c 'sleep 30; true' _ --session-id {uuid}")])
        .spawn().unwrap();
    let mut other = std::process::Command::new("sh").args(["-c", "sleep 30; echo /x/claude-headless/claude"]).spawn().unwrap();
    // O `exec -a` troca o argv do mesmo pid; espera ele aparecer.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    let pid = claude.id();
    while tree::System.argv(pid).is_none_or(|argv| argv.first().map(String::as_str) != Some("claude")) {
        assert!(std::time::Instant::now() < deadline, "argv não mudou");
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert_eq!(tree::boot_claude(pid, &tree::System), (Some(uuid.to_owned()), Some(pid)));
    assert_eq!(tree::boot_claude(other.id(), &tree::System).1.filter(|p| *p == other.id()), None);
    let _ = claude.kill();
    let _ = other.kill();
    let _ = claude.wait();
    let _ = other.wait();
}

/// H-08: sob o Codex (sem ancestral claude) o estado sai, o marcador ativo não.
#[test]
fn h08_without_a_claude_ancestor_only_the_state_marker_is_written() {
    let root = tempfile::tempdir().unwrap();
    let ctx = context(root.path());
    state::handle(&event(json!({"hook_event_name": "Stop", "session_id": "s", "transcript_path": "/r/rollout.jsonl"})), &ctx, &NoClaude, Some(10));
    assert!(root.path().join("cfg/.hangar-state/s.json").is_file());
    assert!(!root.path().join("cfg/.hangar-active").exists());
}

/// R1-D05 (H-12): o `nav-hook` lê bytes e decodifica UTF-8 sempre, então "página" casa a regex mesmo
/// onde o stdin do Python era cp1252 (no Windows ele virava "pÃ¡gina" e a dica não saía).
#[test]
fn d05_nav_reads_utf8_regardless_of_the_platform_encoding() {
    assert!(nav::screen_message("verifica a página nova do cadastro agora"));
    let mojibake: String = "verifica a página nova do cadastro agora".bytes().map(char::from).collect();
    assert!(!nav::screen_message(&mojibake), "o texto decodificado em latin-1/cp1252 não casava");
}

proptest::proptest! {
    #![proptest_config(proptest::test_runner::Config::with_cases(256))]

    /// Fuzz do stdin (bytes quaisquer e JSON com campos de tipo errado): nenhum hook entra em pânico, todo
    /// hook sai 0, estado e pergunta nunca escrevem saída, e a prévia escreve no máximo uma linha no stderr.
    #[test]
    fn any_stdin_never_panics_and_always_exits_zero(raw in proptest::collection::vec(proptest::prelude::any::<u8>(), 0..512)) {
        let root = tempfile::tempdir().unwrap();
        let ctx = context(root.path());
        let out = state::handle(&raw, &ctx, &NoClaude, Some(10));
        proptest::prop_assert_eq!(&out, &hangar_agent_hooks::Outcome::default());
        let out = askq::handle(&raw, &ctx);
        proptest::prop_assert_eq!(&out, &hangar_agent_hooks::Outcome::default());
        let out = preview::handle(&raw, &ctx);
        proptest::prop_assert_eq!(out.code, 0);
        proptest::prop_assert!(out.stdout.is_empty() && out.stderr.iter().filter(|b| **b == b'\n').count() <= 1);
        let out = nav::handle(&raw, &ctx);
        proptest::prop_assert_eq!(out.code, 0);
        proptest::prop_assert!(out.stdout.is_empty());
    }

    #[test]
    fn mistyped_fields_never_panic(
        event_name in proptest::prop_oneof![proptest::strategy::Just(json!("Stop")), proptest::strategy::Just(json!("MessageDisplay")),
            proptest::strategy::Just(json!("UserPromptSubmit")), proptest::strategy::Just(json!("PostToolUse")), proptest::strategy::Just(json!([1])),
            proptest::strategy::Just(json!({"a": 1})), proptest::strategy::Just(json!(7))],
        field in proptest::prop_oneof![proptest::strategy::Just(json!(null)), proptest::strategy::Just(json!(1.5)), proptest::strategy::Just(json!(true)),
            proptest::strategy::Just(json!("../x")), proptest::strategy::Just(json!([])), proptest::strategy::Just(json!({"k": "v"})),
            proptest::strategy::Just(json!("a\u{FFFD}b")), proptest::strategy::Just(json!(123456789012345678u64))],
    ) {
        let root = tempfile::tempdir().unwrap();
        let ctx = context(root.path());
        for key in ["session_id", "transcript_path", "prompt", "index", "message_id", "delta", "tool_use_id", "agent_id", "cwd"] {
            let mut value = json!({"hook_event_name": event_name.clone(), "session_id": "s", "transcript_path": "/p/s.jsonl",
                "tool_name": "AskUserQuestion", "message_id": "m", "index": 1, "delta": "d", "prompt": "abre a tela"});
            value[key] = field.clone();
            let raw = event(value);
            proptest::prop_assert_eq!(state::handle(&raw, &ctx, &NoClaude, Some(10)).code, 0);
            proptest::prop_assert_eq!(askq::handle(&raw, &ctx).code, 0);
            proptest::prop_assert_eq!(preview::handle(&raw, &ctx).code, 0);
            proptest::prop_assert_eq!(nav::handle(&raw, &ctx).code, 0);
        }
    }
}
