//! Hooks e pontes de skills que a subida confere: os casos que o instalador Python provava.
use hangar_server::{
    accounts::{AccountService, environment::AccountEnvironment},
    harness_setup::{self, codex_hooks, hooks, skills},
};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};

fn env(home: &Path, extra: &[(&str, String)]) -> AccountEnvironment {
    let text = home.to_string_lossy().into_owned();
    let mut map = BTreeMap::from([("HOME".to_owned(), text.clone()), ("USERPROFILE".to_owned(), text)]);
    for (key, value) in extra {
        map.insert((*key).to_owned(), value.clone());
    }
    AccountEnvironment::from_map(map)
}

fn claude_home(root: &Path) -> (AccountService, PathBuf, PathBuf) {
    let claude = root.join(".claude");
    fs::create_dir_all(&claude).unwrap();
    (AccountService::new(env(root, &[])), claude.join("settings.json"), root.join("repo"))
}

fn read(path: &Path) -> Value {
    serde_json::from_str(&fs::read_to_string(path).unwrap()).unwrap()
}

/// `"<programa dos hooks>" <subcomando>`: o lançador estável da casa, ou o servidor quando não há.
fn command(home: &Path, mode: &str) -> String {
    let program = hangar_server::codex_setup::hook_command::hook_server(home).unwrap();
    format!("\"{}\" {mode}", program.display())
}

/// Comandos nossos sob o evento, na ordem do arquivo.
fn ours(data: &Value, event: &str, script: &str) -> Vec<String> {
    data["hooks"][event]
        .as_array()
        .into_iter()
        .flatten()
        .flat_map(|block| block["hooks"].as_array().cloned().unwrap_or_default())
        .filter(|hook| hooks::refers(&hook["command"], script, true))
        .map(|hook| hook["command"].as_str().unwrap().to_owned())
        .collect()
}

#[test]
fn askq_is_installed_on_the_three_events_from_missing_or_empty_settings() {
    for seed in [None, Some(""), Some("  \n")] {
        let root = tempfile::tempdir().unwrap();
        let (service, settings, repo) = claude_home(root.path());
        if let Some(text) = seed {
            fs::write(&settings, text).unwrap();
        }
        hooks::sync_installer(&service, &repo, "ensure_askq_hook_installed").unwrap();
        let data = read(&settings);
        let expected = format!("{} || exit 0", command(root.path(), "askq-capture"));
        for event in ["PreToolUse", "PostToolUse", "PostToolUseFailure"] {
            assert_eq!(data["hooks"][event], json!([{"matcher": "AskUserQuestion", "hooks": [{"type": "command", "command": expected}]}]), "{seed:?} {event}");
        }
    }
}

#[test]
fn foreign_hooks_and_keys_survive_and_the_second_run_changes_nothing() {
    let root = tempfile::tempdir().unwrap();
    let (service, settings, repo) = claude_home(root.path());
    fs::write(&settings, json!({"model": "x", "hooks": {"PreToolUse": [{"matcher": "Bash", "hooks": [{"type": "command", "command": "echo hi"}]}]}}).to_string()).unwrap();
    for name in ["ensure_askq_hook_installed", "ensure_state_hooks_installed"] {
        hooks::sync_installer(&service, &repo, name).unwrap();
    }
    let first = fs::read(&settings).unwrap();
    let data = read(&settings);
    assert_eq!(data["model"], "x");
    assert_eq!(data["hooks"]["PreToolUse"][0], json!({"matcher": "Bash", "hooks": [{"type": "command", "command": "echo hi"}]}));
    assert_eq!(ours(&data, "PreToolUse", "askq-capture").len(), 1);
    assert_eq!(ours(&data, "PreToolUse", "state-hook").len(), 1);
    for name in ["ensure_askq_hook_installed", "ensure_state_hooks_installed"] {
        assert_eq!(hooks::sync_installer(&service, &repo, name).unwrap(), json!({"claude": []}), "{name}");
    }
    assert_eq!(fs::read(&settings).unwrap(), first);
}

