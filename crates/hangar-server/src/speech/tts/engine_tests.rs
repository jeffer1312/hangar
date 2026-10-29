use super::*;
use proptest::prelude::*;
use serde_json::json;
use std::{
    path::Path,
    sync::Arc,
    time::{Duration, SystemTime},
};

fn config(dir: &Path) -> SpeechConfig {
    SpeechConfig {
        values: json!({}),
        home: dir.to_owned(),
        cache_dir: dir.join("cache"),
    }
}

async fn empty_cache(directory: &Path) {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if std::fs::read_dir(directory).unwrap().next().is_none() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
}

#[test]
fn a_short_or_interrupted_read_fills_the_prefix_before_audio_validation() {
    struct Short<'a> {
        bytes: &'a [u8],
        interrupted: bool,
    }
    impl std::io::Read for Short<'_> {
        fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
            if self.interrupted {
                self.interrupted = false;
                return Err(std::io::ErrorKind::Interrupted.into());
            }
            if self.bytes.is_empty() || buffer.is_empty() {
                return Ok(0);
            }
            buffer[0] = self.bytes[0];
            self.bytes = &self.bytes[1..];
            Ok(1)
        }
    }
    for bytes in [b"RIFF".as_slice(), b"ID3", b"\xff\xfb", b"\xff"] {
        let (prefix, length) = cache::read_prefix(&mut Short {
            bytes,
            interrupted: true,
        })
        .unwrap();
        assert_eq!(&prefix[..length], bytes);
        assert_eq!(valid_audio(&prefix[..length]), bytes.len() >= 2);
    }
}

#[tokio::test]
async fn an_invalid_mp3_falls_back_to_the_existing_wav_without_running_a_provider() {
    let directory = tempfile::tempdir().unwrap();
    let c = config(directory.path());
    let hash = hash_value("oi", "", "local", &json!({}), "");
    std::fs::create_dir_all(&c.cache_dir).unwrap();
    let mp3 = c.cache_dir.join(format!("{hash}.mp3"));
    let wav = c.cache_dir.join(format!("{hash}.wav"));
    for invalid in [b"garbage".as_slice(), b""] {
        std::fs::write(&mp3, invalid).unwrap();
        std::fs::write(&wav, b"RIFFvalid").unwrap();
        let result = Service::default()
            .synthesize(&c, "oi", "", "local", "")
            .await
            .unwrap();
        assert!(result.cached);
        assert_eq!(cache_path(&c, &result.hash), wav);
        assert!(!mp3.exists());
        assert_eq!(std::fs::read(&wav).unwrap(), b"RIFFvalid");
    }
}

fn command(directory: &Path, mode: &str, paths: &[&Path]) -> String {
    // O executável pertence ao TempDir do teste e sai junto com ele, inclusive no Windows.
    let program = directory.join(if cfg!(windows) {
        "speech-fixture.exe"
    } else {
        "speech-fixture"
    });
    if !program.exists() {
        let output = std::process::Command::new("rustc")
            .args(["--edition=2024", "--crate-name", "speech_fixture"])
            .arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("src/speech/tts/fixture.rs"))
            .arg("-o")
            .arg(&program)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    std::iter::once(program.to_string_lossy().into_owned())
        .chain(std::iter::once(mode.to_owned()))
        .chain(paths.iter().map(|p| p.to_string_lossy().into_owned()))
        .map(|v| crate::orchestration::roles::quote(&v))
        .collect::<Vec<_>>()
        .join(" ")
}

#[test]
fn hashes_match_python_bytes_including_float_spacing_and_empty_instruction() {
    let fixture: Value = serde_json::from_str(include_str!(
        "../../../tests/fixtures/speech/tts_python.json"
    ))
    .unwrap();
    for case in fixture["hashes"].as_array().unwrap() {
        assert_eq!(
            hash_value(
                case["text"].as_str().unwrap(),
                case["voice"].as_str().unwrap(),
                case["provider"].as_str().unwrap(),
                &case["adjustments"],
                case["instruction"].as_str().unwrap()
            ),
            case["hash"].as_str().unwrap(),
            "{case}"
        );
    }
}

