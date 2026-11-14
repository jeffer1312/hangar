//! Recusa transitória do escopo do systemd: só "Unit … not found" se repete (no máximo 3 vezes), na
//! sondagem e na subida; outra falha responde na hora. `systemd-run` falso guiado por um roteiro.
#![cfg(unix)]
use hangar_server::runtime::process::{self, LaunchSpec, Provider};
use std::{io::Write, sync::{Arc, Mutex}};

#[derive(Clone)]
struct Buffer(Arc<Mutex<Vec<u8>>>);
impl Write for Buffer {
    fn write(&mut self, data: &[u8]) -> std::io::Result<usize> { self.0.lock().unwrap().extend_from_slice(data); Ok(data.len()) }
    fn flush(&mut self) -> std::io::Result<()> { Ok(()) }
}

/// Cada chamada consome a primeira linha de `plan`: `unit` (recusa transitória), `other` (outra recusa),
/// `noise` (o processo do escopo sai sem escutar, com uma linha no stderr, como o `eprintln!` do cano)
/// ou nada (sobe). Anota em `calls` o tipo da chamada e o desfecho.
const FAKE: &str = r#"#!/bin/sh
dir=$(dirname "$0")
last=""; for arg in "$@"; do last=$arg; done
kind=launch; [ "$last" = true ] && kind=probe
step=$(head -n 1 "$dir/plan" 2>/dev/null)
tail -n +2 "$dir/plan" > "$dir/plan.next" 2>/dev/null; mv "$dir/plan.next" "$dir/plan"
echo "$kind ${step:-ok}" >> "$dir/calls"
case "$step" in
  unit) echo "Failed to start transient scope unit: Unit run-p1-i1.scope not found." >&2; exit 1;;
  other) echo "Failed to connect to user scope bus: Permission denied" >&2; exit 1;;
  noise) echo "hangar-cano: --log ilegivel" >&2; exit 3;;
esac
while [ "$1" != "--" ]; do shift; done; shift
exec "$@"
"#;