#[test]
fn old_format_and_duplicates_collapse_into_one_entry_in_place() {
    let new = "\"/venv/bin/python\" \"/repo/backend/hooks/state_hook.py\" || exit 0";
    let mut data = json!({"hooks": {"Stop": [
        {"hooks": [{"type": "command", "command": "echo a"}, {"type": "command", "command": "python3 /old/checkout/backend/hooks/state_hook.py"}]},
        {"hooks": [{"type": "command", "command": "echo b"}]},
        {"hooks": [{"type": "command", "command": new}]},
    ]}});
    assert!(hooks::sync_hook(&mut data, "Stop", Some(new), None, "state_hook.py"));
    assert_eq!(data["hooks"]["Stop"], json!([
        {"hooks": [{"type": "command", "command": "echo a"}, {"type": "command", "command": new}]},
        {"hooks": [{"type": "command", "command": "echo b"}]},
    ]));
    assert!(!hooks::sync_hook(&mut data, "Stop", Some(new), None, "state_hook.py"));
}

/// Filtro novo chega à conta já instalada: o hook sai do bloco de filtro antigo, mesmo dividido com outro.
#[test]
fn a_changed_matcher_moves_the_hook_to_its_own_block() {
    let guard = "\"/x/hangar-server\" guard-tmux";
    let mut data = json!({"hooks": {"PreToolUse": [
        {"matcher": "Bash", "hooks": [{"type": "command", "command": "personal --check"}, {"type": "command", "command": guard}]},
    ]}});
    assert!(hooks::sync_hook(&mut data, "PreToolUse", Some(guard), Some("Bash|Read|Grep"), "guard-tmux"));
    assert_eq!(data["hooks"]["PreToolUse"], json!([
        {"matcher": "Bash", "hooks": [{"type": "command", "command": "personal --check"}]},
        {"matcher": "Bash|Read|Grep", "hooks": [{"type": "command", "command": guard}]},
    ]));
    assert!(!hooks::sync_hook(&mut data, "PreToolUse", Some(guard), Some("Bash|Read|Grep"), "guard-tmux"));
}

#[test]
fn broken_settings_are_left_byte_for_byte_and_the_error_returns() {
    for raw in ["{x", "[1]", "{\"hooks\": []}"] {
        let root = tempfile::tempdir().unwrap();
        let (service, settings, repo) = claude_home(root.path());
        fs::write(&settings, raw).unwrap();
        for name in ["ensure_askq_hook_installed", "ensure_state_hooks_installed", "ensure_pair_hook_installed"] {
            assert!(hooks::sync_installer(&service, &repo, name).is_err(), "{raw} {name}");
        }
        assert_eq!(fs::read_to_string(&settings).unwrap(), raw);
    }
}

#[test]
fn state_hook_covers_six_events_and_warns_only_where_stdout_reaches_the_model() {
    let root = tempfile::tempdir().unwrap();
    let (service, settings, repo) = claude_home(root.path());
    // A entrada Python antiga vira o subcomando no lugar.
    fs::write(&settings, json!({"hooks": {"Stop": [{"hooks": [{"type": "command", "command": "python3 /x/backend/hooks/state_hook.py"}]}]}}).to_string()).unwrap();
    hooks::sync_installer(&service, &repo, "ensure_state_hooks_installed").unwrap();
    let data = read(&settings);
    let base = command(root.path(), "state-hook");
    for event in ["PreToolUse", "PostToolUse", "Notification", "Stop"] {
        assert_eq!(ours(&data, event, "state-hook"), vec![format!("{base} || exit 0")], "{event}");
        assert!(ours(&data, event, "state_hook.py").is_empty(), "{event}");
    }
    for event in ["SessionStart", "UserPromptSubmit"] {
        let [command] = ours(&data, event, "state-hook").try_into().unwrap();
        assert!(command.starts_with(&format!("{base} || echo \"[hangar] O hook state-hook do Hangar falhou em {event}")), "{command}");
        assert!(command.contains("AVISE O USUARIO") && !command.contains("exit 0"), "{command}");
    }
    assert_eq!(data["hooks"]["Stop"].as_array().unwrap().len(), 1, "a entrada antiga vira a nova no lugar");
}

