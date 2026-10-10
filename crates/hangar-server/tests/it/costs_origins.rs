use hangar_server::costs::origins;
use std::{path::Path, sync::{Arc, atomic::{AtomicU64, Ordering}}, time::{Duration, Instant}};

fn skill(path: &Path) { std::fs::create_dir_all(path).unwrap(); std::fs::write(path.join("SKILL.md"), "# Skill de teste").unwrap(); }

#[test]
fn roots_are_sorted_and_first_discovery_wins() {
    let d = tempfile::tempdir().unwrap(); let home = d.path().join("home"); let repo = d.path().join("repo");
    skill(&home.join(".claude/skills/shared")); skill(&home.join(".claude/skills/z-last"));
    skill(&repo.join("skills/shared")); skill(&repo.join("skills/repository"));
    skill(&home.join(".agents/skills/standalone"));
    skill(&home.join(".codex/plugins/cache/market/superpowers/1/skills/brainstorming"));
    skill(&home.join(".claude/plugins/marketplaces/pony-marketplace/skills/pony"));
    skill(&home.join(".claude/plugins/cache/ignored/no-skills"));
    let found = origins::scan(&home, &repo);
    assert_eq!(found.keys().map(String::as_str).collect::<Vec<_>>(), ["shared", "z-last", "repository", "standalone", "brainstorming", "pony"]);
    assert_eq!(found["shared"], "@pessoal"); assert_eq!(found["repository"], "@repo");
    assert_eq!(found["standalone"], "@avulsa"); assert_eq!(found["brainstorming"], "superpowers"); assert_eq!(found["pony"], "pony");
}

#[cfg(unix)]
#[test]
fn repository_alias_is_compared_with_the_resolved_skill_path() {
    use std::os::unix::fs::symlink;
    let directory = tempfile::tempdir().unwrap();
    let real = directory.path().join("real");
    let alias = directory.path().join("alias");
    let home = real.join("home");
    let repo = real.join("repo");
    skill(&home.join(".claude/skills/shared"));
    skill(&repo.join("skills/shared"));
    skill(&repo.join("skills/repository"));
    symlink(repo.join("skills/repository"), home.join(".claude/skills/repo-link")).unwrap();
    symlink(&real, &alias).unwrap();

    let expected = origins::scan(&home, &repo);
    let found = origins::scan(&alias.join("home"), &alias.join("repo"));
    assert_eq!(found, expected);
    assert_eq!(found["shared"], "@pessoal");
    assert_eq!(found["repository"], "@repo");
    assert_eq!(found["repo-link"], "@repo");
}

#[cfg(unix)]
#[test]
fn symlink_origin_is_resolved_and_agents_alias_keeps_python_precedence() {
    use std::os::unix::fs::symlink;
    let d = tempfile::tempdir().unwrap(); let home = d.path().join("home"); let repo = d.path().join("repo");
    skill(&repo.join("skills/repo-skill")); skill(&home.join(".claude/plugins/cache/mkt/plug/1/skills/plug-skill"));
    let roots = home.join(".claude/skills"); std::fs::create_dir_all(&roots).unwrap();
    symlink(repo.join("skills/repo-skill"), roots.join("repo-link")).unwrap();
    symlink(home.join(".claude/plugins/cache/mkt/plug/1/skills/plug-skill"), roots.join("plug-link")).unwrap();
    skill(&roots.join("personal")); std::fs::create_dir_all(home.join(".agents")).unwrap();
    symlink(&roots, home.join(".agents/skills")).unwrap();
    let found = origins::scan(&home, &repo);
    assert_eq!(found["repo-link"], "@repo"); assert_eq!(found["plug-link"], "plug"); assert_eq!(found["personal"], "@avulsa");
}

#[cfg(unix)]
#[test]
fn resolved_skill_name_prefix_counts_only_inside_a_skills_path() {
    use std::os::unix::fs::symlink;
    let d = tempfile::tempdir().unwrap(); let home = d.path().join("home"); let repo = d.path().join("repo");
    let root = home.join(".claude/skills"); std::fs::create_dir_all(&root).unwrap();
    skill(&root.join("custom:tool"));
    let outside = home.join("plugins/cache/mkt/ignored/1/folder"); skill(&outside);
    symlink(&outside, root.join("outside-link")).unwrap();
    let found = origins::scan(&home, &repo);
    assert_eq!(found["custom:tool"], "custom");
    assert_eq!(found["outside-link"], "@pessoal");
}

