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

fn command(repo: &Path, script: &str) -> String {
    let python = hangar_server::install_root::python_exe(repo, cfg!(windows));
    format!("\"{}\" \"{}\"", python.display(), repo.join("backend/hooks").join(script).display())
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
        let expected = format!("{} || exit 0", command(&repo, "askq_capture.py"));
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
    assert_eq!(ours(&data, "PreToolUse", "askq_capture.py").len(), 1);
    assert_eq!(ours(&data, "PreToolUse", "state_hook.py").len(), 1);
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
    fs::write(&settings, json!({"hooks": {"Stop": [{"hooks": [{"type": "command", "command": "python3 /x/backend/hooks/state_hook.py"}]}]}}).to_string()).unwrap();
    hooks::sync_installer(&service, &repo, "ensure_state_hooks_installed").unwrap();
    let data = read(&settings);
    let base = command(&repo, "state_hook.py");
    for event in ["PreToolUse", "PostToolUse", "Notification", "Stop"] {
        assert_eq!(ours(&data, event, "state_hook.py"), vec![format!("{base} || exit 0")], "{event}");
    }
    for event in ["SessionStart", "UserPromptSubmit"] {
        let [command] = ours(&data, event, "state_hook.py").try_into().unwrap();
        assert!(command.starts_with(&format!("{base} || echo \"[hangar] O hook state_hook do Hangar falhou em {event}")), "{command}");
        assert!(command.contains("AVISE O USUARIO") && !command.contains("exit 0"), "{command}");
    }
    assert_eq!(data["hooks"]["Stop"].as_array().unwrap().len(), 1, "a entrada antiga vira a nova no lugar");
}

#[test]
#[cfg(unix)]
fn missing_hook_script_exits_zero_and_tells_the_model() {
    let root = tempfile::tempdir().unwrap();
    let (service, settings, repo) = claude_home(root.path());
    hooks::sync_installer(&service, &repo, "ensure_state_hooks_installed").unwrap();
    for (event, warns) in [("UserPromptSubmit", true), ("Stop", false)] {
        let [command] = ours(&read(&settings), event, "state_hook.py").try_into().unwrap();
        let out = std::process::Command::new("sh").args(["-c", &command]).output().unwrap();
        assert!(out.status.success(), "{event}: {out:?}");
        assert_eq!(String::from_utf8_lossy(&out.stdout).contains("O hook state_hook do Hangar falhou"), warns, "{event}");
    }
}

#[test]
fn preview_subagent_and_nav_hooks_land_on_their_events_once() {
    let root = tempfile::tempdir().unwrap();
    let (service, settings, repo) = claude_home(root.path());
    let cases = [
        ("ensure_preview_hook_installed", "preview_hook.py", vec!["MessageDisplay", "Stop"]),
        ("ensure_subagent_hook_installed", "subagent_hook.py", vec!["SubagentStart", "SubagentStop"]),
        ("ensure_nav_hook_installed", "nav_hook.py", vec!["UserPromptSubmit"]),
    ];
    for (name, _, _) in &cases {
        hooks::sync_installer(&service, &repo, name).unwrap();
    }
    let data = read(&settings);
    for (name, script, events) in &cases {
        for event in events {
            assert_eq!(ours(&data, event, script).len(), 1, "{script} {event}");
        }
        assert_eq!(hooks::sync_installer(&service, &repo, name).unwrap(), json!({"claude": []}), "{name}");
    }
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
    let exe = hangar_server::codex_setup::hook_command::persistent_server().unwrap();
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
    let repo = root.path().join("repo");
    // A plataforma da máquina, como na subida: o Python do venv sai dela, não do argumento.
    let windows = cfg!(windows);
    let touched = codex_hooks::sync(&codex, &repo, windows).unwrap();
    for event in ["SessionStart", "UserPromptSubmit", "PreToolUse", "PostToolUse", "Stop"] {
        assert!(touched.iter().any(|e| e == event), "{event}");
    }
    let data = read(&codex.join("hooks.json"));
    let pre = data["hooks"]["PreToolUse"].as_array().unwrap();
    assert_eq!((&pre[0], &pre[1]), (&review, &rtk));
    assert!(pre[2..].iter().any(|g| hooks::refers(&g["hooks"][0]["command"], "state_hook.py", true)));
    assert!(pre[2..].iter().any(|g| g["matcher"] == "Bash" && hooks::refers(&g["hooks"][0]["command"], "guard_tmux.py", true)));
    let nav = ours(&data, "UserPromptSubmit", "nav_hook.py");
    assert_eq!(nav.len(), 1);
    if !windows {
        assert_eq!(nav, vec![format!("{} || exit 0", command(&repo, "nav_hook.py"))]);
    }
    assert!(codex_hooks::sync(&codex, &repo, windows).unwrap().is_empty());
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
    fs::create_dir_all(home.join(".claude")).unwrap();
    fs::create_dir_all(home.join(".codex")).unwrap();
    fs::create_dir_all(home.join(".pi/agent")).unwrap();
    let service = AccountService::new(env(home, &[]));
    harness_setup::sync_startup(&service);
    let current = home.join(".hangar/current");
    let data = read(&home.join(".claude/settings.json"));
    let stop = format!("\"{}\" \"{}\" || exit 0", current.join("python/bin/python3").display(), current.join("backend/hooks/state_hook.py").display());
    assert_eq!(ours(&data, "Stop", "state_hook.py"), vec![stop]);
    for (event, script) in [("PreToolUse", "askq_capture.py"), ("MessageDisplay", "preview_hook.py"),
        ("SubagentStart", "subagent_hook.py"), ("SessionStart", "pair-context"), ("UserPromptSubmit", "nav_hook.py")] {
        assert_eq!(ours(&data, event, script).len(), 1, "{script}");
    }
    assert!(!fs::read_to_string(home.join(".claude/settings.json")).unwrap().contains("versions"));
    assert!(ours(&read(&home.join(".codex/hooks.json")), "Stop", "state_hook.py")[0].contains(&*current.to_string_lossy()));
    assert_eq!(fs::read_link(home.join(".pi/agent/skills-bridge/from-repo")).unwrap(), current.join("skills/from-repo"));
}