/// O comando novo mantém o aviso ao modelo quando falha de verdade (aqui, programa ausente).
#[test]
#[cfg(unix)]
fn missing_hook_program_exits_zero_and_tells_the_model() {
    let mut data = json!({"hooks": {}});
    let program = Path::new("/nonexistent/.hangar/bin/hangar-launcher");
    hooks::reconcile(&mut data, program, Path::new("/x/.hangar-pair"), &|key: &str| key == "state_hook.py", false);
    for (event, warns) in [("UserPromptSubmit", true), ("Stop", false)] {
        let [command] = ours(&data, event, "state-hook").try_into().unwrap();
        let out = std::process::Command::new("sh").args(["-c", &command]).output().unwrap();
        assert!(out.status.success(), "{event}: {out:?}");
        assert_eq!(String::from_utf8_lossy(&out.stdout).contains("O hook state-hook do Hangar falhou"), warns, "{event}");
    }
}

/// Prévia e navegador uma vez por evento; o hook de subagente sai, com o
/// evento que esvaziou, e o que é pessoal no mesmo evento fica.
#[test]
fn preview_and_nav_land_once_and_the_subagent_hook_leaves() {
    let root = tempfile::tempdir().unwrap();
    let (service, settings, repo) = claude_home(root.path());
    let personal = json!({"hooks": [{"type": "command", "command": "notify-send fim"}]});
    fs::write(&settings, json!({"hooks": {
        "SubagentStart": [{"hooks": [{"type": "command", "command": "\"/p/python3\" \"/p/backend/hooks/subagent_hook.py\" || exit 0"}]}],
        "SubagentStop": [personal, {"hooks": [{"type": "command", "command": "\"/p/python3\" \"/p/backend/hooks/subagent_hook.py\" || exit 0"}]}],
    }}).to_string()).unwrap();
    let cases = [
        ("ensure_preview_hook_installed", "preview-hook", vec!["MessageDisplay", "Stop"]),
        ("ensure_nav_hook_installed", "nav-hook", vec!["UserPromptSubmit"]),
    ];
    for name in ["ensure_preview_hook_installed", "ensure_subagent_hook_installed", "ensure_nav_hook_installed"] {
        hooks::sync_installer(&service, &repo, name).unwrap();
    }
    let data = read(&settings);
    for (name, mode, events) in &cases {
        for event in events {
            assert_eq!(ours(&data, event, mode).len(), 1, "{mode} {event}");
        }
        assert_eq!(hooks::sync_installer(&service, &repo, name).unwrap(), json!({"claude": []}), "{name}");
    }
    assert!(data["hooks"].get("SubagentStart").is_none(), "{data}");
    assert_eq!(data["hooks"]["SubagentStop"], json!([personal]));
    assert_eq!(hooks::sync_installer(&service, &repo, "ensure_subagent_hook_installed").unwrap(), json!({"claude": []}));
    assert!(!fs::read_to_string(&settings).unwrap().contains("subagent_hook"));
}

/// A entrada pessoal que chamava `~/.claude/hooks/guard_tmux.py` vira o `guard-tmux` do
/// pacote já na subida (que não instala o guarda onde ele não estava).
#[test]
fn startup_repairs_an_existing_python_guard_but_never_installs_one() {
    let root = tempfile::tempdir().unwrap();
    let (service, settings, repo) = claude_home(root.path());
    let old = format!("\"{0}/.hangar/current/python/bin/python3\" \"{0}/.claude/hooks/guard_tmux.py\"", root.path().display());
    fs::write(&settings, json!({"hooks": {"PreToolUse": [{"matcher": "Bash", "hooks": [{"type": "command", "command": old}]}]}}).to_string()).unwrap();
    let names: Vec<&str> = hangar_server::config_sync::after::HOOKS.iter().copied()
        .filter(|name| *name != "ensure_guard_hooks_installed").collect();
    hooks::sync_installers(&service, &repo, &names).unwrap();
    let data = read(&settings);
    assert!(ours(&data, "PreToolUse", "guard_tmux.py").is_empty(), "{data}");
    let guard: Vec<_> = data["hooks"]["PreToolUse"].as_array().unwrap().iter()
        .filter(|block| hooks::refers(&block["hooks"][0]["command"], "guard-tmux", true)).cloned().collect();
    assert_eq!(guard, vec![json!({"matcher": "Bash|PowerShell|Read|Grep", "hooks": [{"type": "command", "command": command(root.path(), "guard-tmux")}]})]);
    let fresh = tempfile::tempdir().unwrap();
    let (service, settings, repo) = claude_home(fresh.path());
    hooks::sync_installers(&service, &repo, &names).unwrap();
    assert!(!fs::read_to_string(&settings).unwrap().contains("guard-tmux"));
}

