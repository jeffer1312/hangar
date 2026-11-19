//! Puxa os marketplaces de plugins da conta antes do app-server. Ao subir, ele faria a mesma conferência
//! com a sessão aberta e reinstalaria o plugin que mudou, e o Codex recusaria os hooks dele até a
//! reaprovação. Roda depois da atualização do Codex: no Windows o binário não troca com outro aberto.
use std::collections::BTreeMap;
use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use serde_json::Value;

use super::Screen;

/// Sem rede, o `git fetch` não pode segurar a abertura.
pub(crate) const TIMEOUT: Duration = Duration::from_secs(20);

/// Uma conferência por conta por hora: ela custa uns 5 s de rede, e o plugin que mudar dentro da hora
/// é reinstalado pelo app-server e reaprovado pelo ator.
pub(crate) const EVERY: Duration = Duration::from_secs(3600);
const MARKER: &str = ".hangar-marketplaces-upgraded";

/// A conta não teve conferência bem-sucedida na última hora (marca ausente, ilegível ou no futuro).
pub(crate) fn due(env: &BTreeMap<String, String>) -> bool {
    let Some(home) = env.get("CODEX_HOME") else { return true };
    std::fs::metadata(Path::new(home).join(MARKER)).and_then(|meta| meta.modified()).ok()
        .and_then(|at| at.elapsed().ok()).is_none_or(|age| age >= EVERY)
}

/// Só a conferência que terminou bem conta: a que falhou é tentada de novo na próxima abertura.
pub(crate) fn mark_done(env: &BTreeMap<String, String>) {
    let Some(home) = env.get("CODEX_HOME") else { return };
    if let Err(error) = std::fs::write(Path::new(home).join(MARKER), b"") {
        tracing::warn!(io_kind = ?error.kind(), "marca da conferência dos marketplaces não foi gravada");
    }
}

/// As linhas de marketplace que falharam (o Codex escreve uma por marketplace), até 3, em 200 caracteres.
pub(crate) fn failed_lines(stderr: &str) -> Vec<String> {
    stderr.lines().filter(|line| line.starts_with("Failed to upgrade marketplace")).take(3)
        .map(|line| line.chars().take(200).collect()).collect()
}

/// Quantos marketplaces o `--json` diz que atualizou.
pub(crate) fn upgraded(stdout: &str) -> usize {
    serde_json::from_str::<Value>(stdout).ok().and_then(|value| value.get("upgradedRoots").and_then(Value::as_array).map(Vec::len))
        .unwrap_or(0)
}

