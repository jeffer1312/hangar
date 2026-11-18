//! Um teste por comportamento do inventário R6 (`R6-xx`), com o esperado dos testes de
//! `backend/tests/test_prune.py` (c22f75dc8) ou da decisão 10, escrito literal.
use std::collections::{BTreeMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use hangar_sidecar_prune::{Family, Live, MIN_AGE, Report, TMP_MIN_AGE, prune};

fn now() -> SystemTime { UNIX_EPOCH + Duration::from_secs(1_800_000_000) }

fn days(n: f64) -> Duration { Duration::from_secs_f64(n * 86_400.0) }

fn hours(n: u64) -> Duration { Duration::from_secs(n * 3_600) }

fn write_at(base: &Path, sub: &str, name: &str, age: Duration, body: &str) -> PathBuf {
    let dir = sub.split('/').fold(base.to_path_buf(), |p, c| p.join(c));
    fs::create_dir_all(&dir).unwrap();
    let file = dir.join(name);
    fs::write(&file, body).unwrap();
    age_file(&file, age);
    file
}

fn write(base: &Path, sub: &str, name: &str, age: Duration) -> PathBuf { write_at(base, sub, name, age, r#"{"x": 1}"#) }

fn age_file(file: &Path, age: Duration) {
    fs::File::options().write(true).open(file).unwrap().set_modified(now() - age).unwrap();
}

fn set(items: &[&str]) -> Option<HashSet<String>> { Some(items.iter().map(|s| (*s).to_owned()).collect()) }

/// Todos os conjuntos conhecidos, com chave de sessão vazia (nenhum registro em disco).
fn live(stems: &[&str], names: &[&str], panes: &[&str]) -> Live {
    Live { stems: set(stems), names: set(names), panes: set(panes), keys: set(&[]) }
}

fn removed(pairs: &[(&str, usize)]) -> BTreeMap<String, usize> { pairs.iter().map(|(k, n)| ((*k).to_owned(), *n)).collect() }

fn base(dir: &tempfile::TempDir) -> PathBuf { dir.path().join("cfg") }

fn bases(base: &Path) -> Vec<PathBuf> {
    fs::create_dir_all(base).unwrap();
    vec![base.to_path_buf()]
}

/// Processo que já saiu: o pid dele não existe mais.
fn dead_pid() -> u32 {
    let mut child = if cfg!(windows) {
        std::process::Command::new("cmd").args(["/C", "exit"]).spawn().unwrap()
    } else {
        std::process::Command::new("true").spawn().unwrap()
    };
    let pid = child.id();
    child.wait().unwrap();
    pid
}

#[test]
fn r6_02_a_base_listed_twice_is_swept_once_and_a_missing_base_is_not_a_failure() {
    let dir = tempfile::tempdir().unwrap();
    let base = base(&dir);
    write(&base, ".hangar-state", "morto.json", days(8.0));
    let twice = vec![base.clone(), base.join("."), dir.path().join("nao-existe")];
    fs::create_dir_all(&base).unwrap();
    let report = prune(&twice, &live(&["viva"], &["sessao"], &["1"]), now());
    assert_eq!(report, Report { removed: removed(&[(".hangar-state", 1)]), ..Report::default() });
}

#[test]
fn r6_03_session_key_families_go_by_the_stem_of_the_json_on_the_first_level() {
    let dir = tempfile::tempdir().unwrap();
    let base = base(&dir);
    for sub in [".hangar-status", ".hangar-state", ".hangar-preview", ".hangar-askq", ".hangar-pi/models"] {
        write(&base, sub, "morto-stem.json", days(8.0));
        write(&base, sub, "vivo-kimi.json", days(60.0));
    }
    // Só `*.json` do primeiro nível: outro sufixo e subpasta ficam.
    let other = write(&base, ".hangar-status", "morto-stem.jsonl", days(30.0));
    let nested = write(&base, ".hangar-status/sub", "morto-stem.json", days(30.0));
    // A chave do Kimi é a pasta da sessão (o servidor passa `session_key`), não `wire`.
    let report = prune(&bases(&base), &live(&["vivo-kimi"], &["outra-sessao"], &["1"]), now());
    assert_eq!(report.removed, removed(&[(".hangar-askq", 1), (".hangar-pi/models", 1), (".hangar-preview", 1), (".hangar-state", 1), (".hangar-status", 1)]));
    assert!(base.join(".hangar-state").join("vivo-kimi.json").exists());
    assert!(!base.join(".hangar-state").join("morto-stem.json").exists());
    assert!(other.exists() && nested.exists());
}

/// O `.hangar-prompt` do `state-hook` sai pela mesma regra das famílias por chave; a sobra do
/// temporário com pid, pela idade.
#[test]
fn h87_the_prompt_marker_is_pruned_by_key_and_its_tmp_by_age() {
    let dir = tempfile::tempdir().unwrap();
    let base = base(&dir);
    let dead = write(&base, ".hangar-prompt", "morto-stem.json", days(8.0));
    let young = write(&base, ".hangar-prompt", "morto-novo.json", days(1.0));
    let alive = write(&base, ".hangar-prompt", "vivo-kimi.json", days(60.0));
    let tmp = write(&base, ".hangar-prompt", "morto-stem.json.4242.tmp", hours(2));
    let report = prune(&bases(&base), &live(&["vivo-kimi"], &["outra-sessao"], &["1"]), now());
    assert_eq!(report.removed, removed(&[(".hangar-prompt", 1), (".hangar-prompt (.tmp)", 1)]));
    assert!(!dead.exists() && !tmp.exists());
    assert!(young.exists() && alive.exists());
}

#[test]
fn r6_04_the_queue_and_the_baton_go_by_the_sanitized_session_name() {
    let dir = tempfile::tempdir().unwrap();
    let base = base(&dir);
    let queue = write(&base, ".hangar-queue", "minha-sessao.jsonl", days(30.0));
    let dead_queue = write(&base, ".hangar-queue", "morto-nome.jsonl", days(8.0));
    let alive = write(&base, ".hangar-bastao", "sucessora-viva.md", days(30.0));
    let dead = write(&base, ".hangar-bastao", "sucessora-morta.md", days(30.0));
    // O servidor passa o nome já sanitizado: `minha/sessao` vira `minha-sessao`.
    let report = prune(&bases(&base), &live(&["aaa"], &["minha-sessao", "sucessora-viva"], &["1"]), now());
    assert_eq!(report.removed, removed(&[(".hangar-bastao", 1), (".hangar-queue", 1)]));
    assert!(queue.exists() && alive.exists());
    assert!(!dead_queue.exists() && !dead.exists());
}

#[test]
fn r6_05_a_ticket_goes_only_when_no_live_pane_name_or_session_key_owns_it() {
    let dir = tempfile::tempdir().unwrap();
    let base = base(&dir);
    let key = "0123456789abcdef0123456789abcdef";
    let by_pane = write(&base, ".hangar-pi", "42.json", days(60.0));
    let by_key = write(&base, ".hangar-pi", &format!("{key}.json"), days(60.0));
    // psmux: o bilhete do Pi e o do Kimi são pelo nome sanitizado da sessão (`minha sessão` → `minha-sess-o`).
    let by_name = write(&base, ".hangar-pi", "minha-sess-o.json", days(60.0));
    let kimi_by_name = write(&base, ".hangar-kimi", "minha-sess-o.json", days(60.0));
    let dead_pi = write(&base, ".hangar-pi", "999.json", days(8.0));
    let dead_kimi = write(&base, ".hangar-kimi", "888.json", days(8.0));
    let live = Live { stems: set(&["aaa"]), names: set(&["minha-sess-o"]), panes: set(&["42"]), keys: set(&[key]) };
    let report = prune(&bases(&base), &live, now());
    assert_eq!(report.removed, removed(&[(".hangar-kimi", 1), (".hangar-pi", 1)]));
    assert!(by_pane.exists() && by_key.exists() && by_name.exists() && kimi_by_name.exists());
    assert!(!dead_pi.exists() && !dead_kimi.exists());
}

#[test]
fn r6_06_a_live_key_never_goes_however_old() {
    let dir = tempfile::tempdir().unwrap();
    let base = base(&dir);
    write(&base, ".hangar-status", "aaa.json", days(60.0));
    write(&base, ".hangar-queue", "sessao.jsonl", days(60.0));
    write(&base, ".hangar-pi", "42.json", days(60.0));
    let report = prune(&bases(&base), &live(&["aaa"], &["sessao"], &["42"]), now());
    assert_eq!(report, Report::default());
}

#[test]
fn r6_07_a_dead_key_goes_from_seven_days_on_and_a_younger_one_stays() {
    let dir = tempfile::tempdir().unwrap();
    let base = base(&dir);
    let yesterday = write(&base, ".hangar-status", "morreu-ontem.json", days(1.0));
    let edge = write(&base, ".hangar-status", "na-borda.json", MIN_AGE);
    let almost = write(&base, ".hangar-status", "quase.json", MIN_AGE - Duration::from_micros(1));
    let report = prune(&bases(&base), &live(&["outra-viva"], &["outra-sessao"], &["1"]), now());
    assert_eq!(report.removed, removed(&[(".hangar-status", 1)]));
    assert!(yesterday.exists() && almost.exists());
    assert!(!edge.exists());
}

#[test]
fn r6_08_an_unknown_or_empty_live_set_keeps_its_whole_family() {
    let dir = tempfile::tempdir().unwrap();
    let base = base(&dir);
    let stem = write(&base, ".hangar-status", "velho-stem.json", days(30.0));
    let queue = write(&base, ".hangar-queue", "velho-fila.jsonl", days(30.0));
    let ticket = write(&base, ".hangar-pi", "9.json", days(30.0));
    let empty = Live { stems: set(&[]), names: set(&[]), panes: set(&[]), keys: set(&[]) };
    for live in [empty, Live::default()] {
        let report = prune(&bases(&base), &live, now());
        assert_eq!(report, Report { skipped: vec![Family::Stems, Family::Names, Family::Tickets], ..Report::default() });
    }
    assert!(stem.exists() && queue.exists() && ticket.exists());
}

#[test]
fn r6_10_unknown_panes_keep_only_the_tickets() {
    let dir = tempfile::tempdir().unwrap();
    let base = base(&dir);
    write(&base, ".hangar-state", "morto.json", days(8.0));
    write(&base, ".hangar-queue", "morto.jsonl", days(8.0));
    let ticket = write(&base, ".hangar-kimi", "888.json", days(8.0));
    let live = Live { stems: set(&["viva"]), names: set(&["sessao"]), panes: None, keys: set(&[]) };
    let report = prune(&bases(&base), &live, now());
    assert_eq!(report, Report { removed: removed(&[(".hangar-queue", 1), (".hangar-state", 1)]), skipped: vec![Family::Tickets], ..Report::default() });
    assert!(ticket.exists());
}

#[test]
fn r6_11_tmp_leftovers_go_by_age_alone_in_every_hangar_folder_and_one_level_below() {
    let dir = tempfile::tempdir().unwrap();
    let base = base(&dir);
    // O corte no meio, como veio do disco real.
    for name in ["a.json.tmp", "b.json.4321.tmp", "c.json.tmp.99", "d.json.tmp1234"] {
        write_at(&base, ".hangar-preview", name, hours(2), "{\"text\":");
    }
    let models = write_at(&base, ".hangar-pi/models", "aaa.json.5.tmp", hours(2), "x");
    let active = write_at(&base, ".hangar-active", "aaa.json.tmp", hours(2), "x");
    let queue_state = write_at(&base, ".hangar-queue/runtime", "k.json.tmp", hours(2), "x");
    // Escrita em voo (59 min) e um sidecar de verdade ficam.
    let in_flight = write_at(&base, ".hangar-status", "aaa.json.tmp.123", TMP_MIN_AGE - Duration::from_secs(60), "x");
    let real = write_at(&base, ".hangar-status", "aaa.jsonl", hours(2), "x");
    // Sem saber quem está vivo, a sobra de tmp sai igual: ninguém lê um `.tmp`.
    let report = prune(&bases(&base), &Live::default(), now());
    assert_eq!(report.removed, removed(&[(".hangar-active (.tmp)", 1), (".hangar-pi (.tmp)", 1), (".hangar-preview (.tmp)", 4), (".hangar-queue (.tmp)", 1)]));
    assert_eq!(fs::read_dir(base.join(".hangar-preview")).unwrap().count(), 0);
    assert!(!models.exists() && !active.exists() && !queue_state.exists());
    assert!(in_flight.exists() && real.exists());
}

#[test]
fn r6_12_an_active_marker_goes_only_when_old_not_live_and_its_recorded_pid_is_dead() {
    let dir = tempfile::tempdir().unwrap();
    let base = base(&dir);
    let dead = dead_pid();
    let mine = std::process::id();
    let marker = |name: &str, pid: &str, age: Duration| write_at(&base, ".hangar-active", &format!("{name}.json"), age, &format!(r#"{{"jsonl": "/x.jsonl", "pid": {pid}}}"#));
    let old_dead = marker("velho-morto", &dead.to_string(), days(30.0));
    let no_pid = marker("sem-pid", "null", days(30.0));
    let young_dead = marker("novo-morto", &dead.to_string(), days(2.0));
    let old_alive = marker("velho-vivo", &mine.to_string(), days(30.0));
    let of_live = marker("viva", &dead.to_string(), days(30.0));
    // Sem pid de verdade não há prova de morte: `false`, `0`, `true` e JSON torto ficam.
    let falsy = marker("pid-false", "false", days(30.0));
    let zero = marker("pid-zero", "0", days(30.0));
    let truthy = marker("pid-true", "true", days(30.0));
    let broken = write_at(&base, ".hangar-active", "torto.json", days(30.0), "{\"pid\":");
    let report = prune(&bases(&base), &live(&["viva"], &["sessao"], &["1"]), now());
    assert_eq!(report.removed, removed(&[(".hangar-active", 1)]));
    assert!(!old_dead.exists());
    for kept in [no_pid, young_dead, old_alive, of_live, falsy, zero, truthy, broken] {
        assert!(kept.exists(), "{}", kept.display());
    }
}

#[test]
fn r6_13_an_item_that_vanishes_or_is_not_a_file_only_skips_itself() {
    let dir = tempfile::tempdir().unwrap();
    let base = base(&dir);
    let gone = write(&base, ".hangar-status", "sumiu.json", days(30.0));
    fs::remove_file(&gone).unwrap();
    fs::create_dir_all(base.join(".hangar-status").join("pasta.json")).unwrap();
    write(&base, ".hangar-state", "morto.json", days(30.0));
    let report = prune(&bases(&base), &live(&["outra-viva"], &["outra-sessao"], &["1"]), now());
    assert_eq!(report, Report { removed: removed(&[(".hangar-state", 1)]), ..Report::default() });
    assert!(base.join(".hangar-status").join("pasta.json").is_dir());
}

#[cfg(unix)]
#[test]
fn r6_13_an_unreadable_folder_counts_as_a_failure_and_the_others_are_still_swept() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let base = base(&dir);
    let locked = write(&base, ".hangar-status", "morto.json", days(30.0));
    let gone = write(&base, ".hangar-state", "morto.json", days(30.0));
    let folder = base.join(".hangar-status");
    fs::set_permissions(&folder, fs::Permissions::from_mode(0o000)).unwrap();
    // Como root a pasta continua legível e não há falha a provar.
    let root = fs::read_dir(&folder).is_ok();
    let report = prune(&bases(&base), &live(&["outra-viva"], &["outra-sessao"], &["1"]), now());
    fs::set_permissions(&folder, fs::Permissions::from_mode(0o755)).unwrap();
    assert!(!gone.exists());
    if !root {
        // Uma falha por passada que tentou ler a pasta: a da sobra de tmp e a da chave.
        assert_eq!(report, Report { removed: removed(&[(".hangar-state", 1)]), failed: 2, ..Report::default() });
        assert!(locked.exists());
    }
}

/// R6-13 no Windows: o arquivo aberto sem `FILE_SHARE_DELETE` (como o leitor de outro processo) não sai
/// e conta uma falha; os outros da mesma pasta saem.
#[cfg(windows)]
#[test]
fn r6_13_a_file_held_open_only_skips_itself_on_windows() {
    use std::os::windows::fs::OpenOptionsExt;
    const FILE_SHARE_READ: u32 = 0x1;
    let dir = tempfile::tempdir().unwrap();
    let base = base(&dir);
    let held = write(&base, ".hangar-status", "aberto.json", days(30.0));
    let gone = write(&base, ".hangar-status", "morto.json", days(30.0));
    let handle = fs::OpenOptions::new().read(true).share_mode(FILE_SHARE_READ).open(&held).unwrap();
    let report = prune(&bases(&base), &live(&["outra-viva"], &["outra-sessao"], &["1"]), now());
    drop(handle);
    assert_eq!(report, Report { removed: removed(&[(".hangar-status", 1)]), failed: 1, ..Report::default() });
    assert!(held.exists() && !gone.exists());
}

#[cfg(unix)]
#[test]
fn r6_14_links_inside_the_bases_are_swept_once_and_links_leaving_them_are_not_followed() {
    use std::os::unix::fs::symlink;
    let dir = tempfile::tempdir().unwrap();
    let main = dir.path().join(".claude");
    let account = dir.path().join(".claude-conta");
    let elsewhere = dir.path().join("fora");
    let dead = write(&main, ".hangar-status", "morto.json", days(30.0));
    let outside = write(&elsewhere, ".hangar-state", "morto.json", days(30.0));
    let target = write(&elsewhere, "solto", "alvo.json", days(30.0));
    fs::create_dir_all(&account).unwrap();
    // A pasta da conta que é link para a do `~/.claude` é a mesma pasta: varrida uma vez.
    symlink(main.join(".hangar-status"), account.join(".hangar-status")).unwrap();
    symlink(elsewhere.join(".hangar-state"), account.join(".hangar-state")).unwrap();
    // Arquivo que é link: sai o link, o alvo fica.
    fs::create_dir_all(main.join(".hangar-askq")).unwrap();
    symlink(&target, main.join(".hangar-askq").join("morto.json")).unwrap();
    let report = prune(&[main.clone(), account.clone()], &live(&["outra-viva"], &["outra-sessao"], &["1"]), now());
    // `.hangar-state` da conta resolve para fora das bases nas duas passadas (tmp e chave).
    assert_eq!(report, Report { removed: removed(&[(".hangar-askq", 1), (".hangar-status", 1)]), outside: 2, ..Report::default() });
    assert!(!dead.exists());
    assert!(fs::symlink_metadata(main.join(".hangar-askq").join("morto.json")).is_err());
    assert!(outside.exists() && target.exists());
}

#[test]
fn r6_15_the_report_counts_by_folder_keeps_tmp_apart_and_names_the_skipped_families() {
    let dir = tempfile::tempdir().unwrap();
    let main = dir.path().join(".claude");
    let account = dir.path().join(".claude-conta");
    write(&main, ".hangar-status", "morto.json", days(8.0));
    write(&account, ".hangar-status", "morto.json", days(8.0));
    write_at(&account, ".hangar-status", "a.json.tmp", hours(2), "x");
    let live = Live { stems: set(&["viva"]), names: None, panes: set(&["1"]), keys: set(&[]) };
    let report = prune(&[main, account], &live, now());
    assert_eq!(report, Report {
        removed: removed(&[(".hangar-status", 2), (".hangar-status (.tmp)", 1)]),
        skipped: vec![Family::Names, Family::Tickets],
        ..Report::default()
    });
    assert_eq!(report.total(), 3);
    assert_eq!(report.skipped.iter().map(|f| f.as_str()).collect::<Vec<_>>(), ["names", "tickets"]);
}

#[test]
fn r6_16_pruning_is_hygiene_seven_days_for_keys_one_hour_for_tmp() {
    assert_eq!(MIN_AGE, Duration::from_secs(604_800));
    assert_eq!(TMP_MIN_AGE, Duration::from_secs(3_600));
}

#[cfg(windows)]
#[test]
fn the_family_suffix_ignores_case_on_windows_like_the_python_glob() {
    let dir = tempfile::tempdir().unwrap();
    let base = base(&dir);
    let upper = write(&base, ".hangar-status", "MORTO.JSON", days(8.0));
    let report = prune(&bases(&base), &live(&["viva"], &["sessao"], &["1"]), now());
    assert_eq!(report.removed, removed(&[(".hangar-status", 1)]));
    assert!(!upper.exists());
}
