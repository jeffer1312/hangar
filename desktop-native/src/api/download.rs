//! Transferência longa de arquivo: escrita em `<destino>.part` com troca de nome no fim, prazo por silêncio em vez de
//! prazo total e aviso de progresso com teto de frequência.
use std::{future::Future, io::Write, path::{Path, PathBuf}, time::{Duration, Instant}};
use futures::{Stream, StreamExt};
use super::Failure;

/// Sem nenhum byte por este tempo, a transferência é dada por parada. Arquivo grande numa rede lenta continua enquanto
/// chegar alguma coisa; o prazo total cortava esse caso no meio.
pub const IDLE: Duration = Duration::from_secs(30);
/// Teto do aviso de progresso à tela: 10 por segundo.
pub const PROGRESS_EVERY: Duration = Duration::from_millis(100);
/// Escrita no disco em blocos deste tamanho, fora das threads do runtime.
const FLUSH_AT: usize = 1 << 20;

pub fn part_path(target: &Path) -> PathBuf {
    let mut name = target.file_name().map(|n| n.to_os_string()).unwrap_or_default();
    name.push(".part");
    target.with_file_name(name)
}

/// Porcentagem inteira, de 0 a 100; sem tamanho conhecido (ou zero) não há porcentagem.
pub fn percent(done: u64, total: Option<u64>) -> Option<u8> {
    let total = total.filter(|&total| total > 0)?;
    Some((u128::from(done.min(total)) * 100 / u128::from(total)) as u8)
}

/// Fração para o anel, de 0 a 1.
pub fn share(done: u64, total: Option<u64>) -> Option<f32> {
    let total = total.filter(|&total| total > 0)?;
    Some((done.min(total) as f64 / total as f64) as f32)
}

/// Tamanho com uma casa e a vírgula do português (`24,6 MB`); unidades de 1024, como o resto do app.
pub fn size_label(bytes: u64, english: bool) -> String {
    const UNITS: [&str; 4] = ["KB", "MB", "GB", "TB"];
    if bytes < 1024 { return format!("{bytes} B"); }
    let (mut value, mut unit) = (bytes as f64 / 1024., 0);
    while value >= 1024. && unit + 1 < UNITS.len() { value /= 1024.; unit += 1; }
    let text = format!("{value:.1}");
    format!("{} {}", if english { text } else { text.replace('.', ",") }, UNITS[unit])
}

/// Deixa passar um aviso a cada `every`; o primeiro passa sempre.
pub struct Throttle { every: Duration, last: Option<Instant> }

impl Throttle {
    pub fn new(every: Duration) -> Self { Self { every, last: None } }
    pub fn ready(&mut self, now: Instant) -> bool {
        if self.last.is_some_and(|last| now.saturating_duration_since(last) < self.every) { return false; }
        self.last = Some(now);
        true
    }
}

/// Quem avisa o progresso, já com o teto de frequência.
pub struct Reporter<F> { report: F, throttle: Throttle }

impl<F: FnMut(u64, Option<u64>)> Reporter<F> {
    pub fn new(report: F) -> Self { Self { report, throttle: Throttle::new(PROGRESS_EVERY) } }
    /// O fim (tudo o que se esperava) passa sempre: a tela não fica parada em 97%.
    pub fn step(&mut self, done: u64, total: Option<u64>) {
        let end = total.is_some_and(|total| done >= total);
        if self.throttle.ready(Instant::now()) || end { (self.report)(done, total); }
    }
}

const UPLOAD_CHUNK: usize = 64 * 1024;

/// O corpo do envio em pedaços, contando o que o cliente já tirou para mandar.
pub fn counted_chunks(bytes: Vec<u8>, mut report: impl FnMut(u64, u64) + Send + 'static)
    -> impl Stream<Item = Result<Vec<u8>, std::io::Error>> + Send + 'static {
    let total = bytes.len() as u64;
    let mut reporter = Reporter::new(move |done, _| report(done, total));
    let mut sent = 0u64;
    futures::stream::iter((0..bytes.len()).step_by(UPLOAD_CHUNK)).map(move |start| {
        let chunk = bytes[start..(start + UPLOAD_CHUNK).min(bytes.len())].to_vec();
        sent += chunk.len() as u64;
        reporter.step(sent, Some(total));
        Ok(chunk)
    })
}

