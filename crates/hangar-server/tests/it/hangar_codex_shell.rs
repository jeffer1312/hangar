//! `scripts/hangar-codex` (o `codex` digitado no terminal) em shell, com `hangar-server` e `tmux`
//! falsos: o pedido ao `hangar-server open`, as saídas 1/2/3, os comandos do tmux e a limpeza quando
//! a sessão acaba. Os casos são os da captura do script Python (`hangar-codex__*`).
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use serde_json::{Value, json};

struct Shell {
    _root: tempfile::TempDir,
    script: PathBuf,
    home: PathBuf,
    work: PathBuf,
    stubs: PathBuf,
    srvbin: PathBuf,
    state: PathBuf,
}

/// O `hangar-server` falso: `open` grava o argv e responde com o que o caso pôs em `state/`; `codex-close`
/// só grava.
const SERVER: &str = r#"#!/bin/sh
printf '%s\n' "$*" >> "$STUB_STATE/server.log"
if [ "$1" = codex-close ]; then exit 0; fi
printf '%s' "$2" > "$STUB_STATE/body.json"
[ -f "$STUB_STATE/open.out" ] && cat "$STUB_STATE/open.out"
[ -f "$STUB_STATE/open.err" ] && cat "$STUB_STATE/open.err" >&2
exit "$(cat "$STUB_STATE/open.rc" 2>/dev/null || echo 0)"
"#;

/// O `tmux` falso: grava o argv; `has-session` responde a fila de `state/has-session` (a última repete).
const TMUX: &str = r#"#!/bin/sh
printf '%s\n' "$*" >> "$STUB_STATE/tmux.log"
printf '%s\n' "${TMUX:-unset}" >> "$STUB_STATE/tmux-env.log"
case "$1" in
    has-session)
        codes=$(cat "$STUB_STATE/has-session" 2>/dev/null || echo 1)
        first=${codes%% *}; rest=${codes#* }
        [ "$rest" = "$codes" ] || printf '%s' "$rest" > "$STUB_STATE/has-session"
        exit "$first" ;;
    list-panes) exit "$(cat "$STUB_STATE/list-panes" 2>/dev/null || echo 0)" ;;
    switch-client) exit "$(cat "$STUB_STATE/switch" 2>/dev/null || echo 0)" ;;
    attach-session) exit "$(cat "$STUB_STATE/attach" 2>/dev/null || echo 0)" ;;
esac
"#;

impl Shell {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        // A cópia do script fica fora do checkout: o build de `crates/target` dele não conta.
        let scripts = root.path().join("copy/scripts");
        let (home, work, stubs, srvbin, state) = (root.path().join("home"), root.path().join("work"), root.path().join("stubs"),
            root.path().join("srvbin"), root.path().join("state"));
        for dir in [&scripts, &home, &work, &stubs, &srvbin, &state] {
            std::fs::create_dir_all(dir).unwrap();
        }
        let source = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../scripts/hangar-codex");
        let script = scripts.join("hangar-codex");
        crate::copy_executable(&source, &script);
        std::fs::copy(source.with_file_name("hangar-server-lookup"), scripts.join("hangar-server-lookup")).unwrap();
        crate::write_executable(&stubs.join("tmux"), TMUX);
        crate::write_executable(&srvbin.join("hangar-server"), SERVER);
        std::fs::write(state.join("open.out"), json!({"name": "codex-proj", "cwd": "/w", "group": {"name": "outro"}}).to_string()).unwrap();
        Self { _root: root, script, home, work, stubs, srvbin, state }
    }

    fn run(&self, args: &[&str], env: &[(&str, &str)]) -> Output {
        Command::new("/bin/sh").arg(&self.script).args(args).current_dir(&self.work).env_clear()
            .env("HOME", &self.home).env("PATH", format!("{}:/usr/bin:/bin", self.stubs.display())).env("STUB_STATE", &self.state)
            .envs(env.iter().copied()).stdin(Stdio::null()).output().unwrap()
    }

    fn with_server(&self, args: &[&str], env: &[(&str, &str)]) -> Output {
        let server = self.srvbin.join("hangar-server");
        let mut all: Vec<(&str, &str)> = vec![("CP_RUST_SERVER_BIN", server.to_str().unwrap())];
        all.extend_from_slice(env);
        self.run(args, &all)
    }

    fn set(&self, name: &str, text: &str) { std::fs::write(self.state.join(name), text).unwrap(); }

    fn log(&self, name: &str) -> Vec<String> {
        std::fs::read_to_string(self.state.join(name)).unwrap_or_default().lines().map(str::to_owned).collect()
    }

    fn body(&self) -> Value { serde_json::from_slice(&std::fs::read(self.state.join("body.json")).unwrap()).unwrap() }

    fn cwd(&self) -> String { std::fs::canonicalize(&self.work).unwrap().to_string_lossy().into_owned() }
}

