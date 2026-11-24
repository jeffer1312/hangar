//! Hooks no Rust: o que o instalador grava e conserta no Codex, no Kimi, na exportação, na
//! importação e na saúde.
use hangar_server::{
    accounts::environment::AccountEnvironment,
    codex_setup::stage_sources,
    config_sync::export,
    harness_setup::{codex_hooks, health, hooks, kimi_hooks},
};
use serde_json::{Value, json};
use std::{fs, path::Path};

const PROGRAM: &str = "/home/u/.hangar/bin/hangar-launcher";
const ALLOW: &str = "/home/u/.hangar/current/scripts/hangar-server-command";

fn commands(data: &Value, event: &str) -> Vec<String> {
    data["hooks"][event].as_array().into_iter().flatten()
        .flat_map(|block| block["hooks"].as_array().cloned().unwrap_or_default())
        .filter_map(|hook| hook["command"].as_str().map(str::to_owned))
        .collect()
}

/// O hook de estado sai de todo evento do Codex; evento que esvazia sai junto.
#[test]
fn h14_codex_state_hook_entries_leave_every_event() {
    let personal = json!({"hooks": [{"type": "command", "command": "notify-send stop"}]});
    let mut data = json!({"hooks": {
        "SessionStart": [{"hooks": [{"type": "command", "command": "\"/v/python/bin/python3\" \"/v/backend/hooks/state_hook.py\" || exit 0"}]}],
        "Stop": [personal, {"hooks": [{"type": "command", "command": "\"/v/python/bin/python3\" \"/v/backend/hooks/state_hook.py\" || exit 0"}]}],
    }});
    let touched = codex_hooks::reconcile(&mut data, Path::new(PROGRAM), ALLOW, false);
    assert!(touched.contains(&"SessionStart".to_owned()) && touched.contains(&"Stop".to_owned()));
    assert!(data["hooks"].get("SessionStart").is_none(), "{data}");
    assert_eq!(data["hooks"]["Stop"], json!([personal]));
}

/// Formas do comando por plataforma. POSIX: `|| exit 0`, guarda sem sufixo; Windows
/// (PowerShell): `& "exe" sub ; exit 0` e o guarda leva `$LASTEXITCODE`.
#[test]
fn h54_h69_codex_command_forms_per_platform() {
    let program = Path::new(PROGRAM);
    assert_eq!(codex_hooks::command(program, "nav-hook", false, false), format!("\"{PROGRAM}\" nav-hook || exit 0"));
    assert_eq!(codex_hooks::command(program, "guard-tmux", false, true), format!("\"{PROGRAM}\" guard-tmux"));
    assert_eq!(codex_hooks::command(program, "nav-hook", true, false), format!("& \"{PROGRAM}\" nav-hook ; exit 0"));
    assert_eq!(codex_hooks::command(program, "guard-tmux", true, true), format!("& \"{PROGRAM}\" guard-tmux ; exit $LASTEXITCODE"));
}

/// A entrada nossa funcional (de outro checkout, de venv que sumiu, forma Windows
/// sem `&`) é reescrita NO LUGAR; as alheias ficam byte a byte; segunda rodada não muda nada.
#[test]
fn h68_h70_codex_rewrites_our_entries_in_place_and_settles() {
    for windows in [false, true] {
        let review = json!({"matcher": "Bash", "hooks": [{"type": "command", "command": "review"}]});
        let mut data = json!({"hooks": {
            "UserPromptSubmit": [{"hooks": [
                {"type": "command", "command": "echo antes"},
                {"type": "command", "command": "\"/outro/checkout/backend/.venv/bin/python\" \"/outro/checkout/backend/hooks/nav_hook.py\" || exit 0"},
            ]}],
            "PreToolUse": [review, {"matcher": "Bash", "hooks": [
                {"type": "command", "command": "\"C:\\h\\python\\python.exe\" \"C:\\h\\backend\\hooks\\guard_tmux.py\""},
            ]}],
        }});
        let touched = codex_hooks::reconcile(&mut data, Path::new(PROGRAM), ALLOW, windows);
        assert_eq!(touched, vec!["UserPromptSubmit".to_owned(), "PreToolUse".to_owned()], "windows={windows}");
        assert_eq!(commands(&data, "UserPromptSubmit"), vec!["echo antes".to_owned(), codex_hooks::command(Path::new(PROGRAM), "nav-hook", windows, false)]);
        assert_eq!(data["hooks"]["PreToolUse"][0], review);
        if windows {
            assert_eq!(data["hooks"]["PreToolUse"][1], json!({"matcher": "Bash", "hooks": [
                {"type": "command", "command": codex_hooks::command(Path::new(PROGRAM), "guard-tmux", true, true)}]}));
        }
        let before = data.clone();
        assert!(codex_hooks::reconcile(&mut data, Path::new(PROGRAM), ALLOW, windows).is_empty());
        assert_eq!(data, before);
    }
}

