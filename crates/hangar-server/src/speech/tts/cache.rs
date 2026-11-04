use super::{SpeechError, audio::AudioFile, valid_audio};
use std::{
    fs,
    io::{ErrorKind, Read},
    path::{Path, PathBuf},
    sync::{Arc, Mutex, TryLockError},
    time::{Duration, Instant, SystemTime},
};

pub(super) fn path(directory: &Path, hash: &str) -> PathBuf {
    for extension in ["mp3", "wav"] {
        let path = directory.join(format!("{hash}.{extension}"));
        if path.exists() {
            return path;
        }
    }
    directory.join(format!("{hash}.mp3"))
}

pub(super) fn lookup(directory: &Path, hash: &str) -> Result<bool, SpeechError> {
    for extension in ["mp3", "wav"] {
        if lookup_file(&directory.join(format!("{hash}.{extension}")))? {
            return Ok(true);
        }
    }
    Ok(false)
}

/// `Read` pode entregar um único byte sem EOF; a validade só é decidida com o prefixo completo.
pub(super) fn read_prefix(reader: &mut impl Read) -> std::io::Result<([u8; 4], usize)> {
    let mut prefix = [0; 4];
    let mut length = 0;
    while length < prefix.len() {
        match reader.read(&mut prefix[length..]) {
            Ok(0) => break,
            Ok(count) => length += count,
            Err(error) if error.kind() == ErrorKind::Interrupted => continue,
            Err(error) => return Err(error),
        }
    }
    Ok((prefix, length))
}

fn lookup_file(path: &Path) -> Result<bool, SpeechError> {
    let metadata = match fs::metadata(&path) {
        Ok(value) => value,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(false),
        Err(_) => {
            return Err(SpeechError::new(
                500,
                "Não foi possível conferir o áudio em cache.",
            ));
        }
    };
    if !metadata.is_file() {
        return Err(SpeechError::new(
            500,
            "O caminho do áudio em cache não é um arquivo.",
        ));
    }
    if metadata.len() == 0 {
        fs::remove_file(&path).map_err(|_| {
            SpeechError::new(500, "Não foi possível remover o áudio vazio do cache.")
        })?;
        return Ok(false);
    }
    let mut file = fs::File::open(&path)
        .map_err(|_| SpeechError::new(500, "Não foi possível ler o áudio em cache."))?;
    let (prefix, length) = read_prefix(&mut file)
        .map_err(|_| SpeechError::new(500, "Não foi possível ler o áudio em cache."))?;
    drop(file);
    if !valid_audio(&prefix[..length]) {
        fs::remove_file(&path).map_err(|_| {
            SpeechError::new(500, "Não foi possível remover o áudio inválido do cache.")
        })?;
        tracing::warn!(
            code = "speech_cache_invalid",
            "Áudio inválido removido do cache."
        );
        return Ok(false);
    }
    let renewed = fs::OpenOptions::new()
        .write(true)
        .open(&path)
        .and_then(|file| file.set_times(fs::FileTimes::new().set_modified(SystemTime::now())));
    if renewed.is_err() {
        tracing::warn!(
            code = "speech_cache_touch_failed",
            "Não foi possível renovar a retenção do áudio em cache."
        );
    }
    Ok(true)
}

pub(super) fn prepare(directory: &Path) -> Result<(), SpeechError> {
    fs::create_dir_all(directory)
        .map_err(|_| SpeechError::new(500, "Falha ao preparar a pasta de cache do áudio."))
}

pub(super) fn store(directory: &Path, hash: &str, audio: AudioFile) -> Result<(), SpeechError> {
    if audio.is_empty() || !valid_audio(audio.prefix()) {
        return Err(SpeechError::new(
            502,
            "O arquivo de áudio não contém um formato reconhecido.",
        ));
    }
    let extension = if audio.prefix().starts_with(b"RIFF") {
        "wav"
    } else {
        "mp3"
    };
    // O produtor já fechou e sincronizou o temporário; a publicação é uma única troca atômica.
    crate::runtime::queue::replace(
        audio.data.path.as_ref(),
        &directory.join(format!("{hash}.{extension}")),
    )
    .map_err(|_| SpeechError::new(500, "Falha ao gravar o áudio em cache."))?;
    #[cfg(unix)]
    fs::File::open(directory)
        .and_then(|file| file.sync_all())
        .map_err(|_| SpeechError::new(500, "Falha ao sincronizar a pasta de cache do áudio."))?;
    Ok(())
}

#[cfg(test)]
pub(super) fn clean(directory: &Path, now: SystemTime) -> bool {
    clean_with_selection(directory, now, Instant::now(), None)
}