fn stderr(output: &Output) -> String { String::from_utf8_lossy(&output.stderr).into_owned() }

const OPENED: &str = "Codex gerenciado pelo hangar: codex-proj (detach: Ctrl-b d)\n";

/// `hangar-codex__usage-two-args`.
#[test]
fn more_than_one_argument_is_a_usage_error() {
    let shell = Shell::new();
    let output = shell.run(&["a", "b"], &[]);
    assert_eq!((output.status.code(), stderr(&output).as_str()), (Some(2), "uso: hangar-codex [prompt]\n"));
}

/// `hangar-codex__no-hangar-server`, `__rust-bin-invalid` e `__rust-bin-invalid-with-path-stub`: sem
/// servidor sai 3 (o `codex` oficial abre); o explícito errado não cai para o do PATH.
#[test]
fn without_a_server_it_exits_3_for_the_official_codex() {
    let shell = Shell::new();
    let output = shell.run(&[], &[]);
    let missing = "hangar-codex: hangar-server não encontrado (CP_RUST_SERVER_BIN, crates/target/release, ~/.hangar/bin ou PATH): nenhuma sessão foi aberta\n";
    assert_eq!((output.status.code(), stderr(&output).as_str()), (Some(3), missing));
    crate::write_executable(&shell.stubs.join("hangar-server"), SERVER);
    let output = shell.run(&[], &[("CP_RUST_SERVER_BIN", "/nao/existe")]);
    assert_eq!(output.status.code(), Some(3));
    assert_eq!(stderr(&output), format!("rust_bins: CP_RUST_SERVER_BIN=/nao/existe não é executável; seguindo sem hangar-server\n{missing}"));
    assert!(shell.log("server.log").is_empty());
    // `hangar-codex__server-on-path`: sem explícito, o do PATH abre.
    let output = shell.run(&[], &[]);
    assert_eq!((output.status.code(), stderr(&output).as_str()), (Some(0), OPENED));
    assert_eq!(shell.body(), json!({"cwd": shell.cwd(), "provider": "codex", "headless": false, "initial_prompt": null}));
}

/// O prompt chega inteiro ao JSON, com aspas, contrabarra, acento, tab e quebra de linha.
#[test]
fn the_prompt_and_the_folder_reach_the_open_request_as_json() {
    let shell = Shell::new();
    let prompt = "olá, \"prompt\"\nlinha\t2 \\ fim\n";
    let output = shell.with_server(&[prompt], &[]);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    assert_eq!(shell.body(), json!({"cwd": shell.cwd(), "provider": "codex", "headless": false, "initial_prompt": prompt}));
    assert_eq!(shell.log("server.log")[0].split(' ').next(), Some("open"));
}

/// No Git Bash do Windows o `pwd` dá `/c/...`, que o `hangar-server` nativo não acha ("a pasta
/// /c/Users/... não existe"): a pasta vai pelo `cygpath -w`.
#[test]
fn under_git_bash_the_folder_goes_as_a_windows_path() {
    let shell = Shell::new();
    crate::write_executable(&shell.stubs.join("uname"), "#!/bin/sh\necho MINGW64_NT-10.0-26300\n");
    let cygpath = format!("#!/bin/sh\n[ \"$1\" = -w ] && [ \"$2\" = '{}' ] || exit 1\nprintf '%s\\n' 'C:\\Users\\x\\work'\n", shell.cwd());
    crate::write_executable(&shell.stubs.join("cygpath"), &cygpath);
    let output = shell.with_server(&[], &[]);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    assert_eq!(shell.body()["cwd"], json!(r"C:\Users\x\work"));
}