#[test]
fn local_argv_matches_python_shlex_including_quoted_dollar_and_unicode_spaces() {
    let fixture: Value = serde_json::from_str(include_str!(
        "../../../tests/fixtures/speech/tts_commands_python.json"
    ))
    .unwrap();
    for case in fixture["commands"].as_array().unwrap() {
        let result = local::split(case["input"].as_str().unwrap());
        if case["invalid"] == true {
            assert!(result.is_err(), "{case}");
        } else {
            assert_eq!(json!(result.unwrap()), case["args"], "{case}");
        }
    }
}

#[tokio::test]
async fn local_api_wrappers_keep_provider_credentials_but_never_hangar_authority() {
    let dir = tempfile::tempdir().unwrap();
    let args = local::split(&command(dir.path(), "environment", &[])).unwrap();
    let mut process = tokio::process::Command::new(&args[0]);
    process.args(&args[1..]);
    for key in [
        "CP_GROQ_API_KEY",
        "GROQ_API_KEY",
        "OPENAI_API_KEY",
        "OPENROUTER_API_KEY",
        "ELEVENLABS_API_KEY",
    ] {
        process.env(key, "fixture-provider-value");
    }
    for key in crate::terminal_process::PRIVATE_ENV_KEYS {
        process.env(key, "fixture-authority-value");
    }
    local::isolate(&mut process);
    let output = process.output().await.unwrap();
    assert!(output.status.success());
    assert_eq!(output.stdout, b"ID3providers:11111;authority:000000");
    assert!(output.stderr.is_empty());
}

#[test]
fn adjustments_omit_defaults_coerce_integers_and_clamp_before_the_hash() {
    let dir = tempfile::tempdir().unwrap();
    let mut c = config(dir.path());
    assert_eq!(adjustments(&c), json!({}));
    c.values = json!({"tts_stability":50, "tts_similarity_boost":75,"tts_style":0,"tts_speed":100});
    assert_eq!(adjustments(&c), json!({}));
    c.values =
        json!({"tts_stability":500,"tts_speed":5,"tts_style":"bad","tts_similarity_boost":90});
    assert_eq!(
        adjustments(&c),
        json!({"stability":1.0,"speed":0.7,"similarity_boost":0.9})
    );
}

#[test]
fn audio_signatures_and_cache_extensions_preserve_the_original_priority() {
    for bytes in [b"RIFF".as_slice(), b"ID3", b"\xff\xfb", b"\xff\xe0"] {
        assert!(valid_audio(bytes));
    }
    for bytes in [b"".as_slice(), b"\xff", b"FLAC", b"erro"] {
        assert!(!valid_audio(bytes));
    }
    let dir = tempfile::tempdir().unwrap();
    let c = config(dir.path());
    std::fs::create_dir_all(&c.cache_dir).unwrap();
    let hash = "a".repeat(64);
    let mp3 = c.cache_dir.join(format!("{hash}.mp3"));
    let wav = c.cache_dir.join(format!("{hash}.wav"));
    assert_eq!(cache_path(&c, &hash), mp3);
    std::fs::write(&wav, b"RIFF").unwrap();
    assert_eq!(cache_path(&c, &hash), wav);
    std::fs::write(&mp3, b"ID3").unwrap();
    assert_eq!(cache_path(&c, &hash), mp3);
}

#[tokio::test]
async fn local_fallback_cache_and_concurrent_identical_requests_pay_once() {
    let dir = tempfile::tempdir().unwrap();
    let count = dir.path().join("count");
    let mut c = config(dir.path());
    c.values["tts_local_cmd"] = json!(command(dir.path(), "wav", &[&count]));
    let service = Service::default();
    let (a, b) = tokio::join!(
        service.synthesize(&c, "olá", "", "elevenlabs", ""),
        service.synthesize(&c, "olá", "", "elevenlabs", "")
    );
    let (a, b) = (a.unwrap(), b.unwrap());
    assert_eq!(a.provider, "local");
    assert_eq!(b.provider, "local");
    assert_eq!(a.hash, b.hash);
    assert_ne!(a.cached, b.cached);
    assert_eq!(std::fs::read_to_string(&count).unwrap(), "call\n");
    let path = cache_path(&c, &a.hash);
    assert_eq!(path.extension().unwrap(), "wav");
    assert_eq!(std::fs::read(&path).unwrap(), b"RIFFol\xc3\xa1");
    assert_eq!(std::fs::read_dir(&c.cache_dir).unwrap().count(), 1);
    let old = SystemTime::now() - Duration::from_secs(20 * 86400);
    std::fs::OpenOptions::new()
        .write(true)
        .open(&path)
        .unwrap()
        .set_times(std::fs::FileTimes::new().set_modified(old))
        .unwrap();
    assert!(
        service
            .synthesize(&c, "olá", "", "elevenlabs", "")
            .await
            .unwrap()
            .cached
    );
    assert!(std::fs::metadata(path).unwrap().modified().unwrap() > old);
}

