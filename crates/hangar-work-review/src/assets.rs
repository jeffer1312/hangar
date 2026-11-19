//! Imagens da revisão em disco: `<cache>/work-review/<conector>/<hash do ticket>/<hash da rodada>/<hash do asset>.<ext>`.
//! O ID de asset pode ter `:` (proibido no Windows) e ponto solto, por isso o nome é hash. Tudo aqui é
//! cache: falha de disco só faz pedir de novo ao conector.
use hangar_connectors::work_review::{Asset, IMAGE_TYPES};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::SystemTime;

pub(crate) const LIMIT: u64 = 256 * 1024 * 1024;

fn hash(text: &str) -> String { sha1_smol::Sha1::from(text).digest().to_string() }

#[derive(Default)]
struct Disk {
    /// Bytes em disco: lidos na primeira escrita, depois somados e subtraídos. Só a passada da limpeza
    /// relê a árvore, e corrige o que escapou da conta.
    bytes: Option<u64>,
    /// Rodada atual de cada pasta de ticket, gravada por `keep_only` sob esta trava: a imagem atrasada
    /// de outra rodada não recria a pasta que acabou de sair.
    rounds: HashMap<PathBuf, String>,
}

#[derive(Clone)]
pub(crate) struct AssetCache {
    root: PathBuf,
    limit: u64,
    disk: Arc<Mutex<Disk>>,
}

impl AssetCache {
    pub(crate) fn new(cache_dir: &Path, limit: u64) -> Self { Self { root: cache_dir.join("work-review"), limit, disk: Arc::default() } }

    fn disk(&self) -> MutexGuard<'_, Disk> { self.disk.lock().unwrap_or_else(|e| e.into_inner()) }

    /// O conector vira pasta: só `[a-z][a-z0-9-]{0,63}`. O do manifesto já passa por `valid_id` no
    /// cadastro, mas o do vínculo vem do `mission.json`.
    fn ticket_dir(&self, connector: &str, ticket: &str) -> Option<PathBuf> {
        hangar_connectors::manifest::valid_id(connector).then(|| self.root.join(connector).join(hash(ticket)))
    }

    fn review_dir(&self, connector: &str, ticket: &str, review: &str) -> Option<PathBuf> { Some(self.ticket_dir(connector, ticket)?.join(hash(review))) }

    /// O acerto renova o mtime: a limpeza pelo teto tira o menos usado, não o mais antigo.
    pub(crate) async fn get(&self, connector: &str, ticket: &str, review: &str, asset: &str) -> Option<Asset> {
        let base = self.review_dir(connector, ticket, review)?.join(hash(asset));
        tokio::task::spawn_blocking(move || IMAGE_TYPES.iter().find_map(|kind| {
            let path = base.with_extension(kind.ext);
            let bytes = std::fs::read(&path).ok()?;
            let _ = std::fs::File::options().write(true).open(&path).and_then(|file| file.set_modified(SystemTime::now()));
            Some(Asset { mime: kind.mime.to_owned(), bytes })
        })).await.ok().flatten()
    }

    pub(crate) async fn put(&self, connector: &str, ticket: &str, review: &str, asset: &str, image: &Asset) {
        let Some(kind) = IMAGE_TYPES.iter().find(|kind| kind.mime == image.mime) else { return };
        let (Some(keep), Some(dir)) = (self.ticket_dir(connector, ticket), self.review_dir(connector, ticket, review)) else { return };
        let cache = self.clone();
        let path = dir.join(hash(asset)).with_extension(kind.ext);
        let (bytes, round) = (image.bytes.clone(), hash(review));
        let done = tokio::task::spawn_blocking(move || {
            let mut disk = cache.disk();
            if disk.rounds.get(&keep).is_some_and(|current| *current != round) { return; }
            let before = file_size(&path);
            let written = path.parent().map_or(Ok(()), std::fs::create_dir_all).and_then(|()| hangar_core::fs::atomic_write(&path, &bytes));
            if let Err(error) = written { tracing::warn!(code = "work_review_asset_cache_write", error = ?error.kind(), "work_review: imagem não foi ao cache"); }
            let now = match disk.bytes {
                Some(known) => (known + file_size(&path)).saturating_sub(before),
                None => cache.tickets().iter().map(|(_, size, _)| size).sum(),
            };
            disk.bytes = Some(if now > cache.limit { cache.evict(&keep, &mut disk.rounds) } else { now });
        }).await;
        if let Err(error) = done { tracing::warn!(code = "work_review_asset_cache_join", %error, "work_review: gravação no cache interrompida"); }
    }

