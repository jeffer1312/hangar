//! Derivados best-effort, publicados pelo mesmo cofre; transcrição usa o provedor existente.
use super::store::UploadStore;
use bytes::Bytes;
use futures_util::stream;
use std::{
    io,
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};
use tokio::io::AsyncReadExt;

async fn run(program: &str, args: &[String], timeout: u64) -> Option<Vec<u8>> {
    let mut command = tokio::process::Command::new(program);
    command
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true);
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
    let ext = path
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    if !["mp4", "mov", "webm", "mkv", "m4v", "avi"].contains(&ext.as_str()) {
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

pub fn frame_positions(duration: f64) -> Vec<f64> {
    if duration > 0.0 && duration.is_finite() {
        (0..6).map(|i| duration * (i as f64 + 0.5) / 6.0).collect()
    } else {
        vec![0.0]
    }
}
pub async fn transcribe(st: &crate::routes::AppState, name: &str, audio: Vec<u8>) -> String {
    use base64::Engine;
    use http_body_util::BodyExt;
    let name = percent_encoding::utf8_percent_encode(name, percent_encoding::NON_ALPHANUMERIC);
    // Base64 não tem caractere a escapar em JSON: o corpo sai sem cópia intermediária.
    let body = format!(
        r#"{{"audio":"{}"}}"#,
        base64::engine::general_purpose::STANDARD.encode(audio)
    );
    let Ok(request) = axum::http::Request::post(format!(
        "http://{}/internal/sessions/{name}/upload-transcript",
        st.cfg.upstream
    ))
    .header("x-hangar-internal", &st.cfg.internal_secret)
    .header("content-type", "application/json")
    .body(axum::body::Body::from(body)) else {
        return String::new();
    };
    let bytes = tokio::time::timeout(Duration::from_secs(120), async {
        let response = st.http.request(request).await.ok()?;
        if !response.status().is_success() {
            return None;
        }
        Some(response.into_body().collect().await.ok()?.to_bytes())
    })
    .await
    .ok()
    .flatten();
    bytes
        .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
        .and_then(|v| v["text"].as_str().map(str::to_owned))
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
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
