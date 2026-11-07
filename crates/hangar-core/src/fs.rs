//! Escrita e leitura de arquivo que sobrevivem à troca concorrente, inclusive no Windows.
use std::fs::{File, OpenOptions};
use std::io::{self, Write};
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};

/// Recusa da própria fila: frase fixa, sem caminho nem texto da conversa, então pode ir ao log.
#[derive(Debug)]
pub struct QueueRefusal(pub &'static str);
impl std::fmt::Display for QueueRefusal { fn fmt(&self, f:&mut std::fmt::Formatter<'_>) -> std::fmt::Result { f.write_str(self.0) } }
impl std::error::Error for QueueRefusal {}

pub fn invalid(message: &'static str) -> io::Error { io::Error::new(io::ErrorKind::InvalidData, QueueRefusal(message)) }

static TEMP_ID: AtomicU64 = AtomicU64::new(0);

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