/// `hooks.json` torto fica byte a byte (o modo conferido devolve o erro); pasta sem conta, nada.
#[test]
fn h71_broken_codex_hooks_json_is_kept_and_a_missing_account_is_skipped() {
    let root = tempfile::tempdir().unwrap();
    let codex = root.path().join(".codex");
    fs::create_dir_all(&codex).unwrap();
    for raw in ["{x", "[1]", "{\"hooks\": []}"] {
        fs::write(codex.join("hooks.json"), raw).unwrap();
        assert!(codex_hooks::sync(&codex, root.path(), cfg!(windows)).unwrap().is_empty(), "{raw}");
        assert!(codex_hooks::sync_checked(&codex, root.path(), cfg!(windows)).is_err(), "{raw}");
        assert_eq!(fs::read_to_string(codex.join("hooks.json")).unwrap(), raw);
    }
    assert!(codex_hooks::sync(&root.path().join(".codex-nao-existe"), root.path(), cfg!(windows)).unwrap().is_empty());
}

/// No Codex, o `pair-context` que veio de um binário de teste passa ao programa dos
/// hooks com os mesmos argumentos, e o `codex-hook-allow.py` de um checkout vira o `codex-hook-allow`.
#[test]
fn h82_codex_machine_repairs_point_to_the_package_commands() {
    let pair = "\"/home/u/.hangar/teste-renomear/hangar-server\" pair-context \"/home/u/.claude/.hangar-pair\" || exit 0";
    let allow = "\"/home/u/Projetos/hangar/backend/.venv/bin/python3\" \"/home/u/Projetos/hangar/scripts/codex-hook-allow.py\" -- rtk hook claude";
    let personal = "\"/opt/x/pair-context-viewer\" pair-context";
    let mut data = json!({"hooks": {
        "SessionStart": [{"matcher": "startup|resume|clear|compact", "hooks": [{"type": "command", "command": pair}]}],
        "PreToolUse": [{"matcher": "Bash", "hooks": [{"type": "command", "command": allow}, {"type": "command", "command": personal}]}],
    }});
    codex_hooks::reconcile(&mut data, Path::new(PROGRAM), ALLOW, false);
    assert_eq!(commands(&data, "SessionStart"), vec![format!("\"{PROGRAM}\" pair-context \"/home/u/.claude/.hangar-pair\" || exit 0")]);
    let pre = commands(&data, "PreToolUse");
    assert!(pre.iter().any(|c| c.contains(ALLOW) && c.ends_with("codex-hook-allow -- rtk hook claude")), "{pre:?}");
    assert!(pre.contains(&personal.to_owned()), "comando pessoal fica: {pre:?}");
    assert!(!pre.iter().any(|c| c.contains("codex-hook-allow.py")));
}

/// O subcomando só é nosso atrás de um programa do Hangar. Hook pessoal com o
/// mesmo argumento (`pair-context`, `state-hook`, `guard-tmux`) fica, inclusive repetido.
#[test]
fn personal_commands_with_our_subcommand_names_stay() {
    let viewer = "\"/opt/x/pair-context-viewer\" pair-context";
    let mine = "my-tool state-hook --log";
    let guard = "\"/opt/audit\" guard-tmux";
    let mut data = json!({"hooks": {
        "SessionStart": [{"hooks": [{"type": "command", "command": viewer}, {"type": "command", "command": viewer}]}],
        "Stop": [{"hooks": [{"type": "command", "command": mine}]}, {"hooks": [{"type": "command", "command": mine}]}],
        "PreToolUse": [{"matcher": "Bash", "hooks": [{"type": "command", "command": guard}]}],
    }});
    hooks::reconcile(&mut data, Path::new(PROGRAM), Path::new("/h/.claude/.hangar-pair"), &|_: &str| true, false);
    assert_eq!(commands(&data, "SessionStart")[..2], [viewer.to_owned(), viewer.to_owned()]);
    assert_eq!(commands(&data, "SessionStart").iter().filter(|c| c.contains("pair-context")).count(), 3, "o nosso entra num bloco próprio");
    assert_eq!(commands(&data, "Stop")[..2], [mine.to_owned(), mine.to_owned()]);
    assert!(commands(&data, "PreToolUse").contains(&guard.to_owned()));
    let mut codex = json!({"hooks": {"UserPromptSubmit": [{"hooks": [{"type": "command", "command": "notify nav-hook"}]}]}});
    codex_hooks::reconcile(&mut codex, Path::new(PROGRAM), ALLOW, false);
    assert_eq!(commands(&codex, "UserPromptSubmit"), vec!["notify nav-hook".to_owned(), codex_hooks::command(Path::new(PROGRAM), "nav-hook", false, false)]);
}

