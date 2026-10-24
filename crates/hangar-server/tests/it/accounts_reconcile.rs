//! `hangar-server conta` contra o que o `scripts/hangar-conta` do Python fez nos mesmos cenários
//! (`backend/tests/fixtures/contract/accounts-reconcile/`). Os construtores daqui montam o estado
//! inicial dos dois lados; o gerador (`#[ignore]`) roda o Python de origem e grava a fixture, e cada
//! teste roda o Rust e compara pela regra do contrato (as diferenças D* do `decisoes.md`).
use hangar_server::accounts::{AccountKey, Provider};
use serde_json::{Map, Value, json};
use std::collections::BTreeMap;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::Command;

const ORIGIN: &str = "b2832ee04";
const COMMAND: &str = "HANGAR_FIXTURE_PY_ROOT=<checkout com scripts/hangar-conta e backend/app de b2832ee04> \
    cargo test -p hangar-server --test it accounts_reconcile::generate_fixtures -- --ignored --exact";

fn fixture_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../backend/tests/fixtures/contract/accounts-reconcile")
}

// ── construtores ────────────────────────────────────────────────────────────────────────────

type Build = Box<dyn Fn(&Path)>;

/// Arquivo com o modo fixo: o estado inicial não depende do umask de quem roda o teste.
fn put(path: &Path, bytes: impl AsRef<[u8]>) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, bytes).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o644)).unwrap();
}
fn mkdirs(path: &Path) { std::fs::create_dir_all(path).unwrap(); }
fn ln(path: &Path, target: impl AsRef<Path>) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    symlink(target, path).unwrap();
}
fn rm(path: &Path) {
    let meta = std::fs::symlink_metadata(path).unwrap();
    if meta.is_dir() { std::fs::remove_dir_all(path).unwrap() } else { std::fs::remove_file(path).unwrap() }
}
fn chmod(path: &Path, mode: u32) { std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).unwrap(); }
/// mtime da entrada, sem seguir link (a poda da gaveta lê o `lstat`).
fn mtime(path: &Path, secs: i64) {
    let c = std::ffi::CString::new(path.as_os_str().as_encoded_bytes()).unwrap();
    let t = libc::timespec { tv_sec: secs, tv_nsec: 0 };
    let rc = unsafe { libc::utimensat(libc::AT_FDCWD, c.as_ptr(), [t, t].as_ptr(), libc::AT_SYMLINK_NOFOLLOW) };
    assert_eq!(rc, 0, "utimensat {}", path.display());
}
fn fifo(path: &Path) {
    let c = std::ffi::CString::new(path.as_os_str().as_encoded_bytes()).unwrap();
    assert_eq!(unsafe { libc::mkfifo(c.as_ptr(), 0o644) }, 0);
}

