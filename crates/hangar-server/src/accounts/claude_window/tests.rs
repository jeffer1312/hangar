use super::*;
use std::sync::{Arc, Mutex};
use std::time::Duration;

const PSMUX_SCREEN: &str = include_str!("../fixtures/claude-login-psmux-120.txt");
const OP: &str = "0123456789abcdef0123456789abcdef";

#[derive(Default)]
struct World {
    /// Sessão → opções (`@cp_hidden`, `@cp_login_owner`).
    sessions: HashMap<String, HashMap<String, String>>,
    screen: String,
    width: String,
    /// Subcomandos que respondem falha.
    fail: HashSet<&'static str>,
    /// `kill-session` responde 0 e não derruba (o psmux faz isso).
    kill_lies: bool,
    calls: Vec<(Vec<String>, Vec<String>, Vec<u8>)>,
}

#[derive(Clone, Default)]
struct Fake(Arc<Mutex<World>>);

fn out(code: i32, stdout: &str) -> MuxOutput { MuxOutput { code: Some(code), stdout: stdout.into(), stderr: String::new() } }

fn session_of(args: &[String]) -> String {
    let at = args.iter().position(|a| a == "-t" || a == "-s").unwrap();
    args[at + 1].trim_start_matches('=').trim_end_matches(':').to_owned()
}

impl WindowMux for Fake {
    fn scope(&self) -> BoxFuture<'_, Vec<String>> { Box::pin(async { vec!["systemd-run".into(), "--".into()] }) }
    fn run(&self, scope: Vec<String>, args: Vec<String>, stdin: Vec<u8>) -> BoxFuture<'_, MuxOutput> {
        let mut world = self.0.lock().unwrap();
        world.calls.push((scope, args.clone(), stdin));
        let command = args[0].as_str();
        if world.fail.contains(command) { return Box::pin(async { out(1, "") }); }
        let reply = match command {
            "new-session" => { world.sessions.insert(session_of(&args), HashMap::new()); out(0, "") }
            "set-option" => {
                let name = session_of(&args);
                world.sessions.get_mut(&name).unwrap().insert(args[3].clone(), args[4].clone());
                out(0, "")
            }
            "show-options" => match world.sessions.get(&session_of(&args)).and_then(|o| o.get(&args[4])) {
                Some(value) => out(0, &format!("{value}\n")),
                None => out(1, ""),
            },
            "has-session" => out(if world.sessions.contains_key(&session_of(&args)) { 0 } else { 1 }, ""),
            "kill-session" => {
                if !world.kill_lies { world.sessions.remove(&session_of(&args)); }
                out(0, "")
            }
            "list-sessions" => out(0, &world.sessions.keys().map(|n| format!("{n}\n")).collect::<String>()),
            "display-message" => out(0, &format!("{}\n", world.width)),
            "capture-pane" => out(0, &world.screen.clone()),
            _ => out(0, ""),
        };
        Box::pin(async move { reply })
    }
}

impl Fake {
    fn calls(&self) -> Vec<(Vec<String>, Vec<String>, Vec<u8>)> { self.0.lock().unwrap().calls.clone() }
    fn commands(&self) -> Vec<Vec<String>> { self.calls().into_iter().map(|(_, args, _)| args).collect() }
    fn alive(&self, name: &str) -> bool { self.0.lock().unwrap().sessions.contains_key(name) }
}

fn terminal(windows: bool, root: &Path) -> TerminalEnv {
    TerminalEnv { windows, program: "tmux".into(), display: Vec::new(), server_config_dir: None,
        default_config_dir: root.join(".claude"), birth: Duration::ZERO }
}

fn setup(windows: bool, home: &str) -> (tempfile::TempDir, Fake, ClaudeWindows, AccountKey) {
    let root = tempfile::tempdir().unwrap();
    let account = root.path().join(home);
    std::fs::create_dir_all(&account).unwrap();
    let fake = Fake::default();
    let windows = ClaudeWindows::new(Box::new(fake.clone()), terminal(windows, root.path()), Some("true".into()), None);
    let key = AccountKey::new(super::super::Provider::Claude, &account).unwrap();
    (root, fake, windows, key)
}

fn name(op: &str) -> String { format!("term-login-{op}") }

