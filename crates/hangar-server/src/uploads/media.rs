//! Derivados best-effort, publicados pelo mesmo cofre; transcrição usa o provedor existente.
use super::store::UploadStore;
use bytes::Bytes;
use futures_util::stream;
use std::{
    collections::HashMap,
    future::Future,
    io,
    path::{Path, PathBuf},
    process::Stdio,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::{io::AsyncReadExt, sync::watch};

/// Quadros e fala de um vídeo, entregues depois do upload (`media=later`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Media {
    pub frames: Vec<PathBuf>,
    pub transcript: String,
}

/// Teto de um pedido de `upload-media`: abaixo do tempo que o envio nativo espera sem resposta.
pub const WAIT_CAP: Duration = Duration::from_secs(45);
/// Quanto tempo o resultado fica guardado depois de pronto.
const KEEP: Duration = Duration::from_secs(600);

/// Mídia em processamento ou pronta, só pelo id: a sessão muda no meio (`/clear`, troca de nome,
/// conversa nova), o id não, e ele é aleatório o bastante para valer como chave de acesso. Só em
/// memória: reiniciar o servidor perde o registro, e o cliente segue só com o vídeo, como quando a
/// transcrição falha.
#[derive(Clone)]
pub struct MediaJobs {
    jobs: Arc<Mutex<HashMap<String, watch::Receiver<Option<Media>>>>>,
    // Uma por vez: cada uma roda ffmpeg e ocupa uma vaga de transcrição até o fim.
    queue: Arc<tokio::sync::Semaphore>,
}

pub enum Waited {
    Unknown,
    Pending,
    Done(Media),
}

impl Default for MediaJobs {
    fn default() -> Self {
        Self { jobs: Arc::default(), queue: Arc::new(tokio::sync::Semaphore::new(1)) }
    }
}

impl MediaJobs {
    pub fn start(&self, id: String, work: impl Future<Output = Media> + Send + 'static) {
        let (tx, rx) = watch::channel(None);
        self.jobs.lock().unwrap_or_else(|e| e.into_inner()).insert(id.clone(), rx);
        let (jobs, queue) = (self.jobs.clone(), self.queue.clone());
        tokio::spawn(async move {
            let turn = queue.acquire_owned().await;
            // Tarefa própria: se o trabalho cair, o registro ainda sai no fim do prazo.
            if let Ok(media) = tokio::spawn(work).await {
                tx.send_replace(Some(media));
            }
            drop((turn, tx));
            tokio::time::sleep(KEEP).await;
            jobs.lock().unwrap_or_else(|e| e.into_inner()).remove(&id);
        });
    }

    pub async fn wait(&self, id: &str, cap: Duration) -> Waited {
        let Some(mut rx) = self.jobs.lock().unwrap_or_else(|e| e.into_inner()).get(id).cloned() else {
            return Waited::Unknown;
        };
        match tokio::time::timeout(cap, rx.wait_for(Option::is_some)).await {
            Ok(Ok(media)) => media.clone().map_or(Waited::Pending, Waited::Done),
            // A tarefa caiu sem entregar: sem isto o cliente esperaria para sempre.
            Ok(Err(_)) => Waited::Unknown,
            Err(_) => Waited::Pending,
        }
    }
}

fn command(program: &str, args: &[String]) -> tokio::process::Command {
    let mut command = tokio::process::Command::new(program);
    command
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    for key in crate::terminal_process::PRIVATE_ENV_KEYS {
        command.env_remove(key);
    }
    command
}

async fn run(program: &str, args: &[String], timeout: u64) -> Option<Vec<u8>> {
    let mut command = command(program, args);
    let mut tree = crate::terminal_process::CommandTree::configure(&mut command).ok()?;
    let mut child = command.spawn().ok()?;
    if tree.attach(&child).is_err() {
        let _ = child.kill().await;
        return None;
    }
    let mut stdout = child.stdout.take()?;
    let read = tokio::spawn(async move {
        let mut output = Vec::new();
        let mut buffer = [0; 4096];
        loop {
            let n = stdout.read(&mut buffer).await?;
            if n == 0 {
                return Ok::<_, io::Error>(output);
            }
            if output.len() < 4096 {
                output.extend_from_slice(&buffer[..n.min(4096 - output.len())]);
            }
        }
    });
    let waited = tokio::time::timeout(
        Duration::from_secs(timeout),
        crate::terminal_process::leader_exited(&mut child),
    )
    .await;
    crate::terminal_process::finish(&mut tree).await;
    let status = child.wait().await.ok();
    let output = read.await.ok().and_then(Result::ok);
    if matches!(waited, Ok(Ok(()))) && status.is_some_and(|s| s.success()) {
        output
    } else {
        None
    }
}

