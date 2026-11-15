//! Poda dos sidecars de sessão morta (`hangar-sidecar-prune`): o servidor diz quem está vivo, por
//! uma rodada só da descoberta da lista, e grava uma linha no diário por rodada.
use std::collections::HashSet;
use std::future::Future;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::SystemTime;

use hangar_api::session::SessionRow;
use hangar_sidecar_prune::{Live, Report};
use serde_json::json;

use crate::accounts::AccountService;
use crate::list::bridge::ListError;
use crate::list::mux::Pane;
use crate::routes::AppState;
use crate::startup::Readiness;

type Discovered = (Arc<Vec<SessionRow>>, Arc<Vec<Pane>>, bool);

/// Quem ainda pode voltar, pelo registro em disco.
#[derive(Debug, Default, PartialEq)]
struct Recorded { keys: HashSet<String>, names: HashSet<String>, stems: HashSet<String> }

/// Primeira rodada só com o servidor preparado: antes disso a descoberta ainda não responde.
pub(crate) async fn run(state: Arc<AppState>) {
    let readiness = state.startup.clone();
    run_after(readiness, move || round(state.clone())).await;
}

async fn run_after<F, Fut>(readiness: Readiness, round: F)
where
    F: FnMut() -> Fut,
    Fut: Future<Output = bool>,
{
    readiness.wait().await;
    hangar_sidecar_prune::every(round).await;
}

async fn round(state: Arc<AppState>) -> bool {
    // O registro antes da descoberta: sessão retomada entre as duas leituras já está na lista.
    let home = state.accounts.env.home.clone();
    let recorded = match tokio::task::spawn_blocking(move || recorded(&home)).await {
        Ok(recorded) => recorded,
        Err(error) => {
            tracing::error!(code = "sidecar_prune_record_interrupted", panic = error.is_panic(), %error, "leitura do registro da poda interrompida");
            Err(io::Error::other("leitura do registro interrompida"))
        }
    };
    let discovered = state.list.live_round().await;
    let accounts = state.accounts.clone();
    let done = tokio::task::spawn_blocking(move || {
        let (bases, catalog) = bases(&accounts);
        (sweep(discovered, recorded, &bases, SystemTime::now()), catalog)
    }).await;
    let (line, ran) = match done {
        Err(error) => {
            tracing::error!(code = "sidecar_prune_interrupted", panic = error.is_panic(), %error, "rodada da poda interrompida");
            (json!({"nivel": "erro", "codigo": "sidecar_prune_interrupted", "panico": error.is_panic()}), false)
        }
        Ok((swept, catalog)) => verdict(swept, catalog),
    };
    if ran {
        tracing::info!(line = %line, "poda dos sidecars de sessão morta");
    } else {
        tracing::warn!(line = %line, "poda dos sidecars pulada; nada apagado, nova tentativa em breve");
    }
    let mut line = line;
    line["evento"] = json!("rust.sidecar_prune");
    state.diag.record_line(line);
    ran
}

/// Pastas de conta Claude do catálogo e a base do servidor; catálogo ilegível fica só com a base.
fn bases(accounts: &AccountService) -> (Vec<PathBuf>, bool) {
    let base = accounts.env.claude_base.clone();
    match accounts.claude_catalog() {
        Ok(rows) => (rows.as_array().into_iter().flatten()
            .filter_map(|row| row["path"].as_str().map(PathBuf::from)).chain([base]).collect(), true),
        Err(error) => {
            tracing::warn!(code = %error.code, detail = %error.message, "catálogo de contas ilegível; a poda fica só com a pasta base");
            (vec![base], false)
        }
    }
}

