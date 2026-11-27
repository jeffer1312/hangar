//! Memória, swap, carga, núcleos e disco desta máquina, para o bloco "Esta máquina", e a carga que
//! o balanceamento lê na hora.
use std::path::{Component, Path, PathBuf};

use hangar_api::status::{DirSize, DiskUse, HostBlock};
use sysinfo::{CpuRefreshKind, DiskRefreshKind, Disks, MemoryRefreshKind, System};

/// Leitor da máquina que guarda o `System` e a lista de discos entre as rodadas; cada leitura os
/// atualiza (os discos pelo `/proc/mounts` de novo: montagem nova entra, a que saiu sai).
#[derive(Default)]
pub struct HostReader {
    system: System,
    disks: Disks,
}

impl HostReader {
    pub fn new() -> Self { Self::default() }

    /// Lê a máquina com o disco da partição da casa do usuário e da pasta temporária. `None` quando
    /// não há casa ou nenhum disco a contém: sem disco não há bloco.
    pub fn read(&mut self, dir_hangar: DirSize, dir_build_cache: DirSize) -> Option<HostBlock> {
        let home = std::env::home_dir()?;
        self.read_at(&home, &std::env::temp_dir(), dir_hangar, dir_build_cache)
    }

    pub fn read_at(&mut self, home: &Path, temp: &Path, dir_hangar: DirSize, dir_build_cache: DirSize) -> Option<HostBlock> {
        let Self { system, disks } = self;
        system.refresh_memory_specifics(MemoryRefreshKind::nothing().with_ram().with_swap());
        system.refresh_cpu_list(CpuRefreshKind::nothing());
        disks.refresh_specifics(true, DiskRefreshKind::nothing().with_storage());
        let home_disk = disk_of(disks, home)?;
        let temp_disk = disk_of(disks, temp);
        let disk_tmp = temp_disk.filter(|tmp| tmp.mount_point() != home_disk.mount_point()).map(usage);
        Some(HostBlock {
            mem_total: system.total_memory(),
            mem_available: system.available_memory(),
            swap_total: system.total_swap(),
            swap_used: system.used_swap(),
            load: load(),
            cpus: u32::try_from(system.cpus().len()).unwrap_or(u32::MAX),
            disk_home: usage(home_disk),
            disk_tmp,
            dir_hangar,
            dir_build_cache,
        })
    }
}

#[cfg(windows)]
fn load() -> Option<[f64; 3]> { None }

#[cfg(not(windows))]
fn load() -> Option<[f64; 3]> {
    let l = System::load_average();
    Some([l.one, l.five, l.fifteen])
}

/// Memória e processador de agora, para quem escolhe a máquina de uma sessão nova.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PlacementLoad {
    pub mem_total: u64,
    pub mem_available: u64,
    /// `None` quando a máquina não sabe medir.
    pub cpu_pct: Option<f32>,
}

/// Lido na hora, sem medidor ligado: no Windows bloqueia o intervalo de duas amostras.
pub fn placement_load() -> PlacementLoad {
    let mut system = System::new();
    system.refresh_memory_specifics(MemoryRefreshKind::nothing().with_ram());
    PlacementLoad { mem_total: system.total_memory(), mem_available: system.available_memory(), cpu_pct: cpu_pct(&mut system) }
}

/// O Windows não tem carga média: uso medido entre duas amostras.
#[cfg(windows)]
fn cpu_pct(system: &mut System) -> Option<f32> {
    system.refresh_cpu_usage();
    std::thread::sleep(sysinfo::MINIMUM_CPU_UPDATE_INTERVAL);
    system.refresh_cpu_usage();
    Some(system.global_cpu_usage())
}

/// Carga média de 5 min sobre os núcleos: o limite do dono fala dos últimos 5 min.
#[cfg(not(windows))]
fn cpu_pct(system: &mut System) -> Option<f32> {
    system.refresh_cpu_list(CpuRefreshKind::nothing());
    load_pct(System::load_average().five, system.cpus().len())
}