/// Decisões 12/13: com o lançador estável na casa, todo comando gravado o chama (sobrevive à volta de versão).
#[test]
fn hooks_call_the_stable_launcher_when_the_package_installed_it() {
    let root = tempfile::tempdir().unwrap();
    let (service, settings, repo) = claude_home(root.path());
    let launcher = root.path().join(".hangar").join("bin").join(if cfg!(windows) { "hangar-launcher.exe" } else { "hangar-launcher" });
    fs::create_dir_all(launcher.parent().unwrap()).unwrap();
    // O lançador do pacote é cópia deste mesmo binário.
    fs::copy(std::env::current_exe().unwrap(), &launcher).unwrap();
    hooks::sync_installers(&service, &repo, &["ensure_state_hooks_installed", "ensure_pair_hook_installed", "ensure_guard_hooks_installed"]).unwrap();
    let data = read(&settings);
    let quoted = format!("\"{}\" ", launcher.display());
    assert_eq!(ours(&data, "Stop", "state-hook"), vec![format!("{quoted}state-hook || exit 0")]);
    assert!(ours(&data, "SessionStart", "pair-context")[0].starts_with(&format!("{quoted}pair-context ")));
    assert_eq!(ours(&data, "PreToolUse", "guard-tmux"), vec![format!("{quoted}guard-tmux")]);
}

/// O item `claude_hooks` da configuração compartilhada roda os 7 instaladores (a lista `HOOKS`
/// do pós-aplicar). Todos são conhecidos, o guarda Python antigo vira o subcomando com o matcher
/// largo, cada hook cai no comando novo e uma segunda passada não muda nada.
#[test]
fn shared_config_hooks_item_runs_the_seven_installers_with_the_subcommands() {
    let root = tempfile::tempdir().unwrap();
    let (service, settings, repo) = claude_home(root.path());
    let old = format!("python3 \"{}/.claude/hooks/guard_tmux.py\"", root.path().display());
    fs::write(&settings, json!({"hooks": {"PreToolUse": [{"matcher": "Bash", "hooks": [{"type": "command", "command": old}]}]}}).to_string()).unwrap();
    let names = hangar_server::config_sync::after::HOOKS;
    assert_eq!(names.len(), 7);
    hooks::sync_installers(&service, &repo, names).unwrap();
    let data = read(&settings);
    assert!(ours(&data, "PreToolUse", "guard_tmux.py").is_empty(), "{data}");
    let guard: Vec<_> = data["hooks"]["PreToolUse"].as_array().unwrap().iter()
        .filter(|block| hooks::refers(&block["hooks"][0]["command"], "guard-tmux", true)).cloned().collect();
    assert_eq!(guard, vec![json!({"matcher": "Bash|PowerShell|Read|Grep", "hooks": [{"type": "command", "command": command(root.path(), "guard-tmux")}]})]);
    assert_eq!(ours(&data, "Stop", "state-hook"), vec![format!("{} || exit 0", command(root.path(), "state-hook"))]);
    assert_eq!(ours(&data, "MessageDisplay", "preview-hook"), vec![format!("{} || exit 0", command(root.path(), "preview-hook"))]);
    assert_eq!(ours(&data, "PostToolUseFailure", "askq-capture"), vec![format!("{} || exit 0", command(root.path(), "askq-capture"))]);
    assert_eq!(ours(&data, "UserPromptSubmit", "nav-hook").len(), 1);
    assert_eq!(ours(&data, "SessionStart", "pair-context").len(), 1);
    assert!(data["hooks"].get("SubagentStart").is_none() && data["hooks"].get("SubagentStop").is_none(), "{data}");
    let changes = hooks::sync_installers(&service, &repo, names).unwrap();
    assert_eq!(changes["claude"], json!([]), "a segunda passada não muda nada: {changes}");
}

fn seed_old_pair(dir: &Path) {
    fs::create_dir_all(dir).unwrap();
    fs::write(dir.join("settings.json"), json!({"hooks": {"SessionStart": [
        {"matcher": "startup|resume|clear|compact", "hooks": [{"type": "command", "command": "python3 /r/backend/hooks/pair_hook.py"}]},
    ]}}).to_string()).unwrap();
}

