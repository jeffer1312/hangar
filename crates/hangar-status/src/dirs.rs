//! Tamanho do `~/.hangar` e do cache de build. Somar uma árvore grande leva segundos, então a conta
//! roda numa thread, uma por vez, e o resultado vale uma hora; quem lê recebe o último valor e
//! `computing` enquanto a próxima está em andamento.
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use hangar_api::status::DirSize;

pub const VALID_FOR: Duration = Duration::from_secs(3600);

type Measure = Arc<dyn Fn(&Path) -> Option<u64> + Send + Sync>;

/// As duas pastas do bloco "Esta máquina".
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Sizes {
    pub hangar: DirSize,
    pub build_cache: DirSize,
}

#[derive(Default)]
struct State {
    hangar: Option<u64>,
    build_cache: Option<u64>,
    measured_at: Option<Instant>,
    computing: bool,
}

#[derive(Clone)]
pub struct DirSizes {
    shared: Arc<(Mutex<State>, Condvar)>,
    hangar_root: Option<PathBuf>,
    build_root: Option<PathBuf>,
    measure: Measure,
    valid_for: Duration,
}

impl DirSizes {
    /// `~/.hangar` e o cache de build resolvido como o `cargo` (e o `scripts/cargo-pinned`) resolve.
    pub fn new() -> Self {
        let home = std::env::home_dir();
        let hangar = home.as_ref().map(|home| home.join(".hangar"));
        let build = build_cache_dir(home.as_deref(), &|key| std::env::var_os(key));
        Self::with(hangar, build, Arc::new(|path: &Path| dir_bytes(path)), VALID_FOR)
    }

    fn with(hangar_root: Option<PathBuf>, build_root: Option<PathBuf>, measure: Measure, valid_for: Duration) -> Self {
        Self { shared: Arc::default(), hangar_root, build_root, measure, valid_for }
    }

    /// Último valor conhecido. Sem medição válida e nenhuma em andamento, começa uma em segundo
    /// plano; pedidos durante a medição não abrem outra.
    pub fn current(&self) -> Sizes {
        let mut state = self.lock();
        let fresh = state.measured_at.is_some_and(|at| at.elapsed() < self.valid_for);
        if !fresh && !state.computing {
            state.computing = true;
            let this = self.clone();
            let spawned = std::thread::Builder::new().name("hangar-status-dirs".into()).spawn(move || this.run());
            if spawned.is_err() {
                state.computing = false;
            }
        }
        Sizes {
            hangar: DirSize { bytes: state.hangar, computing: state.computing },
            build_cache: DirSize { bytes: state.build_cache, computing: state.computing },
        }
    }

    fn run(&self) {
        let hangar = self.hangar_root.as_deref().and_then(|root| (self.measure)(root));
        let build_cache = self.build_root.as_deref().and_then(|root| (self.measure)(root));
        let mut state = self.lock();
        *state = State { hangar, build_cache, measured_at: Some(Instant::now()), computing: false };
        drop(state);
        self.shared.1.notify_all();
    }

    fn lock(&self) -> MutexGuard<'_, State> { self.shared.0.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) }

    #[cfg(test)]
    fn wait_idle(&self) {
        let mut state = self.lock();
        while state.computing {
            state = self.shared.1.wait(state).unwrap_or_else(|poisoned| poisoned.into_inner());
        }
    }
}

impl Default for DirSizes {
    fn default() -> Self { Self::new() }
}

/// Mesma ordem do cargo: `CARGO_TARGET_DIR`, `build.target-dir` do `config.toml` do usuário, e por
/// fim `~/.cache/cargo/target`. `None` quando a pasta não existe: o bloco fica sem o valor.
fn build_cache_dir(home: Option<&Path>, env: &dyn Fn(&str) -> Option<std::ffi::OsString>) -> Option<PathBuf> {
    let from_env = env("CARGO_TARGET_DIR").filter(|v| !v.is_empty()).map(PathBuf::from);
    let cargo_home = env("CARGO_HOME").filter(|v| !v.is_empty()).map(PathBuf::from)
        .or_else(|| home.map(|home| home.join(".cargo")));
    let from_config = || {
        let cargo_home = cargo_home.as_ref()?;
        ["config.toml", "config"].iter().find_map(|name| {
            let text = std::fs::read_to_string(cargo_home.join(name)).ok()?;
            config_target_dir(&text).map(|dir| cargo_home.parent().map_or_else(|| PathBuf::from(&dir), |base| base.join(&dir)))
        })
    };
    let fallback = || home.map(|home| home.join(".cache").join("cargo").join("target"));
    from_env.or_else(from_config).or_else(fallback).filter(|dir| dir.is_dir())
}