#[cfg_attr(windows, allow(dead_code))]
fn load_pct(five: f64, cpus: usize) -> Option<f32> {
    (cpus > 0).then(|| (five / cpus as f64 * 100.0) as f32)
}

fn usage(disk: &sysinfo::Disk) -> DiskUse {
    let total = disk.total_space();
    DiskUse { used: total.saturating_sub(disk.available_space()), total }
}

/// O disco cujo ponto de montagem é o prefixo mais longo do caminho.
fn disk_of<'a>(disks: &'a Disks, path: &Path) -> Option<&'a sysinfo::Disk> {
    let path = resolved(path);
    disks.list().iter()
        .filter(|disk| disk.total_space() > 0 && contains(disk.mount_point(), &path))
        .max_by_key(|disk| disk.mount_point().components().count())
}

/// Caminho absoluto sem link no meio. No Windows o `canonicalize` devolve `\\?\C:\...`, que não casa
/// com o ponto de montagem `C:\`: ali basta o absoluto.
fn resolved(path: &Path) -> PathBuf {
    if cfg!(windows) {
        return std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
    }
    path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
}

/// Comparação por componente; no Windows a letra do disco e os nomes não diferenciam maiúscula.
fn contains(mount: &Path, path: &Path) -> bool {
    let norm = |c: Component<'_>| {
        let text = c.as_os_str().to_string_lossy().into_owned();
        if cfg!(windows) { text.to_lowercase() } else { text }
    };
    let mut path = path.components();
    mount.components().all(|m| path.next().is_some_and(|p| norm(p) == norm(m)))
}

#[cfg(test)]
mod tests {
    use super::*;

    const NONE: DirSize = DirSize { bytes: None, computing: false };

    #[test]
    fn host_reading_is_within_ranges() {
        let home = tempfile::tempdir().unwrap();
        let temp = tempfile::tempdir().unwrap();
        let mut reader = HostReader::new();
        let host = reader.read_at(home.path(), temp.path(), NONE, NONE).expect("disco da pasta");
        assert!(reader.read_at(home.path(), temp.path(), NONE, NONE).is_some(), "a segunda leitura atualiza o mesmo leitor");
        assert!(host.mem_total > 0);
        assert!(host.mem_available > 0 && host.mem_available <= host.mem_total);
        assert!(host.swap_used <= host.swap_total);
        assert!(host.cpus > 0);
        assert!(0 < host.disk_home.used && host.disk_home.used <= host.disk_home.total);
        if let Some(tmp) = host.disk_tmp {
            assert!(tmp.used <= tmp.total && tmp.total > 0);
        }
        if cfg!(windows) {
            assert_eq!(host.load, None);
        } else {
            assert!(host.load.is_some_and(|l| l.iter().all(|v| *v >= 0.0)));
        }
        assert_eq!((host.dir_hangar, host.dir_build_cache), (NONE, NONE), "tamanhos vêm de quem chama");
    }

    #[test]
    fn load_over_cores_is_percent() {
        assert_eq!(load_pct(4.0, 8), Some(50.0));
        assert_eq!(load_pct(12.0, 8), Some(150.0), "carga acima dos núcleos passa de 100");
        assert_eq!(load_pct(1.0, 0), None, "sem núcleos lidos não há medida");
    }

    #[test]
    fn placement_load_measures_on_every_system() {
        let load = placement_load();
        assert!(load.mem_total > 0 && load.mem_available <= load.mem_total);
        assert!(load.cpu_pct.is_some_and(|pct| pct >= 0.0), "{load:?}");
    }

    #[test]
    fn mount_contains_by_component() {
        assert!(contains(Path::new("/"), Path::new("/home/u")));
        assert!(contains(Path::new("/home"), Path::new("/home/u")));
        assert!(!contains(Path::new("/home"), Path::new("/homes/u")), "prefixo de texto não é prefixo de pasta");
        assert!(!contains(Path::new("/tmp"), Path::new("/")));
    }
}