/// Um teste só neste binário: troca o PATH do processo e o logger global.
#[tokio::test]
async fn only_the_transient_unit_refusal_is_retried_three_times() {
    let output = Arc::new(Mutex::new(Vec::new()));
    let capture = output.clone();
    tracing::subscriber::set_global_default(tracing_subscriber::fmt().without_time().with_ansi(false)
        .with_writer(move || Buffer(capture.clone())).finish()).unwrap();
    let exe = std::env::current_exe().unwrap();
    let cano = exe.ancestors().skip(1).map(|dir| dir.join("hangar-cano")).find(|cano| cano.is_file())
        .unwrap_or_else(|| panic!("rode `cargo build -p hangar-cano` antes: nenhum acima de {}", exe.display()));
    process::use_cano_binary(cano).unwrap();

    let dir = tempfile::tempdir().unwrap();
    let bin = dir.path().join("bin");
    std::fs::create_dir(&bin).unwrap();
    let fake = bin.join("systemd-run");
    std::fs::write(&fake, FAKE).unwrap();
    std::fs::set_permissions(&fake, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
    let path = std::env::join_paths(std::iter::once(bin.clone()).chain(std::env::split_paths(&std::env::var_os("PATH").unwrap()))).unwrap();
    // SAFETY: único teste do binário, antes de qualquer outra linha ler o ambiente.
    unsafe {
        std::env::set_var("PATH", &path);
        std::env::set_var("XDG_RUNTIME_DIR", dir.path());
    }
    let plan = |steps: &[&str]| {
        std::fs::write(bin.join("plan"), steps.iter().map(|s| format!("{s}\n")).collect::<String>()).unwrap();
        let _ = std::fs::remove_file(bin.join("calls"));
    };
    let calls = || std::fs::read_to_string(bin.join("calls")).unwrap_or_default().lines().map(str::to_owned).collect::<Vec<_>>();
    let retries = || String::from_utf8(output.lock().unwrap().clone()).unwrap().lines()
        .filter(|line| line.contains("code=\"systemd_scope_unit_missing\"") || line.contains("code=systemd_scope_unit_missing")).count();
    let logged = |needles: &[&str]| String::from_utf8(output.lock().unwrap().clone()).unwrap().lines()
        .filter(|line| needles.iter().all(|needle| line.contains(needle))).count();
    let sessions = tempfile::tempdir().unwrap();
    let spec = |key: &str| LaunchSpec { provider: Provider::Codex, key: key.into(), cwd: sessions.path().into(),
        program: vec!["sleep".into(), "30".into()], env: vec![("PATH".into(), "/bin:/usr/bin".into())], sidecar_dir: sessions.path().into() };

    // Sondagem recusada nas 4 tentativas: esta subida segue sem escopo, e "sem escopo" não fica gravado.
    plan(&["unit", "unit", "unit", "unit"]);
    let started = process::spawn(&spec("scope-retry-a")).await.expect("sem escopo, o cano sobe direto");
    assert_eq!(calls(), ["probe unit"; 4]);
    assert_eq!(retries(), 4, "três repetições e o fim, cada um com o código");
    process::kill(&started, "scope-retry-a", sessions.path()).await.unwrap();

    // Outra recusa na sondagem: esta subida segue sem escopo, a causa vai ao log com código e nada é gravado.
    plan(&["other"]);
    let started = process::spawn(&spec("scope-retry-a2")).await.expect("sem escopo, o cano sobe direto");
    assert_eq!(calls(), ["probe other"]);
    assert_eq!(logged(&["systemd_scope_refused", "Permission denied"]), 1);
    process::kill(&started, "scope-retry-a2", sessions.path()).await.unwrap();

    // A sondagem volta a rodar e sobe na 3ª; a subida do cano sobe na 4ª (3 recusas).
    plan(&["unit", "unit", "", "unit", "unit", "unit", ""]);
    let started = process::spawn(&spec("scope-retry-b")).await.expect("sobe depois das recusas transitórias");
    assert_eq!(calls(), ["probe unit", "probe unit", "probe ok", "launch unit", "launch unit", "launch unit", "launch ok"]);
    process::kill(&started, "scope-retry-b", sessions.path()).await.unwrap();
    assert_eq!(retries(), 4 + 2 + 3);

    // Quatro recusas na subida: o erro original aparece.
    plan(&["unit", "unit", "unit", "unit"]);
    let error = process::spawn(&spec("scope-retry-c")).await.err().expect("recusa esgotada vira erro");
    assert!(matches!(&error, process::ProcessError::Spawn(text) if text.contains("transient scope unit")), "{error:?}");
    assert_eq!(calls(), ["launch unit"; 4]);

    // Outra recusa: nenhuma repetição, a resposta não espera o prazo da escuta e o erro aponta o escopo.
    plan(&["other"]);
    let before = retries();
    let at = std::time::Instant::now();
    let error = process::spawn(&spec("scope-retry-d")).await.err().expect("outra recusa vira erro");
    assert!(matches!(&error, process::ProcessError::Spawn(text) if text.contains("escopo do systemd recusado") && text.contains("Permission denied")), "{error:?}");
    assert_eq!(calls(), ["launch other"]);
    assert_eq!(retries(), before);
    assert_eq!(logged(&["systemd_scope_refused", "Permission denied"]), 2, "sondagem acima e esta subida");
    assert!(at.elapsed() < std::time::Duration::from_secs(5), "esperou o prazo da escuta: {:?}", at.elapsed());

    // Processo que sai sem escutar e deixa uma linha no stderr: a causa vai ao log, sem culpar o escopo.
    plan(&["noise"]);
    let error = process::spawn(&spec("scope-retry-e")).await.err().expect("saída antes de escutar vira erro");
    assert_eq!(error.code(), "cano_nao_escutou");
    assert_eq!(logged(&["cano_saiu_antes_de_escutar", "--log ilegivel"]), 1);
    assert!(std::fs::read_dir(sessions.path()).unwrap().all(|entry| !entry.unwrap().file_name().to_string_lossy().ends_with(".scope-err")),
        "o stderr do escopo não fica na pasta da sessão");
}