#[tokio::test]
async fn corrupt_and_zero_byte_cache_entries_are_regenerated_without_serving_garbage() {
    let dir = tempfile::tempdir().unwrap();
    let mut c = config(dir.path());
    c.values["tts_local_cmd"] = json!(command(dir.path(), "echo", &[]));
    let service = Service::default();
    let a = service.synthesize(&c, "oi", "", "local", "").await.unwrap();
    for bad in [b"garbage".as_slice(), b""] {
        std::fs::write(cache_path(&c, &a.hash), bad).unwrap();
        assert!(
            !service
                .synthesize(&c, "oi", "", "local", "")
                .await
                .unwrap()
                .cached
        );
        assert_eq!(std::fs::read(cache_path(&c, &a.hash)).unwrap(), b"ID3oi");
    }
}

#[tokio::test]
async fn local_errors_are_visible_and_never_poison_the_cache() {
    let dir = tempfile::tempdir().unwrap();
    let mut c = config(dir.path());
    let service = Service::default();
    assert_eq!(
        service
            .synthesize(&c, "oi", "", "local", "")
            .await
            .unwrap_err()
            .status,
        503
    );
    for (mode, status) in [("empty", 502), ("invalid", 502), ("fail", 502)] {
        c.values["tts_local_cmd"] = json!(command(dir.path(), mode, &[]));
        let error = service
            .synthesize(&c, "oi", "", "local", "")
            .await
            .unwrap_err();
        assert_eq!(error.status, status);
        if mode == "fail" {
            assert_eq!(error.detail.matches('á').count(), 300);
        }
        empty_cache(&c.cache_dir).await;
    }
    c.values["tts_local_cmd"] = json!("missing-speech-executable-fixture");
    assert_eq!(
        service
            .synthesize(&c, "oi", "", "local", "")
            .await
            .unwrap_err()
            .status,
        502
    );
    c.values["tts_local_cmd"] = json!("cmd \"unclosed");
    assert_eq!(
        service
            .synthesize(&c, "oi", "", "local", "")
            .await
            .unwrap_err()
            .status,
        503
    );
}

#[tokio::test]
async fn character_cap_counts_unicode_codepoints_and_refuses_before_creating_the_cache() {
    let dir = tempfile::tempdir().unwrap();
    let mut c = config(dir.path());
    c.values["tts_local_cmd"] = json!(command(dir.path(), "echo", &[]));
    let service = Service::default();
    assert_eq!(
        service
            .synthesize(&c, &"á".repeat(CHARACTER_LIMIT + 1), "", "local", "")
            .await
            .unwrap_err()
            .status,
        413
    );
    assert!(!c.cache_dir.exists());
    let exact = service
        .synthesize(&c, &"á".repeat(CHARACTER_LIMIT), "", "local", "")
        .await
        .unwrap();
    assert_eq!(
        std::fs::read(cache_path(&c, &exact.hash)).unwrap().len(),
        3 + CHARACTER_LIMIT * 2
    );
}

#[tokio::test]
async fn an_old_empty_mp3_never_hides_a_new_wav_and_disk_failures_are_explicit() {
    let dir = tempfile::tempdir().unwrap();
    let mut c = config(dir.path());
    c.values["tts_local_cmd"] = json!(command(dir.path(), "wav", &[]));
    let hash = hash_value("oi", "", "local", &json!({}), "");
    std::fs::create_dir_all(&c.cache_dir).unwrap();
    std::fs::write(c.cache_dir.join(format!("{hash}.mp3")), b"").unwrap();
    let service = Service::default();
    let result = service.synthesize(&c, "oi", "", "local", "").await.unwrap();
    assert!(!result.cached);
    assert_eq!(cache_path(&c, &hash).extension().unwrap(), "wav");
    assert!(
        service
            .synthesize(&c, "oi", "", "local", "")
            .await
            .unwrap()
            .cached
    );
    let blocked = dir.path().join("blocked");
    std::fs::write(&blocked, b"file").unwrap();
    c.cache_dir = blocked.join("nested");
    assert_eq!(
        service
            .synthesize(&c, "oi", "", "local", "")
            .await
            .unwrap_err()
            .status,
        500
    );
}