/// `target-dir` da seção `[build]` (ou `build.target-dir` no topo). Leitura mínima de TOML: só
/// string entre aspas, que é como o cargo documenta a chave.
fn config_target_dir(text: &str) -> Option<String> {
    let mut section = String::new();
    for line in text.lines() {
        let line = line.trim();
        if let Some(name) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
            section = name.trim().to_owned();
            continue;
        }
        let Some((key, value)) = line.split_once('=') else { continue };
        let key = key.trim();
        let hit = (section == "build" && key == "target-dir") || (section.is_empty() && key == "build.target-dir");
        if hit {
            let value = value.trim();
            let quote = value.chars().next().filter(|c| matches!(c, '"' | '\''))?;
            return value[1..].split(quote).next().map(str::to_owned).filter(|v| !v.is_empty());
        }
    }
    None
}

/// Soma dos arquivos da árvore sem seguir link (o `current` aponta para dentro do próprio
/// `~/.hangar`), contando uma vez o arquivo com vários hardlinks, como o `du`. Pasta ou arquivo
/// ilegível no meio fica de fora; raiz ilegível é `None`.
pub fn dir_bytes(root: &Path) -> Option<u64> {
    if !std::fs::symlink_metadata(root).ok()?.is_dir() {
        return None;
    }
    let mut seen = HashSet::new();
    let mut total = 0u64;
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else { continue };
        for entry in entries.flatten() {
            let Ok(meta) = entry.metadata() else { continue };
            let kind = meta.file_type();
            if kind.is_dir() {
                stack.push(entry.path());
            } else if kind.is_file() && first_link(&meta, &mut seen) {
                total = total.saturating_add(meta.len());
            }
        }
    }
    Some(total)
}

#[cfg(unix)]
fn first_link(meta: &std::fs::Metadata, seen: &mut HashSet<(u64, u64)>) -> bool {
    use std::os::unix::fs::MetadataExt;
    meta.nlink() < 2 || seen.insert((meta.dev(), meta.ino()))
}