/// Script pessoal com o nome de um hook antigo do Hangar fica no Codex: só o de `backend/hooks` é nosso.
#[test]
fn codex_personal_script_named_like_an_old_hook_stays() {
    let state = "python3 ~/tools/state_hook.py";
    let nav = "python3 ~/tools/nav_hook.py";
    let legacy = "\"/v/python/bin/python3\" \"/v/backend/hooks/state_hook.py\" || exit 0";
    let mut data = json!({"hooks": {
        "Stop": [{"hooks": [{"type": "command", "command": state}, {"type": "command", "command": legacy}]}],
        "UserPromptSubmit": [{"hooks": [{"type": "command", "command": nav}]}],
    }});
    codex_hooks::reconcile(&mut data, Path::new(PROGRAM), ALLOW, false);
    assert_eq!(commands(&data, "Stop"), vec![state.to_owned()]);
    assert_eq!(commands(&data, "UserPromptSubmit"), vec![nav.to_owned(), codex_hooks::command(Path::new(PROGRAM), "nav-hook", false, false)]);
}

fn kimi_env(home: &Path) -> AccountEnvironment {
    AccountEnvironment::from_map([("HOME".into(), home.to_string_lossy().into_owned()), ("USERPROFILE".into(), home.to_string_lossy().into_owned())].into())
}

/// Um `config.toml` com dezenas de entradas antigas de várias raízes de venv para o mesmo evento vira uma entrada por evento, com o resto do
/// TOML byte a byte e a cópia `.bak-hangar` uma vez.
#[test]
fn h74_h75_kimi_old_entries_collapse_into_one_per_event() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join(".kimi-code/config.toml");
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    let mut raw = String::from("# minha config\ndefault_model = 'k2'\n\n[providers.x]\nbase_url = 'y'\n");
    for venv in ["/a/backend/.venv", "/tmp/pr9-1/backend/.venv", "/h/.hangar/current/backend/.venv"] {
        for event in ["SessionStart", "Stop"] {
            raw += &format!("\n# Remova este bloco para desligar o rastreio de sessoes Kimi pelo app.\n[[hooks]]\nevent = \"{event}\"\ncommand = '\"{venv}/bin/python\" \"{venv}/../hooks/kimi_state_hook.py\"'\ntimeout = 5\n");
        }
        raw += &format!("\n[[hooks]]\nevent = \"PreToolUse\"\nmatcher = \"Bash\"\ncommand = '\"{venv}/bin/python\" \"{venv}/../hooks/guard_tmux.py\"'\ntimeout = 5\n");
    }
    raw += "\n[[hooks]]\nevent = \"Stop\"\ncommand = 'notifier --fim'\n\n# fim do arquivo\n[outra]\nx = 1\n";
    fs::write(&path, &raw).unwrap();
    let env = kimi_env(root.path());
    assert_eq!(kimi_hooks::sync(&env, root.path()).unwrap().len(), 1);
    let first = fs::read_to_string(&path).unwrap();
    let data: toml::Value = toml::from_str(&first).unwrap();
    let hooks = data["hooks"].as_array().unwrap();
    let ours: Vec<_> = hooks.iter().filter(|h| h["command"].as_str().is_some_and(|c| c.contains("kimi-state-hook") || c.contains("guard-tmux"))).collect();
    assert_eq!(ours.len(), kimi_hooks::ENTRIES.len(), "{first}");
    for &(event, matcher, mode) in kimi_hooks::ENTRIES {
        let count = ours.iter().filter(|h| h["event"].as_str() == Some(event) && h.get("matcher").and_then(toml::Value::as_str) == matcher
            && h["command"].as_str().unwrap().ends_with(mode)).count();
        assert_eq!(count, 1, "{event} {matcher:?} {mode}");
    }
    assert!(!first.contains(".py"), "{first}");
    assert!(first.starts_with("# minha config\ndefault_model = 'k2'\n\n[providers.x]\nbase_url = 'y'\n"), "{first}");
    assert!(first.contains("command = 'notifier --fim'") && first.contains("# fim do arquivo\n[outra]\nx = 1\n"), "{first}");
    assert_eq!(fs::read_to_string(path.with_file_name("config.toml.bak-hangar")).unwrap(), raw);
    assert!(kimi_hooks::sync(&env, root.path()).unwrap().is_empty());
}