fn args(values: &[&str]) -> Vec<String> {
    values.iter().map(|s| s.to_string()).collect()
}
async fn publish(
    store: &UploadStore,
    project: &str,
    session: &str,
    source: &Path,
    name: &str,
) -> Option<PathBuf> {
    let file = tokio::fs::File::open(source).await.ok()?;
    let stream = stream::try_unfold(file, |mut file| async move {
        let mut chunk = vec![0; 64 * 1024];
        let n = file.read(&mut chunk).await?;
        chunk.truncate(n);
        Ok::<_, io::Error>((n > 0).then(|| (Bytes::from(chunk), file)))
    });
    store
        .publish_derivative(project, session, name, stream)
        .await
        .ok()
}

pub async fn extract(
    store: &UploadStore,
    project: &str,
    session: &str,
    path: &Path,
) -> (Vec<PathBuf>, Option<Vec<u8>>) {
    if !is_video(path) {
        return (vec![], None);
    }
    let scratch = std::env::temp_dir().join(format!(
        "hangar-media-{}",
        match crate::accounts::claude_login::nonce() {
            Ok(v) => v,
            Err(_) => return (vec![], None),
        }
    ));
    let Ok(_owned) = crate::accounts::storage::NewDirectory::create(&scratch) else {
        return (vec![], None);
    };
    let Some(name) = path.file_name().and_then(|s| s.to_str()) else {
        return (vec![], None);
    };
    let Ok((_, retained)) = store.open(project, session, name) else {
        return (vec![], None);
    };
    let ext = path.extension().and_then(|s| s.to_str()).unwrap_or("").to_ascii_lowercase();
    let src = scratch.join(format!("source.{ext}"));
    let Ok(mut output) = tokio::fs::File::create(&src).await else {
        return (vec![], None);
    };
    if tokio::io::copy(&mut tokio::fs::File::from_std(retained), &mut output)
        .await
        .is_err()
    {
        return (vec![], None);
    };
    drop(output);
    let src = src.to_string_lossy().into_owned();
    let probe = run(
        "ffprobe",
        &args(&[
            "-v",
            "error",
            "-show_entries",
            "format=duration",
            "-of",
            "default=noprint_wrappers=1:nokey=1",
            &src,
        ]),
        30,
    )
    .await;
    let duration = probe
        .and_then(|v| String::from_utf8(v).ok())
        .and_then(|v| v.trim().parse::<f64>().ok())
        .filter(|v| v.is_finite() && *v > 0.0)
        .unwrap_or(0.0);
    let marks = frame_positions(duration);
    let stem = path.file_stem().unwrap_or_default().to_string_lossy();
    let mut frames = vec![];
    for (i, time) in marks.into_iter().enumerate() {
        let dest = scratch.join(format!("frame-{i}.jpg"));
        let dest_text = dest.to_string_lossy();
        if run(
            "ffmpeg",
            &args(&[
                "-nostdin",
                "-y",
                "-ss",
                &format!("{time:.2}"),
                "-i",
                &src,
                "-frames:v",
                "1",
                "-vf",
                "scale='min(1568,iw)':-2",
                "-q:v",
                "4",
                &dest_text,
            ]),
            25,
        )
        .await
        .is_some()
            && let Some(path) = publish(
                store,
                project,
                session,
                &dest,
                &format!("{stem}-q{}.jpg", i + 1),
            )
            .await
        {
            frames.push(path);
        }
    }
    let audio = scratch.join("audio.m4a");
    let audio_text = audio.to_string_lossy();
    let audio = if run(
        "ffmpeg",
        &args(&[
            "-nostdin",
            "-y",
            "-i",
            &src,
            "-vn",
            "-ac",
            "1",
            "-ar",
            "16000",
            "-c:a",
            "aac",
            "-b:a",
            "64k",
            &audio_text,
        ]),
        90,
    )
    .await
    .is_some()
    {
        if publish(
            store,
            project,
            session,
            &audio,
            &format!("{stem}-audio.m4a"),
        )
        .await
        .is_some()
        {
            tokio::fs::read(&audio).await.ok()
        } else {
            None
        }
    } else {
        None
    };
    (frames, audio)
}