#[cfg(not(unix))]
fn first_link(_meta: &std::fs::Metadata, _seen: &mut HashSet<(u64, u64)>) -> bool { true }

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn write(path: &Path, bytes: usize) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, vec![0u8; bytes]).unwrap();
    }

    #[test]
    fn dir_bytes_sums_the_tree() {
        let dir = tempfile::tempdir().unwrap();
        write(&dir.path().join("a.bin"), 1000);
        write(&dir.path().join("sub/b.bin"), 234);
        write(&dir.path().join("sub/deeper/c.bin"), 10);
        assert_eq!(dir_bytes(dir.path()), Some(1244));
        assert_eq!(dir_bytes(&dir.path().join("missing")), None);
        assert_eq!(dir_bytes(&dir.path().join("a.bin")), None, "arquivo não é pasta");
    }

    #[cfg(unix)]
    #[test]
    fn dir_bytes_skips_links_and_counts_hardlinks_once() {
        let dir = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        write(&dir.path().join("a.bin"), 500);
        write(&outside.path().join("big.bin"), 9000);
        std::os::unix::fs::symlink(outside.path(), dir.path().join("current")).unwrap();
        std::fs::hard_link(dir.path().join("a.bin"), dir.path().join("a-again.bin")).unwrap();
        assert_eq!(dir_bytes(dir.path()), Some(500));
    }

    #[test]
    fn second_read_within_the_hour_does_not_measure_again() {
        let dir = tempfile::tempdir().unwrap();
        write(&dir.path().join("a.bin"), 4096);
        let sizes = DirSizes::with(Some(dir.path().to_path_buf()), None, Arc::new(|p: &Path| dir_bytes(p)), VALID_FOR);

        let first = sizes.current();
        assert!(first.hangar.computing, "sem valor, a medição começa");
        assert_eq!(first.hangar.bytes, None);
        sizes.wait_idle();
        let measured = sizes.current();
        assert_eq!(measured.hangar, DirSize { bytes: Some(4096), computing: false });
        assert_eq!(measured.build_cache, DirSize { bytes: None, computing: false }, "sem pasta, sem valor");

        write(&dir.path().join("b.bin"), 1000);
        assert_eq!(sizes.current(), measured, "dentro da hora a pasta não é medida de novo");
    }

    #[test]
    fn expired_value_is_kept_while_measuring_again() {
        let dir = tempfile::tempdir().unwrap();
        write(&dir.path().join("a.bin"), 100);
        let sizes = DirSizes::with(Some(dir.path().to_path_buf()), None, Arc::new(|p: &Path| dir_bytes(p)), Duration::ZERO);
        sizes.current();
        sizes.wait_idle();
        write(&dir.path().join("b.bin"), 50);
        let again = sizes.current();
        assert_eq!(again.hangar, DirSize { bytes: Some(100), computing: true }, "o valor velho fica enquanto mede");
        sizes.wait_idle();
        assert_eq!(sizes.lock().hangar, Some(150));
    }

    #[test]
    fn requests_during_a_measurement_become_one() {
        let gate = Arc::new((Mutex::new(false), Condvar::new()));
        let calls = Arc::new(AtomicUsize::new(0));
        let measure: Measure = {
            let (gate, calls) = (gate.clone(), calls.clone());
            Arc::new(move |_: &Path| {
                calls.fetch_add(1, Ordering::SeqCst);
                let mut open = gate.0.lock().unwrap();
                while !*open {
                    open = gate.1.wait(open).unwrap();
                }
                Some(7)
            })
        };
        let sizes = DirSizes::with(Some(PathBuf::from("hangar")), Some(PathBuf::from("target")), measure, VALID_FOR);

        assert!(sizes.current().hangar.computing);
        assert!(sizes.current().build_cache.computing, "o segundo pedido vê a medição em andamento");
        *gate.0.lock().unwrap() = true;
        gate.1.notify_all();
        sizes.wait_idle();

        assert_eq!(calls.load(Ordering::SeqCst), 2, "uma medição, uma conta por pasta");
        let done = sizes.current();
        assert_eq!(done.hangar, DirSize { bytes: Some(7), computing: false });
        assert_eq!(done.build_cache, DirSize { bytes: Some(7), computing: false });
        assert_eq!(calls.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn build_cache_follows_cargo_order() {
        let home = tempfile::tempdir().unwrap();
        let env_dir = home.path().join("env-target");
        let cfg_dir = home.path().join("cfg-target");
        let fallback = home.path().join(".cache/cargo/target");
        for dir in [&env_dir, &cfg_dir, &fallback] {
            std::fs::create_dir_all(dir).unwrap();
        }
        let cargo_home = home.path().join(".cargo");
        std::fs::create_dir_all(&cargo_home).unwrap();
        std::fs::write(cargo_home.join("config.toml"), format!("[build]\ntarget-dir = \"{}\"\njobs = 4\n", cfg_dir.display())).unwrap();

        let with_env = |key: &str| (key == "CARGO_TARGET_DIR").then(|| env_dir.clone().into_os_string());
        assert_eq!(build_cache_dir(Some(home.path()), &with_env), Some(env_dir.clone()));
        let no_env = |_: &str| None;
        assert_eq!(build_cache_dir(Some(home.path()), &no_env), Some(cfg_dir.clone()));
        std::fs::remove_file(cargo_home.join("config.toml")).unwrap();
        assert_eq!(build_cache_dir(Some(home.path()), &no_env), Some(fallback.clone()));
        std::fs::remove_dir(&fallback).unwrap();
        assert_eq!(build_cache_dir(Some(home.path()), &no_env), None, "pasta ausente fica sem valor");
    }

    #[test]
    fn config_target_dir_reads_only_the_build_key() {
        assert_eq!(config_target_dir("[build]\ntarget-dir = \"/x/t\"\n"), Some("/x/t".into()));
        assert_eq!(config_target_dir("build.target-dir = '/y'\n"), Some("/y".into()));
        assert_eq!(config_target_dir("[profile]\ntarget-dir = \"/no\"\n"), None);
        assert_eq!(config_target_dir("[build]\njobs = 4\n"), None);
    }
}