    /// Rodada nova do ticket: as imagens das outras rodadas não voltam mais.
    pub(crate) async fn keep_only(&self, connector: &str, ticket: &str, review: &str) {
        let (Some(dir), keep) = (self.ticket_dir(connector, ticket), hash(review)) else { return };
        let cache = self.clone();
        let done = tokio::task::spawn_blocking(move || {
            let mut disk = cache.disk();
            disk.rounds.insert(dir.clone(), keep.clone());
            let Ok(entries) = std::fs::read_dir(&dir) else { return };
            for entry in entries.flatten().filter(|entry| entry.file_name().to_str() != Some(keep.as_str())) {
                let before = review_size(&entry.path());
                let _ = std::fs::remove_dir_all(entry.path());
                // O que não saiu continua na conta.
                let gone = before.saturating_sub(review_size(&entry.path()));
                if let Some(known) = disk.bytes.as_mut() { *known = known.saturating_sub(gone); }
            }
        }).await;
        if let Err(error) = done { tracing::warn!(code = "work_review_asset_cache_join", %error, "work_review: limpeza das rodadas antigas interrompida"); }
    }

    /// Pastas de ticket com o arquivo mais novo e o tamanho de cada uma.
    fn tickets(&self) -> Vec<(SystemTime, u64, PathBuf)> {
        std::fs::read_dir(&self.root).into_iter().flatten().flatten()
            .flat_map(|connector| std::fs::read_dir(connector.path()).into_iter().flatten().flatten())
            .map(|ticket| { let (newest, size) = usage(&ticket.path()); (newest, size, ticket.path()) })
            .collect()
    }

    /// Acima do teto, saem as pastas de ticket menos usadas, nunca a que acabou de receber a imagem.
    /// Devolve o total relido depois da limpeza.
    fn evict(&self, keep: &Path, rounds: &mut HashMap<PathBuf, String>) -> u64 {
        let mut tickets = self.tickets();
        let mut total: u64 = tickets.iter().map(|(_, size, _)| size).sum();
        tickets.sort_by_key(|(newest, _, _)| *newest);
        for (_, size, path) in tickets {
            if total <= self.limit { break; }
            if path == keep { continue; }
            if std::fs::remove_dir_all(&path).is_ok() {
                total = total.saturating_sub(size);
                rounds.remove(&path);
            }
        }
        total
    }
}

fn file_size(path: &Path) -> u64 { std::fs::metadata(path).map_or(0, |meta| meta.len()) }

/// Tamanho das imagens de uma rodada.
fn review_size(dir: &Path) -> u64 {
    std::fs::read_dir(dir).into_iter().flatten().flatten().filter_map(|file| file.metadata().ok()).map(|meta| meta.len()).sum()
}