#[test]
fn first_call_waits_and_refresh_changes_generation_only_when_map_changes() {
    let d = tempfile::tempdir().unwrap(); let home = d.path().join("home"); let repo = d.path().join("repo");
    skill(&home.join(".claude/skills/one"));
    let clock = Arc::new(AtomicU64::new(1)); let c = clock.clone();
    let cache = origins::Origins::with_clock(home.clone(), repo, Arc::new(move || c.load(Ordering::SeqCst)));
    let (generation, map) = cache.recent(); assert_eq!(map["one"], "@pessoal");
    skill(&home.join(".claude/skills/two"));
    clock.store(30_000_000_001, Ordering::SeqCst);
    assert_eq!(cache.recent().0, generation);
    assert!(!cache.recent().1.contains_key("two"));
    clock.store(31_000_000_001, Ordering::SeqCst);
    let deadline = Instant::now() + Duration::from_secs(5);
    let changed = loop { let r = cache.recent(); if r.1.contains_key("two") { break r.0; } assert!(Instant::now() < deadline); std::thread::yield_now(); };
    assert!(changed > generation);
    std::fs::write(home.join(".claude/skills/one/other.md"), "Documento de teste").unwrap();
    clock.store(62_000_000_001, Ordering::SeqCst);
    assert_eq!(cache.recent().0, changed);
    let deadline = Instant::now() + Duration::from_millis(100);
    while Instant::now() < deadline { assert_eq!(cache.recent().0, changed); std::thread::yield_now(); }
}

/// Muda a data de uma pasta; o Windows só abre pasta com acesso de atributos e semântica de backup.
fn set_mtime(dir: &Path, time: std::time::SystemTime) {
    let mut options = std::fs::OpenOptions::new();
    #[cfg(unix)] options.read(true);
    #[cfg(windows)] { use std::os::windows::fs::OpenOptionsExt; options.access_mode(0x100).custom_flags(0x0200_0000); }
    options.open(dir).unwrap().set_modified(time).unwrap();
}

/// Skill nova no mesmo degrau de relógio do sistema de arquivos em que a pasta foi varrida (no Windows o
/// degrau é de 1 a 16 ms): a data da pasta não muda, e a verificação seguinte ainda tem de achá-la.
#[test]
fn change_in_the_same_timestamp_tick_as_the_scan_is_seen() {
    let d = tempfile::tempdir().unwrap(); let home = d.path().join("home"); let root = home.join(".claude/skills");
    skill(&root.join("one"));
    let clock = Arc::new(AtomicU64::new(1)); let c = clock.clone();
    let cache = origins::Origins::with_clock(home.clone(), d.path().join("repo"), Arc::new(move || c.load(Ordering::SeqCst)));
    let (generation, _) = cache.recent();
    let tick = std::fs::metadata(&root).unwrap().modified().unwrap();
    skill(&root.join("two"));
    set_mtime(&root, tick);
    clock.store(31_000_000_001, Ordering::SeqCst);
    let deadline = Instant::now() + Duration::from_secs(5);
    let changed = loop {
        let r = cache.recent(); if r.1.contains_key("two") { break r.0; }
        assert!(Instant::now() < deadline, "a skill nova no mesmo degrau não apareceu"); std::thread::yield_now();
    };
    assert!(changed > generation);
}

#[test]
fn missing_roots_initialize_an_empty_snapshot_shared_by_concurrent_calls() {
    let d = tempfile::tempdir().unwrap();
    let cache = origins::Origins::new(d.path().join("home"), d.path().join("repo"));
    let gate = Arc::new(std::sync::Barrier::new(8));
    let workers: Vec<_> = (0..8).map(|_| {
        let cache = cache.clone(); let gate = gate.clone();
        std::thread::spawn(move || { gate.wait(); cache.recent() })
    }).collect();
    let snapshots: Vec<_> = workers.into_iter().map(|w| w.join().unwrap()).collect();
    assert!(snapshots[0].0 > 0); assert!(snapshots[0].1.is_empty());
    assert!(snapshots.iter().all(|s| s == &snapshots[0]));
}

#[test]
fn default_generation_keeps_monotonic_time_when_an_origin_cache_is_recreated() {
    let d = tempfile::tempdir().unwrap();
    let first = origins::Origins::new(d.path().join("home"), d.path().join("repo"));
    std::thread::sleep(Duration::from_millis(20));
    let before = first.recent().0;
    let second = origins::Origins::new(d.path().join("home"), d.path().join("repo"));
    assert!(second.recent().0 > before);
}