/// `cygpath` que falha não pode mandar `"cwd": ""` ao servidor: sai com erro antes do `open`.
#[test]
fn under_git_bash_a_failed_cygpath_stops_before_the_server() {
    let shell = Shell::new();
    crate::write_executable(&shell.stubs.join("uname"), "#!/bin/sh\necho MINGW64_NT-10.0-26300\n");
    crate::write_executable(&shell.stubs.join("cygpath"), "#!/bin/sh\nexit 1\n");
    let output = shell.with_server(&[], &[]);
    assert_eq!(output.status.code(), Some(1), "{}", stderr(&output));
    assert!(stderr(&output).contains("cygpath"), "{}", stderr(&output));
    assert!(shell.log("server.log").is_empty());
}

/// `hangar-codex__codex-home-*`: a casa padrão não manda conta; `~/.codex-<id>` manda o id; outra recusa.
#[test]
fn the_inherited_codex_home_picks_the_account_or_refuses() {
    let shell = Shell::new();
    let default = shell.home.join(".codex").to_string_lossy().into_owned();
    shell.with_server(&[], &[("CODEX_HOME", &default)]);
    assert!(shell.body().get("codex_account").is_none());
    let account = shell.home.join(".codex-trabalho").to_string_lossy().into_owned();
    shell.with_server(&["olá, prompt"], &[("CODEX_HOME", &account)]);
    assert_eq!(shell.body()["codex_account"], "trabalho");
    assert_eq!(shell.body()["initial_prompt"], "olá, prompt");
    std::fs::remove_file(shell.state.join("body.json")).unwrap();
    let output = shell.with_server(&[], &[("CODEX_HOME", "/outra")]);
    assert_eq!((output.status.code(), stderr(&output).as_str()),
        (Some(1), "hangar-codex: CODEX_HOME personalizado não é uma conta Codex gerenciada pelo Hangar (~/.codex-<conta>)\n"));
    assert!(!shell.state.join("body.json").exists(), "nada foi aberto");
}

/// `hangar-codex__open-exit-*`: 2 e 4 saem 3 (nada enviado); recusa sai 1 com a mensagem do envelope.
#[test]
fn the_open_exit_code_decides_between_refusal_and_the_official_codex() {
    let shell = Shell::new();
    for (rc, err, out, code, wanted) in [
        ("2", "saida 2\n", "", 3, "hangar-codex: o hangar-server não abriu a sessão (saída 2): saida 2\n"),
        ("4", "saida 4\n", "", 3, "hangar-codex: o hangar-server não abriu a sessão (saída 4): saida 4\n"),
        ("1", "{\"detail\":{\"code\":\"x\",\"msg\":\"conta recusada\"}}\n", "", 1, "hangar-codex: conta recusada\n"),
        ("1", "texto simples\n", "", 1, "hangar-codex: texto simples\n"),
        ("3", "", "spec não aberto\n", 1, "hangar-codex: spec não aberto\n"),
        ("5", "", "", 1, "hangar-codex: hangar-server open saiu com 5\n"),
    ] {
        shell.set("open.rc", rc);
        shell.set("open.err", err);
        shell.set("open.out", out);
        std::fs::remove_file(shell.state.join("tmux.log")).ok();
        let output = shell.with_server(&[], &[]);
        assert_eq!((output.status.code(), stderr(&output).as_str()), (Some(code), wanted), "saída {rc}");
        assert!(shell.log("tmux.log").is_empty(), "nada anexado: saída {rc}");
    }
}

/// `hangar-codex__open-stdout-not-json`: o Python saía com traceback; agora é uma frase, saída 1.
#[test]
fn an_open_without_the_session_name_is_a_phrase_not_a_traceback() {
    let shell = Shell::new();
    shell.set("open.out", "isto não é json\n");
    let output = shell.with_server(&[], &[]);
    assert_eq!((output.status.code(), stderr(&output).as_str()),
        (Some(1), "hangar-codex: o hangar-server abriu sem dizer o nome da sessão; confira a lista no app\n"));
    assert!(shell.log("tmux.log").is_empty());
}