#[tokio::test]
async fn a_read_only_cached_audio_survives_a_failed_retention_touch_without_a_provider() {
    let dir = tempfile::tempdir().unwrap();
    let c = config(dir.path());
    let hash = hash_value("oi", "", "local", &json!({}), "");
    std::fs::create_dir_all(&c.cache_dir).unwrap();
    let path = c.cache_dir.join(format!("{hash}.mp3"));
    std::fs::write(&path, b"ID3audio").unwrap();
    let writable = std::fs::metadata(&path).unwrap().permissions();
    let mut read_only = writable.clone();
    read_only.set_readonly(true);
    std::fs::set_permissions(&path, read_only).unwrap();
    assert!(std::fs::OpenOptions::new().write(true).open(&path).is_err());
    let result = Service::default()
        .synthesize(&c, "oi", "", "local", "")
        .await;
    std::fs::set_permissions(&path, writable).unwrap();
    let result = result.unwrap();
    assert!(result.cached);
    assert_eq!(result.hash, hash);
    assert_eq!(std::fs::read(&path).unwrap(), b"ID3audio");
}

#[test]
fn retention_removes_only_old_regular_files_and_never_directories() {
    let dir = tempfile::tempdir().unwrap();
    let old = dir.path().join("old.mp3");
    let current = dir.path().join("new.wav");
    let folder = dir.path().join("folder");
    std::fs::create_dir(&folder).unwrap();
    std::fs::write(&old, b"ID3old").unwrap();
    std::fs::write(&current, b"RIFFnew").unwrap();
    let now = SystemTime::now();
    std::fs::OpenOptions::new()
        .write(true)
        .open(&old)
        .unwrap()
        .set_times(std::fs::FileTimes::new().set_modified(now - Duration::from_secs(31 * 86400)))
        .unwrap();
    cache::clean(dir.path(), now);
    assert!(!old.exists());
    assert!(current.exists());
    assert!(folder.exists());
}

