//! Escrita e leitura de arquivo que sobrevivem à troca concorrente, inclusive no Windows.
use std::fs::{File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

/// Recusa da própria fila: frase fixa, sem caminho nem texto da conversa, então pode ir ao log.
#[derive(Debug)]
pub struct QueueRefusal(pub &'static str);
impl std::fmt::Display for QueueRefusal { fn fmt(&self, f:&mut std::fmt::Formatter<'_>) -> std::fmt::Result { f.write_str(self.0) } }
impl std::error::Error for QueueRefusal {}

pub fn invalid(message: &'static str) -> io::Error { io::Error::new(io::ErrorKind::InvalidData, QueueRefusal(message)) }

static TEMP_ID: AtomicU64 = AtomicU64::new(0);

/// Ponte de leitura de sidecar: o nome antigo só vale enquanto o novo não existir.
pub fn legacy_sidecar_path(new: PathBuf) -> PathBuf {
    if new.exists() { return new; }
    let old = new.with_file_name(new.file_name().and_then(|name| name.to_str()).unwrap_or("")
        .replacen(".hangar-", ".claude-pocket-", 1));
    if old.exists() { old } else { new }
}

/// Lê um arquivo que outro escritor troca por `atomic_write`. No Windows quem abre no instante da
/// troca recebe NotFound ou acesso negado, inclusive na troca POSIX: ausente ou recusado de verdade é o
/// que continua assim depois da espera curta. As primeiras tentativas não dependem do relógio (a
/// thread preemptada perderia a espera inteira entre duas leituras) e só cedem a vez, porque o sono
/// de 1 ms dura um tique (~15 ms); depois dorme, para o arquivo ausente de verdade não girar a CPU.
pub fn read_replaced(path: &Path) -> io::Result<Vec<u8>> {
    let deadline = std::time::Instant::now() + std::time::Duration::from_millis(50);
    let mut attempts = 0u32;
    loop {
        attempts += 1;
        match std::fs::read(path) {
            Err(error) if cfg!(windows) && matches!(error.kind(), io::ErrorKind::NotFound | io::ErrorKind::PermissionDenied)
                && (attempts <= 100 || std::time::Instant::now() < deadline) => if attempts <= 100 {
                    std::thread::yield_now()
                } else {
                    std::thread::sleep(std::time::Duration::from_millis(1))
                },
            outcome => return outcome,
        }
    }
}
pub fn read_replaced_to_string(path: &Path) -> io::Result<String> {
    String::from_utf8(read_replaced(path)?).map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
}

/// `rename` que sobrevive ao Windows: lá um leitor aberto sem FILE_SHARE_DELETE (o CLI lendo
/// o JSON) recusa a troca por um instante. No POSIX a troca nunca falha por leitor aberto.
pub fn replace(from: &Path, to: &Path) -> io::Result<()> {
    #[cfg(windows)]
    for wait in [20,40,60,80,100,120] {
        match std::fs::rename(from,to) {
            Ok(()) => return Ok(()),
            Err(error) if error.kind() == io::ErrorKind::PermissionDenied => std::thread::sleep(std::time::Duration::from_millis(wait)),
            Err(error) => return Err(error),
        }
    }
    std::fs::rename(from,to)
}
pub fn atomic_write(path: &Path, bytes: &[u8]) -> io::Result<()> {
    atomic_write_inner(path,bytes, |_| Ok(()))
}
#[cfg(any(test, feature = "test-support"))]
pub fn atomic_write_inspected(path:&Path,bytes:&[u8],before:impl FnOnce(&File)->io::Result<()>)->io::Result<()> {atomic_write_inner(path,bytes,before)}
fn atomic_write_inner(path:&Path,bytes:&[u8],before:impl FnOnce(&File)->io::Result<()>)->io::Result<()> {
    let parent = path.parent().ok_or_else(||invalid("arquivo sem diretório"))?;
    let tick = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)
        .map_err(|_|invalid("relógio do arquivo temporário inválido"))?.as_nanos();
    let temp = parent.join(format!(".queue-{}-{tick}-{}.tmp",std::process::id(),TEMP_ID.fetch_add(1,Ordering::Relaxed)));
    let mut created = false;
    let result = (|| {
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)] { use std::os::unix::fs::OpenOptionsExt; options.mode(0o600); }
        let mut file = options.open(&temp)?;
        created = true;
        before(&file)?;
        file.write_all(bytes)?; file.sync_all()?; drop(file);
        replace(&temp,path)?;
        #[cfg(unix)] { File::open(parent)?.sync_all()?; }
        Ok(())
    })();
    if created {
        match std::fs::remove_file(&temp) {
            Err(error) if error.kind() != io::ErrorKind::NotFound && result.is_ok() => return Err(error),
            _ => {}
        }
    }
    result
}

/// O `canonicalize` do Windows devolve `\\?\C:\...`; o Python escreve `C:\...`, e a conta da sessão
/// (`claude:<pasta>`) precisa bater letra a letra com o id da credencial que ele monta.
pub fn without_verbatim(path: std::path::PathBuf) -> std::path::PathBuf {
    if !cfg!(windows) { return path; }
    path.to_str().and_then(verbatim_stripped).map_or(path, std::path::PathBuf::from)
}

fn verbatim_stripped(text: &str) -> Option<String> {
    if let Some(rest) = text.strip_prefix(r"\\?\UNC\") { return Some(format!(r"\\{rest}")); }
    text.strip_prefix(r"\\?\").filter(|rest| rest.as_bytes().get(1) == Some(&b':')).map(str::to_owned)
}

#[cfg(test)]
mod verbatim_tests {
    use super::verbatim_stripped;

    #[test]
    fn verbatim_prefix_is_written_like_python() {
        assert_eq!(verbatim_stripped(r"\\?\C:\Users\u\.claude-x").as_deref(), Some(r"C:\Users\u\.claude-x"));
        assert_eq!(verbatim_stripped(r"\\?\UNC\srv\share\d").as_deref(), Some(r"\\srv\share\d"));
        // Volume sem letra não tem forma curta: fica como veio.
        assert_eq!(verbatim_stripped(r"\\?\Volume{abc}\d"), None);
        assert_eq!(verbatim_stripped(r"C:\Users\u"), None);
    }
}

#[cfg(all(test, windows))]
mod replaced_read_tests {
    use super::*;
    #[test]
    fn a_file_being_replaced_never_reads_as_missing() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("key.queue-state.json");
        atomic_write(&path, b"{}").unwrap();
        let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let writer = { let (path, stop) = (path.clone(), stop.clone()); std::thread::spawn(move || {
            // A troca de produção sem o fsync do `atomic_write`: o que está à prova é o leitor, e mais trocas por segundo.
            let mut n = 0u64;
            while !stop.load(Ordering::Relaxed) {
                n += 1;
                let temp = path.with_extension(format!("{n}.tmp"));
                std::fs::write(&temp, format!("{{\"n\":{n}}}")).unwrap();
                replace(&temp, &path).unwrap();
            }
        }) };
        let (mut reads, mut missing) = (0u32, 0u32);
        let started = std::time::Instant::now();
        while started.elapsed() < std::time::Duration::from_secs(3) {
            match read_replaced(&path) {
                Ok(_) => reads += 1,
                Err(error) if error.kind() == io::ErrorKind::NotFound => missing += 1,
                Err(error) => panic!("leitura recusada: {error}"),
            }
        }
        stop.store(true, Ordering::Relaxed);
        writer.join().unwrap();
        assert!(reads > 100, "poucas leituras para provar algo: {reads}");
        assert_eq!(missing, 0, "o arquivo trocado sumiu em {missing} de {} leituras", reads + missing);
    }

    /// Leitor aberto que compartilha a exclusão (o `atomico.read` do Python, o `std::fs::read`) não faz a
    /// troca falhar: a outra metade da #145, agora que o Rust grava o sidecar do Claude.
    #[test]
    fn a_reader_that_shares_deletion_does_not_block_the_replace() {
        let dir = tempfile::tempdir().unwrap();
        let (path, temp) = (dir.path().join("cl.json"), dir.path().join("cl.tmp"));
        std::fs::write(&path, b"velho").unwrap();
        std::fs::write(&temp, b"novo").unwrap();
        let mut reader = std::fs::File::open(&path).unwrap();
        replace(&temp, &path).unwrap();
        let mut old = Vec::new();
        std::io::Read::read_to_end(&mut reader, &mut old).unwrap();
        assert_eq!(old, b"velho");
        assert_eq!(std::fs::read(&path).unwrap(), b"novo");
    }
}