/// O comando nativo de antes (`hangar-server`, `hangar-server-command`) é reconhecido como nosso e
/// passa ao programa dos hooks, inclusive o próprio lançador estável.
#[test]
fn h74_kimi_native_commands_follow_the_hook_program() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join(".kimi-code/config.toml");
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    let launcher = root.path().join(".hangar/bin").join(if cfg!(windows) { "hangar-launcher.exe" } else { "hangar-launcher" });
    fs::create_dir_all(launcher.parent().unwrap()).unwrap();
    fs::copy(std::env::current_exe().unwrap(), &launcher).unwrap();
    fs::write(&path, "[[hooks]]\nevent = 'Stop'\ncommand = '\"/h/.hangar/current/scripts/hangar-server-command\" kimi-state-hook'\ntimeout = 5\n").unwrap();
    let env = kimi_env(root.path());
    kimi_hooks::sync(&env, root.path()).unwrap();
    let text = fs::read_to_string(&path).unwrap();
    assert!(!text.contains("hangar-server-command"), "{text}");
    let data: toml::Value = toml::from_str(&text).unwrap();
    assert!(data["hooks"].as_array().unwrap().iter().all(|h| kimi_hooks::installed(root.path(), h["command"].as_str().unwrap(), if h["command"].as_str().unwrap().ends_with("guard-tmux") { "guard-tmux" } else { "kimi-state-hook" })));
}

/// A subida só conserta e tira repetidas; não instala onde não havia,
/// não reescreve entrada já certa (aspas literais ficam) e não cria a cópia `.bak-hangar` à toa.
#[test]
fn kimi_boot_repair_never_installs_and_leaves_correct_entries_byte_identical() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join(".kimi-code/config.toml");
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    let env = kimi_env(root.path());
    let personal = "default_model = 'k2'\n\n[[hooks]]\nevent = 'Stop'\ncommand = 'notifier --fim'\n";
    fs::write(&path, personal).unwrap();
    assert!(kimi_hooks::repair(&env).unwrap().is_empty());
    assert_eq!(fs::read_to_string(&path).unwrap(), personal);
    assert!(!path.with_file_name("config.toml.bak-hangar").exists());
    let program = hangar_server::codex_setup::hook_command::hook_server(root.path()).unwrap();
    let correct = format!("[[hooks]]\nevent = 'Stop'\ncommand = '\"{}\" kimi-state-hook'\ntimeout = 5\n", program.display());
    fs::write(&path, &correct).unwrap();
    assert!(kimi_hooks::repair(&env).unwrap().is_empty());
    assert_eq!(fs::read_to_string(&path).unwrap(), correct);
    assert!(!path.with_file_name("config.toml.bak-hangar").exists());
}

/// O guarda antigo sem `matcher` vale para o Bash; o instalador não põe outro.
#[test]
fn kimi_guard_without_matcher_does_not_get_a_second_one() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join(".kimi-code/config.toml");
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(&path, "[[hooks]]\nevent = 'PreToolUse'\ncommand = 'python /v/backend/hooks/guard_tmux.py'\n").unwrap();
    kimi_hooks::sync(&kimi_env(root.path()), root.path()).unwrap();
    let data: toml::Value = toml::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
    let guards = data["hooks"].as_array().unwrap().iter().filter(|h| h["command"].as_str().unwrap().ends_with("guard-tmux")).count();
    assert_eq!(guards, 1);
    assert!(kimi_hooks::sync(&kimi_env(root.path()), root.path()).unwrap().is_empty());
}