/// Fora do tmux: anexa; sessão encerrada fecha pelo servidor; detach deixa viva
/// (`hangar-codex__outside-tmux-*`, `__tmux-without-pane`, `__pane-dead-attaches`).
#[test]
fn outside_tmux_it_attaches_and_closes_only_an_ended_session() {
    let shell = Shell::new();
    shell.set("has-session", "1");
    let output = shell.with_server(&[], &[("TMUX", "/tmp/tmux-1000/default,1,0")]);
    assert_eq!((output.status.code(), stderr(&output).as_str()), (Some(0), OPENED));
    assert_eq!(shell.log("tmux.log"), ["attach-session -t =codex-proj", "has-session -t =codex-proj"]);
    assert_eq!(shell.log("tmux-env.log")[0], "unset", "TMUX sem pane sai do ambiente");
    assert_eq!(shell.log("server.log").last().unwrap(), "codex-close codex-proj");

    let shell = Shell::new();
    shell.set("has-session", "0");
    shell.set("attach", "1");
    let output = shell.with_server(&[], &[]);
    assert_eq!(output.status.code(), Some(1), "o código do attach");
    assert!(!shell.log("server.log").iter().any(|line| line.starts_with("codex-close")), "detach: a sessão segue viva");

    let shell = Shell::new();
    shell.set("list-panes", "1");
    shell.set("has-session", "0");
    let output = shell.with_server(&[], &[("TMUX", "/tmp/tmux-1000/default,1,0"), ("TMUX_PANE", "%99")]);
    assert_eq!(output.status.code(), Some(0));
    assert_eq!(shell.log("tmux.log"), ["list-panes -t %99", "attach-session -t =codex-proj", "has-session -t =codex-proj"]);
    assert_eq!(shell.log("tmux-env.log")[1], "unset", "pane morto herdado não vale");
}

/// Dentro do tmux com pane vivo: troca o cliente e espera a sessão acabar (`hangar-codex__inside-tmux-*`).
#[test]
fn inside_tmux_it_switches_and_waits_for_the_session_to_end() {
    let shell = Shell::new();
    shell.set("has-session", "0 0 1 1");
    let tmux = [("TMUX", "/tmp/tmux-1000/default,1,0"), ("TMUX_PANE", "%3")];
    let output = shell.with_server(&[], &tmux);
    assert_eq!((output.status.code(), stderr(&output).as_str()), (Some(0), OPENED));
    assert_eq!(shell.log("tmux.log"), ["list-panes -t %3", "switch-client -t =codex-proj", "has-session -t =codex-proj",
        "has-session -t =codex-proj", "has-session -t =codex-proj", "has-session -t =codex-proj"]);
    assert_eq!(shell.log("server.log").last().unwrap(), "codex-close codex-proj");

    let shell = Shell::new();
    shell.set("switch", "1");
    shell.set("has-session", "0");
    let output = shell.with_server(&[], &tmux);
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(shell.log("tmux.log"), ["list-panes -t %3", "switch-client -t =codex-proj", "has-session -t =codex-proj"]);
}

/// A função `codex` de bash/zsh manda `app` ao oficial, como a do PowerShell (antes só ela).
#[cfg(target_os = "linux")]
#[test]
fn the_posix_function_sends_the_app_subcommand_to_the_official_codex() {
    if Command::new("script").arg("--version").output().is_err() { return; }
    let shell = Shell::new();
    let function = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../scripts/shell/codex.posix.sh");
    crate::write_executable(&shell.stubs.join("codex"), "#!/bin/sh\necho \"oficial $*\" >> \"$STUB_STATE/codex.log\"\n");
    crate::write_executable(&shell.stubs.join("hangar-codex"), "#!/bin/sh\necho \"hangar $*\" >> \"$STUB_STATE/codex.log\"\nexit 3\n");
    for argument in ["app", "exec", "um prompt"] {
        let line = format!(". '{}'; codex '{argument}'", function.display());
        // O `script` dá um terminal à entrada: sem ele a função manda tudo ao oficial.
        let status = Command::new("script").args(["-qec", &format!("/bin/sh -c \"{}\"", line.replace('"', "\\\"")), "/dev/null"])
            .current_dir(&shell.work).env_clear().env("HOME", &shell.home).env("PATH", format!("{}:/usr/bin:/bin", shell.stubs.display()))
            .env("STUB_STATE", &shell.state).stdin(Stdio::null()).status().unwrap();
        assert!(status.success(), "{argument}");
    }
    assert_eq!(shell.log("codex.log"), ["oficial app", "oficial exec", "hangar um prompt", "oficial um prompt"]);
}