/// Um passo da transferência com o prazo de silêncio: estourou, é `download_stalled`.
pub async fn within<T>(idle: Duration, step: impl Future<Output = T>) -> Result<T, Failure> {
    tokio::time::timeout(idle, step).await.map_err(|_| Failure::local("download_stalled"))
}

/// O próximo pedaço da resposta, com o prazo de silêncio. `None`: acabou.
pub async fn next_chunk<S, B, E>(chunks: &mut S, idle: Duration) -> Result<Option<B>, Failure>
where S: Stream<Item = Result<B, E>> + Unpin {
    match within(idle, chunks.next()).await? {
        None => Ok(None),
        Some(Ok(chunk)) => Ok(Some(chunk)),
        Some(Err(_)) => Err(Failure::transport(false)),
    }
}

fn write_failed() -> Failure { Failure::local("save_failed") }

/// Arquivo sendo gravado em `<destino>.part`. Só `finish` o põe no nome final; solto antes disso (falha, cancelamento
/// pelo `abort` da tarefa), apaga o `.part`, para não sobrar arquivo pela metade com cara de completo.
pub struct PartWriter {
    // Antes do caminho: no `drop` o arquivo fecha primeiro, e só então o `.part` sai.
    file: Option<std::fs::File>,
    buffer: Vec<u8>,
    part: PathBuf,
    target: PathBuf,
    written: u64,
    kept: bool,
}

impl PartWriter {
    pub async fn create(target: PathBuf) -> Result<Self, Failure> {
        let part = part_path(&target);
        let path = part.clone();
        let file = tokio::task::spawn_blocking(move || std::fs::File::create(path)).await
            .map_err(|_| write_failed())?.map_err(|_| write_failed())?;
        Ok(Self { file: Some(file), buffer: Vec::with_capacity(FLUSH_AT), part, target, written: 0, kept: false })
    }

    pub fn written(&self) -> u64 { self.written }

    pub async fn write(&mut self, chunk: &[u8]) -> Result<(), Failure> {
        self.buffer.extend_from_slice(chunk);
        self.written += chunk.len() as u64;
        if self.buffer.len() >= FLUSH_AT { self.flush().await?; }
        Ok(())
    }

    async fn flush(&mut self) -> Result<(), Failure> {
        if self.buffer.is_empty() { return Ok(()); }
        let mut file = self.file.take().ok_or_else(write_failed)?;
        let mut buffer = std::mem::take(&mut self.buffer);
        let (file, buffer, result) = tokio::task::spawn_blocking(move || {
            let result = file.write_all(&buffer);
            buffer.clear();
            (file, buffer, result)
        }).await.map_err(|_| write_failed())?;
        self.file = Some(file);
        self.buffer = buffer;
        result.map_err(|_| write_failed())
    }

    /// Grava o resto, sincroniza e troca o nome: o destino só aparece inteiro.
    pub async fn finish(mut self) -> Result<PathBuf, Failure> {
        self.flush().await?;
        let file = self.file.take();
        let (part, target) = (self.part.clone(), self.target.clone());
        tokio::task::spawn_blocking(move || {
            if let Some(file) = file { file.sync_all()?; }
            std::fs::rename(&part, &target)
        }).await.map_err(|_| write_failed())?.map_err(|_| write_failed())?;
        self.kept = true;
        Ok(self.target.clone())
    }
}

impl Drop for PartWriter {
    fn drop(&mut self) {
        if self.kept { return; }
        drop(self.file.take());
        let _ = std::fs::remove_file(&self.part);
    }
}

/// Grava bytes já em memória pelo mesmo caminho do `.part`. Vazio não vira arquivo: devolve `None`.
pub async fn write_bytes(target: PathBuf, bytes: &[u8]) -> Result<Option<PathBuf>, Failure> {
    if bytes.is_empty() { return Ok(None); }
    let mut writer = PartWriter::create(target).await?;
    writer.write(bytes).await?;
    writer.finish().await.map(Some)
}

