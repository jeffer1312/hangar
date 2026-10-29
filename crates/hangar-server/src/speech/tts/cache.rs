use super::{SpeechError, audio::AudioFile, valid_audio};
use std::{
    fs,
    io::{ErrorKind, Read},
    path::{Path, PathBuf},
    time::{Duration, SystemTime},
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
    clean(directory, SystemTime::now());
    Ok(())
}

pub(super) fn clean(directory: &Path, now: SystemTime) {
    let Some(cutoff) = now.checked_sub(Duration::from_secs(30 * 86400)) else {
        return;
    };
    let Ok(entries) = fs::read_dir(directory) else {
        tracing::warn!(
            code = "speech_cache_cleanup_failed",
            "Não foi possível listar os áudios antigos."
        );
        return;
    };
    for entry in entries {
        let result = (|| -> std::io::Result<()> {
            let entry = entry?;
            let metadata = entry.metadata()?;
            if metadata.is_file() && metadata.modified()? < cutoff {
                fs::remove_file(entry.path())?;
            }
            Ok(())
        })();
        if result.is_err() {
            tracing::warn!(
                code = "speech_cache_cleanup_failed",
                "Não foi possível remover um áudio antigo."
            );
        }
    }
}
