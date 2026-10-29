use super::{
    Lease, SpeechConfig, SpeechError,
    audio::{AudioFile, CHUNK_BYTES, Transfer},
    valid_audio,
};
use crate::terminal_process::{CommandTree, OwnedTree};
use std::{io, process::Stdio, time::Duration};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWriteExt},
    process::{Child, Command},
};

/// Sem shell: as aspas só delimitam o argv e o texto da conversa passa pelo stdin.
pub(super) fn split(command: &str) -> Result<Vec<String>, SpeechError> {
    let mut args = Vec::new();
    let (mut word, mut quote, mut active) = (String::new(), None, false);
    let mut chars = command.chars();
    while let Some(c) = chars.next() {
        match (quote, c) {
            (Some('\''), '\'') | (Some('"'), '"') => quote = None,
            (Some('\''), _) => word.push(c),
            (Some('"'), '\\') => {
                let next = chars.next().ok_or_else(|| {
                    SpeechError::new(503, "Comando de voz local malformado: escape incompleto.")
                })?;
                if !matches!(next, '\\' | '"') {
                    word.push('\\');
                }
                word.push(next);
            }
            (Some('"'), _) => word.push(c),
            (None, '\'' | '"') => {
                quote = Some(c);
                active = true;
            }
            (None, '\\') => {
                word.push(chars.next().ok_or_else(|| {
                    SpeechError::new(503, "Comando de voz local malformado: escape incompleto.")
                })?);
                active = true;
            }
            (None, ' ' | '\t' | '\r' | '\n') => {
                if active {
                    args.push(std::mem::take(&mut word));
                    active = false;
                }
            }
            _ => {
                word.push(c);
                active = true;
            }
        }
    }
    if quote.is_some() {
        return Err(SpeechError::new(
            503,
            "Comando de voz local malformado: aspas sem par.",
        ));
    }
    if active {
        args.push(word);
    }
    if args.first().is_none_or(String::is_empty) {
        return Err(SpeechError::new(
            503,
            "Comando de voz local sem executável.",
        ));
    }
    Ok(args)
}

struct Managed<T: OwnedTree + 'static = CommandTree> {
    child: Option<Child>,
    tree: Option<T>,
    transfer: Option<Transfer>,
}

impl<T: OwnedTree + 'static> Managed<T> {
    async fn stop(&mut self) -> io::Result<Option<std::process::ExitStatus>> {
        if let Some(tree) = &mut self.tree {
            crate::terminal_process::finish(tree).await;
        }
        let status = match &mut self.child {
            Some(child) => Some(child.wait().await?),
            None => None,
        };
        self.tree.take();
        self.child.take();
        Ok(status)
    }
}

impl<T: OwnedTree + 'static> Drop for Managed<T> {
    fn drop(&mut self) {
        let mut tree = self.tree.take();
        if tree.as_mut().is_some_and(|tree| tree.terminate().is_err()) {
            tracing::warn!(
                code = "speech_process_cleanup_failed",
                "Não foi possível sinalizar o fim do motor local."
            );
        }
        let mut child = self.child.take();
        let mut transfer = self.transfer.take();
        if let Some(transfer) = &mut transfer {
            transfer.close();
        }
        if tree.is_none() && child.is_none() && transfer.is_none() {
            return;
        }
        if let Ok(runtime) = tokio::runtime::Handle::try_current() {
            runtime.spawn(async move {
                if let Some(tree) = &mut tree {
                    crate::terminal_process::finish(tree).await;
                }
                if let Some(child) = &mut child {
                    let _ = child.wait().await;
                }
                // O guard continua dentro da transferência até a árvore e o escritor terminarem.
                if let Some(transfer) = transfer {
                    transfer.discard().await;
                }
            });
        } else if let Some(child) = &mut child {
            let _ = child.start_kill();
        }
    }
}