#[test]
fn pair_context_replaces_the_old_script_in_every_account_and_the_base() {
    let root = tempfile::tempdir().unwrap();
    let base = root.path().join(".claude");
    let marked = root.path().join(".claude-a");
    // Conta antiga, sem o marcador do Hangar: o catálogo a reconhece pelo login e pelo `projects/`.
    let legacy = root.path().join(".claude-b");
    let fixed = root.path().join("fixa");
    for dir in [&base, &marked, &legacy, &fixed] {
        seed_old_pair(dir);
    }
    fs::write(marked.join(".hangar-conta"), "").unwrap();
    fs::write(legacy.join(".credentials.json"), "{}").unwrap();
    fs::create_dir_all(legacy.join("projects")).unwrap();
    let repo = root.path().join("repo");
    let exe = hangar_server::codex_setup::hook_command::hook_server(root.path()).unwrap();
    let expected = json!([{"matcher": "startup|resume|clear|compact", "hooks": [{"type": "command",
        "command": format!("\"{}\" pair-context \"{}\" || exit 0", exe.display(), base.join(".hangar-pair").display())}]}]);
    let scanned = AccountService::new(env(root.path(), &[]));
    let listed = AccountService::new(env(root.path(), &[("CP_CLAUDE_CONFIG_DIRS", format!("f:{}", fixed.display()))]));
    // A lista fixa vem antes: no catálogo ela substitui a varredura, e a conta marcada ainda precisa receber.
    for (service, dirs) in [(&listed, vec![&base, &fixed, &marked]), (&scanned, vec![&base, &marked, &legacy])] {
        hooks::sync_installer(service, &repo, "ensure_pair_hook_installed").unwrap();
        for dir in dirs {
            assert_eq!(read(&dir.join("settings.json"))["hooks"]["SessionStart"], expected, "{}", dir.display());
        }
        assert_eq!(hooks::sync_installer(service, &repo, "ensure_pair_hook_installed").unwrap(), json!({"claude": []}));
    }
}

#[test]
fn codex_hooks_append_after_the_existing_groups_and_settle() {
    let root = tempfile::tempdir().unwrap();
    let codex = root.path().join(".codex");
    fs::create_dir_all(&codex).unwrap();
    let review = json!({"matcher": "Bash", "hooks": [{"type": "command", "command": "review"}]});
    let rtk = json!({"matcher": "Bash", "hooks": [{"type": "command", "command": "rtk"}]});
    fs::write(codex.join("hooks.json"), json!({"hooks": {"PreToolUse": [review, rtk]}}).to_string()).unwrap();
    let windows = cfg!(windows);
    let touched = codex_hooks::sync(&codex, root.path(), windows).unwrap();
    assert_eq!(touched, vec!["UserPromptSubmit".to_owned(), "PreToolUse".to_owned()]);
    let data = read(&codex.join("hooks.json"));
    let pre = data["hooks"]["PreToolUse"].as_array().unwrap();
    assert_eq!((&pre[0], &pre[1]), (&review, &rtk));
    assert_eq!(pre.len(), 3, "{data}");
    assert!(pre[2]["matcher"] == "Bash" && hooks::refers(&pre[2]["hooks"][0]["command"], "guard-tmux", true));
    // O hook de estado não entra mais no Codex.
    for event in ["SessionStart", "PostToolUse", "Stop"] {
        assert!(data["hooks"].get(event).is_none(), "{event}: {data}");
    }
    let nav = ours(&data, "UserPromptSubmit", "nav-hook");
    assert_eq!(nav.len(), 1);
    if !windows {
        assert_eq!(nav, vec![format!("{} || exit 0", command(root.path(), "nav-hook"))]);
        assert_eq!(ours(&data, "PreToolUse", "guard-tmux"), vec![command(root.path(), "guard-tmux")]);
    }
    assert!(codex_hooks::sync(&codex, root.path(), windows).unwrap().is_empty());
}

#[cfg(unix)]
fn skill(dir: &Path) -> PathBuf {
    fs::create_dir_all(dir).unwrap();
    fs::write(dir.join("SKILL.md"), "x").unwrap();
    dir.to_owned()
}