#[test]
fn link_cut_at_the_psmux_margin_comes_out_whole() {
    let whole = oauth_url(&join_wrapped(PSMUX_SCREEN, Some(120))).unwrap();
    assert!(whole.starts_with("https://claude.com/cai/oauth/authorize?code=true&client_id=9d1c250a-e61b-44d9-88ed-5944d1962f5e&"));
    assert!(whole.contains("&redirect_uri=https%3A%2F%2Fplatform.claude.com%2Foauth%2Fcode%2Fcallback&"));
    assert!(whole.ends_with("&state=ynFY1ge6jDyzgfhaWwsODCawzBIo2pqw-a7keufkDu0"), "{whole}");
}

#[test]
fn line_shorter_than_the_pane_is_not_glued_to_the_next() {
    let screen = "https://claude.ai/oauth/authorize?fixture=1\nPaste code here if prompted";
    assert_eq!(oauth_url(&join_wrapped(screen, Some(80))).as_deref(), Some("https://claude.ai/oauth/authorize?fixture=1"));
    assert_eq!(oauth_url(&join_wrapped(screen, None)).as_deref(), Some("https://claude.ai/oauth/authorize?fixture=1"));
}

#[test]
fn link_ending_exactly_at_the_margin_is_not_glued_to_the_prompt() {
    let screen = "https://a.example/x?state=1234\nPaste code here if prompted >";
    let width = "https://a.example/x?state=1234".chars().count();
    assert_eq!(oauth_url(&join_wrapped(screen, Some(width))).as_deref(), Some("https://a.example/x?state=1234"));
    assert!(join_wrapped(screen, Some(width)).contains("\nPaste code here"));
}

#[test]
fn link_stops_at_the_osc8_escape() {
    let screen = "visit: \u{1b}]8;;https://claude.ai/oauth/authorize?a=1\u{1b}\\https://claude.ai/oauth/authorize?a=1\u{1b}]8;;\u{1b}\\";
    assert_eq!(oauth_url(screen).as_deref(), Some("https://claude.ai/oauth/authorize?a=1"));
}

#[tokio::test]
async fn read_returns_the_whole_link_from_the_psmux_window() {
    let (_root, fake, windows, key) = setup(true, "work");
    windows.open(&key, OP).await.unwrap();
    { let mut world = fake.0.lock().unwrap(); world.screen = PSMUX_SCREEN.into(); world.width = "120".into(); }
    let url = windows.read(&key, OP).await.unwrap().unwrap();
    assert!(url.contains("&redirect_uri="), "{url}");
}

#[tokio::test]
async fn login_window_on_tmux_is_scoped_hidden_owned_and_typed() {
    let (root, fake, windows, key) = setup(false, "work");
    windows.open(&key, OP).await.unwrap();
    let calls = fake.calls();
    let home = key.canonical_home.to_string_lossy().into_owned();
    let (scope, create, _) = calls.iter().find(|(_, args, _)| args[0] == "new-session").unwrap();
    assert_eq!(scope, &vec!["systemd-run".to_owned(), "--".to_owned()]);
    assert_eq!(create, &["new-session", "-d", "-s", &name(OP), "-c", &home, "-x", "1000", "-y", "50",
        "-e", &format!("CLAUDE_CONFIG_DIR={home}"), "-e", "BROWSER=true"]);
    let options = fake.0.lock().unwrap().sessions[&name(OP)].clone();
    assert_eq!(options["@cp_hidden"], "1");
    assert_eq!(options["@cp_login_owner"], root.path().join(".claude").to_string_lossy());
    let commands = fake.commands();
    let pane = format!("={}:", name(OP));
    assert!(commands.contains(&["send-keys", "-t", &pane, "-l", "--", "claude auth login --claudeai"].map(str::to_owned).to_vec()));
    assert_eq!(commands.last().unwrap(), &["send-keys", "-t", &pane, "-l", "--", "\r"].map(str::to_owned).to_vec());
}