pub fn is_video(path: &Path) -> bool {
    let ext = path.extension().and_then(|s| s.to_str()).unwrap_or("").to_ascii_lowercase();
    ["mp4", "mov", "webm", "mkv", "m4v", "avi"].contains(&ext.as_str())
}

pub fn frame_positions(duration: f64) -> Vec<f64> {
    if duration > 0.0 && duration.is_finite() {
        (0..6).map(|i| duration * (i as f64 + 0.5) / 6.0).collect()
    } else {
        vec![0.0]
    }
}
/// Quadros e fala do vídeo; outro arquivo devolve vazio.
pub async fn process(st: &crate::routes::AppState, store: &UploadStore, project: &str, session: &str, path: &Path) -> Media {
    let (frames, audio) = extract(store, project, session, path).await;
    let transcript = match audio {
        Some(audio) => transcribe(st, audio).await,
        None => String::new(),
    };
    Media { frames, transcript }
}

/// O áudio do vídeo, no perfil de vídeo; falha vira texto vazio e o anexo segue.
pub async fn transcribe(st: &crate::routes::AppState, audio: Vec<u8>) -> String {
    crate::transcription::routes::transcribe_here(st, audio.into(), Some("audio.m4a".into()), crate::transcription::model::Profile::Video)
        .await
        .map(|t| t.text.trim().to_owned())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn done(text: &str) -> Media {
        Media { frames: vec![PathBuf::from("/up/q1.jpg")], transcript: text.into() }
    }

    #[tokio::test]
    async fn wait_returns_pending_at_the_cap_and_the_result_once_ready() {
        let jobs = MediaJobs::default();
        let (finish, finished) = tokio::sync::oneshot::channel::<()>();
        jobs.start("id1".into(), async move {
            finished.await.ok();
            done("ola")
        });
        assert!(matches!(jobs.wait("id1", Duration::from_millis(20)).await, Waited::Pending));
        finish.send(()).unwrap();
        match jobs.wait("id1", Duration::from_secs(5)).await {
            Waited::Done(media) => assert_eq!(media, done("ola")),
            _ => panic!("esperava a mídia pronta"),
        }
    }

    #[tokio::test]
    async fn unknown_id_or_a_job_that_died_is_unknown() {
        let jobs = MediaJobs::default();
        assert!(matches!(jobs.wait("nada", Duration::from_millis(20)).await, Waited::Unknown));
        jobs.start("caiu".into(), async { panic!("ffmpeg caiu") });
        assert!(matches!(jobs.wait("caiu", Duration::from_secs(5)).await, Waited::Unknown));
    }

    #[tokio::test]
    async fn jobs_run_one_at_a_time() {
        let jobs = MediaJobs::default();
        let (finish, finished) = tokio::sync::oneshot::channel::<()>();
        jobs.start("primeiro".into(), async move {
            finished.await.ok();
            done("1")
        });
        jobs.start("segundo".into(), async { done("2") });
        assert!(matches!(jobs.wait("segundo", Duration::from_millis(50)).await, Waited::Pending));
        finish.send(()).unwrap();
        assert!(matches!(jobs.wait("segundo", Duration::from_secs(5)).await, Waited::Done(_)));
    }

    #[test]
    fn media_tools_do_not_inherit_server_authority() {
        let command = command("ffprobe", &args(&["-v", "error"]));
        let removed: Vec<_> = command
            .as_std()
            .get_envs()
            .filter(|(_, value)| value.is_none())
            .map(|(key, _)| key.to_string_lossy().into_owned())
            .collect();
        for key in [
            "HANGAR_INTERNAL_SECRET",
            "CP_AUTH_TOKEN",
            "HANGAR_RUNTIME_INSTANCE", "HANGAR_AUTH_CONTEXT",
            "HANGAR_PLUGIN_TOKEN",
        ] {
            assert!(removed.iter().any(|name| name == key), "{key} chega ao ffmpeg");
        }
    }
    #[test]
    fn frames_cover_entire_video_and_unknown_duration_uses_start() {
        assert_eq!(
            frame_positions(120.0),
            vec![10.0, 30.0, 50.0, 70.0, 90.0, 110.0]
        );
        assert_eq!(frame_positions(0.0), vec![0.0]);
        assert_eq!(frame_positions(f64::NAN), vec![0.0]);
    }
}