/// Cópia de um arquivo deste disco, em blocos, com progresso e cancelável entre um bloco e outro. Vazio: `None`.
pub async fn copy_file(from: PathBuf, target: PathBuf, report: impl FnMut(u64, Option<u64>)) -> Result<Option<PathBuf>, Failure> {
    use std::io::Read;
    let unreadable = || Failure::local("invalid_response");
    let (mut file, total) = tokio::task::spawn_blocking(move || {
        let file = std::fs::File::open(from)?;
        let total = file.metadata()?.len();
        Ok::<_, std::io::Error>((file, total))
    }).await.map_err(|_| unreadable())?.map_err(|_| unreadable())?;
    if total == 0 { return Ok(None); }
    let (mut writer, mut reporter) = (PartWriter::create(target).await?, Reporter::new(report));
    let mut chunk = vec![0; FLUSH_AT];
    loop {
        let (back, read, result) = tokio::task::spawn_blocking(move || {
            let result = file.read(&mut chunk);
            (file, chunk, result)
        }).await.map_err(|_| unreadable())?;
        file = back;
        let read_bytes = result.map_err(|_| unreadable())?;
        if read_bytes == 0 { break; }
        writer.write(&read[..read_bytes]).await?;
        chunk = read;
        reporter.step(writer.written(), Some(total));
    }
    if writer.written() == 0 { return Ok(None); }
    writer.finish().await.map(Some)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("hangar-download-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn progress_math_clamps_and_needs_a_size() {
        assert_eq!(percent(0, Some(200)), Some(0));
        assert_eq!(percent(84, Some(200)), Some(42));
        assert_eq!(percent(200, Some(200)), Some(100));
        // Chegou mais do que o declarado: não passa de 100.
        assert_eq!(percent(300, Some(200)), Some(100));
        assert_eq!(percent(10, None), None);
        assert_eq!(percent(10, Some(0)), None);
        assert_eq!(share(50, Some(200)), Some(0.25));
        assert_eq!(share(50, None), None);
    }

    #[test]
    fn size_label_follows_the_language() {
        assert_eq!(size_label(512, false), "512 B");
        assert_eq!(size_label(25_794_970, false), "24,6 MB");
        assert_eq!(size_label(25_794_970, true), "24.6 MB");
        assert_eq!(size_label(3 << 30, true), "3.0 GB");
    }

    #[test]
    fn throttle_lets_through_at_most_one_per_interval() {
        let start = Instant::now();
        let mut throttle = Throttle::new(PROGRESS_EVERY);
        assert!(throttle.ready(start));
        assert!(!throttle.ready(start + Duration::from_millis(50)));
        assert!(!throttle.ready(start + Duration::from_millis(99)));
        assert!(throttle.ready(start + Duration::from_millis(100)));
        assert!(!throttle.ready(start + Duration::from_millis(150)));
        // Um segundo de avisos a cada milissegundo: no máximo 10 passam.
        let mut throttle = Throttle::new(PROGRESS_EVERY);
        let passed = (0..1000).filter(|ms| throttle.ready(start + Duration::from_millis(*ms))).count();
        assert_eq!(passed, 10);
    }

    #[test]
    fn upload_body_is_the_whole_file_and_ends_at_full_progress() {
        let bytes: Vec<u8> = (0..UPLOAD_CHUNK * 3 + 7).map(|n| n as u8).collect();
        let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let log = seen.clone();
        let chunks: Vec<Vec<u8>> = futures::executor::block_on(counted_chunks(bytes.clone(), move |sent, total| log.lock().unwrap().push((sent, total)))
            .map(Result::unwrap).collect());
        assert_eq!(chunks.len(), 4);
        assert_eq!(chunks.concat(), bytes);
        let seen = seen.lock().unwrap();
        // O primeiro passa, o fim também, e os do meio esperam o intervalo.
        assert_eq!(seen.first(), Some(&(UPLOAD_CHUNK as u64, bytes.len() as u64)));
        assert_eq!(seen.last(), Some(&(bytes.len() as u64, bytes.len() as u64)));
        assert!(seen.len() <= 4);
    }

    #[test]
    fn part_path_sits_next_to_the_target() {
        assert_eq!(part_path(Path::new("/tmp/x/relatorio.pdf")), PathBuf::from("/tmp/x/relatorio.pdf.part"));
    }

    #[tokio::test]
    async fn silence_past_the_idle_limit_is_a_stall() {
        let mut never = futures::stream::pending::<Result<Vec<u8>, std::io::Error>>();
        let error = next_chunk(&mut never, Duration::from_millis(20)).await.unwrap_err();
        assert_eq!(error.detail, "download_stalled");
        let mut one = futures::stream::iter([Ok::<_, std::io::Error>(vec![1u8])]);
        assert_eq!(next_chunk(&mut one, Duration::from_millis(20)).await.unwrap(), Some(vec![1]));
        assert_eq!(next_chunk(&mut one, Duration::from_millis(20)).await.unwrap(), None);
        let mut broken = futures::stream::iter([Err::<Vec<u8>, _>(std::io::Error::other("cut"))]);
        assert_eq!(next_chunk(&mut broken, Duration::from_millis(20)).await.unwrap_err().detail, "network_error");
    }

    #[tokio::test]
    async fn finished_download_replaces_the_part_with_the_target() {
        let dir = scratch("finish");
        let target = dir.join("a.bin");
        let mut writer = PartWriter::create(target.clone()).await.unwrap();
        writer.write(b"hello ").await.unwrap();
        assert!(part_path(&target).is_file());
        assert!(!target.exists());
        writer.write(b"world").await.unwrap();
        assert_eq!(writer.finish().await.unwrap(), target);
        assert_eq!(std::fs::read(&target).unwrap(), b"hello world");
        assert!(!part_path(&target).exists());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn dropped_download_leaves_neither_part_nor_target() {
        let dir = scratch("drop");
        let target = dir.join("b.bin");
        let mut writer = PartWriter::create(target.clone()).await.unwrap();
        writer.write(&vec![7; FLUSH_AT + 10]).await.unwrap();
        assert!(part_path(&target).is_file());
        drop(writer);
        assert!(!part_path(&target).exists());
        assert!(!target.exists());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn aborted_task_removes_the_part() {
        let dir = scratch("abort");
        let target = dir.join("c.bin");
        let (started, ready) = tokio::sync::oneshot::channel();
        let path = target.clone();
        let task = tokio::spawn(async move {
            let mut writer = PartWriter::create(path).await.unwrap();
            writer.write(b"partial").await.unwrap();
            let _ = started.send(());
            futures::future::pending::<()>().await;
            drop(writer);
        });
        ready.await.unwrap();
        task.abort();
        let _ = task.await;
        assert!(!part_path(&target).exists());
        assert!(!target.exists());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn empty_sources_do_not_become_files() {
        let dir = scratch("empty");
        assert_eq!(write_bytes(dir.join("e.bin"), b"").await.unwrap(), None);
        let source = dir.join("source.bin");
        std::fs::write(&source, b"").unwrap();
        assert_eq!(copy_file(source, dir.join("f.bin"), |_, _| {}).await.unwrap(), None);
        assert!(!dir.join("e.bin").exists() && !dir.join("f.bin").exists());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn local_copy_reports_progress_and_lands_whole() {
        let dir = scratch("copy");
        let source = dir.join("source.bin");
        std::fs::write(&source, vec![3u8; FLUSH_AT * 2 + 5]).unwrap();
        let mut seen = Vec::new();
        let target = copy_file(source, dir.join("g.bin"), |done, total| seen.push((done, total))).await.unwrap().unwrap();
        assert_eq!(std::fs::metadata(&target).unwrap().len(), (FLUSH_AT * 2 + 5) as u64);
        assert!(!part_path(&target).exists());
        // O primeiro aviso passa sempre; os outros esperam o intervalo.
        assert!(!seen.is_empty() && seen.iter().all(|(_, total)| *total == Some((FLUSH_AT * 2 + 5) as u64)));
        let _ = std::fs::remove_dir_all(dir);
    }
}
