use super::{SpeechConfig, SpeechError, valid_audio};
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

struct Managed {
    child: Option<Child>,
    tree: Option<CommandTree>,
}

impl Managed {
    async fn stop(&mut self) -> io::Result<std::process::ExitStatus> {
        if let Some(tree) = &mut self.tree {
            crate::terminal_process::finish(tree).await;
        }
        let status = self.child.as_mut().unwrap().wait().await;
        self.tree.take();
        self.child.take();
        status
    }
}

impl Drop for Managed {
    fn drop(&mut self) {
        let Some(mut tree) = self.tree.take() else {
            return;
        };
        if tree.terminate().is_err() {
            tracing::warn!(
                code = "speech_process_cleanup_failed",
                "Não foi possível sinalizar o fim do motor local."
            );
        }
        let mut child = self.child.take();
        if let Ok(runtime) = tokio::runtime::Handle::try_current() {
            runtime.spawn(async move {
                crate::terminal_process::finish(&mut tree).await;
                if let Some(child) = &mut child {
                    let _ = child.wait().await;
                }
            });
        } else if let Some(child) = &mut child {
            let _ = child.start_kill();
        }
    }
}

async fn audio(mut pipe: impl AsyncRead + Unpin) -> io::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    pipe.read_to_end(&mut bytes).await?;
    Ok(bytes)
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

pub(super) async fn run(config: &SpeechConfig, text: &str) -> Result<Vec<u8>, SpeechError> {
    run_with_deadline(config, text, Duration::from_secs(180)).await
}

pub(super) fn isolate(command: &mut Command) {
    // O motor escolhido pode ser um wrapper de API; só a autoridade do Hangar é retirada.
    for key in crate::terminal_process::PRIVATE_ENV_KEYS {
        command.env_remove(key);
    }
}

pub(super) async fn run_with_deadline(
    config: &SpeechConfig,
    text: &str,
    timeout: Duration,
) -> Result<Vec<u8>, SpeechError> {
    let configured = config.string("tts_local_cmd");
    if configured.trim().is_empty() {
        return Err(SpeechError::new(
            503,
            "Comando de voz local não configurado.",
        ));
    }
    let args = split(configured.trim())?;
    let mut command = Command::new(&args[0]);
    command
        .args(&args[1..])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    isolate(&mut command);
    let mut tree = CommandTree::configure(&mut command)
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
    let mut owned = Managed {
        child: Some(child),
        tree: None,
    };
    if tree.attach(owned.child.as_ref().unwrap()).is_err() {
        let _ = owned.child.as_mut().unwrap().kill().await;
        return Err(SpeechError::new(
            502,
            "Não foi possível controlar o motor de voz local.",
        ));
    }
    owned.tree = Some(tree);
    let child = owned.child.as_mut().unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let stdout = child.stdout.take().unwrap();
    let stderr = child.stderr.take().unwrap();
    let operation = async {
        let (_, output, error, ()) = tokio::try_join!(
            async {
                let result = stdin.write_all(text.as_bytes()).await;
                drop(stdin);
                match result {
                    Err(error) if error.kind() == io::ErrorKind::BrokenPipe => Ok(()),
                    result => result,
                }
            },
            audio(stdout),
            error_text(stderr),
            crate::terminal_process::leader_exited(child),
        )?;
        Ok::<_, io::Error>((output, error))
    };
    let result = tokio::time::timeout(timeout, operation).await;
    let status = owned.stop().await.map_err(|_| {
        SpeechError::new(
            502,
            "Não foi possível confirmar o fim do motor de voz local.",
        )
    })?;
    let (output, error) = match result {
        Err(_) => {
            return Err(SpeechError::new(
                504,
                format!("O comando de voz local passou de {} s.", timeout.as_secs()),
            ));
        }
        Ok(Err(_)) => {
            return Err(SpeechError::new(
                502,
                "Falha ao ler ou escrever o áudio do comando de voz local.",
            ));
        }
        Ok(Ok(value)) => value,
    };
    if !status.success() {
        let error = if error.is_empty() {
            format!(
                "código {}",
                status
                    .code()
                    .map_or_else(|| "desconhecido".to_owned(), |v| v.to_string())
            )
        } else {
            error
        };
        return Err(SpeechError::new(
            502,
            format!("Comando de voz local falhou: {error}"),
        ));
    }
    if output.is_empty() {
        return Err(SpeechError::new(
            502,
            "O comando de voz local não escreveu áudio na saída.",
        ));
    }
    if !valid_audio(&output) {
        return Err(SpeechError::new(
            502,
            "O comando de voz local devolveu um formato de áudio não reconhecido (nem WAV nem MP3).",
        ));
    }
    Ok(output)
}