#[tokio::test]
async fn login_with_a_target_points_browser_at_the_script_even_with_a_space_in_the_path() {
    let (_root, fake, windows, key) = setup(false, "work");
    let script = std::path::Path::new("/tmp/a b/claude-login-browser-x");
    windows.open_with_browser(&key, OP, script).await.unwrap();
    let calls = fake.calls();
    let (_, create, _) = calls.iter().find(|(_, args, _)| args[0] == "new-session").unwrap();
    assert!(create.contains(&"BROWSER=/tmp/a b/claude-login-browser-x".to_owned()), "{create:?}");
    assert!(!create.contains(&"BROWSER=true".to_owned()));
}

#[tokio::test]
async fn default_account_on_tmux_still_goes_explicit() {
    let (_root, fake, windows, _) = setup(false, ".claude");
    let key = AccountKey::new(super::super::Provider::Claude, &_root.path().join(".claude")).unwrap();
    windows.open(&key, OP).await.unwrap();
    let create = fake.commands().into_iter().find(|args| args[0] == "new-session").unwrap();
    assert!(create.iter().any(|arg| arg.starts_with("CLAUDE_CONFIG_DIR=")));
}

#[tokio::test]
async fn default_account_on_psmux_goes_without_the_variable_and_named_enter() {
    let (root, fake, _, _) = setup(true, ".claude");
    let key = AccountKey::new(super::super::Provider::Claude, &root.path().join(".claude")).unwrap();
    // A pasta temporária do CI do Windows vem com nome curto (`RUNNER~1`); a conta padrão é a canônica.
    let env = TerminalEnv { default_config_dir: key.canonical_home.clone(), ..terminal(true, root.path()) };
    let windows = ClaudeWindows::new(Box::new(fake.clone()), env, Some("true".into()), None);
    windows.open(&key, OP).await.unwrap();
    let calls = fake.calls();
    let (scope, create, _) = calls.iter().find(|(_, args, _)| args[0] == "new-session").unwrap();
    assert!(scope.is_empty(), "o psmux não usa o escopo do systemd");
    assert!(!create.iter().any(|arg| arg.starts_with("CLAUDE_CONFIG_DIR=")), "{create:?}");
    assert_eq!(fake.commands().last().unwrap().last().unwrap(), "Enter");
}

#[tokio::test]
async fn open_is_idempotent_and_a_closed_operation_never_reopens() {
    let (_root, fake, windows, key) = setup(false, "work");
    windows.open(&key, OP).await.unwrap();
    windows.open(&key, OP).await.unwrap();
    assert_eq!(fake.commands().iter().filter(|args| args[0] == "new-session").count(), 1);
    windows.close(&key, OP).await.unwrap();
    assert!(!fake.alive(&name(OP)));
    assert_eq!(windows.open(&key, OP).await, Err("window_operation_closed"));
    assert_eq!(windows.read(&key, OP).await, Err("window_operation_closed"));
    assert_eq!(windows.close(&key, OP).await, Ok(()), "fechar de novo é idempotente");
}

#[tokio::test]
async fn another_account_cannot_take_over_an_operation() {
    let (root, _fake, windows, key) = setup(false, "work");
    std::fs::create_dir_all(root.path().join("other")).unwrap();
    let other = AccountKey::new(super::super::Provider::Claude, &root.path().join("other")).unwrap();
    windows.open(&key, OP).await.unwrap();
    assert_eq!(windows.open(&other, OP).await, Err("window_operation_diverged"));
    assert_eq!(windows.read(&other, OP).await, Err("window_operation_missing"));
    assert_eq!(windows.close(&other, OP).await, Err("window_operation_diverged"));
}

#[tokio::test]
async fn failed_typing_kills_the_window_and_forgets_it() {
    let (_root, fake, windows, key) = setup(false, "work");
    fake.0.lock().unwrap().fail.insert("send-keys");
    assert_eq!(windows.open(&key, OP).await, Err("window_type_failed"));
    assert!(!fake.alive(&name(OP)));
    assert_eq!(windows.read(&key, OP).await, Err("window_operation_missing"));
}

#[tokio::test]
async fn window_without_the_hidden_mark_is_killed_and_refused() {
    let (_root, fake, windows, key) = setup(false, "work");
    fake.0.lock().unwrap().fail.insert("set-option");
    assert_eq!(windows.open(&key, OP).await, Err("window_hide_failed"));
    assert!(!fake.alive(&name(OP)));
}