/// O resultado de `codex plugin marketplace upgrade --json` em linhas do pane.
pub(crate) enum Outcome { Ran { code: Option<i32>, stdout: String, stderr: String }, NotRun(&'static str) }

pub(crate) fn lines(outcome: &Outcome) -> Vec<String> {
    match outcome {
        Outcome::NotRun(kind) => vec![format!("não deu para conferir os plugins ({kind}); o Codex confere depois de abrir e o Hangar reaprova os hooks que mudarem")],
        Outcome::Ran { code: Some(0), stdout, .. } => match upgraded(stdout) {
            0 => Vec::new(),
            count => vec![format!("plugins atualizados em {count} marketplace(s)")],
        },
        Outcome::Ran { code, stderr, .. } => {
            let mut lines = failed_lines(stderr);
            let code = code.map_or_else(|| "None".to_owned(), |code| code.to_string());
            lines.push(format!("a conferência dos plugins saiu com código {code}; a sessão abre assim"));
            lines
        }
    }
}

pub(crate) async fn run(env: &BTreeMap<String, String>, screen: &dyn Screen) {
    run_with(env, TIMEOUT, screen).await
}

async fn run_with(env: &BTreeMap<String, String>, limit: Duration, screen: &dyn Screen) {
    if !due(env) { return; }
    let Some(codex) = crate::runtime::process::resolve_program("codex", env) else { return };
    screen.say("conferindo os plugins…");
    let spawned = tokio::process::Command::new(codex).args(["plugin", "marketplace", "upgrade", "--json"]).env_clear().envs(env)
        .stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).kill_on_drop(true).spawn();
    let outcome = match spawned {
        Err(error) => Outcome::NotRun(super::app_server::error_kind(&error)),
        Ok(child) => match tokio::time::timeout(limit, child.wait_with_output()).await {
            Err(_) => Outcome::NotRun("TimeoutExpired"),
            Ok(Err(error)) => Outcome::NotRun(super::app_server::error_kind(&error)),
            Ok(Ok(output)) => Outcome::Ran { code: hangar_core::terminal_process::exit_code(output.status),
                stdout: String::from_utf8_lossy(&output.stdout).into_owned(), stderr: String::from_utf8_lossy(&output.stderr).into_owned() },
        },
    };
    if matches!(outcome, Outcome::Ran { code: Some(0), .. }) { mark_done(env); }
    for line in lines(&outcome) {
        screen.say(&line);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_successful_check_holds_for_an_hour_per_account() {
        let dir = tempfile::tempdir().unwrap();
        let env: BTreeMap<String, String> = [("CODEX_HOME".to_owned(), dir.path().to_string_lossy().into_owned())].into();
        assert!(due(&env), "conta sem conferência");
        mark_done(&env);
        assert!(!due(&env), "conferida agora");
        let old = std::time::SystemTime::now() - EVERY - Duration::from_secs(1);
        std::fs::File::options().write(true).open(dir.path().join(MARKER)).unwrap().set_modified(old).unwrap();
        assert!(due(&env), "a hora passou");
        let other = tempfile::tempdir().unwrap();
        assert!(due(&[("CODEX_HOME".to_owned(), other.path().to_string_lossy().into_owned())].into()), "a marca é da conta");
        assert!(due(&BTreeMap::new()), "sem CODEX_HOME confere");
    }

    /// `launcher__marketplaces-*`: atualizados, falha (3 linhas cortadas em 200), JSON torto e prazo.
    #[test]
    fn the_outcome_becomes_the_launcher_lines() {
        let ran = |code: i32, stdout: &str, stderr: &str| Outcome::Ran { code: Some(code), stdout: stdout.into(), stderr: stderr.into() };
        assert_eq!(lines(&ran(0, r#"{"upgradedRoots":["/a","/b"]}"#, "")), ["plugins atualizados em 2 marketplace(s)"]);
        assert!(lines(&ran(0, "torto", "")).is_empty());
        assert!(lines(&ran(0, r#"{"upgradedRoots":[]}"#, "")).is_empty());
        let long = "y".repeat(400);
        let stderr = (0..5).map(|i| format!("Failed to upgrade marketplace m{i}: {long}")).chain(["outra linha".into()]).collect::<Vec<_>>().join("\n");
        let failed = lines(&ran(1, "", &stderr));
        assert_eq!(failed.len(), 4);
        assert!(failed[..3].iter().all(|line| line.chars().count() == 200 && line.starts_with("Failed to upgrade marketplace m")));
        assert_eq!(failed[3], "a conferência dos plugins saiu com código 1; a sessão abre assim");
        assert_eq!(lines(&Outcome::NotRun("TimeoutExpired")),
            ["não deu para conferir os plugins (TimeoutExpired); o Codex confere depois de abrir e o Hangar reaprova os hooks que mudarem"]);
    }

    struct Lines(std::sync::Mutex<Vec<String>>);
    impl Screen for Lines {
        fn say(&self, line: &str) { self.0.lock().unwrap().push(line.to_owned()); }
    }

    /// Roda na casa da conta (o `CODEX_HOME` do ambiente da sessão), com a entrada fechada.
    #[cfg(unix)]
    #[tokio::test]
    async fn the_upgrade_runs_in_the_account_home_and_a_slow_one_is_cut() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("log");
        let codex = dir.path().join("codex");
        std::fs::write(&codex, format!("#!/bin/sh\necho \"$*|$CODEX_HOME\" > '{}'\n[ -n \"$SLOW\" ] && exec sleep 5\necho '{{\"upgradedRoots\":[\"/m\"]}}'\n", log.display())).unwrap();
        std::fs::set_permissions(&codex, std::fs::Permissions::from_mode(0o755)).unwrap();
        let mut env = BTreeMap::from([("PATH".to_owned(), format!("{}:/usr/bin:/bin", dir.path().display())), ("CODEX_HOME".to_owned(), "/h/.codex-x".to_owned())]);
        let lines = Lines(Default::default());
        run_with(&env, TIMEOUT, &lines).await;
        assert_eq!(lines.0.lock().unwrap().as_slice(), ["conferindo os plugins…", "plugins atualizados em 1 marketplace(s)"]);
        assert_eq!(std::fs::read_to_string(&log).unwrap(), "plugin marketplace upgrade --json|/h/.codex-x\n");
        env.insert("SLOW".into(), "1".into());
        let lines = Lines(Default::default());
        run_with(&env, Duration::from_millis(300), &lines).await;
        assert_eq!(lines.0.lock().unwrap()[1], "não deu para conferir os plugins (TimeoutExpired); o Codex confere depois de abrir e o Hangar reaprova os hooks que mudarem");
    }
}