#[cfg(unix)]
fn skill_home() -> (tempfile::TempDir, AccountEnvironment, PathBuf, PathBuf) {
    let root = tempfile::tempdir().unwrap();
    let home = root.path().to_owned();
    fs::create_dir_all(home.join(".pi/agent")).unwrap();
    let environment = env(&home, &[]);
    (root, environment, home.join("repo"), home.join(".pi/agent/skills-bridge"))
}

#[test]
#[cfg(unix)]
fn bridge_follows_precedence_and_only_the_newest_plugin_version() {
    let (_root, env, repo, bridge) = skill_home();
    let home = env.home.clone();
    let mine = skill(&home.join(".claude/skills/dup"));
    skill(&home.join(".agents/skills/dup"));
    let cache = home.join(".claude/plugins/cache/p/p");
    skill(&cache.join("2.2.0/skills/old"));
    let newest = skill(&cache.join("2.10.0/skills/new"));
    let agents = skill(&home.join(".agents/skills/only-agents"));
    let stats = skills::rebuild(&env, &repo, false);
    assert_eq!(stats["pi"]["criados"], 3);
    assert_eq!(fs::read_link(bridge.join("dup")).unwrap(), mine);
    assert_eq!(fs::read_link(bridge.join("new")).unwrap(), newest);
    assert_eq!(fs::read_link(bridge.join("only-agents")).unwrap(), agents);
    assert!(!bridge.join("old").exists());
}

#[test]
#[cfg(unix)]
fn bridge_prunes_dead_links_swaps_versions_and_settles() {
    let (_root, env, repo, bridge) = skill_home();
    let home = env.home.clone();
    let cache = home.join(".claude/plugins/cache/p/p");
    skill(&cache.join("1.0.0/skills/s"));
    skills::rebuild(&env, &repo, false);
    fs::remove_dir_all(cache.join("1.0.0")).unwrap();
    let newer = skill(&cache.join("1.1.0/skills/s"));
    std::os::unix::fs::symlink(home.join(".claude/skills/gone"), bridge.join("gone")).unwrap();
    let stats = skills::rebuild(&env, &repo, false);
    assert_eq!(fs::read_link(bridge.join("s")).unwrap(), newer);
    assert!(fs::symlink_metadata(bridge.join("gone")).is_err());
    assert_eq!(stats["pi"]["removidos"], 2, "{stats}");
    assert_eq!(stats["pi"]["criados"], 1, "{stats}");
    let again = skills::rebuild(&env, &repo, false);
    assert_eq!((&again["pi"]["criados"], &again["pi"]["trocados"], &again["pi"]["removidos"]), (&json!(0), &json!(0), &json!(0)));
}

#[test]
#[cfg(unix)]
fn bridge_never_touches_real_entries_or_links_outside_the_sources() {
    let (_root, env, repo, bridge) = skill_home();
    let home = env.home.clone();
    skill(&home.join(".claude/skills/real"));
    skill(&home.join(".claude/skills/hand"));
    skill(&home.join(".claude/skills/backup"));
    fs::create_dir_all(bridge.join("real")).unwrap();
    std::os::unix::fs::symlink("/somewhere/else", bridge.join("hand")).unwrap();
    // O marcador como pedaço do caminho não faz o link ser da ponte.
    let backup = home.join("mnt/backup").join(home.strip_prefix("/").unwrap()).join(".claude/skills/backup");
    std::os::unix::fs::symlink(&backup, bridge.join("backup")).unwrap();
    let stats = skills::rebuild(&env, &repo, false);
    assert!(bridge.join("real").is_dir() && !bridge.join("real").is_symlink());
    assert_eq!(fs::read_link(bridge.join("hand")).unwrap(), Path::new("/somewhere/else"));
    assert_eq!(fs::read_link(bridge.join("backup")).unwrap(), backup);
    assert_eq!(stats["pi"]["pulados_reais"], 3, "{stats}");
}