/// Rodada sem descoberta, com pane descartado ou com registro ilegível tenta de novo logo; sem
/// sessão nenhuma (conjunto vazio de verdade) espera o dia, como a que deu certo.
fn verdict(swept: Result<(Report, Option<&'static str>), &'static str>, catalog: bool) -> (serde_json::Value, bool) {
    match swept {
        Err(code) => (json!({"nivel": "erro", "codigo": code}), false),
        Ok((report, degraded)) => {
            let mut line = summary(&report, catalog);
            if let Some(code) = degraded { line["codigo"] = json!(code); }
            // Catálogo ilegível varreu só a base: as pastas de conta esperam a nova tentativa, não o dia.
            (line, degraded.is_none() && catalog)
        }
    }
}

/// Descoberta que falhou pula a rodada inteira: sem ela nem a sobra de tmp sai. O `Option` é o
/// motivo de uma rodada que rodou sem saber quem estava vivo.
fn sweep(discovered: Result<Discovered, ListError>, recorded: io::Result<Recorded>, bases: &[PathBuf], now: SystemTime)
    -> Result<(Report, Option<&'static str>), &'static str> {
    let (rows, panes, partial) = discovered.map_err(|error| error.code)?;
    let (live, degraded) = live(&rows, &panes, partial, recorded);
    Ok((hangar_sidecar_prune::prune(bases, &live, now), degraded))
}

fn live(rows: &[SessionRow], panes: &[Pane], partial: bool, recorded: io::Result<Recorded>) -> (Live, Option<&'static str>) {
    // Pane descartado na leitura: a linha dele sumiu da rodada, então nomes e chaves também estão incompletos.
    if partial { return (Live::default(), Some("list_panes_partial")); }
    let panes = panes.iter().map(|pane| pane.pane_id.trim_start_matches('%').to_owned()).filter(|id| !id.is_empty()).collect();
    let mut recorded = match recorded {
        Ok(recorded) => recorded,
        Err(error) => {
            tracing::warn!(kind = ?error.kind(), "registro das sessões ilegível; famílias por chave e nome ficam nesta rodada");
            return (Live { stems: None, names: None, panes: Some(panes), keys: None }, Some("session_registry_unreadable"));
        }
    };
    recorded.stems.extend(rows.iter().filter_map(|row| row.jsonl.as_deref()).filter(|jsonl| !jsonl.is_empty())
        .map(crate::locate::session_key));
    recorded.names.extend(rows.iter().map(|row| crate::session_rename::files::stem(&row.name)));
    (Live { stems: Some(recorded.stems), names: Some(recorded.names), panes: Some(panes), keys: Some(recorded.keys) }, None)
}

/// Sessão na fotografia do restore ainda pode voltar depois do reboot: conta como viva. O spec não
/// conta: ele não sai quando a sessão fecha e protegeria todo nome que já existiu.
fn recorded(home: &Path) -> io::Result<Recorded> {
    use crate::session_open::restore;
    let stem = crate::session_rename::files::stem;
    let mut out = Recorded::default();
    let raw = match std::fs::read(home.join(".hangar").join(restore::FILE)) {
        Ok(raw) => raw,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(out),
        Err(error) => return Err(error),
    };
    let snapshot: restore::Snapshot = serde_json::from_slice(&raw).map_err(io::Error::other)?;
    for (key, entry) in snapshot.sessions {
        out.keys.insert(key);
        out.names.insert(stem(&entry.name));
        out.stems.extend(entry.conversation);
    }
    Ok(out)
}

/// Só escalares: o diário descarta lista e objeto.
fn summary(report: &Report, catalog: bool) -> serde_json::Value {
    let removed: Vec<String> = report.removed.iter().map(|(dir, n)| format!("{dir}={n}")).collect();
    let skipped: Vec<&str> = report.skipped.iter().map(|family| family.as_str()).collect();
    let clean = catalog && report.skipped.is_empty() && report.failed == 0 && report.outside == 0;
    let mut line = json!({"nivel": if clean { "ok" } else { "aviso" }, "total": report.total(), "apagados": removed.join(", "),
        "pulados": skipped.join(", "), "fora_das_bases": report.outside, "falhas": report.failed});
    if !catalog { line["codigo"] = json!("accounts_catalog_failed"); }
    line
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::{Duration, UNIX_EPOCH};

    fn now() -> SystemTime { UNIX_EPOCH + Duration::from_secs(1_800_000_000) }

    fn old_file(dir: &Path, name: &str) -> PathBuf {
        fs::create_dir_all(dir).unwrap();
        let file = dir.join(name);
        fs::write(&file, "{}").unwrap();
        fs::File::options().write(true).open(&file).unwrap().set_modified(now() - Duration::from_secs(30 * 86_400)).unwrap();
        file
    }

    fn row(name: &str, jsonl: Option<&str>) -> SessionRow {
        serde_json::from_value(json!({"name": name, "jsonl": jsonl})).unwrap()
    }

    fn pane(id: &str) -> Pane { Pane { pane_id: id.into(), ..Pane::default() } }

    fn set(items: &[&str]) -> Option<HashSet<String>> { Some(items.iter().map(|s| (*s).to_owned()).collect()) }

    #[tokio::test(start_paused = true)]
    async fn the_first_round_waits_for_readiness() {
        let readiness = Readiness::pending();
        let calls = Arc::new(AtomicUsize::new(0));
        let counted = calls.clone();
        let task = tokio::spawn(run_after(readiness.clone(), move || {
            counted.fetch_add(1, Ordering::SeqCst);
            async { true }
        }));
        tokio::time::sleep(Duration::from_secs(3_600)).await;
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        readiness.finish();
        tokio::time::sleep(Duration::from_secs(1)).await;
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        task.abort();
    }

    #[test]
    fn r6_09_a_failed_discovery_skips_the_round_and_deletes_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let base = dir.path().join(".claude");
        let dead = old_file(&base.join(".hangar-status"), "morta.json");
        let tmp = old_file(&base.join(".hangar-preview"), "a.json.tmp");
        let marker = old_file(&base.join(".hangar-active"), "x.json");
        let failed = Err(ListError { code: "list_bridge_unavailable", detail: "descoberta indisponível" });
        assert_eq!(sweep(failed, Ok(Recorded::default()), &[base], now()), Err("list_bridge_unavailable"));
        assert!(dead.exists() && tmp.exists() && marker.exists());
    }

    #[test]
    fn live_comes_from_the_rows_and_panes_of_one_round_and_from_the_session_records() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path();
        let rows = [row("minha/sessao", Some("/claude/projects/x/aaa.jsonl")),
            row("kimi", Some("/kimi/sessions/wd/vivo-kimi/agents/main/wire.jsonl")), row("sem-transcript", None)];
        let panes = [pane("%42"), pane("")];
        let key = "0123456789abcdef0123456789abcdef";
        let specs = home.join(".hangar").join("session-specs");
        fs::create_dir_all(&specs).unwrap();
        let spec = json!({"version": 1, "key": key, "name": "a voltar", "cwd": "/x", "provider": "pi", "surface": "terminal",
            "account": {"kind": "default"}, "account_source": "default", "model": null, "effort": null, "service_tier": null,
            "permission_mode": null, "subagent_model": null, "customizations": null, "jev": false, "read_only": false,
            "worktree": null, "creator": null, "created_at": 1.0, "conversation": "conversa-do-spec"});
        fs::write(specs.join(format!("{key}.json")), spec.to_string()).unwrap();
        fs::write(specs.join("nao-e-chave.json"), "{").unwrap();
        fs::write(home.join(".hangar").join("session-restore.json"), json!({"sessions": {"fedcba9876543210fedcba9876543210":
            {"name": "restaurar", "provider": "claude", "conversation": "conversa-do-restore"}}}).to_string()).unwrap();
        // Só a fotografia do restore conta; o spec, mesmo com conversa, fica de fora.
        let (live, degraded) = live(&rows, &panes, false, recorded(home));
        assert_eq!(degraded, None);
        assert_eq!(live.stems, set(&["aaa", "vivo-kimi", "conversa-do-restore"]));
        assert_eq!(live.names, set(&["minha-sessao", "kimi", "sem-transcript", "restaurar"]));
        assert_eq!(live.panes, set(&["42"]));
        assert_eq!(live.keys, set(&["fedcba9876543210fedcba9876543210"]));
    }

    /// Catálogo ilegível varreu só a base: a rodada tenta de novo logo, mesmo com a descoberta boa.
    #[test]
    fn an_unreadable_catalog_retries_soon() {
        let (line, ran) = verdict(Ok((Report::default(), None)), false);
        assert!(!ran);
        assert_eq!(line["codigo"], "accounts_catalog_failed");
    }

    #[test]
    fn a_dropped_pane_makes_every_live_set_unknown_and_the_round_retries_soon() {
        let (live, degraded) = live(&[row("s", Some("/p/aaa.jsonl"))], &[pane("%1")], true, Ok(Recorded::default()));
        assert_eq!((live.stems, live.names, live.panes, live.keys), (None, None, None, None));
        assert_eq!(degraded, Some("list_panes_partial"));
        let (line, ran) = verdict(Ok((Report::default(), degraded)), true);
        assert!(!ran);
        assert_eq!(line["codigo"], "list_panes_partial");
    }

    #[test]
    fn an_unreadable_restore_record_keeps_the_key_and_name_families_and_the_round_retries_soon() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join(".hangar")).unwrap();
        fs::write(dir.path().join(".hangar").join("session-restore.json"), "{").unwrap();
        let (live, degraded) = live(&[row("s", Some("/p/aaa.jsonl"))], &[pane("%1")], false, recorded(dir.path()));
        assert_eq!((live.stems, live.names, live.panes, live.keys), (None, None, set(&["1"]), None));
        assert_eq!(degraded, Some("session_registry_unreadable"));
        assert!(!verdict(Ok((Report::default(), degraded)), true).1);
    }

    /// Catálogo ilegível (HOME que é arquivo): a poda fica só com a base e o log diz o código e o detalhe.
    #[test]
    fn an_unreadable_catalog_keeps_the_base_and_logs_why() {
        #[derive(Clone)] struct Capture(Arc<std::sync::Mutex<Vec<u8>>>);
        impl std::io::Write for Capture {
            fn write(&mut self, data: &[u8]) -> io::Result<usize> { self.0.lock().unwrap().extend_from_slice(data); Ok(data.len()) }
            fn flush(&mut self) -> io::Result<()> { Ok(()) }
        }
        let output = Arc::new(std::sync::Mutex::new(Vec::new()));
        let capture = Capture(output.clone());
        let subscriber = tracing_subscriber::fmt().without_time().with_ansi(false).with_writer(move || capture.clone()).finish();
        let _log = tracing::subscriber::set_default(subscriber);
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join("arquivo");
        fs::write(&home, "").unwrap();
        let home = home.to_string_lossy().into_owned();
        let accounts = AccountService::new(crate::accounts::environment::AccountEnvironment::from_map(
            [("HOME".into(), home.clone()), ("USERPROFILE".into(), home)].into()));
        let (found, catalog) = bases(&accounts);
        assert!(!catalog);
        assert_eq!(found, vec![accounts.env.claude_base.clone()]);
        let text = String::from_utf8(output.lock().unwrap().clone()).unwrap();
        assert!(text.contains("code=account_storage_unavailable") && text.contains("detail="), "{text}");
    }

    #[test]
    fn an_idle_machine_with_no_session_waits_the_day() {
        let dir = tempfile::tempdir().unwrap();
        let (live, degraded) = live(&[], &[], false, recorded(dir.path()));
        assert_eq!(degraded, None);
        let report = hangar_sidecar_prune::prune(&[dir.path().to_path_buf()], &live, now());
        let (line, ran) = verdict(Ok((report, degraded)), true);
        assert!(ran);
        // Conjuntos vazios de verdade: famílias puladas, mas a rodada aconteceu.
        assert_eq!(line["pulados"], "stems, names, tickets");
        assert!(!verdict(Err("list_bridge_unavailable"), true).1);
    }

    /// R6-02: com o catálogo bom, as bases são as pastas de conta com login e a base do servidor; uma
    /// `~/.claude-*` sem login (`~/.claude-pocket-uploads`) fica de fora.
    #[test]
    fn r6_02_the_bases_are_the_logged_in_accounts_and_the_base() {
        let dir = tempfile::tempdir().unwrap();
        let home = fs::canonicalize(dir.path()).unwrap();
        let account = home.join(".claude-conta");
        fs::create_dir_all(account.join("projects")).unwrap();
        fs::write(account.join(".credentials.json"), "{}").unwrap();
        fs::create_dir_all(home.join(".claude-pocket-uploads")).unwrap();
        fs::create_dir_all(home.join(".claude")).unwrap();
        let text = home.to_string_lossy().into_owned();
        let accounts = AccountService::new(crate::accounts::environment::AccountEnvironment::from_map(
            [("HOME".into(), text.clone()), ("USERPROFILE".into(), text)].into()));
        let (found, catalog) = bases(&accounts);
        assert!(catalog);
        let found: HashSet<PathBuf> = found.iter().map(|path| fs::canonicalize(path).unwrap()).collect();
        assert_eq!(found, HashSet::from([account, home.join(".claude")]), "sem a pasta sem login");
    }

    /// Estado do servidor com a conta em `home`, a lista pelo multiplexador `mux` e o diário em `diary`.
    fn state_for(home: &Path, mux: &std::ffi::OsStr, diary: &Path) -> AppState {
        use crate::list::bridge::{ListBridge, ListEnv, parse_dirs};
        let cfg = crate::config::Config { listen: "127.0.0.1:0".parse().unwrap(), upstream: "127.0.0.1:9".parse().unwrap(),
            internal_secret: "s".into(), auth_token: "dono".into(), log_path: None, trusted: crate::auth::TrustedHosts::parse("127.0.0.1") };
        let mut state = AppState::new(cfg);
        let text = home.to_string_lossy().into_owned();
        state.accounts = AccountService::new(crate::accounts::environment::AccountEnvironment::from_map(
            [("HOME".into(), text.clone()), ("USERPROFILE".into(), text)].into()));
        let dirs = parse_dirs(&json!({"home": home, "claude": home.join(".claude"), "codex_home": home.join(".codex"),
            "pi_sessions": home.join(".pi/agent/sessions"), "omp_config": home.join(".omp"), "omp_agent": home.join(".omp/agent"),
            "kimi_home": home.join(".kimi-code")}).to_string());
        state.list = Arc::new(ListBridge::new(ListEnv { mux: crate::list::mux::Mux::with_program(mux, Duration::from_secs(5)),
            capture_program: mux.to_owned(), procs: Arc::new(crate::list::procs::SystemProcs::default()), dirs },
            crate::list::facts::FactsClient::new("127.0.0.1:9".parse().unwrap(), "s".into())));
        state.diag = state.diag.clone().with_diary(Some(diary.to_owned()));
        state
    }

    /// As linhas da poda que a rodada gravou no diário.
    async fn prune_lines(diag: &crate::diag::DiagClient, diary: &Path) -> Vec<serde_json::Value> {
        diag.flush().await;
        crate::diag::test_support::rows(diary).into_iter().filter(|line| line["evento"] == "rust.sidecar_prune").collect()
    }

    /// R6-10 (decisão da rodada 3): o multiplexador que não responde pula a rodada inteira: nada é
    /// apagado (nem a sobra de tmp), a rodada diz que não aconteceu (nova tentativa em `RETRY`, não em
    /// um dia) e o diário leva `mux_unavailable`.
    #[tokio::test]
    async fn r6_10_a_mux_that_does_not_answer_skips_the_whole_round_and_retries_soon() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join("home");
        let dead = old_file(&home.join(".claude/.hangar-status"), "morta.json");
        let tmp = home.join(".claude/.hangar-status/a.json.tmp");
        fs::write(&tmp, "x").unwrap();
        fs::File::options().write(true).open(&tmp).unwrap().set_modified(SystemTime::now() - Duration::from_secs(2 * 3_600)).unwrap();
        let diary = dir.path().join("diario");
        let state = state_for(&home, dir.path().join("nao-existe").as_os_str(), &diary);
        let diag = state.diag.clone();
        assert!(!round(Arc::new(state)).await, "rodada pulada: tenta de novo em breve");
        assert!(dead.exists() && tmp.exists(), "nada apagado");
        let lines = prune_lines(&diag, &diary).await;
        assert_eq!(lines.iter().map(|line| (line["nivel"].clone(), line["codigo"].clone())).collect::<Vec<_>>(),
            [(json!("erro"), json!("mux_unavailable"))], "{lines:?}");
    }

    /// R6-15: cada rodada grava no diário `evento: "rust.sidecar_prune"` com `total`, `apagados`, `pulados`
    /// e `nivel`; a que não rodou leva também o `codigo`.
    #[cfg(unix)]
    #[tokio::test]
    async fn r6_15_each_round_writes_its_counts_to_the_diary() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join("home");
        let tmp = home.join(".claude/.hangar-status/a.json.tmp");
        fs::create_dir_all(tmp.parent().unwrap()).unwrap();
        fs::write(&tmp, "x").unwrap();
        fs::File::options().write(true).open(&tmp).unwrap().set_modified(SystemTime::now() - Duration::from_secs(2 * 3_600)).unwrap();
        let diary = dir.path().join("diario");
        // `true` responde ao `list-panes` com zero panes: a rodada acontece sem sessão viva.
        let state = state_for(&home, std::ffi::OsStr::new("true"), &diary);
        let diag = state.diag.clone();
        assert!(round(Arc::new(state)).await);
        assert!(!tmp.exists(), "a sobra de tmp saiu");
        let lines = prune_lines(&diag, &diary).await;
        let [line] = lines.as_slice() else { panic!("uma linha por rodada: {lines:?}") };
        assert_eq!((&line["nivel"], &line["total"], &line["pulados"]), (&json!("aviso"), &json!(1), &json!("stems, names, tickets")), "{line}");
        assert!(line["apagados"].as_str().is_some_and(|text| text.contains(".tmp") && text.ends_with("=1")), "{line}");
        let failed = dir.path().join("diario-falha");
        let state = state_for(&home, dir.path().join("nao-existe").as_os_str(), &failed);
        let diag = state.diag.clone();
        assert!(!round(Arc::new(state)).await);
        assert_eq!(prune_lines(&diag, &failed).await[0]["codigo"], "mux_unavailable");
    }
}