/// Arquivo mais novo e tamanho de uma pasta de ticket (rodadas e imagens).
fn usage(dir: &Path) -> (SystemTime, u64) {
    let mut newest = SystemTime::UNIX_EPOCH;
    let mut size = 0;
    for review in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        for file in std::fs::read_dir(review.path()).into_iter().flatten().flatten() {
            let Ok(meta) = file.metadata() else { continue };
            size += meta.len();
            if let Ok(modified) = meta.modified() { newest = newest.max(modified); }
        }
    }
    (newest, size)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn png(size: usize) -> Asset {
        let mut bytes = b"\x89PNG\r\n\x1a\n".to_vec();
        bytes.resize(size, 0);
        Asset { mime: "image/png".into(), bytes }
    }

    fn age(cache: &AssetCache, ticket: &str, seconds: u64) {
        let path = cache.review_dir("help", ticket, "r").unwrap().join(hash("a:1")).with_extension("png");
        let file = std::fs::File::options().write(true).open(path).unwrap();
        file.set_modified(SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(seconds)).unwrap();
    }

    #[tokio::test]
    async fn over_the_limit_the_oldest_ticket_leaves() {
        let dir = tempfile::tempdir().unwrap();
        let cache = AssetCache::new(dir.path(), 2500);
        cache.put("help", "t-1", "r", "a:1", &png(1000)).await;
        age(&cache, "t-1", 1000);
        cache.put("help", "t-2", "r", "a:1", &png(1000)).await;
        age(&cache, "t-2", 2000);
        cache.put("help", "t-3", "r", "a:1", &png(1000)).await;
        assert!(cache.get("help", "t-1", "r", "a:1").await.is_none(), "o mais antigo saiu");
        assert!(cache.get("help", "t-2", "r", "a:1").await.is_some());
        assert_eq!(cache.get("help", "t-3", "r", "a:1").await.unwrap().bytes.len(), 1000);
    }

    #[tokio::test]
    async fn a_hit_renews_the_ticket_so_the_least_used_leaves() {
        let dir = tempfile::tempdir().unwrap();
        let cache = AssetCache::new(dir.path(), 2500);
        cache.put("help", "t-1", "r", "a:1", &png(1000)).await;
        age(&cache, "t-1", 1000);
        cache.put("help", "t-2", "r", "a:1", &png(1000)).await;
        age(&cache, "t-2", 2000);
        assert!(cache.get("help", "t-1", "r", "a:1").await.is_some());
        cache.put("help", "t-3", "r", "a:1", &png(1000)).await;
        assert!(cache.get("help", "t-1", "r", "a:1").await.is_some(), "o usado agora fica");
        assert!(cache.get("help", "t-2", "r", "a:1").await.is_none(), "o menos usado sai");
    }

    #[tokio::test]
    async fn the_running_total_follows_puts_and_dropped_rounds() {
        let dir = tempfile::tempdir().unwrap();
        let cache = AssetCache::new(dir.path(), LIMIT);
        cache.put("help", "t-1", "r-1", "a:1", &png(1000)).await;
        cache.put("help", "t-1", "r-1", "a:2", &png(500)).await;
        cache.put("help", "t-1", "r-1", "a:2", &png(700)).await;
        assert_eq!(cache.disk().bytes, Some(1700), "a mesma imagem regravada troca o tamanho, não soma");
        cache.put("help", "t-1", "r-2", "a:1", &png(300)).await;
        cache.keep_only("help", "t-1", "r-2").await;
        assert_eq!(cache.disk().bytes, Some(300), "a rodada apagada sai da conta");
    }

    #[tokio::test]
    async fn a_late_image_of_a_dropped_round_does_not_come_back() {
        let dir = tempfile::tempdir().unwrap();
        let cache = AssetCache::new(dir.path(), LIMIT);
        cache.put("help", "t-1", "r-1", "a:1", &png(100)).await;
        cache.keep_only("help", "t-1", "r-2").await;
        cache.put("help", "t-1", "r-1", "a:2", &png(100)).await;
        assert!(!cache.review_dir("help", "t-1", "r-1").unwrap().exists(), "a pasta da rodada apagada não volta");
        cache.put("help", "t-1", "r-2", "a:1", &png(100)).await;
        assert!(cache.get("help", "t-1", "r-2", "a:1").await.is_some(), "a rodada atual grava");
    }

    #[tokio::test]
    async fn connector_outside_the_id_format_never_touches_the_disk() {
        let dir = tempfile::tempdir().unwrap();
        let cache = AssetCache::new(dir.path(), LIMIT);
        cache.put("../fora", "t-1", "r", "a:1", &png(100)).await;
        assert!(!dir.path().join("fora").exists() && !dir.path().join("work-review").exists());
    }
}