/// O HOME do `casa` do `test_contas.py`.
fn casa(h: &Path) {
    put(&h.join(".claude/skills/falar/SKILL.md"), "oi");
    put(&h.join(".claude/projects/-tmp-x/memory/MEMORY.md"), "m");
    put(&h.join(".claude/settings.json"), r#"{"theme":"dark"}"#);
    put(&h.join(".claude/CLAUDE.md"), "# regras");
    put(&h.join(".claude/.credentials.json"), r#"{"claudeAiOauth":{}}"#);
    put(&h.join(".claude.json"), r#"{"oauthAccount": {"emailAddress": "um@exemplo.com"}, "mcpServers": {"tavily": {}}, "projects": {"/tmp/x": {"allowedTools": ["Bash"]}}}"#);
}

/// Conta carimbada como o `criar` a deixa antes de reconciliar.
fn conta(h: &Path, name: &str) {
    let dir = h.join(format!(".claude-{name}"));
    put(&dir.join(".hangar-conta"), "");
    put(&dir.join(".claude.json"), r#"{"mcpServers": {"tavily": {}}, "projects": {"/tmp/x": {"allowedTools": ["Bash"]}}}"#);
    mkdirs(&dir.join("projects"));
}

fn long_name() -> String { format!("{}.md", "l".repeat(240)) }

// ── cenários ────────────────────────────────────────────────────────────────────────────────

enum Step {
    Cli(Vec<String>),
    Fs(Build),
    /// Roda o comando com a guarda da conta presa em exclusivo por outro processo.
    Busy(Vec<String>),
}

#[derive(Default)]
struct Rules {
    /// D2: na linha `line` do stderr do passo `step` só o texto fixo em volta do erro do sistema conta.
    d2: Vec<(usize, usize, &'static str, &'static str)>,
    /// O texto do sistema muda (D2), o caminho que a mensagem do Python trazia não: o trecho variável da
    /// linha leva este caminho nos dois lados.
    path: Vec<(usize, usize, &'static str)>,
    /// D3: exceção crua do Python; o Rust sai com o mesmo código e uma linha `hangar-conta:` em português.
    d3: Vec<usize>,
    /// D6: avisos de memória sem ordem, e as `.drift/memory.N` comparadas pelo conteúdo.
    d6: bool,
}

struct Scenario { build: Build, steps: Vec<Step>, env: Vec<(&'static str, &'static str)>, rules: Rules }

fn cli(args: &[&str]) -> Step { Step::Cli(args.iter().map(|a| (*a).to_owned()).collect()) }
fn cli_s(args: Vec<String>) -> Step { Step::Cli(args) }
fn fs(f: impl Fn(&Path) + 'static) -> Step { Step::Fs(Box::new(f)) }
fn sc(build: impl Fn(&Path) + 'static, steps: Vec<Step>) -> Scenario {
    Scenario { build: Box::new(build), steps, env: vec![], rules: Rules::default() }
}
fn d2(mut s: Scenario, step: usize, line: usize, prefix: &'static str, suffix: &'static str) -> Scenario {
    s.rules.d2.push((step, line, prefix, suffix));
    s
}
fn d3(mut s: Scenario, step: usize) -> Scenario { s.rules.d3.push(step); s }
fn with_path(mut s: Scenario, step: usize, line: usize, path: &str) -> Scenario {
    s.rules.path.push((step, line, Box::leak(path.to_owned().into_boxed_str())));
    s
}

const SYMLINK_SUFFIX: &str = ". No Windows isso exige o Modo Desenvolvedor ligado (Configurações → Sistema → Para \
    desenvolvedores). Sem ele a conta ficaria com uma CÓPIA, que passa a divergir do original sem ninguém perceber.";
const FAILED: &str = "hangar-conta: falha ao executar: ";
const PREP: [&str; 2] = ["--prep", "conta2"];
const NEW: [&str; 2] = ["--new", "conta2"];

fn shared_plugins(h: &Path, enabled: &str, registry: Option<&str>) {
    put(&h.join(".claude/settings.json"), format!(r#"{{"enabledPlugins": {enabled}}}"#));
    if let Some(registry) = registry { put(&h.join(".claude/plugins/installed_plugins.json"), registry); }
}

#[allow(clippy::too_many_lines)]
fn scenario(name: &str) -> Scenario {
    match name {
        // ── criação (`--new`) ──
        "new_complete" => sc(casa, vec![cli(&NEW), cli(&["--list"])]),
        "new_existing_dir" => sc(|h| { casa(h); mkdirs(&h.join(".claude-conta2")); }, vec![cli(&NEW)]),
        "new_broken_symlink" => sc(|h| { casa(h); ln(&h.join(".claude-conta2"), h.join("nada")); }, vec![cli(&NEW)]),
        "new_invalid_names" => {
            let mut steps: Vec<Step> = ["Conta 2", "conta2\n", "conta/2", "..", &"x".repeat(33), "-abc", "_abc", ""]
                .iter().map(|n| cli(&["--new", n])).collect();
            steps.push(cli_s(vec!["--new".into(), "x".repeat(32)]));
            steps.push(cli(&["--list"]));
            sc(casa, steps)
        }
        "new_without_shared" => sc(|h| put(&h.join(".claude.json"), r#"{"mcpServers": {"tavily": {}}}"#),
            vec![cli(&["--new", "nova"]), cli(&["--list"])]),
        "new_symlink_refused" => {
            let s = sc(|h| { casa(h); put(&h.join(".claude").join(long_name()), "longo"); }, vec![cli(&NEW), cli(&["--list"])]);
            let prefix: &'static str = Box::leak(format!("hangar-conta: não consegui criar o atalho {}: ", long_name()).into_boxed_str());
            let s = with_path(d2(s, 0, 0, prefix, SYMLINK_SUFFIX), 0, 0, &format!("<HOME>/.claude/{}", long_name()));
            with_path(s, 0, 0, &format!("<HOME>/.claude-conta2/{}.hangar-novo.<PID>.<HEX>", long_name()))
        }
        "new_claude_json_truncated" => d2(sc(|h| { casa(h); put(&h.join(".claude.json"), r#"{"oauthAccount": {"emailA"#); },
            vec![cli(&NEW), cli(&["--list"])]), 0, 0, "hangar-conta: não consegui ler o ~/.claude.json pra semear a conta: ", ""),
        "new_claude_json_not_object" => sc(|h| { casa(h); put(&h.join(".claude.json"), "[1, 2, 3]"); }, vec![cli(&NEW)]),
        "new_claude_json_unreadable" => with_path(d2(sc(|h| { casa(h); chmod(&h.join(".claude.json"), 0); }, vec![cli(&NEW)]),
            0, 0, "hangar-conta: não consegui ler o ~/.claude.json pra semear a conta: ", ""), 0, 0, "<HOME>/.claude.json"),
        "new_apelidos" => sc(|h| { casa(h); put(&h.join(".claude/.hangar-apelidos.json"), r#"{"claude:/x": "Nome Bom"}"#); },
            vec![cli(&NEW), fs(|h| put(&h.join(".claude-conta2/.hangar-apelidos.json"), r#"{"claude:/x": "velho"}"#)), cli(&PREP)]),
        "new_without_claude_json" => sc(|h| { casa(h); rm(&h.join(".claude.json")); }, vec![cli(&NEW)]),
        "new_seed_format" => sc(|h| {
            casa(h);
            put(&h.join(".claude.json"), "{\"oauthAccount\": {\"x\": 1}, \"nome\": \"ação ✓ 😀\", \"num\": 1e16, \"meio\": 0.5, \
                \"vazio\": {}, \"lista\": [], \"aninhado\": {\"a\": [1, {\"b\": null}]}, \"ctl\": \"a\\u001fb\\u007f\"}");
        }, vec![cli(&NEW)]),

        // ── lista, ajuda e uso ──
        "list_rules" => sc(|h| {
            casa(h);
            for n in ["b", "a", "10", "Maiuscula"] { conta(h, n); }
            mkdirs(&h.join(".claude-backup"));
            put(&h.join("fora/.hangar-conta"), "");
            ln(&h.join(".claude-evil"), h.join("fora"));
            conta(h, "pend");
            put(&h.join(".claude-pend/.hangar-account-pending"), "");
            mkdirs(&h.join(".claude-markdir/.hangar-conta"));
            put(&h.join("marcador"), "");
            mkdirs(&h.join(".claude-marklink"));
            ln(&h.join(".claude-marklink/.hangar-conta"), h.join("marcador"));
            put(&h.join(".claude-arquivo"), "");
        }, vec![cli(&["--list"])]),
        "list_empty" => sc(|_| {}, vec![cli(&["--list"])]),
        "help_flags" => sc(|_| {}, vec![cli(&["--help"]), cli(&["-h"])]),
        "usage_errors" => sc(|_| {}, vec![cli(&[]), cli(&["--list", "x"]), cli(&["--prep"]), cli(&["--prep", "a", "b", "c"]),
            cli(&["--new", "a", "b"]), cli(&["--bogus"]), cli(&["--help", "x"])]),

        // ── reconciliação (`--prep`) ──
        "prep_fresh_idempotent" => sc(|h| { casa(h); conta(h, "conta2"); }, vec![cli(&PREP), cli(&PREP)]),
        "prep_project_new" => sc(|h| { casa(h); conta(h, "conta2"); }, vec![cli(&["--prep", "conta2", "-home-jefferson-novo"])]),
        "prep_project_invalid" => sc(|h| { casa(h); conta(h, "conta2"); },
            ["/tmp/fora", "../../fora", "a/b", "a\\b", ""].iter().map(|p| cli(&["--prep", "conta2", p])).collect()),
        "prep_settings_copy" => sc(casa, vec![cli(&NEW), fs(|h| put(&h.join(".claude-conta2/settings.json"), r#"{"stripped": true}"#)), cli(&PREP)]),
        "prep_mirror_principal_wins" => sc(casa, vec![cli(&NEW), fs(|h| {
            put(&h.join(".claude/settings.json"), r#"{"theme":"dark","enabledPlugins":{"superpowers@oficial":true}}"#);
            put(&h.join(".claude-conta2/settings.json"), r#"{"theme":"light","enabledPlugins":{"velho@x":true},"model":"opus"}"#);
        }), cli(&PREP)]),
        "prep_plugin_from_account" => sc(casa, vec![cli(&NEW), fs(|h| {
            put(&h.join(".claude/settings.json"), r#"{"enabledPlugins":{"ecc@x":true,"humanizer@x":false}}"#);
            put(&h.join(".claude-conta2/settings.json"), r#"{"enabledPlugins":{"ecc@x":true,"humanizer@x":true,"novo@x":true}}"#);
        }), cli(&PREP), cli(&PREP), fs(|h| put(&h.join(".claude/settings.json"),
            r#"{"enabledPlugins":{"ecc@x":true,"humanizer@x":false,"outro@x":true}}"#)), cli(&PREP)]),
        "prep_mcp_mirror" => sc(casa, vec![cli(&NEW), fs(|h| {
            put(&h.join(".claude.json"), r#"{"oauthAccount": {"emailAddress": "um@exemplo.com"}, "mcpServers": {"tavily": {"url": "nova"}, "context7": {}}}"#);
            put(&h.join(".claude-conta2/.claude.json"), r#"{"mcpServers": {"tavily": {}, "so-da-conta": {}}, "projects": {"/tmp/x": {"allowedTools": ["Bash", "Edit"]}}}"#);
        }), cli(&PREP), cli(&PREP)]),
        "prep_mirror_warns_changed_key" => sc(casa, vec![cli(&NEW), fs(|h| {
            put(&h.join(".claude/settings.json"), r#"{"theme":"dark","outputStyle":"Concise"}"#);
            put(&h.join(".claude-conta2/settings.json"), r#"{"theme":"light"}"#);
        }), cli(&PREP), cli(&PREP)]),
        "prep_absent_key_kept" => sc(casa, vec![cli(&NEW),
            fs(|h| put(&h.join(".claude-conta2/settings.json"), r#"{"enabledPlugins":{"superpowers@oficial":true}}"#)), cli(&PREP)]),
        "prep_settings_old_symlink" => sc(casa, vec![cli(&NEW), fs(|h| {
            rm(&h.join(".claude-conta2/settings.json"));
            ln(&h.join(".claude-conta2/settings.json"), h.join(".claude/settings.json"));
        }), cli(&PREP)]),
        "prep_dir_appears_later" => sc(casa, vec![cli(&NEW), fs(|h| mkdirs(&h.join(".claude/plugins"))), cli(&PREP)]),
        "prep_runtime_stays_local" => sc(casa, vec![cli(&NEW), fs(|h| {
            mkdirs(&h.join(".claude/telemetry"));
            put(&h.join(".claude/.last-update-result.json"), "{}");
            ln(&h.join(".claude-conta2/telemetry"), h.join(".claude/telemetry"));
        }), cli(&PREP), fs(|h| {
            mkdirs(&h.join(".claude-conta2/telemetry"));
            put(&h.join(".claude-conta2/.last-update-result.json"), r#"{"local": 1}"#);
        }), cli(&PREP)]),
        "prep_runtime_links_elsewhere" => sc(|h| {
            casa(h);
            conta(h, "conta2");
            for d in ["sessions", "feedback", "image-cache"] { mkdirs(&h.join(".claude").join(d)); }
            put(&h.join(".claude/.hangar-models.json"), "{}");
            mkdirs(&h.join("fora1"));
            put(&h.join("fora2.json"), "{}");
            ln(&h.join(".claude-conta2/sessions"), h.join("fora1"));
            ln(&h.join(".claude-conta2/.hangar-models.json"), h.join("fora2.json"));
            ln(&h.join(".claude-conta2/feedback"), h.join(".claude/feedback"));
            ln(&h.join(".claude-conta2/image-cache"), h.join(".claude/image-cache"));
        }, vec![cli(&PREP)]),
        "prep_dead_link_pruned" => sc(casa, vec![cli(&NEW), fs(|h| rm(&h.join(".claude/CLAUDE.md"))), cli(&PREP)]),
        "prep_local_change_goes_up" => sc(casa, vec![cli(&NEW), fs(|h| {
            rm(&h.join(".claude-conta2/CLAUDE.md"));
            put(&h.join(".claude-conta2/CLAUDE.md"), "# regras editadas");
        }), cli(&PREP)]),
        "prep_local_identical" => sc(casa, vec![cli(&NEW), fs(|h| {
            rm(&h.join(".claude-conta2/CLAUDE.md"));
            put(&h.join(".claude-conta2/CLAUDE.md"), "# regras");
        }), cli(&PREP)]),
        "prep_drift_cap" => {
            let mut steps = vec![cli(&NEW)];
            for i in 0..5 {
                steps.push(fs(move |h| {
                    let skills = h.join(".claude-conta2/skills");
                    rm(&skills);
                    put(&skills.join("versao.txt"), i.to_string());
                    mtime(&skills, 1_700_000_000 + i);
                }));
                steps.push(cli(&PREP));
            }
            sc(casa, steps)
        }
        "prep_not_an_account" => sc(|h| { casa(h); mkdirs(&h.join(".claude-backup")); }, vec![cli(&["--prep", "backup"])]),
        "prep_file_vs_dir_drawer" => sc(casa, vec![cli(&NEW), fs(|h| {
            put(&h.join(".claude-conta2/hooks"), "dado do usuário");
            mkdirs(&h.join(".claude/hooks"));
        }), cli(&PREP)]),
        "prep_unexpected_symlink" => sc(casa, vec![cli(&NEW), fs(|h| {
            put(&h.join("segredo.txt"), "dado externo");
            rm(&h.join(".claude-conta2/CLAUDE.md"));
            ln(&h.join(".claude-conta2/CLAUDE.md"), h.join("segredo.txt"));
        }), cli(&PREP)]),
        "prep_drift_symlink" => sc(casa, vec![cli(&NEW), fs(|h| {
            put(&h.join("fora/sentinela.txt"), "intacto");
            ln(&h.join(".claude-conta2/.drift"), h.join("fora"));
            rm(&h.join(".claude-conta2/skills"));
            mkdirs(&h.join(".claude-conta2/skills"));
        }), cli(&PREP)]),
        "prep_drift_is_file" => sc(casa, vec![cli(&NEW), fs(|h| {
            put(&h.join(".claude-conta2/.drift"), "não é pasta");
            rm(&h.join(".claude-conta2/skills"));
            mkdirs(&h.join(".claude-conta2/skills"));
        }), cli(&PREP)]),
        "prep_root_symlink" => sc(|h| { casa(h); put(&h.join("fora/.hangar-conta"), ""); ln(&h.join(".claude-evil"), h.join("fora")); },
            vec![cli(&["--list"]), cli(&["--prep", "evil"])]),
        "prep_memory_local_drawer" => sc(casa, vec![cli(&NEW), fs(|h| {
            let memo = h.join(".claude-conta2/projects/-tmp-x/memory");
            rm(&memo);
            put(&memo.join("MEMORY.md"), "local");
        }), cli(&PREP)]),
        "prep_memory_project_removed" => sc(casa, vec![cli(&NEW), fs(|h| rm(&h.join(".claude/projects/-tmp-x"))), cli(&PREP)]),
        "prep_tmp_name_user_file" => sc(casa, vec![cli(&NEW), fs(|h| {
            rm(&h.join(".claude-conta2/skills"));
            put(&h.join(".claude-conta2/skills.hangar-novo"), "dado do usuário");
        }), cli(&PREP)]),
        "prep_drift_cap_per_origin" => {
            let mut steps = vec![cli(&["--new", "gaveta"]), fs(|h| {
                let plugins = h.join(".claude-gaveta/plugins");
                rm(&plugins);
                put(&plugins.join("meu.json"), "unico");
            }), cli(&["--prep", "gaveta"])];
            for i in 0..4 {
                steps.push(fs(move |h| {
                    let skills = h.join(".claude-gaveta/skills");
                    rm(&skills);
                    put(&skills.join(format!("v{i}.md")), i.to_string());
                    mtime(&skills, 1_700_000_100 + i);
                }));
                steps.push(cli(&["--prep", "gaveta"]));
            }
            sc(|h| { casa(h); mkdirs(&h.join(".claude/plugins")); }, steps)
        }
        "prep_plugins_warnings" => sc(casa, vec![cli(&NEW), fs(|h| {
            let pasta = h.join(".claude/plugins/cache/oficial/ecc/1.0");
            mkdirs(&pasta);
            put(&h.join(".claude/plugins/installed_plugins.json"), json!({"version": 2, "plugins": {
                "ecc@oficial": [{"scope": "user", "installPath": pasta}],
                "sumiu@oficial": [{"scope": "user", "installPath": pasta.parent().unwrap().join("nada")}]}}).to_string());
            put(&h.join(".claude/settings.json"), r#"{"enabledPlugins": {"ecc@oficial": true, "sumiu@oficial": true, "superpowers@oficial": true, "desligado@oficial": false}}"#);
        }), cli(&PREP)]),
        "prep_no_plugins_silent" => sc(casa, vec![cli(&NEW), cli(&PREP)]),
        "prep_invalid_name" => sc(casa, vec![cli(&["--prep", "Conta"]), cli(&["--prep", "a/b"]), cli(&["--prep", ""]),
            cli(&["--prep", "conta2\n"])]),
        "prep_name_32_chars" => sc(|h| { casa(h); conta(h, &"a".repeat(32)); },
            vec![cli_s(vec!["--prep".into(), "a".repeat(32)]), cli_s(vec!["--prep".into(), "a".repeat(33)])]),
        "prep_pending_account" => sc(|h| { casa(h); conta(h, "conta2"); put(&h.join(".claude-conta2/.hangar-account-pending"), ""); },
            vec![cli(&PREP)]),
        "prep_marker_symlink_or_dir" => sc(|h| {
            casa(h);
            put(&h.join("marcador"), "");
            mkdirs(&h.join(".claude-x"));
            ln(&h.join(".claude-x/.hangar-conta"), h.join("marcador"));
            mkdirs(&h.join(".claude-y/.hangar-conta"));
        }, vec![cli(&["--prep", "x"]), cli(&["--prep", "y"])]),
        "prep_shared_missing" => with_path(d2(sc(|h| conta(h, "conta2"), vec![cli(&PREP)]), 0, 0, FAILED, ""), 0, 0, "<HOME>/.claude"),
        "prep_shared_is_file" => with_path(d2(sc(|h| { put(&h.join(".claude"), "arquivo"); conta(h, "conta2"); }, vec![cli(&PREP)]),
            0, 0, FAILED, ""), 0, 0, "<HOME>/.claude"),
        "prep_guard_busy" => sc(|h| { casa(h); conta(h, "conta2"); }, vec![Step::Busy(PREP.map(str::to_owned).into())]),
        "prep_symlink_refused" => {
            let s = sc(|h| { casa(h); conta(h, "conta2"); put(&h.join(".claude").join(long_name()), "longo"); }, vec![cli(&PREP)]);
            let prefix: &'static str = Box::leak(format!("hangar-conta: não consegui criar o atalho {}: ", long_name()).into_boxed_str());
            let s = with_path(d2(s, 0, 0, prefix, SYMLINK_SUFFIX), 0, 0, &format!("<HOME>/.claude/{}", long_name()));
            with_path(s, 0, 0, &format!("<HOME>/.claude-conta2/{}.hangar-novo.<PID>.<HEX>", long_name()))
        }
        "prep_account_readonly" => {
            let s = d2(sc(|h| { casa(h); conta(h, "conta2"); chmod(&h.join(".claude-conta2"), 0o555); }, vec![cli(&PREP)]),
                0, 0, "hangar-conta: não consegui criar o atalho CLAUDE.md: ", SYMLINK_SUFFIX);
            with_path(with_path(s, 0, 0, "<HOME>/.claude/CLAUDE.md"), 0, 0, "<HOME>/.claude-conta2/CLAUDE.md.hangar-novo.<PID>.<HEX>")
        }

        // ── settings.json ──
        "settings_principal_invalid" => d2(sc(|h| { casa(h); conta(h, "conta2"); put(&h.join(".claude-conta2/settings.json"), "{}");
            put(&h.join(".claude/settings.json"), "{bad"); }, vec![cli(&PREP)]),
            0, 0, "hangar-conta: não consegui espelhar o settings.json pra conta: ", ""),
        "settings_copy_invalid" => d2(sc(|h| { casa(h); conta(h, "conta2"); put(&h.join(".claude-conta2/settings.json"), "{bad"); },
            vec![cli(&PREP)]), 0, 0, "hangar-conta: não consegui espelhar o settings.json pra conta: ", ""),
        "settings_not_object" => sc(|h| { casa(h); conta(h, "conta2"); put(&h.join(".claude-conta2/settings.json"), "{}");
            put(&h.join(".claude/settings.json"), "[1]"); }, vec![cli(&PREP), fs(|h| {
            put(&h.join(".claude/settings.json"), "{}");
            put(&h.join(".claude-conta2/settings.json"), "\"texto\"");
        }), cli(&PREP)]),
        "settings_bom" => d2(sc(|h| { casa(h); conta(h, "conta2"); put(&h.join(".claude-conta2/settings.json"), "{}");
            put(&h.join(".claude/settings.json"), "\u{feff}{\"theme\":\"dark\"}"); }, vec![cli(&PREP)]),
            0, 0, "hangar-conta: não consegui espelhar o settings.json pra conta: ", ""),
        "settings_principal_missing_copy_invalid" => d2(sc(|h| { casa(h); conta(h, "conta2"); rm(&h.join(".claude/settings.json"));
            put(&h.join(".claude-conta2/settings.json"), "{bad"); }, vec![cli(&PREP)]),
            0, 0, "hangar-conta: não consegui ler o settings.json da conta: ", ""),
        "settings_principal_missing" => sc(|h| { casa(h); conta(h, "conta2"); rm(&h.join(".claude/settings.json")); }, vec![cli(&PREP)]),
        "settings_principal_corrupt_seeded" => d2(sc(|h| { casa(h); conta(h, "conta2"); put(&h.join(".claude/settings.json"), "{bad"); },
            vec![cli(&PREP)]), 0, 0, "hangar-conta: não consegui ler o settings.json da conta: ", ""),
        "settings_dead_old_link" => sc(|h| { casa(h); conta(h, "conta2"); rm(&h.join(".claude/settings.json"));
            ln(&h.join(".claude-conta2/settings.json"), h.join(".claude/settings.json")); }, vec![cli(&PREP)]),
        "settings_link_elsewhere" => sc(|h| { casa(h); conta(h, "conta2"); put(&h.join("outro.json"), r#"{"x": 1}"#);
            ln(&h.join(".claude-conta2/settings.json"), h.join("outro.json")); }, vec![cli(&PREP)]),
        "settings_copy_is_dir" => with_path(d2(sc(|h| { casa(h); conta(h, "conta2"); mkdirs(&h.join(".claude-conta2/settings.json")); },
            vec![cli(&PREP)]), 0, 0, "hangar-conta: não consegui espelhar o settings.json pra conta: ", ""),
            0, 0, "<HOME>/.claude-conta2/settings.json"),
        "settings_copy_unreadable" => with_path(d2(sc(|h| { casa(h); conta(h, "conta2"); put(&h.join(".claude-conta2/settings.json"), "{}");
            chmod(&h.join(".claude-conta2/settings.json"), 0); }, vec![cli(&PREP)]),
            0, 0, "hangar-conta: não consegui espelhar o settings.json pra conta: ", ""), 0, 0, "<HOME>/.claude-conta2/settings.json"),
        "settings_python_equality" => sc(|h| {
            casa(h);
            conta(h, "conta2");
            put(&h.join(".claude/settings.json"), r#"{"a": 1, "b": true, "c": 0, "d": [1.0, {"x": false}], "e": {"k": 2, "j": 1}, "f": -0.0}"#);
            put(&h.join(".claude-conta2/settings.json"), r#"{"e": {"j": 1.0, "k": 2}, "d": [1, {"x": 0}], "c": false, "b": 1, "a": 1.0, "f": 0, "extra": 1}"#);
        }, vec![cli(&PREP)]),
        "settings_format" => sc(|h| {
            casa(h);
            conta(h, "conta2");
            put(&h.join(".claude/settings.json"), "{\"theme\": \"dark\", \"num\": 1e16, \"small\": 1e-05, \"half\": 1.5, \"neg0\": -0.0, \
                \"big\": 12345678901234567890, \"int\": 10, \"txt\": \"ação ✓ 😀\", \"ctl\": \"a\\u001fb\\tc\\u007fd\\u2028e\\\"f\\\\g\", \
                \"empty\": {}, \"list\": [], \"nested\": {\"a\": [1, {\"b\": null}], \"c\": true}, \"model\": \"opus\"}");
            put(&h.join(".claude-conta2/settings.json"), r#"{"model": "sonnet", "extra": "só da conta", "theme": "light"}"#);
        }, vec![cli(&PREP)]),
        "settings_duplicate_keys" => sc(|h| { casa(h); conta(h, "conta2"); put(&h.join(".claude-conta2/settings.json"), "{}");
            put(&h.join(".claude/settings.json"), r#"{"a": 1, "b": 2, "a": 3}"#); }, vec![cli(&PREP)]),
        "settings_enabled_plugins_shapes" => sc(|h| {
            casa(h);
            conta(h, "conta2");
            put(&h.join(".claude/settings.json"), r#"{"enabledPlugins": {"a@x": true, "b@x": false}}"#);
            put(&h.join(".claude-conta2/settings.json"), r#"{"enabledPlugins": ["lista"]}"#);
        }, vec![cli(&PREP), fs(|h| put(&h.join(".claude/settings.json"), r#"{"enabledPlugins": null}"#)), cli(&PREP),
            fs(|h| {
                put(&h.join(".claude-conta2/settings.json"), r#"{"enabledPlugins": {"a@x": true}}"#);
                put(&h.join(".claude/settings.json"), r#"{"enabledPlugins": {"a@x": true, "c@x": true}}"#);
            }), cli(&PREP)]),
        "settings_lone_surrogate_equal" => sc(|h| { casa(h); conta(h, "conta2");
            put(&h.join(".claude/settings.json"), r#"{"x": "\ud800"}"#);
            put(&h.join(".claude-conta2/settings.json"), r#"{"x": "\ud800", "y": 1}"#); }, vec![cli(&PREP)]),

        // ── ~/.claude.json ──
        "mcp_copy_missing" => sc(|h| { casa(h); conta(h, "conta2"); rm(&h.join(".claude-conta2/.claude.json")); },
            vec![cli(&PREP), fs(|h| { rm(&h.join(".claude.json")); put(&h.join(".claude-conta2/.claude.json"), "{\"mcpServers\": {}}"); }), cli(&PREP)]),
        "mcp_principal_invalid" => d2(sc(|h| { casa(h); conta(h, "conta2"); put(&h.join(".claude.json"), "{bad"); }, vec![cli(&PREP)]),
            0, 0, "hangar-conta: não consegui espelhar o ~/.claude.json pra conta: ", ""),
        "mcp_not_object" => sc(|h| { casa(h); conta(h, "conta2"); put(&h.join(".claude.json"), "[1]"); }, vec![cli(&PREP)]),
        "mcp_principal_empty_or_not_dict" => sc(|h| { casa(h); conta(h, "conta2"); put(&h.join(".claude.json"), r#"{"mcpServers": {}}"#); },
            vec![cli(&PREP), fs(|h| put(&h.join(".claude.json"), r#"{"mcpServers": ["x"]}"#)), cli(&PREP),
                fs(|h| put(&h.join(".claude.json"), r#"{"outra": 1}"#)), cli(&PREP)]),
        "mcp_copy_not_dict" => sc(|h| { casa(h); conta(h, "conta2");
            put(&h.join(".claude-conta2/.claude.json"), r#"{"mcpServers": null, "projects": {}}"#); }, vec![cli(&PREP)]),
        "mcp_format" => sc(|h| {
            casa(h);
            conta(h, "conta2");
            put(&h.join(".claude.json"), r#"{"mcpServers": {"novo": {"url": "x"}, "tavily": {}}}"#);
            put(&h.join(".claude-conta2/.claude.json"), "{\"nome\": \"ação ✓ 😀\", \"num\": 1e16, \"meio\": 0.5, \"vazio\": {}, \
                \"lista\": [], \"mcpServers\": {\"tavily\": {\"velho\": true}}, \"z\": {\"a\": [1, {\"b\": null}]}}");
        }, vec![cli(&PREP)]),
        "mcp_python_equality" => sc(|h| { casa(h); conta(h, "conta2");
            put(&h.join(".claude.json"), r#"{"mcpServers": {"x": {"n": 1, "b": true}}}"#);
            put(&h.join(".claude-conta2/.claude.json"), r#"{"mcpServers": {"x": {"b": 1, "n": 1.0}}}"#); }, vec![cli(&PREP)]),
        "mcp_lone_surrogate" => sc(|h| { casa(h); conta(h, "conta2");
            put(&h.join(".claude.json"), r#"{"mcpServers": {"novo": {}}}"#);
            put(&h.join(".claude-conta2/.claude.json"), r#"{"projects": {"/p": {"t": "\udc80"}}, "mcpServers": {}}"#); }, vec![cli(&PREP)]),
        "mcp_bom" => d2(sc(|h| { casa(h); conta(h, "conta2");
            put(&h.join(".claude-conta2/.claude.json"), "\u{feff}{\"mcpServers\": {}}"); }, vec![cli(&PREP)]),
            0, 0, "hangar-conta: não consegui espelhar o ~/.claude.json pra conta: ", ""),

        // ── plugins ──
        "plugins_registry_unreadable" => with_path(d2(sc(|h| { casa(h); conta(h, "conta2");
            shared_plugins(h, r#"{"a@x": true}"#, Some(r#"{"plugins": {}}"#)); chmod(&h.join(".claude/plugins/installed_plugins.json"), 0); },
            vec![cli(&PREP)]), 0, 0, "hangar-conta: plugins: não consegui ler <HOME>/.claude/plugins/installed_plugins.json: ", ""),
            0, 0, "<HOME>/.claude/plugins/installed_plugins.json"),
        "plugins_registry_invalid" => d2(sc(|h| { casa(h); conta(h, "conta2"); shared_plugins(h, r#"{"a@x": true}"#, Some("{bad")); },
            vec![cli(&PREP)]), 0, 0, "hangar-conta: plugins: não consegui ler <HOME>/.claude/plugins/installed_plugins.json: ", ""),
        "plugins_registry_not_object" => d3(sc(|h| { casa(h); conta(h, "conta2"); shared_plugins(h, r#"{"a@x": true}"#, Some("[1]")); },
            vec![cli(&PREP)]), 0),
        "plugins_entry_not_dict" => d3(sc(|h| { casa(h); conta(h, "conta2");
            shared_plugins(h, r#"{"a@x": true}"#, Some(r#"{"plugins": {"a@x": ["texto"]}}"#)); }, vec![cli(&PREP)]), 0),
        "plugins_install_path_number" => d3(sc(|h| { casa(h); conta(h, "conta2");
            shared_plugins(h, r#"{"a@x": true}"#, Some(r#"{"plugins": {"a@x": [{"installPath": 5}]}}"#)); }, vec![cli(&PREP)]), 0),
        "plugins_install_path_shapes" => sc(|h| {
            casa(h);
            conta(h, "conta2");
            mkdirs(&h.join("rel-existe"));
            mkdirs(&h.join("abs"));
            let registry = json!({"plugins": {
                "a@x": [{"installPath": 0}, {"installPath": ""}, {}, {"installPath": null}, {"installPath": false}, {"installPath": []}, {"installPath": {}}],
                "b@x": [{"installPath": "rel-existe"}], "c@x": [{"installPath": "rel-falta"}],
                "d@x": [{"installPath": h.join("abs")}, {"installPath": h.join("sumiu")}], "e@x": []}});
            shared_plugins(h, r#"{"a@x": true, "b@x": true, "c@x": true, "d@x": true, "e@x": true}"#, Some(&registry.to_string()));
        }, vec![cli(&PREP)]),
        "plugins_enabled_shapes" => sc(|h| { casa(h); conta(h, "conta2"); shared_plugins(h, r#"["a@x"]"#, None); },
            vec![cli(&PREP), fs(|h| shared_plugins(h,
                r#"{"h@x": {}, "a@x": "false", "b@x": 0, "c@x": null, "d@x": "", "e@x": [], "f@x": 1, "g@x": ["x"], "i@x": 0.0, "j@x": 0.5}"#, None)),
                cli(&PREP)]),
        "plugins_list_truthy" => d3(sc(|h| { casa(h); conta(h, "conta2"); shared_plugins(h, r#"{"a@x": true}"#, Some(r#"{"plugins": ["x"]}"#)); },
            vec![cli(&PREP)]), 0),
        "plugins_list_falsy" => sc(|h| { casa(h); conta(h, "conta2"); shared_plugins(h, r#"{"a@x": true}"#, Some(r#"{"plugins": []}"#)); },
            vec![cli(&PREP), fs(|h| put(&h.join(".claude/plugins/installed_plugins.json"), r#"{"plugins": null, "x": 1}"#)), cli(&PREP)]),
        "plugins_seeded_not_object" => d3(sc(|h| { casa(h); conta(h, "conta2"); put(&h.join(".claude/settings.json"), "[1]"); },
            vec![cli(&PREP)]), 0),

        // ── atalhos, gaveta e memória ──
        "shared_dead_link" => sc(|h| { casa(h); conta(h, "conta2"); ln(&h.join(".claude/morto"), h.join("nada")); }, vec![cli(&PREP), cli(&PREP)]),
        "user_dead_link_pruned" => sc(|h| { casa(h); conta(h, "conta2"); put(&h.join("arquivo"), "x");
            ln(&h.join(".claude-conta2/meu-link"), h.join("nada")); ln(&h.join(".claude-conta2/vivo"), h.join("arquivo")); }, vec![cli(&PREP)]),
        "drift_slot_reuse" => sc(|h| { casa(h); conta(h, "conta2");
            put(&h.join(".claude-conta2/.drift/skills.1"), "1"); put(&h.join(".claude-conta2/.drift/skills.3"), "3");
            mkdirs(&h.join(".claude-conta2/skills")); }, vec![cli(&PREP)]),
        "drift_slot_dead_link_file" => sc(|h| { casa(h); conta(h, "conta2"); mkdirs(&h.join(".claude/hooks"));
            ln(&h.join(".claude-conta2/.drift/hooks.1"), h.join("nada")); put(&h.join(".claude-conta2/hooks"), "meu"); }, vec![cli(&PREP)]),
        "drift_slot_dead_link_dir" => with_path(d2(sc(|h| { casa(h); conta(h, "conta2");
            ln(&h.join(".claude-conta2/.drift/skills.1"), h.join("nada")); put(&h.join(".claude-conta2/skills/a.md"), "meu"); },
            vec![cli(&PREP)]), 0, 0, FAILED, ""), 0, 0, "<HOME>/.claude-conta2/.drift/skills.1"),
        "drift_unicode_digit" => sc(|h| {
            casa(h);
            conta(h, "conta2");
            let drift = h.join(".claude-conta2/.drift");
            for (n, t) in [("skills.1", 2_000_000_000), ("skills.2", 2_000_000_000), ("skills.²", 1_000)] {
                put(&drift.join(n), n);
                mtime(&drift.join(n), t);
            }
            put(&h.join(".claude-conta2/skills/novo.md"), "novo");
            mtime(&h.join(".claude-conta2/skills"), 2_100_000_000);
        }, vec![cli(&PREP)]),
        "drift_just_moved_pruned" => sc(|h| {
            casa(h);
            conta(h, "conta2");
            mkdirs(&h.join(".claude/hooks"));
            for n in 1..=3 {
                let p = h.join(format!(".claude-conta2/.drift/hooks.{n}"));
                put(&p, n.to_string());
                mtime(&p, 2_000_000_000);
            }
            put(&h.join(".claude-conta2/hooks"), "velho");
            mtime(&h.join(".claude-conta2/hooks"), 1_000_000_000);
        }, vec![cli(&PREP)]),
        "top_order" => sc(|h| {
            casa(h);
            conta(h, "conta2");
            for n in ["B.md", "Z.md", "_x.md", "a.md", "é.md"] {
                put(&h.join(".claude").join(n), "principal");
                put(&h.join(".claude-conta2").join(n), format!("conta {n}"));
            }
        }, vec![cli(&PREP)]),
        "fifo_goes_to_drift" => sc(|h| { casa(h); conta(h, "conta2"); fifo(&h.join(".claude-conta2/CLAUDE.md")); }, vec![cli(&PREP)]),
        "memory_cases" => {
            let mut s = sc(|h| {
                casa(h);
                conta(h, "conta2");
                mkdirs(&h.join(".claude/projects/-sem-memoria"));
                put(&h.join(".claude/projects/-link-errado/memory/MEMORY.md"), "le");
                put(&h.join(".claude/projects/-arquivo/memory/MEMORY.md"), "ar");
                mkdirs(&h.join("outro"));
                ln(&h.join(".claude-conta2/projects/-link-errado/memory"), h.join("outro"));
                put(&h.join(".claude-conta2/projects/-arquivo/memory"), "x");
                put(&h.join(".claude-conta2/projects/-removido/memory/MEMORY.md"), "fica");
            }, vec![cli(&PREP), cli(&PREP)]);
            s.rules.d6 = true;
            s
        }
        "memory_local_project_is_file" => with_path(d2(sc(|h| { casa(h); conta(h, "conta2"); put(&h.join(".claude-conta2/projects/-tmp-x"), "x"); },
            vec![cli(&PREP)]), 0, 0, FAILED, ""), 0, 0, "<HOME>/.claude-conta2/projects/-tmp-x"),
        "shared_pending_marker" => sc(|h| { casa(h); conta(h, "conta2"); put(&h.join(".claude/.hangar-account-pending"), ""); },
            vec![cli(&PREP), cli(&PREP), cli(&["--list"])]),
        "relative_link_kept" => sc(|h| { casa(h); conta(h, "conta2"); }, vec![cli(&PREP), fs(|h| {
            rm(&h.join(".claude-conta2/skills"));
            ln(&h.join(".claude-conta2/skills"), "../.claude/skills");
        }), cli(&PREP)]),
        "link_loop_drawer" => sc(|h| { casa(h); conta(h, "conta2"); ln(&h.join(".claude-conta2/CLAUDE.md"), "CLAUDE.md"); }, vec![cli(&PREP)]),
        // B1 (reproduzido até o conserto): runtime do CLI fora da lista vai para a gaveta ou sobe.
        "b1_runtime_not_listed" => sc(|h| {
            casa(h);
            conta(h, "conta2");
            mkdirs(&h.join(".claude/.claude.json.lock"));
            put(&h.join(".claude/mcp-needs-auth-cache.json"), "{}");
            mkdirs(&h.join(".claude-conta2/.claude.json.lock"));
            put(&h.join(".claude-conta2/mcp-needs-auth-cache.json"), r#"{"conta": 1}"#);
        }, vec![cli(&PREP)]),
        // B3 (mantido): a memória de todos os projetos divide o mesmo teto da gaveta.
        "b3_memory_shared_bucket" => {
            let mut s = sc(|h| {
                casa(h);
                conta(h, "conta2");
                for (i, p) in ["-p1", "-p2", "-p3", "-p4"].iter().enumerate() {
                    put(&h.join(format!(".claude/projects/{p}/memory/MEMORY.md")), *p);
                    let local = h.join(format!(".claude-conta2/projects/{p}/memory"));
                    put(&local.join("MEMORY.md"), format!("local {p}"));
                    mtime(&local, 1_900_000_000 + i as i64);
                }
            }, vec![cli(&PREP)]);
            s.rules.d6 = true;
            s
        }
        "prep_ignores_claude_config_dir" => Scenario { env: vec![("CLAUDE_CONFIG_DIR", "<HOME>/outra")], ..sc(|h| {
            casa(h);
            conta(h, "conta2");
            put(&h.join("outra/settings.json"), r#"{"theme":"outra"}"#);
            put(&h.join("outra/so-da-outra.md"), "x");
        }, vec![cli(&PREP)]) },
        "drift_prunes_link_entry" => sc(|h| {
            casa(h);
            conta(h, "conta2");
            mkdirs(&h.join(".claude/hooks"));
            let drift = h.join(".claude-conta2/.drift");
            // A entrada mais velha é um link vivo: a poda lê o mtime dele (lstat) e apaga o link, não o alvo.
            put(&h.join("alvo-do-link"), "fica");
            mtime(&h.join("alvo-do-link"), 2_100_000_000);
            ln(&drift.join("hooks.1"), h.join("alvo-do-link"));
            mtime(&drift.join("hooks.1"), 1_000);
            for n in 2..=4 {
                put(&drift.join(format!("hooks.{n}")), n.to_string());
                mtime(&drift.join(format!("hooks.{n}")), 2_000_000_000 + n);
            }
            put(&h.join(".claude-conta2/hooks"), "novo");
            mtime(&h.join(".claude-conta2/hooks"), 2_000_000_100);
        }, vec![cli(&PREP)]),
        "never_link_names" => sc(|h| {
            casa(h);
            conta(h, "conta2");
            for n in [".claude.json", ".hangar-conta", ".hangar-apelidos.json"] { put(&h.join(".claude").join(n), "{}"); }
            mkdirs(&h.join(".claude/.drift"));
        }, vec![cli(&PREP)]),

        // ── diferenças aceitas (o Rust é conferido contra o "depois" do decisoes.md) ──
        "new_fixed_dirs" => Scenario { env: vec![("CP_CLAUDE_CONFIG_DIRS", "<HOME>/.claude")], ..sc(casa, vec![cli(&NEW)]) },
        "list_pending_dead_link" => sc(|h| { conta(h, "a"); conta(h, "b"); ln(&h.join(".claude-b/.hangar-account-pending"), h.join("nada")); },
            vec![cli(&["--list"])]),
        "prep_pending_dead_link" => sc(|h| { casa(h); conta(h, "conta2"); ln(&h.join(".claude-conta2/.hangar-account-pending"), h.join("nada")); },
            vec![cli(&PREP)]),
        other => panic!("cenário desconhecido: {other}"),
    }
}

/// D12 e D13: o gerador grava o Python, e o teste confere o Rust contra o que foi aceito.
const ACCEPTED: [&str; 3] = ["new_fixed_dirs", "list_pending_dead_link", "prep_pending_dead_link"];

macro_rules! scenarios {
    ($($name:ident),* $(,)?) => {
        const NAMES: &[&str] = &[$(stringify!($name)),*];
        $( #[test] fn $name() { check(stringify!($name)); } )*
    };
}

scenarios!(
    new_complete, new_existing_dir, new_broken_symlink, new_invalid_names, new_without_shared, new_symlink_refused,
    new_claude_json_truncated, new_claude_json_not_object, new_claude_json_unreadable, new_apelidos, new_without_claude_json,
    new_seed_format, list_rules, list_empty, help_flags, usage_errors, prep_fresh_idempotent, prep_project_new,
    prep_project_invalid, prep_settings_copy, prep_mirror_principal_wins, prep_plugin_from_account, prep_mcp_mirror,
    prep_mirror_warns_changed_key, prep_absent_key_kept, prep_settings_old_symlink, prep_dir_appears_later,
    prep_runtime_stays_local, prep_runtime_links_elsewhere, prep_dead_link_pruned, prep_local_change_goes_up,
    prep_local_identical, prep_drift_cap, prep_not_an_account, prep_file_vs_dir_drawer, prep_unexpected_symlink,
    prep_drift_symlink, prep_drift_is_file, prep_root_symlink, prep_memory_local_drawer, prep_memory_project_removed,
    prep_tmp_name_user_file, prep_drift_cap_per_origin, prep_plugins_warnings, prep_no_plugins_silent, prep_invalid_name,
    prep_name_32_chars, prep_pending_account, prep_marker_symlink_or_dir, prep_shared_missing, prep_shared_is_file,
    prep_guard_busy, prep_symlink_refused, prep_account_readonly, settings_principal_invalid, settings_copy_invalid,
    settings_not_object, settings_bom, settings_principal_missing_copy_invalid, settings_principal_missing,
    settings_principal_corrupt_seeded, settings_dead_old_link, settings_link_elsewhere, settings_copy_is_dir,
    settings_copy_unreadable, settings_python_equality, settings_format, settings_duplicate_keys,
    settings_enabled_plugins_shapes, settings_lone_surrogate_equal, mcp_copy_missing, mcp_principal_invalid, mcp_not_object,
    mcp_principal_empty_or_not_dict, mcp_copy_not_dict, mcp_format, mcp_python_equality, mcp_lone_surrogate, mcp_bom,
    plugins_registry_unreadable, plugins_registry_invalid, plugins_registry_not_object, plugins_entry_not_dict,
    plugins_install_path_number, plugins_install_path_shapes, plugins_enabled_shapes, plugins_list_truthy,
    plugins_list_falsy, plugins_seeded_not_object, shared_dead_link, user_dead_link_pruned, drift_slot_reuse,
    drift_slot_dead_link_file, drift_slot_dead_link_dir, drift_unicode_digit, drift_just_moved_pruned, top_order,
    fifo_goes_to_drift, memory_cases, memory_local_project_is_file, shared_pending_marker, relative_link_kept,
    link_loop_drawer, never_link_names, b1_runtime_not_listed, b3_memory_shared_bucket, prep_ignores_claude_config_dir,
    drift_prunes_link_entry,
);

// ── execução ────────────────────────────────────────────────────────────────────────────────

enum Side<'a> { Python(&'a Path), Rust }

struct Run { steps: Vec<Value>, tree: Value, diary: Vec<Value>, raw_diary: Vec<Value>, initial: Value }

static TMP: std::sync::LazyLock<regex::Regex> =
    std::sync::LazyLock::new(|| regex::Regex::new(r"\.hangar-novo\.\d+\.[0-9a-f]{8}").unwrap());

fn norm(text: &str, home: &Path, py_root: Option<&Path>) -> String {
    let mut text = text.replace(home.to_str().unwrap(), "<HOME>");
    if let Some(root) = py_root { text = text.replace(root.to_str().unwrap(), "<PY_ROOT>"); }
    TMP.replace_all(&text, ".hangar-novo.<PID>.<HEX>").into_owned()
}

fn command(side: &Side, home: &Path, args: &[String], env: &[(&str, &str)]) -> Command {
    let mut cmd = match side {
        Side::Python(root) => {
            let mut cmd = Command::new("python3");
            cmd.arg(root.join("scripts/hangar-conta"));
            cmd
        }
        Side::Rust => {
            let mut cmd = Command::new(env!("CARGO_BIN_EXE_hangar-server"));
            cmd.arg("conta");
            cmd
        }
    };
    cmd.args(args).env_clear().current_dir(home).env("HOME", home).env("PATH", "/usr/bin:/bin")
        .env("LANG", "C.UTF-8").env("LC_ALL", "C.UTF-8").env("PYTHONDONTWRITEBYTECODE", "1");
    for (key, value) in env { cmd.env(key, value.replace("<HOME>", home.to_str().unwrap())); }
    // Os dois lados com o mesmo umask: o modo dos arquivos novos entra na comparação.
    unsafe { cmd.pre_exec(|| { libc::umask(0o022); Ok(()) }); }
    cmd
}

/// A guarda da conta presa em exclusivo por um `flock` à parte, até o comando terminar.
fn busy(home: &Path, account: &str) -> std::process::Child {
    let key = AccountKey::new(Provider::Claude, &home.join(format!(".claude-{account}"))).unwrap();
    let lock = home.join(".hangar/account-locks").join(format!("{}.lock", key.digest().unwrap()));
    mkdirs(lock.parent().unwrap());
    let child = Command::new("flock").arg("-x").arg(&lock).arg("sleep").arg("60").spawn().unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while Command::new("flock").arg("-n").arg("-s").arg(&lock).arg("true").status().unwrap().success() {
        assert!(std::time::Instant::now() < deadline, "o flock não pegou a guarda");
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    child
}

fn run(name: &str, side: &Side) -> Run { run_checked(name, side, &mut |_, _| {}) }

/// `run` com `seen` chamado a cada comando, antes do passo seguinte: a falha aponta o passo que divergiu.
fn run_checked(name: &str, side: &Side, seen: &mut dyn FnMut(usize, &Value)) -> Run {
    let scenario = scenario(name);
    let dir = tempfile::Builder::new().prefix("conta-rec-").tempdir().unwrap();
    let home = dir.path().canonicalize().unwrap();
    let py_root = match side { Side::Python(root) => Some(*root), Side::Rust => None };
    (scenario.build)(&home);
    let initial = tree(&home, py_root);
    let mut accounts = std::collections::BTreeSet::new();
    let mut steps = Vec::new();
    for step in &scenario.steps {
        let (args, held) = match step {
            Step::Fs(f) => { f(&home); continue; }
            Step::Cli(args) => (args, None),
            Step::Busy(args) => (args, Some(busy(&home, &args[1]))),
        };
        if args.len() >= 2 && matches!(args[0].as_str(), "--prep" | "--new") { accounts.insert(args[1].clone()); }
        let out = command(side, &home, args, &scenario.env).output().unwrap_or_else(|e| panic!("{name}: não rodou: {e}"));
        if let Some(mut held) = held { held.kill().unwrap(); held.wait().unwrap(); }
        let stderr: Vec<String> = String::from_utf8_lossy(&out.stderr).lines().map(|l| norm(l, &home, py_root)).collect();
        steps.push(json!({"args": args.iter().map(|a| norm(a, &home, py_root)).collect::<Vec<_>>(), "exit": out.status.code(),
            "stdout": norm(&String::from_utf8_lossy(&out.stdout), &home, py_root), "stderr": stderr}));
        seen(steps.len() - 1, steps.last().unwrap());
    }
    let tree = tree(&home, py_root);
    let (diary, raw_diary) = diary(&home, &accounts);
    unlock(&home);
    Run { steps, tree, diary, raw_diary, initial }
}

/// O estado de cada caminho do HOME, menos `~/.hangar` (diário e guardas): tipo, alvo do link, bytes e
/// modo do arquivo. mtime fica fora.
fn tree(home: &Path, py_root: Option<&Path>) -> Value {
    fn walk(home: &Path, dir: &Path, py_root: Option<&Path>, out: &mut BTreeMap<String, Value>) {
        let Ok(entries) = std::fs::read_dir(dir) else { return };
        for entry in entries.flatten() {
            let path = entry.path();
            if path == home.join(".hangar") { continue; }
            let key = norm(&path.to_string_lossy(), home, py_root);
            let meta = std::fs::symlink_metadata(&path).unwrap();
            let value = if meta.file_type().is_symlink() {
                json!({"kind": "symlink", "target": norm(&std::fs::read_link(&path).unwrap().to_string_lossy(), home, py_root)})
            } else if meta.is_dir() {
                walk(home, &path, py_root, out);
                if std::fs::read_dir(&path).is_ok() { json!({"kind": "dir"}) } else { json!({"kind": "dir", "unreadable": true}) }
            } else if meta.is_file() {
                let mode = format!("{:04o}", meta.permissions().mode() & 0o7777);
                match std::fs::read(&path) {
                    Ok(bytes) => match String::from_utf8(bytes) {
                        Ok(text) => json!({"kind": "file", "mode": mode, "text": norm(&text, home, py_root)}),
                        Err(e) => json!({"kind": "file", "mode": mode, "hex": e.as_bytes().iter().map(|b| format!("{b:02x}")).collect::<String>()}),
                    },
                    Err(_) => json!({"kind": "file", "mode": mode, "unreadable": true}),
                }
            } else {
                json!({"kind": "other"})
            };
            out.insert(key, value);
        }
    }
    let mut out = BTreeMap::new();
    walk(home, home, py_root, &mut out);
    serde_json::to_value(out).unwrap()
}

/// Devolve as permissões que os cenários tiraram, para a pasta temporária sair inteira.
fn unlock(home: &Path) {
    for entry in walkdir(home) {
        let Ok(meta) = std::fs::symlink_metadata(&entry) else { continue };
        if meta.file_type().is_symlink() { continue; }
        let _ = std::fs::set_permissions(&entry, std::fs::Permissions::from_mode(if meta.is_dir() { 0o755 } else { 0o644 }));
    }
}
fn walkdir(root: &Path) -> Vec<PathBuf> {
    let _ = std::fs::set_permissions(root, std::fs::Permissions::from_mode(0o755));
    let mut out = vec![root.to_owned()];
    let mut i = 0;
    while i < out.len() {
        let dir = out[i].clone();
        i += 1;
        if std::fs::symlink_metadata(&dir).is_ok_and(|m| m.is_dir()) {
            let _ = std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755));
            for entry in std::fs::read_dir(&dir).into_iter().flatten().flatten() { out.push(entry.path()); }
        }
    }
    out
}

fn conta_id(home: &Path, account: &str) -> String {
    let path = home.join(format!(".claude-{account}"));
    let digest = ring::digest::digest(&ring::digest::SHA256, path.to_str().unwrap().as_bytes());
    digest.as_ref().iter().map(|b| format!("{b:02x}")).collect::<String>()[..16].to_owned()
}

/// Os eventos `conta.reconciliar.*` de todo `uso-*.jsonl` sob o diário, com os campos do contrato.
fn diary(home: &Path, accounts: &std::collections::BTreeSet<String>) -> (Vec<Value>, Vec<Value>) {
    let mut files: Vec<PathBuf> = walkdir(&home.join(".hangar/logs/diario")).into_iter()
        .filter(|p| p.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.starts_with("uso-") && n.ends_with(".jsonl"))).collect();
    files.sort();
    let ids: BTreeMap<String, String> = accounts.iter().map(|a| (conta_id(home, a), "<CONTA_ID>".to_owned())).collect();
    let (mut kept, mut raw) = (vec![], vec![]);
    for file in files {
        for line in std::fs::read_to_string(&file).unwrap().lines().filter(|l| !l.trim().is_empty()) {
            let event: Value = serde_json::from_str(line).unwrap();
            if !event["evento"].as_str().is_some_and(|e| e.starts_with("conta.reconciliar")) { continue; }
            let mut fields = Map::new();
            for key in ["evento", "nivel", "provider", "etapa", "conta_id", "codigo", "quantidade"] {
                if let Some(value) = event.get(key) {
                    let value = match (key, value.as_str()) {
                        ("conta_id", Some(id)) => json!(ids.get(id).cloned().unwrap_or_else(|| id.to_owned())),
                        _ => value.clone(),
                    };
                    fields.insert(key.into(), value);
                }
            }
            kept.push(Value::Object(fields));
            raw.push(event);
        }
    }
    (kept, raw)
}

fn py_root() -> PathBuf {
    std::env::var_os("HANGAR_FIXTURE_PY_ROOT").map(PathBuf::from)
        .unwrap_or_else(|| Path::new(env!("CARGO_MANIFEST_DIR")).join("../..").canonicalize().unwrap())
}

/// Gera as fixtures rodando o `scripts/hangar-conta` do commit de origem. Sem `python3`, falha.
#[test]
#[ignore = "gerador das fixtures: roda o Python de origem"]
fn generate_fixtures() {
    let root = py_root();
    let python = Command::new("python3").arg("--version").output();
    assert!(python.is_ok_and(|o| o.status.success()), "python3 ausente: o gerador precisa do Python de origem");
    let files = ["backend/app/contas.py", "backend/app/atomico.py", "backend/app/diag.py", "backend/app/log_paths.py", "scripts/hangar-conta"];
    let same = Command::new("git").arg("-C").arg(&root).args(["diff", "--quiet", ORIGIN, "--"]).args(files).status().unwrap();
    assert!(same.success(), "{} não está no commit de origem {ORIGIN} para {files:?}", root.display());
    std::fs::create_dir_all(fixture_dir()).unwrap();
    // `HANGAR_FIXTURE_ONLY=a,b`: só esses cenários; os outros ficam como foram gravados.
    let only = std::env::var("HANGAR_FIXTURE_ONLY").ok();
    let wanted = |name: &str| only.as_deref().is_none_or(|list| list.split(',').any(|n| n == name));
    for name in NAMES.iter().chain(ACCEPTED.iter()).filter(|n| wanted(n)) {
        let out = run(name, &Side::Python(&root));
        for event in &out.diary {
            if let Some(id) = event["conta_id"].as_str() { assert_eq!(id, "<CONTA_ID>", "{name}: conta_id fora de sha256(caminho)[:16]"); }
        }
        let fixture = json!({"origin_commit": ORIGIN, "command": COMMAND, "scenario": name, "steps": out.steps,
            "tree": out.tree, "diary": out.diary});
        std::fs::write(fixture_dir().join(format!("{name}.json")), serde_json::to_string_pretty(&fixture).unwrap() + "\n").unwrap();
    }
}

// ── comparação ──────────────────────────────────────────────────────────────────────────────

fn fixture(name: &str) -> Value {
    let path = fixture_dir().join(format!("{name}.json"));
    let value: Value = serde_json::from_slice(&std::fs::read(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))).unwrap();
    assert_eq!(value["origin_commit"], ORIGIN, "{name}");
    value
}

/// D6: `.drift/memory.N` renumeradas pela ordem do conteúdo, igual nos dois lados.
fn d6_tree(tree: &Value) -> Value {
    let map = tree.as_object().unwrap();
    let slot = regex::Regex::new(r"^(<HOME>/[^/]+/\.drift/memory\.)(\d+)(/.*)?$").unwrap();
    let mut groups: BTreeMap<String, BTreeMap<String, Value>> = BTreeMap::new();
    for (key, value) in map {
        if let Some(c) = slot.captures(key) {
            groups.entry(c[2].to_owned()).or_default().insert(c.get(3).map_or("", |m| m.as_str()).to_owned(), value.clone());
        }
    }
    let mut order: Vec<(String, String)> = groups.iter().map(|(n, g)| (serde_json::to_string(g).unwrap(), n.clone())).collect();
    order.sort();
    let rank: BTreeMap<String, usize> = order.iter().enumerate().map(|(i, (_, n))| (n.clone(), i + 1)).collect();
    let mut out = Map::new();
    for (key, value) in map {
        let key = match slot.captures(key) {
            Some(c) => format!("{}{}{}", &c[1], rank[&c[2]], c.get(3).map_or("", |m| m.as_str())),
            None => key.clone(),
        };
        out.insert(key, value.clone());
    }
    Value::Object(out)
}

fn compare_step(name: &str, i: usize, want: &Value, got: &Value, rules: &Rules) {
    assert_eq!(want["args"], got["args"], "{name}[{i}]");
    assert_eq!(want["exit"], got["exit"], "{name}[{i}] {}: código de saída; stderr {}", want["args"], got["stderr"]);
    assert_eq!(want["stdout"], got["stdout"], "{name}[{i}] {}: stdout", want["args"]);
    let mut want_err: Vec<String> = serde_json::from_value(want["stderr"].clone()).unwrap();
    let mut got_err: Vec<String> = serde_json::from_value(got["stderr"].clone()).unwrap();
    if rules.d3.contains(&i) {
        assert!(got_err.len() == 1 && got_err[0].starts_with("hangar-conta: ") && !got_err[0].starts_with(FAILED),
            "{name}[{i}] D3: uma linha `hangar-conta:` com a frase do Rust: {got_err:?}");
        return;
    }
    for (step, line, prefix, suffix) in &rules.d2 {
        if *step != i { continue; }
        for (side, lines) in [("Python", &mut want_err), ("Rust", &mut got_err)] {
            let text = lines.get(*line).unwrap_or_else(|| panic!("{name}[{i}] D2: {side} sem a linha {line}: {lines:?}"));
            assert!(text.starts_with(prefix) && text.ends_with(suffix) && text.len() > prefix.len() + suffix.len(),
                "{name}[{i}] D2: {side} fora do texto fixo {prefix:?}…{suffix:?}: {text:?}");
            let detail = &text[prefix.len()..text.len() - suffix.len()];
            for (_, _, path) in rules.path.iter().filter(|(s, l, _)| *s == i && l == line) {
                assert!(detail.contains(path), "{name}[{i}] {side}: o erro do sistema perdeu o caminho {path:?}: {detail:?}");
            }
            lines[*line] = format!("{prefix}<ERRO>{suffix}");
        }
    }
    if rules.d6 {
        want_err.sort();
        got_err.sort();
    }
    assert_eq!(want_err, got_err, "{name}[{i}] {}: stderr", want["args"]);
}

fn check(name: &str) {
    let expected = fixture(name);
    let rules = scenario(name).rules;
    let want_steps = expected["steps"].as_array().unwrap();
    let got = run_checked(name, &Side::Rust, &mut |i, got| {
        let want = want_steps.get(i).unwrap_or_else(|| panic!("{name}: passo {i} a mais"));
        compare_step(name, i, want, got, &rules);
    });
    assert_eq!(want_steps.len(), got.steps.len(), "{name}: passos");
    let (want_tree, got_tree) = if rules.d6 { (d6_tree(&expected["tree"]), d6_tree(&got.tree)) } else { (expected["tree"].clone(), got.tree.clone()) };
    assert_tree(name, &want_tree, &got_tree);
    assert_eq!(expected["diary"], json!(got.diary), "{name}: diário");
    one_operation_per_run(name, &got.raw_diary);
}

fn assert_tree(name: &str, want: &Value, got: &Value) {
    let (want, got) = (want.as_object().unwrap(), got.as_object().unwrap());
    let mut diff = vec![];
    for key in want.keys().chain(got.keys()).collect::<std::collections::BTreeSet<_>>() {
        if want.get(key) != got.get(key) { diff.push(format!("{key}: Python {:?} / Rust {:?}", want.get(key), got.get(key))); }
    }
    assert!(diff.is_empty(), "{name}: árvore difere\n{}", diff.join("\n"));
}

/// Todos os eventos de uma reconciliação levam a mesma `operacao`, e o `.fim` leva `ms`.
fn one_operation_per_run(name: &str, events: &[Value]) {
    let mut current: Option<&str> = None;
    for event in events {
        let evento = event["evento"].as_str().unwrap();
        let op = event["operacao"].as_str().unwrap_or_else(|| panic!("{name}: {evento} sem operacao"));
        if evento == "conta.reconciliar.inicio" { current = Some(op); continue; }
        assert_eq!(Some(op), current, "{name}: {evento} fora da operação do início");
        if evento == "conta.reconciliar.fim" {
            assert!(event["ms"].is_u64(), "{name}: .fim sem ms");
            current = None;
        }
    }
}

// ── diferenças aceitas ──────────────────────────────────────────────────────────────────────

/// D12: com `CP_CLAUDE_CONFIG_DIRS` setado a criação recusa como a do app; o Python criava.
#[test]
fn new_fixed_dirs() {
    let python = fixture("new_fixed_dirs");
    assert_eq!(python["steps"][0]["exit"], 0, "o Python criava a conta");
    let got = run("new_fixed_dirs", &Side::Rust);
    assert_eq!(got.steps[0]["exit"], 1);
    assert_eq!(got.steps[0]["stderr"], json!(["hangar-conta: CP_CLAUDE_CONFIG_DIRS está setado: a lista de contas é fixa por ambiente. \
        Remova a variável ou acrescente a conta nela."]));
    assert_eq!(got.steps[0]["stdout"], "");
    assert_tree("new_fixed_dirs", &got.initial, &got.tree);
}

/// D13: pendência que é link morto também tira a pasta da lista (a abertura do Rust já fazia assim).
#[test]
fn list_pending_dead_link() {
    let python = fixture("list_pending_dead_link");
    assert_eq!(python["steps"][0]["stdout"], "a\nb\n", "o Python listava a pendente");
    let got = run("list_pending_dead_link", &Side::Rust);
    assert_eq!((got.steps[0]["exit"].clone(), got.steps[0]["stdout"].clone()), (json!(0), json!("a\n")));
}

/// D13: e a reconciliação recusa como conta inexistente, sem mexer em nada.
#[test]
fn prep_pending_dead_link() {
    let python = fixture("prep_pending_dead_link");
    assert_eq!(python["steps"][0]["exit"], 0, "o Python reconciliava a pendente");
    let got = run("prep_pending_dead_link", &Side::Rust);
    assert_eq!(got.steps[0]["exit"], 1);
    assert_eq!(got.steps[0]["stderr"], json!(["hangar-conta: <HOME>/.claude-conta2 não é uma conta criada pelo hangar"]));
    assert_tree("prep_pending_dead_link", &got.initial, &got.tree);
    assert!(got.diary.is_empty());
}

// ── o núcleo pela biblioteca ────────────────────────────────────────────────────────────────

use hangar_server::accounts::claude_config::{self, ConfigError};
use hangar_server::accounts::environment::AccountEnvironment;

fn env_of(home: &Path) -> AccountEnvironment {
    AccountEnvironment::from_map([("HOME".to_owned(), home.to_string_lossy().into_owned())].into())
}

/// T33: duas contas com versões diferentes do mesmo arquivo ao mesmo tempo. A trava compartilhada
/// serializa: o `~/.claude` fica com uma das versões inteira, as duas religadas e as duas avisam.
#[test]
fn two_accounts_reconciling_together_keep_one_whole_version() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().canonicalize().unwrap();
    casa(&home);
    let versions = [("conta_a", "# regras da a"), ("conta_b", "# regras da b")];
    for (name, text) in versions {
        conta(&home, name);
        put(&home.join(format!(".claude-{name}/CLAUDE.md")), text);
    }
    // Com a trava das contas presa por outro dono, a conta_b espera sem tocar no `~/.claude`.
    let holder = std::fs::OpenOptions::new().read(true).append(true).create(true).open(home.join(".hangar-contas.lock")).unwrap();
    holder.lock().unwrap();
    std::thread::scope(|s| {
        let waiting = s.spawn(|| claude_config::reconcile_account(&home, "conta_b", None).result);
        std::thread::sleep(std::time::Duration::from_millis(300));
        assert_eq!(std::fs::read_to_string(home.join(".claude/CLAUDE.md")).unwrap(), "# regras", "a conta_b subiu o arquivo sem a trava");
        assert!(!std::fs::symlink_metadata(home.join(".claude-conta_b/CLAUDE.md")).unwrap().file_type().is_symlink());
        holder.unlock().unwrap();
        assert!(waiting.join().unwrap().is_ok());
    });
    put(&home.join(".claude/CLAUDE.md"), "# regras");
    let link = home.join(".claude-conta_b/CLAUDE.md");
    std::fs::remove_file(&link).unwrap();
    put(&link, "# regras da b");
    let barrier = std::sync::Barrier::new(2);
    let warnings: Vec<Vec<String>> = std::thread::scope(|s| {
        let runs: Vec<_> = versions.iter().map(|(name, _)| s.spawn(|| {
            barrier.wait();
            claude_config::reconcile_account(&home, name, None).result.unwrap()
        })).collect();
        runs.into_iter().map(|r| r.join().unwrap()).collect()
    });
    let shared = std::fs::read_to_string(home.join(".claude/CLAUDE.md")).unwrap();
    assert!(versions.iter().any(|(_, text)| *text == shared), "{shared:?}");
    for (name, _) in versions {
        assert!(std::fs::symlink_metadata(home.join(format!(".claude-{name}/CLAUDE.md"))).unwrap().file_type().is_symlink());
    }
    for list in warnings { assert!(list.iter().any(|w| w.contains("CLAUDE.md")), "{list:?}"); }
}

/// A mesma conta reconciliada duas vezes ao mesmo tempo: as travas serializam (sem elas, as duas mandam
/// a mesma pasta local para a gaveta e a segunda falha no meio) e as duas terminam sem erro.
#[test]
fn the_same_account_reconciling_twice_at_once_never_fails() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().canonicalize().unwrap();
    casa(&home);
    conta(&home, "conta2");
    for i in 0..30 {
        // Uma pasta local no lugar do link: as duas querem tirá-la para a gaveta ao mesmo tempo.
        let skills = home.join(".claude-conta2/skills");
        if std::fs::symlink_metadata(&skills).is_ok() { rm(&skills); }
        put(&skills.join("v.md"), i.to_string());
        let barrier = std::sync::Barrier::new(2);
        let results: Vec<_> = std::thread::scope(|s| {
            let runs: Vec<_> = (0..2).map(|_| s.spawn(|| {
                barrier.wait();
                claude_config::reconcile_account(&home, "conta2", Some("-p")).result
            })).collect();
            runs.into_iter().map(|r| r.join().unwrap()).collect()
        });
        for result in results { assert!(result.is_ok(), "{result:?}"); }
    }
    assert_eq!(std::fs::read_link(home.join(".claude-conta2/skills")).unwrap(), home.join(".claude/skills"));
}

/// M3: trava presa por outro dono recusa no teto, com o caminho dela, sem entrar na reconciliação.
#[test]
fn a_held_configuration_lock_refuses_at_the_deadline() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().canonicalize().unwrap();
    casa(&home);
    conta(&home, "conta2");
    let account = home.join(".claude-conta2");
    for (path, create) in [(home.join(".hangar-contas.lock"), true), (account.join(".hangar-conta"), false)] {
        let holder = std::fs::OpenOptions::new().read(true).write(true).create(create).truncate(false).open(&path).unwrap();
        holder.lock().unwrap();
        let started = std::time::Instant::now();
        let outcome = claude_config::reconcile_locked(&home, &account, None, ".hangar-conta", std::time::Duration::from_millis(300));
        let waited = started.elapsed();
        match outcome.result {
            Err(ConfigError::Refused(detail)) => assert_eq!(detail,
                format!("a trava {} ficou presa por outro processo por mais de 0 s", path.display())),
            other => panic!("{}: esperava a recusa no teto: {other:?}", path.display()),
        }
        assert!(waited >= std::time::Duration::from_millis(300), "desistiu antes do teto: {waited:?}");
        assert!(outcome.diary.is_empty(), "nada da reconciliação rodou");
        assert!(!account.join("skills").exists());
        holder.unlock().unwrap();
    }
}

fn json_value() -> impl proptest::strategy::Strategy<Value = Value> {
    use proptest::prelude::*;
    let leaf = prop_oneof![
        Just(Value::Null), any::<bool>().prop_map(Value::from), any::<i64>().prop_map(Value::from), any::<u64>().prop_map(Value::from),
        any::<f64>().prop_filter("finito", |f| f.is_finite()).prop_map(Value::from),
        "[a-z@./é\\\\\"\u{0}-\u{1f}\u{7f}\u{2028}]{0,6}".prop_map(Value::from),
    ];
    leaf.prop_recursive(3, 24, 4, |inner| prop_oneof![
        proptest::collection::vec(inner.clone(), 0..4).prop_map(Value::from),
        proptest::collection::btree_map("[a-z@_]{0,4}|enabledPlugins|mcpServers|plugins|installPath|a@x", inner, 0..4)
            .prop_map(|m| Value::Object(m.into_iter().collect())),
    ])
}

/// Texto UTF-8 que não é JSON válido para o serde: truncado, com BOM, os literais que só o Python aceita,
/// surrogate solto ou texto qualquer.
fn broken_text() -> impl proptest::strategy::Strategy<Value = String> {
    use proptest::prelude::*;
    prop_oneof![
        (json_value(), any::<prop::sample::Index>()).prop_map(|(v, cut)| {
            let text = v.to_string();
            let chars: Vec<char> = text.chars().collect();
            chars[..cut.index(chars.len() + 1).min(chars.len())].iter().collect()
        }),
        json_value().prop_map(|v| format!("\u{feff}{v}")),
        prop_oneof![Just("NaN"), Just("[Infinity]"), Just("[1e400]"), Just("{bad"), Just(""), Just(r#"{"x": "\ud800"}"#),
            Just(r#"{"enabledPlugins": {"a@x": NaN}}"#)].prop_map(str::to_owned),
        "\\PC{0,16}",
    ]
}

/// Conteúdo de um arquivo de entrada: ausente, bytes quaisquer, texto quebrado ou JSON.
fn input() -> impl proptest::strategy::Strategy<Value = Option<Vec<u8>>> {
    use proptest::prelude::*;
    prop_oneof![
        Just(None),
        proptest::collection::vec(any::<u8>(), 0..48).prop_map(Some),
        broken_text().prop_map(|t| Some(t.into_bytes())),
        json_value().prop_map(|v| Some(v.to_string().into_bytes())),
        (json_value(), json_value()).prop_map(|(on, entries)| Some(json!({"enabledPlugins": {"a@x": on, "b@x": true}, "plugins": {"a@x": entries,
            "b@x": [{"installPath": entries}]}, "mcpServers": {"s": on}}).to_string().into_bytes())),
    ]
}

proptest::proptest! {
    #![proptest_config(proptest::prelude::ProptestConfig::with_cases(96))]

    /// `settings.json` dos dois lados, os dois `.claude.json` e o registro de plugins: qualquer
    /// conteúdo vira aviso, erro ou sucesso, nunca pânico.
    #[test]
    fn external_files_never_panic(files in proptest::collection::vec(input(), 5)) {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().canonicalize().unwrap();
        casa(&home);
        conta(&home, "conta2");
        let account = home.join(".claude-conta2");
        let targets = [home.join(".claude/settings.json"), account.join("settings.json"), home.join(".claude.json"),
            account.join(".claude.json"), home.join(".claude/plugins/installed_plugins.json")];
        for (path, bytes) in targets.iter().zip(&files) {
            let _ = std::fs::remove_file(path);
            if let Some(bytes) = bytes { put(path, bytes); }
        }
        let _ = claude_config::reconcile(&home, &account, Some("-p"));
        let _ = claude_config::reconcile(&home, &account, None);
    }

    /// Qualquer argumento sai com um dos códigos do contrato.
    #[test]
    fn any_arguments_end_with_a_contract_code(args in proptest::collection::vec("--prep|--new|--list|--help|-h|conta2|[a-z0-9_/.\\- \n]{0,6}", 0..4)) {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().canonicalize().unwrap();
        casa(&home);
        conta(&home, "conta2");
        let (mut out, mut err) = (Vec::new(), Vec::new());
        let code = claude_config::run(&args, &env_of(&home), &mut out, &mut err);
        proptest::prop_assert!([0, 1, 2, 126].contains(&code), "{args:?} → {code}");
    }
}