#[test]
#[cfg(unix)]
fn bridge_recreates_a_dead_link_from_another_source_in_the_same_run() {
    let (_root, env, repo, bridge) = skill_home();
    let home = env.home.clone();
    let alive = skill(&home.join(".agents/skills/s"));
    fs::create_dir_all(&bridge).unwrap();
    std::os::unix::fs::symlink(home.join(".claude/skills/s"), bridge.join("s")).unwrap();
    let stats = skills::rebuild(&env, &repo, false);
    assert_eq!((&stats["pi"]["removidos"], &stats["pi"]["criados"]), (&json!(1), &json!(1)), "{stats}");
    assert_eq!(fs::read_link(bridge.join("s")).unwrap(), alive);
}

#[test]
#[cfg(unix)]
fn bridge_keeps_a_relative_link_into_a_source() {
    let (_root, env, repo, bridge) = skill_home();
    skill(&env.home.join(".claude/skills/s"));
    fs::create_dir_all(&bridge).unwrap();
    std::os::unix::fs::symlink("../../../.claude/skills/s", bridge.join("s")).unwrap();
    let stats = skills::rebuild(&env, &repo, false);
    assert_eq!(stats["pi"]["mantidos"], 1, "{stats}");
    assert_eq!(fs::read_link(bridge.join("s")).unwrap(), Path::new("../../../.claude/skills/s"));
}

#[test]
#[cfg(unix)]
fn bridge_without_sources_or_harness_changes_nothing() {
    let (_root, env, repo, bridge) = skill_home();
    fs::create_dir_all(&bridge).unwrap();
    std::os::unix::fs::symlink(env.home.join(".claude/skills/s"), bridge.join("s")).unwrap();
    assert_eq!(skills::rebuild(&env, &repo, false), json!({}));
    assert!(bridge.join("s").is_symlink(), "sem fonte nenhuma a poda não roda");
    let empty = tempfile::tempdir().unwrap();
    skill(&empty.path().join(".claude/skills/s"));
    fs::create_dir_all(empty.path().join(".codex/skills")).unwrap();
    assert_eq!(skills::rebuild(&self::env(empty.path(), &[]), &empty.path().join("repo"), false), json!({}));
    assert_eq!(fs::read_dir(empty.path().join(".codex/skills")).unwrap().count(), 0);
}

#[test]
#[cfg(unix)]
fn startup_writes_hooks_and_bridges_through_the_unresolved_current() {
    let root = tempfile::tempdir().unwrap();
    let home = root.path();
    let version = home.join(".hangar/versions/X");
    fs::create_dir_all(version.join("python/bin")).unwrap();
    fs::write(version.join("python/bin/python3"), "").unwrap();
    skill(&version.join("skills/from-repo"));
    std::os::unix::fs::symlink(&version, home.join(".hangar/current")).unwrap();
    let launcher = home.join(".hangar/bin/hangar-launcher");
    fs::create_dir_all(launcher.parent().unwrap()).unwrap();
    fs::copy(std::env::current_exe().unwrap(), &launcher).unwrap();
    fs::create_dir_all(home.join(".claude")).unwrap();
    fs::create_dir_all(home.join(".codex")).unwrap();
    fs::create_dir_all(home.join(".pi/agent")).unwrap();
    let service = AccountService::new(env(home, &[]));
    harness_setup::sync_startup(&service);
    let current = home.join(".hangar/current");
    let data = read(&home.join(".claude/settings.json"));
    let stop = format!("\"{}\" state-hook || exit 0", launcher.display());
    assert_eq!(ours(&data, "Stop", "state-hook"), vec![stop]);
    for (event, mode) in [("PreToolUse", "askq-capture"), ("MessageDisplay", "preview-hook"),
        ("SessionStart", "pair-context"), ("UserPromptSubmit", "nav-hook")] {
        assert_eq!(ours(&data, event, mode).len(), 1, "{mode}");
    }
    assert!(data["hooks"].get("SubagentStart").is_none());
    let text = fs::read_to_string(home.join(".claude/settings.json")).unwrap();
    assert!(!text.contains("versions") && !text.contains("python"), "{text}");
    let codex = read(&home.join(".codex/hooks.json"));
    assert_eq!(ours(&codex, "UserPromptSubmit", "nav-hook"), vec![format!("\"{}\" nav-hook || exit 0", launcher.display())]);
    assert!(codex["hooks"].get("Stop").is_none());
    assert_eq!(fs::read_link(home.join(".pi/agent/skills-bridge/from-repo")).unwrap(), current.join("skills/from-repo"));
}
