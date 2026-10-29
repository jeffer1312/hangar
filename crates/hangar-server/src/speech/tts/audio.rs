//! O áudio atravessa um canal limitado até o temporário; só o prefixo fica no estado da síntese.
use super::{Lease, SpeechError};
use std::{io, path::Path};
use tokio::{io::AsyncWriteExt, sync::mpsc, task::JoinHandle};

pub(super) const CHUNK_BYTES: usize = 64 * 1024;

pub(super) struct AudioFile {
    pub data: Data,
    // A posse acompanha o arquivo até a publicação ou o descarte do temporário.
    _lease: Option<Lease>,
}

impl std::fmt::Debug for AudioFile {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        out.debug_struct("AudioFile")
            .field("length", &self.data.length)
            .finish_non_exhaustive()
    }
}

impl AudioFile {
    pub fn prefix(&self) -> &[u8] {
        &self.data.prefix[..self.data.prefix_length]
    }
    pub fn is_empty(&self) -> bool {
        self.data.length == 0
    }
}

pub(super) struct Data {
    pub path: tempfile::TempPath,
    prefix: [u8; 4],
    prefix_length: usize,
    pub length: u64,
    #[cfg(test)]
    pub largest_chunk: usize,
}

pub(super) struct Transfer {
    sender: Option<mpsc::Sender<bytes::Bytes>>,
    task: Option<JoinHandle<Result<Data, SpeechError>>>,
    lease: Option<Lease>,
}

impl Transfer {
    pub fn new(directory: &Path, lease: Lease) -> Result<Self, SpeechError> {
        let file = tempfile::Builder::new()
            .prefix(".speech-audio-")
            .tempfile_in(directory)
            .map_err(|_| {
                SpeechError::new(500, "Falha ao preparar o arquivo temporário do áudio.")
            })?;
        let (file, path) = file.into_parts();
        let (sender, mut receiver) = mpsc::channel::<bytes::Bytes>(2);
        let task = tokio::spawn(async move {
            let mut file = tokio::fs::File::from_std(file);
            let mut data = Data {
                path,
                prefix: [0; 4],
                prefix_length: 0,
                length: 0,
                #[cfg(test)]
                largest_chunk: 0,
            };
            let result = async {
                while let Some(chunk) = receiver.recv().await {
                    let count = (4 - data.prefix_length).min(chunk.len());
                    data.prefix[data.prefix_length..data.prefix_length + count]
                        .copy_from_slice(&chunk[..count]);
                    data.prefix_length += count;
                    data.length = data.length.checked_add(chunk.len() as u64).ok_or_else(|| {
                        SpeechError::new(
                            500,
                            "O tamanho do arquivo de áudio não pôde ser representado.",
                        )
                    })?;
                    #[cfg(test)]
                    {
                        data.largest_chunk = data.largest_chunk.max(chunk.len());
                    }
                    file.write_all(&chunk)
                        .await
                        .map_err(|_| SpeechError::new(500, "Falha ao gravar o áudio em cache."))?;
                }
                file.flush()
                    .await
                    .map_err(|_| SpeechError::new(500, "Falha ao concluir o arquivo de áudio."))?;
                file.sync_all().await.map_err(|_| {
                    SpeechError::new(500, "Falha ao sincronizar o arquivo de áudio.")
                })?;
                Ok::<_, SpeechError>(())
            }
            .await;
            // A conversão espera qualquer escrita pendente antes de fechar e renomear no Windows.
            drop(file.into_std().await);
            result?;
            Ok(data)
        });
        Ok(Self {
            sender: Some(sender),
            task: Some(task),
            lease: Some(lease),
        })
    }

    pub async fn write(&self, data: &[u8]) -> io::Result<()> {
        let sender = self
            .sender
            .as_ref()
            .ok_or_else(|| io::Error::other("escritor de áudio encerrado"))?;
        for chunk in data.chunks(CHUNK_BYTES) {
            sender
                .send(bytes::Bytes::copy_from_slice(chunk))
                .await
                .map_err(|_| io::Error::other("escrita do áudio interrompida"))?;
        }
        Ok(())
    }

    pub fn close(&mut self) {
        self.sender.take();
    }

    async fn completed(&mut self) -> Result<Data, SpeechError> {
        self.close();
        let result = self
            .task
            .as_mut()
            .ok_or_else(|| SpeechError::new(500, "O escritor de áudio já foi recolhido."))?
            .await
            .map_err(|_| SpeechError::new(500, "Não foi possível concluir o arquivo de áudio."));
        self.task.take();
        result?
    }

    pub async fn finish(&mut self) -> Result<AudioFile, SpeechError> {
        let data = self.completed().await?;
        Ok(AudioFile {
            data,
            _lease: self.lease.take(),
        })
    }

    pub async fn discard(mut self) {
        if self.task.is_some() && self.completed().await.is_err() {
            tracing::warn!(
                code = "speech_audio_cleanup_failed",
                "Não foi possível concluir o descarte do áudio temporário."
            );
        }
        self.lease.take();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{sync::Arc, time::Duration};

    #[tokio::test]
    async fn streamed_audio_keeps_only_a_prefix_and_bounded_chunks_until_publication() {
        let directory = tempfile::tempdir().unwrap();
        let lock = Arc::new(tokio::sync::Mutex::new(()));
        let mut transfer =
            Transfer::new(directory.path(), lock.clone().lock_owned().await).unwrap();
        for piece in [b"R".as_slice(), b"I", b"FF"] {
            transfer.write(piece).await.unwrap();
        }
        let input = vec![0; CHUNK_BYTES * 4 + 3];
        for _ in 0..16 {
            transfer.write(&input).await.unwrap();
        }
        let file = transfer.finish().await.unwrap();
        assert_eq!(file.prefix(), b"RIFF");
        assert_eq!(file.data.length, 4 + (input.len() * 16) as u64);
        assert_eq!(file.data.largest_chunk, CHUNK_BYTES);
        assert_eq!(
            std::fs::metadata(&file.data.path).unwrap().len(),
            file.data.length
        );
        let path = file.data.path.to_path_buf();
        assert!(lock.try_lock().is_err());
        drop(file);
        assert!(!path.exists());
        assert!(lock.try_lock().is_ok());
    }

    #[tokio::test]
    async fn cancelling_a_transfer_drains_its_writer_before_releasing_the_hash() {
        let directory = tempfile::tempdir().unwrap();
        let lock = Arc::new(tokio::sync::Mutex::new(()));
        let transfer = Transfer::new(directory.path(), lock.clone().lock_owned().await).unwrap();
        transfer.write(b"ID3partial").await.unwrap();
        drop(transfer);
        let guard = tokio::time::timeout(Duration::from_secs(5), lock.lock())
            .await
            .unwrap();
        assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 0);
        drop(guard);
    }
}

impl Drop for Transfer {
    fn drop(&mut self) {
        self.close();
        let Some(task) = self.task.take() else { return };
        let lease = self.lease.take();
        if let Ok(runtime) = tokio::runtime::Handle::try_current() {
            runtime.spawn(async move {
                // Nunca abortar o escritor: ele fecha o arquivo antes de apagar o temporário.
                let result = task.await;
                if !matches!(&result, Ok(Ok(_))) {
                    tracing::warn!(
                        code = "speech_audio_cleanup_failed",
                        "Não foi possível concluir o descarte do áudio temporário."
                    );
                }
                drop(result);
                drop(lease);
            });
        }
    }
}
