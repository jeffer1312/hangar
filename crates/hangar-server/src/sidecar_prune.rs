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

/// Ligado, todo spec de `~/.hangar/session-specs` conta como sessão viva; desligado, só a fotografia
/// do restore. O spec não sai quando a sessão fecha: ligado, protege todo nome que já existiu.
const SPECS_COUNT_AS_LIVE: bool = false;

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
    let recorded = match tokio::task::spawn_blocking(move || recorded(&home, SPECS_COUNT_AS_LIVE)).await {
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
            (line, degraded.is_none())
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

/// Sessão na fotografia do restore (e, com `specs`, com spec gravado) ainda pode voltar depois do
/// reboot: conta como viva.
fn recorded(home: &Path, specs: bool) -> io::Result<Recorded> {
    use crate::session_open::{record, restore};
    let stem = crate::session_rename::files::stem;
    let mut out = Recorded::default();
    let listing = if specs { Some(std::fs::read_dir(record::dir(home))) } else { None };
    match listing {
        None => {}
        Some(Err(error)) if error.kind() == io::ErrorKind::NotFound => {}
        Some(Err(error)) => return Err(error),
        Some(Ok(entries)) => for entry in entries {
            let file = entry?.file_name();
            // Só a chave que o Hangar grava (hex de 32); `.switching` e sobra de tmp ficam de fora.
            let Some(key) = file.to_str().and_then(|n| n.strip_suffix(".json"))
                .filter(|k| k.len() == 32 && k.bytes().all(|b| b.is_ascii_hexdigit())) else { continue };
            let Some(spec) = record::read(home, key)? else { continue };
            out.keys.insert(spec.key);
            out.names.insert(stem(&spec.name));
            out.stems.extend(spec.conversation);
        },
    }
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
        // Com o interruptor desligado, só a fotografia do restore conta; o spec fica de fora.
        let (live, degraded) = live(&rows, &panes, false, recorded(home, false));
        assert_eq!(degraded, None);
        assert_eq!(live.stems, set(&["aaa", "vivo-kimi", "conversa-do-restore"]));
        assert_eq!(live.names, set(&["minha-sessao", "kimi", "sem-transcript", "restaurar"]));
        assert_eq!(live.panes, set(&["42"]));
        assert_eq!(live.keys, set(&["fedcba9876543210fedcba9876543210"]));
        // Ligado, o spec conta. Se o formato dele mudar, este teste mostra: spec ilegível é registro ilegível.
        let (live, _) = live_of(&rows, &panes, recorded(home, true));
        assert_eq!(live.stems, set(&["aaa", "vivo-kimi", "conversa-do-spec", "conversa-do-restore"]));
        assert_eq!(live.names, set(&["minha-sessao", "kimi", "sem-transcript", "a-voltar", "restaurar"]));
        assert_eq!(live.keys, set(&[key, "fedcba9876543210fedcba9876543210"]));
    }

    fn live_of(rows: &[SessionRow], panes: &[Pane], recorded: io::Result<Recorded>) -> (Live, Option<&'static str>) {
        live(rows, panes, false, recorded)
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
        let (live, degraded) = live(&[row("s", Some("/p/aaa.jsonl"))], &[pane("%1")], false, recorded(dir.path(), false));
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
        let (live, degraded) = live(&[], &[], false, recorded(dir.path(), false));
        assert_eq!(degraded, None);
        let report = hangar_sidecar_prune::prune(&[dir.path().to_path_buf()], &live, now());
        let (line, ran) = verdict(Ok((report, degraded)), true);
        assert!(ran);
        // Conjuntos vazios de verdade: famílias puladas, mas a rodada aconteceu.
        assert_eq!(line["pulados"], "stems, names, tickets");
        assert!(!verdict(Err("list_bridge_unavailable"), true).1);
    }
}