/// A lista "do Hangar" da exportação é fixa e cobre os subcomandos, o guarda e os
/// scripts Python de máquinas não atualizadas; comando pessoal não entra.
#[test]
fn h64_export_own_hooks_is_a_fixed_list_with_the_subcommands() {
    let own: std::collections::BTreeSet<String> = export::OWN_HOOKS.iter().map(|n| (*n).to_owned()).collect();
    for command in [
        format!("\"{PROGRAM}\" state-hook || exit 0"),
        format!("\"{PROGRAM}\" nav-hook || echo \"[hangar] O hook nav-hook do Hangar falhou\""),
        format!("\"{PROGRAM}\" guard-tmux"),
        format!("\"{PROGRAM}\" askq-capture || exit 0"),
        format!("\"{PROGRAM}\" preview-hook || exit 0"),
        format!("\"{PROGRAM}\" pair-context \"/h/.claude/.hangar-pair\" || exit 0"),
        format!("\"{PROGRAM}\" agy-state-hook"),
        "\"/v/python3\" \"/v/backend/hooks/state_hook.py\" || exit 0".to_owned(),
        "\"/v/python3\" \"/h/.claude/hooks/guard_tmux.py\"".to_owned(),
    ] {
        assert!(export::is_own_hook(&command, &own), "{command}");
    }
    for command in ["rtk hook claude", "notify-send state-hooked", "/x/kimi-state-hook-viewer"] {
        assert!(!export::is_own_hook(command, &own), "{command}");
    }
}

/// Hook pessoal com um subcomando do Hangar como argumento viaja na exportação.
#[test]
fn export_personal_hook_with_our_subcommand_as_argument_is_foreign() {
    let own: std::collections::BTreeSet<String> = export::OWN_HOOKS.iter().map(|n| (*n).to_owned()).collect();
    assert!(!export::is_own_hook("~/bin/notify guard-tmux", &own));
    assert!(export::is_own_hook(&format!("\"{PROGRAM}\" guard-tmux"), &own));
}

/// A importação Claude→Codex deixa de fora os subcomandos do Hangar (o Codex tem os
/// dele), além dos scripts Python de antes; o que é pessoal atravessa.
#[test]
fn h67_import_to_codex_excludes_the_hangar_subcommands() {
    let ours = [
        format!("\"{PROGRAM}\" state-hook || exit 0"),
        format!("\"{PROGRAM}\" pair-context \"/h/.claude/.hangar-pair\" || exit 0"),
        "\"/h/.hangar/current/scripts/hangar-server-command\" guard-tmux".to_owned(),
        "\"C:\\Users\\u\\.hangar\\bin\\hangar-launcher.exe\" nav-hook || exit 0".to_owned(),
        "\"/v/python3\" \"/v/backend/hooks/askq_capture.py\" || exit 0".to_owned(),
    ];
    let mut hooks_value = json!({"UserPromptSubmit": [{"hooks": []}]});
    let list = hooks_value["UserPromptSubmit"][0]["hooks"].as_array_mut().unwrap();
    for command in &ours {
        list.push(json!({"type": "command", "command": command}));
    }
    list.push(json!({"type": "command", "command": "rtk hook claude"}));
    let kept = stage_sources::without_app_hooks(&hooks_value);
    assert_eq!(kept, json!({"UserPromptSubmit": [{"hooks": [{"type": "command", "command": "rtk hook claude"}]}]}));
}

/// A saúde do Claude confere os subcomandos (cinco hooks); o Python antigo conta como faltando.
#[test]
fn h65_claude_health_checks_the_subcommands() {
    let root = tempfile::tempdir().unwrap();
    let dir = root.path().join(".claude");
    fs::create_dir_all(&dir).unwrap();
    let mut data = json!({"hooks": {}});
    let program = std::env::current_exe().unwrap();
    hooks::reconcile(&mut data, &program, &dir.join(".hangar-pair"), &|_: &str| true, cfg!(windows));
    fs::write(dir.join("settings.json"), data.to_string()).unwrap();
    let ok = health::claude_hooks(&dir);
    assert_eq!(ok["ok"], true, "{ok}");
    let old = json!({"hooks": {"Stop": [{"hooks": [{"type": "command", "command": "python3 /v/backend/hooks/state_hook.py"}]}]}});
    fs::write(dir.join("settings.json"), old.to_string()).unwrap();
    let missing = health::claude_hooks(&dir);
    assert_eq!(missing["ok"], false, "{missing}");
    assert!(missing.to_string().contains("state-hook"), "{missing}");
}