async fn audio(mut pipe: impl AsyncRead + Unpin, transfer: &Transfer) -> io::Result<()> {
    let mut chunk = vec![0; CHUNK_BYTES];
    loop {
        let count = pipe.read(&mut chunk).await?;
        if count == 0 {
            return Ok(());
        }
        transfer.write(&chunk[..count]).await?;
    }
}

async fn error_text(mut pipe: impl AsyncRead + Unpin) -> io::Result<String> {
    let mut bytes = Vec::new();
    let mut chunk = [0; 4096];
    loop {
        let count = pipe.read(&mut chunk).await?;
        if count == 0 {
            break;
        }
        // Continua drenando para não prender o processo, conservando só o trecho exibível.
        if bytes.len() < 4096 {
            bytes.extend_from_slice(&chunk[..count.min(4096 - bytes.len())]);
        }
    }
    Ok(String::from_utf8_lossy(&bytes).chars().take(300).collect())
}

pub(super) async fn run(
    config: &SpeechConfig,
    text: &str,
    lease: Lease,
) -> Result<AudioFile, SpeechError> {
    run_inner(config, text, Duration::from_secs(180), lease).await
}

pub(super) fn isolate(command: &mut Command) {
    // O motor escolhido pode ser um wrapper de API; só a autoridade do Hangar é retirada.
    for key in crate::terminal_process::PRIVATE_ENV_KEYS {
        command.env_remove(key);
    }
}

#[cfg(test)]
pub(super) async fn run_with_deadline(
    config: &SpeechConfig,
    text: &str,
    timeout: Duration,
) -> Result<AudioFile, SpeechError> {
    let lease = std::sync::Arc::new(tokio::sync::Mutex::new(()))
        .lock_owned()
        .await;
    run_inner(config, text, timeout, lease).await
}

async fn deadline<T, F>(timeout: Duration, work: F) -> Result<T, SpeechError>
where
    F: std::future::Future<Output = Result<T, SpeechError>>,
{
    tokio::time::timeout(timeout, work)
        .await
        .unwrap_or_else(|_| {
            Err(SpeechError::new(
                504,
                format!("O comando de voz local passou de {} s.", timeout.as_secs()),
            ))
        })
}