#[tokio::test]
async fn code_goes_through_stdin_only_at_the_prompt() {
    let (_root, fake, windows, key) = setup(false, "work");
    windows.open(&key, OP).await.unwrap();
    fake.0.lock().unwrap().screen = "Opening browser to sign in…\n".into();
    assert_eq!(windows.code(&key, OP, "secret-code#state").await, Err("window_cli_not_waiting"));
    assert!(!fake.commands().iter().any(|args| args[0] == "load-buffer"));
    fake.0.lock().unwrap().screen = PSMUX_SCREEN.into();
    windows.code(&key, OP, "secret-code#state").await.unwrap();
    let calls = fake.calls();
    let load = calls.iter().find(|(_, args, _)| args[0] == "load-buffer").unwrap();
    assert_eq!(load.2, b"secret-code#state");
    assert!(calls.iter().all(|(_, args, _)| args.iter().all(|arg| !arg.contains("secret-code"))), "o código nunca vai no argv");
    let buffer = &load.1[2];
    let pane = format!("={}:", name(OP));
    let commands = fake.commands();
    let paste = commands.iter().position(|args| args[0] == "paste-buffer").unwrap();
    assert_eq!(commands[paste], ["paste-buffer", "-t", &pane, "-b", buffer, "-d"].map(str::to_owned).to_vec());
    assert_eq!(commands[paste + 1], ["send-keys", "-t", &pane, "-l", "--", "\r"].map(str::to_owned).to_vec());
    assert_eq!(commands.last().unwrap(), &["delete-buffer", "-b", buffer].map(str::to_owned).to_vec());
}

#[tokio::test]
async fn unreadable_screen_is_no_link_yet_but_refuses_the_code_with_its_own_cause() {
    let (_root, fake, windows, key) = setup(false, "work");
    windows.open(&key, OP).await.unwrap();
    fake.0.lock().unwrap().fail.insert("capture-pane");
    assert_eq!(windows.read(&key, OP).await, Ok(None));
    assert_eq!(windows.code(&key, OP, "c").await, Err("window_capture_failed"));
    assert!(!fake.commands().iter().any(|args| args[0] == "load-buffer"));
}

#[tokio::test]
async fn invalid_code_is_refused_before_any_io() {
    let (_root, fake, windows, key) = setup(false, "work");
    windows.open(&key, OP).await.unwrap();
    let before = fake.calls().len();
    for code in ["", "a\nb", "a\rb", "a\0b", &"x".repeat(4097)] {
        assert_eq!(windows.code(&key, OP, code).await, Err("window_invalid_code"));
    }
    assert_eq!(fake.calls().len(), before);
}

#[tokio::test]
async fn failed_paste_still_deletes_the_buffer() {
    let (_root, fake, windows, key) = setup(false, "work");
    windows.open(&key, OP).await.unwrap();
    { let mut world = fake.0.lock().unwrap(); world.screen = PSMUX_SCREEN.into(); world.fail.insert("paste-buffer"); }
    assert_eq!(windows.code(&key, OP, "c").await, Err("window_paste_failed"));
    assert_eq!(fake.commands().last().unwrap()[0], "delete-buffer");
}

#[tokio::test]
async fn close_is_proven_by_reading_back_and_a_survivor_is_swept() {
    let (_root, fake, windows, key) = setup(false, "work");
    windows.open(&key, OP).await.unwrap();
    fake.0.lock().unwrap().kill_lies = true;
    assert_eq!(windows.close(&key, OP).await, Err("window_still_open"));
    fake.0.lock().unwrap().kill_lies = false;
    windows.sweep().await;
    assert!(!fake.alive(&name(OP)), "a janela que não morreu no fechamento sai na varredura");
}