async fn pid_at(path: &Path) -> u32 {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if let Ok(value) = std::fs::read_to_string(path) {
                if let Ok(pid) = value.parse() {
                    return pid;
                }
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap()
}
fn alive(pid: u32) -> bool {
    #[cfg(target_os = "linux")]
    {
        std::fs::read_to_string(format!("/proc/{pid}/stat"))
            .ok()
            .is_some_and(|s| {
                !s.rsplit_once(')')
                    .unwrap()
                    .1
                    .trim_start()
                    .starts_with(['Z', 'X'])
            })
    }
    #[cfg(not(target_os = "linux"))]
    {
        let mut system = sysinfo::System::new();
        let pid = sysinfo::Pid::from_u32(pid);
        system.refresh_processes(sysinfo::ProcessesToUpdate::Some(&[pid]), true);
        system.process(pid).is_some()
    }
}
async fn gone(pid: u32) {
    tokio::time::timeout(Duration::from_secs(5), async {
        while alive(pid) {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cancellation_stops_the_local_engine_and_descendants_without_a_cache_file() {
    let dir = tempfile::tempdir().unwrap();
    let mut c = config(dir.path());
    let parent = dir.path().join("parent");
    let child = dir.path().join("child");
    c.values["tts_local_cmd"] = json!(command(dir.path(), "hold", &[&parent, &child]));
    let cache = c.cache_dir.clone();
    let task = tokio::spawn(async move {
        Service::default()
            .synthesize(&c, "oi", "", "local", "")
            .await
    });
    let parent = pid_at(&parent).await;
    let child = pid_at(&child).await;
    task.abort();
    let _ = task.await;
    gone(parent).await;
    gone(child).await;
    empty_cache(&cache).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn deadline_kills_the_tree_even_when_a_descendant_holds_the_pipes() {
    let dir = tempfile::tempdir().unwrap();
    let mut c = config(dir.path());
    let parent = dir.path().join("parent");
    let child = dir.path().join("child");
    c.values["tts_local_cmd"] = json!(command(dir.path(), "hold", &[&parent, &child]));
    std::fs::create_dir_all(&c.cache_dir).unwrap();
    let started = tokio::time::Instant::now();
    let error = local::run_with_deadline(&c, "oi", Duration::from_millis(300))
        .await
        .unwrap_err();
    assert_eq!(error.status, 504);
    assert!(started.elapsed() < Duration::from_secs(5));
    gone(pid_at(&parent).await).await;
    gone(pid_at(&child).await).await;
}

struct Server {
    url: String,
    task: tokio::task::JoinHandle<()>,
}
impl Drop for Server {
    fn drop(&mut self) {
        self.task.abort();
    }
}
async fn server(
    response: impl Fn(
        axum::extract::Request,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = axum::response::Response> + Send>,
    > + Send
    + Sync
    + 'static,
) -> Server {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let response = Arc::new(response);
    let router = axum::Router::new().fallback(axum::routing::any(move |r| {
        let response = response.clone();
        async move { response(r).await }
    }));
    let task = tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    Server {
        url: format!("http://{address}"),
        task,
    }
}

#[tokio::test]
async fn elevenlabs_receives_the_effective_voice_normalization_and_adjustments() {
    use axum::response::IntoResponse;
    let records = Arc::new(std::sync::Mutex::new(Vec::new()));
    let writes = records.clone();
    let remote = server(move |request| {
        let records = writes.clone();
        Box::pin(async move {
            let uri = request.uri().to_string();
            let key = request.headers()["xi-api-key"].to_str().unwrap().to_owned();
            let body = axum::body::to_bytes(request.into_body(), 1 << 20)
                .await
                .unwrap();
            let body: Value = serde_json::from_slice(&body).unwrap();
            records.lock().unwrap().push((uri, key, body));
            b"ID3audio".to_vec().into_response()
        })
    })
    .await;
    let dir = tempfile::tempdir().unwrap();
    let mut c = config(dir.path());
    c.values = json!({"elevenlabs_api_key":"fixture","elevenlabs_voice_id":"configured","tts_stability":30});
    let service = Service::at(remote.url.clone());
    let first = service
        .synthesize(&c, "oi", "", "elevenlabs", "")
        .await
        .unwrap();
    assert!(!first.cached);
    assert_eq!(first.provider, "elevenlabs");
    assert!(
        service
            .synthesize(&c, "oi", "", "elevenlabs", "")
            .await
            .unwrap()
            .cached
    );
    c.values["elevenlabs_voice_id"] = json!("other");
    let second = service
        .synthesize(&c, "oi", "", "elevenlabs", "")
        .await
        .unwrap();
    assert_ne!(first.hash, second.hash);
    let records = records.lock().unwrap();
    assert_eq!(records.len(), 2);
    assert_eq!(
        records[0].0,
        "/text-to-speech/configured?output_format=mp3_44100_128"
    );
    assert_eq!(records[0].1, "fixture");
    assert_eq!(
        records[0].2,
        json!({"text":"oi","model_id":"eleven_multilingual_v2","apply_text_normalization":"on","voice_settings":{"stability":0.3}})
    );
}

#[tokio::test]
async fn malformed_remote_audio_catalog_and_balance_fail_visibly_without_fallback() {
    use axum::response::IntoResponse;
    let dir = tempfile::tempdir().unwrap();
    let mut c = config(dir.path());
    c.values["elevenlabs_api_key"] = json!("fixture-secret");
    c.values["tts_local_cmd"] = json!(command(dir.path(), "echo", &[]));
    let remote = server(|_| Box::pin(async { b"garbage".to_vec().into_response() })).await;
    assert_eq!(
        Service::at(remote.url.clone())
            .synthesize(&c, "oi", "", "elevenlabs", "")
            .await
            .unwrap_err()
            .status,
        502
    );
    assert_eq!(
        network::voices(&c, &remote.url).await.unwrap_err().status,
        502
    );
    assert_eq!(
        network::balance(&c, &remote.url).await.unwrap_err().status,
        502
    );
    assert_eq!(std::fs::read_dir(&c.cache_dir).unwrap().count(), 0);
    let remote = server(|_| {
        Box::pin(async {
            (
                axum::http::StatusCode::TOO_MANY_REQUESTS,
                "fixture-secret private",
            )
                .into_response()
        })
    })
    .await;
    let error = network::voices(&c, &remote.url).await.unwrap_err();
    assert_eq!(error.status, 502);
    assert!(error.detail.contains("429"));
    assert!(!error.detail.contains("fixture-secret"));
}

#[tokio::test]
async fn catalog_and_balance_keep_the_original_fields_and_nulls() {
    use axum::response::IntoResponse;
    let remote = server(|r| {
        Box::pin(async move {
            (
                [(axum::http::header::CONTENT_TYPE, "application/json")],
                (if r.uri().path() == "/voices" {
                    json!({"voices":[{"voice_id":"v","name":"Voz"},{}]})
                } else {
                    json!({"character_count":12,"character_limit":100})
                })
                .to_string(),
            )
                .into_response()
        })
    })
    .await;
    let dir = tempfile::tempdir().unwrap();
    let mut c = config(dir.path());
    c.values["elevenlabs_api_key"] = json!("fixture");
    assert_eq!(
        network::voices(&c, &remote.url).await.unwrap(),
        json!([{"id":"v","nome":"Voz"},{"id":null,"nome":null}])
    );
    assert_eq!(
        network::balance(&c, &remote.url).await.unwrap(),
        json!({"usados":12,"limite":100})
    );
}

#[tokio::test]
async fn missing_credentials_and_empty_remote_audio_have_the_original_statuses() {
    use axum::response::IntoResponse;
    let dir = tempfile::tempdir().unwrap();
    let mut c = config(dir.path());
    assert_eq!(voices(&c).await.unwrap_err().status, 503);
    assert_eq!(balance(&c).await.unwrap_err().status, 503);
    let service = Service::default();
    assert_eq!(
        service
            .synthesize(&c, "oi", "", "elevenlabs", "")
            .await
            .unwrap_err()
            .status,
        503
    );
    c.values["elevenlabs_api_key"] = json!("fixture");
    let remote = server(|_| Box::pin(async { Vec::<u8>::new().into_response() })).await;
    let error = Service::at(remote.url.clone())
        .synthesize(&c, "oi", "", "elevenlabs", "")
        .await
        .unwrap_err();
    assert_eq!(error.status, 502);
    assert!(error.detail.contains("vazio"));
    assert_eq!(std::fs::read_dir(&c.cache_dir).unwrap().count(), 0);
}

#[tokio::test]
async fn a_write_failure_after_download_is_typed_and_removes_its_unique_temporary_file() {
    use axum::response::IntoResponse;
    let dir = tempfile::tempdir().unwrap();
    let mut c = config(dir.path());
    c.values["elevenlabs_api_key"] = json!("fixture");
    let hash = hash_value("oi", "v", "elevenlabs", &json!({}), "");
    let destination = c.cache_dir.join(format!("{hash}.mp3"));
    let block = destination.clone();
    let remote = server(move |_| {
        let block = block.clone();
        Box::pin(async move {
            std::fs::create_dir(block).unwrap();
            b"ID3audio".to_vec().into_response()
        })
    })
    .await;
    let error = Service::at(remote.url.clone())
        .synthesize(&c, "oi", "v", "elevenlabs", "")
        .await
        .unwrap_err();
    assert_eq!(error.status, 500);
    assert!(error.detail.contains("cache"));
    let entries: Vec<_> = std::fs::read_dir(&c.cache_dir)
        .unwrap()
        .map(|v| v.unwrap().path())
        .collect();
    assert_eq!(entries, vec![destination]);
}

#[tokio::test]
async fn a_redirect_never_receives_the_synthesis_credential_on_another_host() {
    use axum::response::IntoResponse;
    let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let copied = calls.clone();
    let destination = server(move |_| {
        copied.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        Box::pin(async { b"ID3audio".to_vec().into_response() })
    })
    .await;
    let location = destination.url.clone();
    let source = server(move |_| {
        let location = location.clone();
        Box::pin(async move {
            (
                axum::http::StatusCode::FOUND,
                [(axum::http::header::LOCATION, location)],
                "redirect",
            )
                .into_response()
        })
    })
    .await;
    let dir = tempfile::tempdir().unwrap();
    let mut c = config(dir.path());
    c.values["elevenlabs_api_key"] = json!("fixture");
    assert_eq!(
        network::voices(&c, &source.url).await.unwrap_err().status,
        502
    );
    assert_eq!(calls.load(std::sync::atomic::Ordering::Relaxed), 0);
}

#[tokio::test]
async fn uncompressed_local_and_remote_audio_above_64_mib_remains_supported() {
    use axum::response::IntoResponse;
    let dir = tempfile::tempdir().unwrap();
    let mut c = config(dir.path());
    c.values["tts_local_cmd"] = json!(command(dir.path(), "large", &[]));
    let local = Service::default()
        .synthesize(&c, "oi", "", "local", "")
        .await
        .unwrap();
    assert_eq!(
        std::fs::metadata(cache_path(&c, &local.hash))
            .unwrap()
            .len(),
        65 * 1024 * 1024 + 4
    );
    let remote = server(|_| {
        Box::pin(async {
            let chunk = bytes::Bytes::from(vec![0; 1024 * 1024]);
            let chunks =
                std::iter::once(Ok::<_, std::io::Error>(bytes::Bytes::from_static(b"RIFF")))
                    .chain((0..65).map(move |_| Ok(chunk.clone())));
            axum::body::Body::from_stream(futures_util::stream::iter(chunks)).into_response()
        })
    })
    .await;
    c.values["elevenlabs_api_key"] = json!("fixture");
    let audio = Service::at(remote.url.clone())
        .synthesize(&c, "oi", "", "elevenlabs", "")
        .await
        .unwrap();
    assert_eq!(
        std::fs::metadata(cache_path(&c, &audio.hash))
            .unwrap()
            .len(),
        65 * 1024 * 1024 + 4
    );
}

#[tokio::test]
async fn cancelling_a_remote_stream_never_publishes_partial_audio_or_releases_an_unfinished_writer()
{
    use axum::response::IntoResponse;
    use futures_util::StreamExt;
    let remote = server(|_| {
        Box::pin(async {
            let stream = futures_util::stream::once(async {
                Ok::<_, std::io::Error>(bytes::Bytes::from_static(b"RIFFpartial"))
            })
            .chain(futures_util::stream::pending());
            axum::body::Body::from_stream(stream).into_response()
        })
    })
    .await;
    let directory = tempfile::tempdir().unwrap();
    let mut c = config(directory.path());
    c.values["elevenlabs_api_key"] = json!("fixture");
    let service = Arc::new(Service::at(remote.url.clone()));
    let copied = service.clone();
    let request = c.clone();
    let task = tokio::spawn(async move {
        copied
            .synthesize(&request, "oi", "", "elevenlabs", "")
            .await
    });
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if std::fs::read_dir(&c.cache_dir).is_ok_and(|mut entries| entries.next().is_some()) {
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    let identity = c.cache_dir.join(hash_value(
        "oi",
        "ORgG8rwdAiMYRug8RJwR",
        "elevenlabs",
        &json!({}),
        "",
    ));
    let gate = service
        .flights
        .lock()
        .unwrap()
        .get(&identity)
        .unwrap()
        .upgrade()
        .unwrap();
    task.abort();
    let _ = task.await;
    let _guard = tokio::time::timeout(Duration::from_secs(5), gate.lock())
        .await
        .unwrap();
    assert_eq!(std::fs::read_dir(&c.cache_dir).unwrap().count(), 0);
}

proptest! {
    #[test]
    fn arbitrary_text_and_command_never_panic(text in ".{0,2048}") {
        let prepared=prepare(&text);
        prop_assert!(!prepared.starts_with(char::is_whitespace));
        prop_assert!(!prepared.ends_with(char::is_whitespace));
        let hash=hash_value(&text,"","local",&json!({}),"");
        prop_assert_eq!(hash.len(),64);
        let _=local::split(&text);
    }
    #[test]
    fn arbitrary_audio_prefixes_never_panic(bytes in proptest::collection::vec(any::<u8>(),0..256)) {
        let _=valid_audio(&bytes);
    }
}