const RETENTION: Duration = Duration::from_secs(30 * 86400);

fn clean_with_selection(
    directory: &Path,
    now: SystemTime,
    selected_now: Instant,
    selected: Option<&Mutex<std::collections::HashMap<String, Instant>>>,
) -> bool {
    let Some(cutoff) = now.checked_sub(RETENTION) else {
        return true;
    };
    let Ok(entries) = fs::read_dir(directory) else {
        tracing::warn!(
            code = "speech_cache_cleanup_failed",
            "Não foi possível listar os áudios antigos."
        );
        return false;
    };
    let mut success = true;
    for entry in entries {
        let result = (|| -> std::io::Result<()> {
            let entry = entry?;
            let name = entry.file_name();
            let owned = name
                .to_str()
                .and_then(|name| name.rsplit_once('.'))
                .is_some_and(|(hash, extension)| {
                    hash.len() == 64
                        && hash
                            .bytes()
                            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
                        && matches!(extension, "mp3" | "wav")
                });
            if !owned || !entry.file_type()?.is_file() {
                return Ok(());
            }
            // A seleção e a remoção usam a mesma exclusão; quem já começou a leitura fica protegido.
            let selections = selected
                .map(|selected| selected.lock())
                .transpose()
                .map_err(|_| std::io::Error::other("controle de retenção indisponível"))?;
            let hash = name
                .to_str()
                .and_then(|name| name.rsplit_once('.'))
                .unwrap()
                .0;
            if selections.as_ref().is_some_and(|selections| {
                selections
                    .get(hash)
                    .is_some_and(|used| selected_now.saturating_duration_since(*used) < RETENTION)
            }) {
                return Ok(());
            }
            let metadata = fs::symlink_metadata(entry.path())?;
            if metadata.is_file() && metadata.modified()? < cutoff {
                fs::remove_file(entry.path())?;
            }
            Ok(())
        })();
        if result.is_err() {
            success = false;
            tracing::warn!(
                code = "speech_cache_cleanup_failed",
                "Não foi possível remover um áudio antigo."
            );
        }
    }
    success
}

#[derive(Default)]
struct DirectoryMaintenance {
    next: Mutex<Option<Instant>>,
    selected: Mutex<std::collections::HashMap<String, Instant>>,
}

type Schedule = Arc<DirectoryMaintenance>;

#[derive(Default)]
pub(super) struct Maintenance {
    directories: Mutex<std::collections::HashMap<PathBuf, Schedule>>,
}

impl Maintenance {
    fn state(&self, directory: &Path) -> Result<Schedule, SpeechError> {
        self.directories
            .lock()
            .map(|mut directories| directories.entry(directory.to_owned()).or_default().clone())
            .map_err(|_| {
                SpeechError::new(500, "O controle de retenção do cache está indisponível.")
            })
    }

    pub fn select(&self, directory: &Path, hash: &str) -> Result<(), SpeechError> {
        let state = self.state(directory)?;
        let mut selected = state.selected.lock().map_err(|_| {
            SpeechError::new(500, "O controle de retenção do cache está indisponível.")
        })?;
        // O último uso vale também quando o sistema recusa renovar o mtime do arquivo.
        selected.insert(hash.to_owned(), Instant::now());
        Ok(())
    }

    pub fn run(&self, directory: &Path, now: Instant) -> bool {
        let state = match self.state(directory) {
            Ok(state) => state,
            Err(_) => {
                tracing::warn!(
                    code = "speech_cache_cleanup_failed",
                    "O controle de limpeza do cache está indisponível."
                );
                return false;
            }
        };
        // Uma varredura por pasta e por hora; pedidos concorrentes não disputam o disco.
        let mut next = match state.next.try_lock() {
            Ok(next) => next,
            Err(TryLockError::WouldBlock) => return false,
            Err(TryLockError::Poisoned(_)) => {
                tracing::warn!(
                    code = "speech_cache_cleanup_failed",
                    "O agendamento da limpeza do cache está indisponível."
                );
                return false;
            }
        };
        if next.is_some_and(|deadline| now < deadline) {
            return false;
        }
        match state.selected.lock() {
            Ok(mut selected) => {
                selected.retain(|_, used| now.saturating_duration_since(*used) < RETENTION)
            }
            Err(_) => {
                tracing::warn!(
                    code = "speech_cache_cleanup_failed",
                    "O controle de retenção do cache está indisponível."
                );
                return false;
            }
        }
        let success =
            clean_with_selection(directory, SystemTime::now(), now, Some(&state.selected));
        *next = Some(now + Duration::from_secs(if success { 3600 } else { 60 }));
        true
    }
}