#[tokio::test]
async fn sweep_kills_only_hidden_orphans_of_this_server() {
    let (root, fake, windows, key) = setup(false, "work");
    let owner = root.path().join(".claude").to_string_lossy().into_owned();
    let live = "1".repeat(32);
    windows.open(&key, &live).await.unwrap();
    let mark = |hidden: bool, owner: &str| {
        let mut options = HashMap::new();
        if hidden { options.insert("@cp_hidden".to_owned(), "1".to_owned()); }
        options.insert("@cp_login_owner".to_owned(), owner.to_owned());
        options
    };
    {
        let mut world = fake.0.lock().unwrap();
        world.sessions.insert(name(&"a".repeat(32)), mark(true, &owner));
        world.sessions.insert(name(&"b".repeat(32)), mark(false, &owner));
        world.sessions.insert(name(&"c".repeat(32)), mark(true, "/other/.claude"));
        world.sessions.insert("term-login-notanoperation".into(), mark(true, &owner));
        world.sessions.insert("hangar".into(), mark(true, &owner));
    }
    windows.sweep().await;
    assert!(!fake.alive(&name(&"a".repeat(32))), "órfã escondida deste servidor sai");
    for kept in [name(&live), name(&"b".repeat(32)), name(&"c".repeat(32)), "term-login-notanoperation".into(), "hangar".into()] {
        assert!(fake.alive(&kept), "{kept} não pode sair");
    }
}

#[tokio::test]
async fn refresh_opens_the_bare_cli_only_in_a_trusted_folder() {
    let (root, fake, windows, key) = setup(false, "work");
    assert_eq!(windows.refresh(&key, OP).await, Err("window_no_trusted_folder"));
    assert!(fake.calls().is_empty());
    let gone = root.path().join("gone");
    let trusted = root.path().join("repo");
    std::fs::create_dir_all(&trusted).unwrap();
    let config = json!({"projects": {
        root.path().join("untrusted").to_string_lossy(): {"hasTrustDialogAccepted": "true"},
        gone.to_string_lossy(): {"hasTrustDialogAccepted": true},
        trusted.to_string_lossy(): {"hasTrustDialogAccepted": true},
    }});
    std::fs::write(key.canonical_home.join(".claude.json"), config.to_string()).unwrap();
    windows.refresh(&key, OP).await.unwrap();
    let create = fake.commands().into_iter().find(|args| args[0] == "new-session").unwrap();
    assert_eq!(create[5], trusted.to_string_lossy());
    assert!(fake.commands().contains(&["send-keys", "-t", &format!("={}:", name(OP)), "-l", "--", "claude"].map(str::to_owned).to_vec()));
}

#[tokio::test]
async fn a_closed_operation_is_never_refreshed() {
    let (root, fake, windows, key) = setup(false, "work");
    let trusted = root.path().join("repo");
    std::fs::create_dir_all(&trusted).unwrap();
    std::fs::write(key.canonical_home.join(".claude.json"),
        json!({"projects": {trusted.to_string_lossy(): {"hasTrustDialogAccepted": true}}}).to_string()).unwrap();
    windows.close(&key, OP).await.unwrap();
    assert_eq!(windows.refresh(&key, OP).await, Err("window_operation_closed"));
    assert!(!fake.commands().iter().any(|args| args[0] == "new-session"));
}

#[test]
fn broken_claude_json_has_no_trusted_folder() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join(".claude.json"), b"{quebrado").unwrap();
    assert_eq!(trusted_folder(root.path()), None);
}

/// Prova com o multiplexador e a CLI da máquina, numa conta temporária:
/// `cargo test -p hangar-server --lib real_window -- --ignored`.
#[tokio::test]
#[ignore]
async fn real_window_shows_the_whole_link_and_closes() {
    let root = tempfile::tempdir().unwrap();
    let account = root.path().join(".claude-prova");
    std::fs::create_dir_all(&account).unwrap();
    let mut env = super::super::environment::AccountEnvironment::from_map(std::env::vars().collect());
    env.home = root.path().into();
    let windows = local(&env);
    let key = AccountKey::new(super::super::Provider::Claude, &account).unwrap();
    let op = crate::mods::state::random_hex(16);
    windows.open(&key, &op).await.unwrap();
    let mut url = None;
    for _ in 0..60 {
        if let Some(found) = windows.read(&key, &op).await.unwrap().filter(|found| found.contains("&state=")) {
            url = Some(found);
            break;
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    windows.close(&key, &op).await.unwrap();
    let url = url.expect("o link não apareceu na janela");
    assert!(url.contains("&redirect_uri=") && url.contains("&code_challenge="), "{url}");
}

proptest::proptest! {
    #[test]
    fn any_screen_parses_without_panic(screen in "\\PC*", width in proptest::option::of(0usize..300)) {
        let _ = oauth_url(&join_wrapped(&screen, width));
    }
}