async fn run_inner(
    config: &SpeechConfig,
    text: &str,
    timeout: Duration,
    lease: Lease,
) -> Result<AudioFile, SpeechError> {
    let configured = config.string("tts_local_cmd");
    if configured.trim().is_empty() {
        return Err(SpeechError::new(
            503,
            "Comando de voz local não configurado.",
        ));
    }
    let args = split(configured.trim())?;
    let mut owned = Managed {
        child: None,
        tree: None,
        transfer: Some(Transfer::new(&config.cache_dir, lease)?),
    };
    let mut command = Command::new(&args[0]);
    command
        .args(&args[1..])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    isolate(&mut command);
    let tree = CommandTree::configure(&mut command)
        .map_err(|_| SpeechError::new(502, "Não foi possível controlar o motor de voz local."))?;
    let child = command.spawn().map_err(|error| {
        if error.kind() == io::ErrorKind::NotFound {
            SpeechError::new(
                502,
                format!("Comando de voz local não encontrado: {}", args[0]),
            )
        } else {
            SpeechError::new(502, "Não foi possível iniciar o comando de voz local.")
        }
    })?;
    owned.child = Some(child);
    owned.tree = Some(tree);
    if owned
        .tree
        .as_mut()
        .unwrap()
        .attach(owned.child.as_ref().unwrap())
        .is_err()
    {
        return Err(SpeechError::new(
            502,
            "Não foi possível controlar o motor de voz local.",
        ));
    }
    deadline(timeout, async {
        let transfer = owned.transfer.as_ref().unwrap();
        let child = owned.child.as_mut().unwrap();
        let mut stdin = child.stdin.take().unwrap();
        let stdout = child.stdout.take().unwrap();
        let stderr = child.stderr.take().unwrap();
        let result = tokio::try_join!(
            async {
                let result = stdin.write_all(text.as_bytes()).await;
                drop(stdin);
                match result {
                    Err(error) if error.kind() == io::ErrorKind::BrokenPipe => Ok(()),
                    result => result,
                }
            },
            audio(stdout, transfer),
            error_text(stderr),
            crate::terminal_process::leader_exited(child),
        );
        let status = owned.stop().await
            .map_err(|_| SpeechError::new(502, "Não foi possível confirmar o fim do motor de voz local."))?
            .ok_or_else(|| SpeechError::new(502, "O motor de voz local perdeu a identidade."))?;
        let output = owned.transfer.as_mut().unwrap().finish().await?;
        let (_, (), error, ()) = result.map_err(|_| SpeechError::new(
                502,
                "Falha ao ler ou escrever o áudio do comando de voz local.",
        ))?;
        if !status.success() {
            let error = if error.is_empty() {
                format!("código {}", status.code().map_or_else(|| "desconhecido".to_owned(), |v| v.to_string()))
            } else { error };
            return Err(SpeechError::new(502, format!("Comando de voz local falhou: {error}")));
        }
        if output.is_empty() {
            return Err(SpeechError::new(502, "O comando de voz local não escreveu áudio na saída."));
        }
        if !valid_audio(output.prefix()) {
            return Err(SpeechError::new(502, "O comando de voz local devolveu um formato de áudio não reconhecido (nem WAV nem MP3)."));
        }
        owned.transfer.take();
        Ok(output)
    }).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    };

    struct DelayedTree {
        active: Arc<AtomicBool>,
        attempts: Arc<AtomicUsize>,
    }

    impl OwnedTree for DelayedTree {
        fn active(&mut self) -> io::Result<bool> {
            Ok(self.active.load(Ordering::Acquire))
        }
        fn terminate(&mut self) -> io::Result<()> {
            self.attempts.fetch_add(1, Ordering::Release);
            Ok(())
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_504_returns_on_deadline_but_a_live_tree_keeps_the_hash_until_cleanup_finishes() {
        let directory = tempfile::tempdir().unwrap();
        let gate = Arc::new(tokio::sync::Mutex::new(()));
        let active = Arc::new(AtomicBool::new(true));
        let attempts = Arc::new(AtomicUsize::new(0));
        let mut owned = Managed {
            child: None,
            tree: Some(DelayedTree {
                active: active.clone(),
                attempts: attempts.clone(),
            }),
            transfer: Some(
                Transfer::new(directory.path(), gate.clone().lock_owned().await).unwrap(),
            ),
        };
        owned
            .transfer
            .as_ref()
            .unwrap()
            .write(b"RIFFpartial")
            .await
            .unwrap();
        let started = tokio::time::Instant::now();
        let response = deadline(Duration::from_millis(40), async {
            owned
                .stop()
                .await
                .map(|_| ())
                .map_err(|_| SpeechError::new(502, "Falha de limpeza."))
        })
        .await
        .unwrap_err();
        assert_eq!(response.status, 504);
        assert!(started.elapsed() < Duration::from_secs(1));
        drop(owned);
        assert!(gate.try_lock().is_err());
        let waiting = super::super::acquire(gate.clone(), Duration::from_millis(40))
            .await
            .unwrap_err();
        assert_eq!(waiting.status, 503);
        assert!(gate.try_lock().is_err());
        let next_gate = gate.clone();
        let mut next = tokio::spawn(async move { next_gate.lock_owned().await });
        assert!(
            tokio::time::timeout(Duration::from_millis(60), &mut next)
                .await
                .is_err()
        );
        assert!(attempts.load(Ordering::Acquire) > 0);
        active.store(false, Ordering::Release);
        let _guard = tokio::time::timeout(Duration::from_secs(5), next)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 0);
    }
}
