use bytes::Bytes;
use futures_util::stream;
use hangar_server::uploads::store::{MAX_UPLOAD_BYTES, UploadStore, project_key, slug};
use std::{
    fs, io,
    path::Path,
    time::{Duration, UNIX_EPOCH},
};

#[tokio::test]
async fn interrupted_stream_preserves_previous_file_and_leaves_no_partial() {
    let fixture = tempfile::tempdir().unwrap();
    let folder = fixture.path().join("project/session");
    fs::create_dir_all(&folder).unwrap();
    fs::write(folder.join("previous.bin"), b"anterior").unwrap();
    let store = UploadStore::new(fixture.path()).unwrap();
    let input = stream::iter([
        Ok(Bytes::from_static(b"primeiro bloco")),
        Err(io::Error::new(
            io::ErrorKind::Interrupted,
            "fluxo interrompido",
        )),
    ]);
    let error = store
        .publish("project", "session", "previous.bin", None, input)
        .await
        .unwrap_err();
    assert_eq!(
        error.kind(),
        io::ErrorKind::Interrupted,
        "o erro do fluxo precisa ser preservado"
    );
    assert_eq!(fs::read(folder.join("previous.bin")).unwrap(), b"anterior");
    let names: Vec<_> = fs::read_dir(&folder)
        .unwrap()
        .map(|x| x.unwrap().file_name())
        .collect();
    assert_eq!(
        names,
        ["previous.bin"],
        "nenhum arquivo parcial pode sobreviver"
    );
}

#[tokio::test]
async fn exact_limit_is_accepted_with_and_without_content_length() {
    for length in [None, Some(MAX_UPLOAD_BYTES)] {
        let fixture = tempfile::tempdir().unwrap();
        let store = UploadStore::new(fixture.path()).unwrap();
        let block = Bytes::from(vec![0x5a; 1024 * 1024]);
        let input = stream::iter((0..100).map(|_| Ok(block.clone())));
        let path = store
            .publish("project", "session", "large.bin", length, input)
            .await
            .unwrap();
        assert_eq!(fs::metadata(path).unwrap().len(), 104_857_600);
    }
}

#[tokio::test]
async fn overflow_without_content_length_cleans_its_own_temporary() {
    let fixture = tempfile::tempdir().unwrap();
    let folder = fixture.path().join("project/session");
    fs::create_dir_all(&folder).unwrap();
    fs::write(folder.join("unrelated.tmp"), b"alheio").unwrap();
    let store = UploadStore::new(fixture.path()).unwrap();
    let block = Bytes::from(vec![0x5a; 1024 * 1024]);
    let input = stream::iter((0..101).map(|_| Ok(block.clone())));
    let error = store
        .publish("project", "session", "large.bin", None, input)
        .await
        .unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::FileTooLarge);
    assert_eq!(fs::read_dir(&folder).unwrap().count(), 1);
    assert_eq!(fs::read(folder.join("unrelated.tmp")).unwrap(), b"alheio");
}

#[tokio::test]
async fn declared_overflow_does_not_poll_body() {
    let fixture = tempfile::tempdir().unwrap();
    let store = UploadStore::new(fixture.path()).unwrap();
    let input = stream::poll_fn(|_| -> std::task::Poll<Option<io::Result<Bytes>>> {
        panic!("o corpo não deveria ser consultado");
    });
    let error = store
        .publish("project", "session", "large.bin", Some(104_857_601), input)
        .await
        .unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::FileTooLarge);
}

#[tokio::test]
async fn empty_body_never_publishes() {
    let fixture = tempfile::tempdir().unwrap();
    let store = UploadStore::new(fixture.path()).unwrap();
    let error = store
        .publish("project", "session", "empty.bin", None, stream::empty())
        .await
        .unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
    assert_eq!(count_files(fixture.path()), 0);
}

fn count_files(path: &Path) -> usize {
    fs::read_dir(path)
        .unwrap()
        .map(|e| {
            let p = e.unwrap().path();
            if p.is_dir() { count_files(&p) } else { 1 }
        })
        .sum()
}

#[tokio::test]
async fn cancellation_unlinks_temporary_and_preserves_previous() {
    let fixture = tempfile::tempdir().unwrap();
    let folder = fixture.path().join("project/session");
    fs::create_dir_all(&folder).unwrap();
    fs::write(folder.join("previous.bin"), b"anterior").unwrap();
    let store = UploadStore::new(fixture.path()).unwrap();
    let (sent, received) = tokio::sync::oneshot::channel();
    let mut sent = Some(sent);
    let mut first = true;
    let input = stream::poll_fn(move |_| {
        if first {
            first = false;
            return std::task::Poll::Ready(Some(Ok(Bytes::from_static(b"parcial"))));
        }
        if let Some(sent) = sent.take() {
            let _ = sent.send(());
        }
        std::task::Poll::Pending
    });
    let task = tokio::spawn(async move {
        store
            .publish("project", "session", "previous.bin", None, input)
            .await
    });
    received
        .await
        .expect("a publicação precisa consumir o primeiro bloco antes do cancelamento");
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    assert_eq!(fs::read_dir(&folder).unwrap().count(), 1);
    assert_eq!(fs::read(folder.join("previous.bin")).unwrap(), b"anterior");
}

#[tokio::test]
async fn simultaneous_uploads_of_same_name_do_not_collide() {
    let fixture = tempfile::tempdir().unwrap();
    let store = std::sync::Arc::new(UploadStore::new(fixture.path()).unwrap());
    let mut tasks = Vec::new();
    for n in 0..64u8 {
        let store = store.clone();
        tasks.push(tokio::spawn(async move {
            let p = store
                .publish(
                    "project",
                    "session",
                    "same.bin",
                    None,
                    stream::iter([Ok(Bytes::from(vec![n]))]),
                )
                .await
                .unwrap();
            (p, n)
        }));
    }
    let mut paths = std::collections::HashSet::new();
    for task in tasks {
        let (path, value) = task.await.unwrap();
        assert_eq!(fs::read(&path).unwrap(), [value]);
        assert!(paths.insert(path));
    }
    assert_eq!(count_files(fixture.path()), 64);
}

#[test]
fn compatibility_slug_keeps_case_dots_and_separate_replacements() {
    for (input, expected) in [
        ("ação", "acao"),
        ("Ｆｏｏ ①", "Foo-1"),
        ("a / b", "a---b"),
        ("..", "_"),
        (".", "_"),
        ("../..", "..-.."),
        ("..\\x", "..-x"),
        ("a/b", "a-b"),
        ("東京", "_"),
    ] {
        assert_eq!(slug(input), expected, "slug de {input}");
    }
    assert_eq!(slug(&"a".repeat(70)), "a".repeat(64));
}

#[tokio::test]
async fn leading_dots_extension_matches_python_suffix() {
    let fixture = tempfile::tempdir().unwrap();
    let store = UploadStore::new(fixture.path()).unwrap();
    for (name, expected) in [
        ("..txt", "bin"),
        ("...txt", "bin"),
        (".hidden", "bin"),
        ("foo..txt", "txt"),
        ("...foo.txt", "txt"),
        ("arquivo.", "bin"),
    ] {
        let path = store
            .publish(
                "project",
                "session",
                name,
                None,
                stream::iter([Ok(Bytes::from_static(b"x"))]),
            )
            .await
            .unwrap();
        assert_eq!(
            path.extension().unwrap(),
            expected,
            "sufixo de {name} difere do Python"
        );
    }
}

#[test]
fn project_missing_component_followed_by_parent_keeps_real_identity() {
    let fixture = tempfile::tempdir().unwrap();
    let real = fixture.path().join("real");
    fs::create_dir(&real).unwrap();
    assert_eq!(
        project_key(&fixture.path().join("missing/../real")).unwrap(),
        project_key(&real).unwrap(),
        "componente ausente não divide a identidade do projeto"
    );
}

#[cfg(unix)]
#[test]
fn project_dangling_link_is_expanded_before_missing_child_and_parent() {
    let fixture = tempfile::tempdir().unwrap();
    let target = fixture.path().join("missing-target");
    let alias = fixture.path().join("alias");
    std::os::unix::fs::symlink(&target, &alias).unwrap();
    assert_eq!(
        project_key(&alias.join("child")).unwrap(),
        project_key(&target.join("child")).unwrap(),
        "link pendente mantém a identidade do destino"
    );
    assert_eq!(
        project_key(&alias.join("../sibling")).unwrap(),
        project_key(&fixture.path().join("sibling")).unwrap(),
        "parent é resolvido após expandir o link"
    );
}

#[cfg(unix)]
#[test]
fn absolute_audio_accepts_trusted_root_alias_and_rejects_changed_destination() {
    let fixture = tempfile::tempdir().unwrap();
    let vault = fixture.path().join("vault");
    let alias = fixture.path().join("alias");
    let audio = vault.join("project/previous/audio.webm");
    fs::create_dir_all(audio.parent().unwrap()).unwrap();
    fs::write(&audio, b"audio").unwrap();
    std::os::unix::fs::symlink(&vault, &alias).unwrap();
    let store = UploadStore::new(&alias).unwrap();
    let reference = alias.join("project/previous/audio.webm");
    // A resposta é o arquivo real; no macOS o próprio tempdir já fica atrás de um link.
    let real = fs::canonicalize(&audio).unwrap();
    assert_eq!(
        store
            .resolve_audio("project", "after-clear", reference.to_str().unwrap(), true)
            .unwrap(),
        real
    );
    assert_eq!(
        store
            .resolve_audio("project", "after-clear", real.to_str().unwrap(), true)
            .unwrap(),
        real
    );
    assert_eq!(
        store
            .resolve_audio("project", "after-clear", reference.to_str().unwrap(), false)
            .unwrap_err()
            .kind(),
        io::ErrorKind::PermissionDenied
    );
    assert_eq!(
        store
            .resolve_audio("other", "after-clear", reference.to_str().unwrap(), true)
            .unwrap_err()
            .kind(),
        io::ErrorKind::InvalidInput
    );
    assert_eq!(
        store
            .resolve_audio("project", "after-clear", "audio.webm", true)
            .unwrap_err()
            .kind(),
        io::ErrorKind::NotFound
    );
    let outside = fixture.path().join("outside");
    fs::create_dir_all(outside.join("project/previous")).unwrap();
    fs::write(outside.join("project/previous/audio.webm"), b"alheio").unwrap();
    fs::remove_file(&alias).unwrap();
    std::os::unix::fs::symlink(&outside, &alias).unwrap();
    assert_eq!(
        store
            .resolve_audio("project", "after-clear", reference.to_str().unwrap(), true)
            .unwrap_err()
            .kind(),
        io::ErrorKind::InvalidInput
    );
    assert_eq!(fs::read(audio).unwrap(), b"audio");
    assert_eq!(
        fs::read(outside.join("project/previous/audio.webm")).unwrap(),
        b"alheio"
    );
}

#[tokio::test]
async fn partial_file_is_hidden_while_stream_is_open() {
    let fixture = tempfile::tempdir().unwrap();
    let store = UploadStore::new(fixture.path()).unwrap();
    let mut step = 0;
    let input = stream::poll_fn(|_| {
        step += 1;
        if step == 1 {
            return std::task::Poll::Ready(Some(Ok(Bytes::from_static(b"primeiro"))));
        }
        assert!(
            store.list("project", "session", 0, 0.0).unwrap().is_empty(),
            "galeria não pode listar conteúdo parcial"
        );
        std::task::Poll::Ready(None)
    });
    let path = store
        .publish("project", "session", "safe.bin", None, input)
        .await
        .unwrap();
    assert_eq!(fs::read(path).unwrap(), b"primeiro");
    assert_eq!(store.list("project", "session", 0, 0.0).unwrap().len(), 1);
}

#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
#[test]
fn removal_interleavings_preserve_foreign_identity_and_cutoff() {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;
    use std::os::unix::fs::MetadataExt;
    if let Ok(root) = std::env::var("HANGAR_REMOVAL_FIXTURE") {
        let root = std::path::PathBuf::from(root);
        let mode = std::env::var("HANGAR_REMOVAL_MODE").unwrap();
        let vault = root.join("vault");
        let store = UploadStore::new(&vault).unwrap();
        let session = vault.join("project/session");
        fs::create_dir_all(&session).unwrap();
        fs::write(session.join("previous.bin"), b"anterior").unwrap();
        if mode.starts_with("prune") {
            fs::write(session.join("old.bin"), b"original").unwrap();
            std::fs::File::options()
                .write(true)
                .open(session.join("old.bin"))
                .unwrap()
                .set_times(
                    std::fs::FileTimes::new()
                        .set_modified(UNIX_EPOCH + std::time::Duration::from_secs(1)),
                )
                .unwrap();
            let count = store.prune("project", 1, 172800.0).unwrap();
            fs::write(root.join("count"), count.to_string()).unwrap();
        } else {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .build()
                .unwrap();
            let input = if mode.starts_with("cleanup") {
                vec![
                    Ok(Bytes::from_static(b"original")),
                    Err(io::ErrorKind::Interrupted.into()),
                ]
            } else {
                vec![Ok(Bytes::from_static(b"original"))]
            };
            let result = runtime.block_on(store.publish(
                "project",
                "session",
                "safe.bin",
                None,
                stream::iter(input),
            ));
            if mode.starts_with("cleanup") {
                assert_eq!(result.unwrap_err().kind(), io::ErrorKind::Interrupted);
            } else if mode == "rollback" {
                assert_eq!(result.unwrap_err().kind(), io::ErrorKind::InvalidInput);
            } else {
                assert_eq!(fs::read(result.unwrap()).unwrap(), b"original");
            }
        }
        assert_eq!(fs::read(session.join("previous.bin")).unwrap(), b"anterior");
        return;
    }
    unsafe fn trace(
        request: libc::c_uint,
        tid: libc::pid_t,
        addr: usize,
        data: usize,
    ) -> libc::c_long {
        let result = unsafe {
            libc::ptrace(
                request,
                tid,
                addr as *mut libc::c_void,
                data as *mut libc::c_void,
            )
        };
        if request != libc::PTRACE_PEEKDATA {
            assert_ne!(result, -1, "ptrace falhou: {}", io::Error::last_os_error());
        }
        result
    }
    unsafe fn name_at(tid: libc::pid_t, address: u64) -> String {
        let mut bytes = Vec::new();
        for offset in (0..4096).step_by(8) {
            let word = unsafe { trace(libc::PTRACE_PEEKDATA, tid, address as usize + offset, 0) }
                .to_ne_bytes();
            for byte in word {
                if byte == 0 {
                    return String::from_utf8(bytes).unwrap();
                }
                bytes.push(byte);
            }
        }
        panic!("nome sem terminador");
    }
    let mut failures = Vec::new();
    for mode in [
        "cleanup-control",
        "cleanup",
        "cleanup-occupied",
        "prune-control",
        "prune",
        "prune-refresh",
        "rollback-control",
        "rollback",
    ] {
        let result = std::panic::catch_unwind(|| {
            let fixture = tempfile::tempdir().unwrap();
            let binary =
                CString::new(std::env::current_exe().unwrap().as_os_str().as_bytes()).unwrap();
            let args = [
                binary.clone(),
                CString::new("--exact").unwrap(),
                CString::new("removal_interleavings_preserve_foreign_identity_and_cutoff").unwrap(),
                CString::new("--nocapture").unwrap(),
            ];
            let argv: Vec<_> = args
                .iter()
                .map(|v| v.as_ptr())
                .chain(std::iter::once(std::ptr::null()))
                .collect();
            let mut environment: Vec<CString> = std::env::vars_os()
                .map(|(key, value)| {
                    let mut bytes = key.as_bytes().to_vec();
                    bytes.push(b'=');
                    bytes.extend_from_slice(value.as_bytes());
                    CString::new(bytes).unwrap()
                })
                .collect();
            environment.push(
                CString::new(format!(
                    "HANGAR_REMOVAL_FIXTURE={}",
                    fixture.path().display()
                ))
                .unwrap(),
            );
            environment.push(CString::new(format!("HANGAR_REMOVAL_MODE={mode}")).unwrap());
            let envp: Vec<_> = environment
                .iter()
                .map(|v| v.as_ptr())
                .chain(std::iter::once(std::ptr::null()))
                .collect();
            let child = unsafe { libc::fork() };
            assert!(child >= 0);
            if child == 0 {
                unsafe {
                    libc::ptrace(libc::PTRACE_TRACEME, 0, 0, 0);
                    libc::raise(libc::SIGSTOP);
                    libc::execve(binary.as_ptr(), argv.as_ptr(), envp.as_ptr());
                    libc::_exit(127);
                }
            }
            struct ChildGuard(libc::pid_t);
            impl Drop for ChildGuard {
                fn drop(&mut self) {
                    if self.0 > 0 {
                        unsafe {
                            libc::kill(self.0, libc::SIGKILL);
                            libc::waitpid(self.0, std::ptr::null_mut(), 0);
                        }
                    }
                }
            }
            let mut guard = ChildGuard(child);
            let mut status = 0;
            assert_eq!(unsafe { libc::waitpid(child, &mut status, 0) }, child);
            unsafe {
                trace(
                    libc::PTRACE_SETOPTIONS,
                    child,
                    0,
                    (libc::PTRACE_O_TRACESYSGOOD
                        | libc::PTRACE_O_TRACECLONE
                        | libc::PTRACE_O_EXITKILL) as usize,
                );
                trace(libc::PTRACE_SYSCALL, child, 0, 0);
            }
            let mut tids = vec![child];
            let mut pending_link = std::collections::HashMap::new();
            let mut observed = None;
            let mut occupied = false;
            let started = std::time::Instant::now();
            while !tids.is_empty() {
                assert!(
                    started.elapsed().as_secs() < 30,
                    "sonda nativa não terminou: {mode}"
                );
                let mut progress = false;
                for tid in tids.clone() {
                    let ready =
                        unsafe { libc::waitpid(tid, &mut status, libc::__WALL | libc::WNOHANG) };
                    if ready == 0 {
                        continue;
                    }
                    assert_eq!(ready, tid);
                    progress = true;
                    if libc::WIFEXITED(status) {
                        assert_eq!(libc::WEXITSTATUS(status), 0, "filho falhou: {mode}");
                        tids.retain(|&id| id != tid);
                        if tid == child {
                            guard.0 = 0;
                        }
                        continue;
                    }
                    assert!(!libc::WIFSIGNALED(status), "filho recebeu sinal: {mode}");
                    let signal = libc::WSTOPSIG(status);
                    let event = status >> 16;
                    if event == libc::PTRACE_EVENT_CLONE {
                        let mut new_tid = 0u64;
                        unsafe {
                            trace(
                                libc::PTRACE_GETEVENTMSG,
                                tid,
                                0,
                                (&mut new_tid as *mut u64) as usize,
                            );
                        }
                        tids.push(new_tid as libc::pid_t);
                    }
                    if signal == (libc::SIGTRAP | 0x80) {
                        let mut registers: libc::user_regs_struct = unsafe { std::mem::zeroed() };
                        unsafe {
                            trace(
                                libc::PTRACE_GETREGS,
                                tid,
                                0,
                                (&mut registers as *mut _) as usize,
                            );
                        }
                        let entry = registers.rax == (-38i64) as u64;
                        if mode.starts_with("rollback") && registers.orig_rax == 265 {
                            if entry {
                                let name = unsafe { name_at(tid, registers.r10) };
                                let folder =
                                    fs::read_link(format!("/proc/{tid}/fd/{}", registers.rdx))
                                        .unwrap();
                                pending_link.insert(tid, folder.join(name));
                            } else if registers.rax == 0 && observed.is_none() {
                                let path = pending_link.remove(&tid).unwrap();
                                let identity = path.metadata().unwrap().ino();
                                if mode == "rollback" {
                                    fs::rename(
                                        &path,
                                        path.parent().unwrap().join("original-moved.bin"),
                                    )
                                    .unwrap();
                                    fs::write(&path, b"substituto").unwrap();
                                    assert_ne!(path.metadata().unwrap().ino(), identity);
                                }
                                observed = Some(path);
                            }
                        }
                        if entry && (registers.orig_rax == 263 || registers.orig_rax == 316) {
                            let name = unsafe { name_at(tid, registers.rsi) };
                            if observed.is_none()
                                && ((mode.starts_with("cleanup")
                                    && name.starts_with(".upload-")
                                    && name.ends_with(".tmp"))
                                    || (mode.starts_with("prune") && name == "old.bin"))
                            {
                                let folder =
                                    fs::read_link(format!("/proc/{tid}/fd/{}", registers.rdi))
                                        .unwrap();
                                let path = folder.join(name);
                                let identity = path.metadata().unwrap().ino();
                                if mode == "prune-refresh" {
                                    std::fs::File::options()
                                        .write(true)
                                        .open(&path)
                                        .unwrap()
                                        .set_times(std::fs::FileTimes::new().set_modified(
                                            UNIX_EPOCH + std::time::Duration::from_secs(172800),
                                        ))
                                        .unwrap();
                                } else if !mode.ends_with("control") {
                                    fs::rename(&path, folder.join("original-moved.bin")).unwrap();
                                    fs::write(&path, b"substituto").unwrap();
                                    assert_ne!(path.metadata().unwrap().ino(), identity);
                                }
                                observed = Some(path);
                            } else if mode == "cleanup-occupied"
                                && registers.orig_rax == 316
                                && name.starts_with("captured-")
                                && !occupied
                            {
                                let path = observed.as_ref().unwrap();
                                assert!(!path.exists());
                                fs::write(path, b"ocupante").unwrap();
                                occupied = true;
                            }
                        }
                    }
                    let deliver = if signal == libc::SIGTRAP
                        || signal == libc::SIGSTOP
                        || signal == (libc::SIGTRAP | 0x80)
                    {
                        0
                    } else {
                        signal
                    };
                    unsafe {
                        trace(libc::PTRACE_SYSCALL, tid, 0, deliver as usize);
                    }
                }
                if !progress {
                    std::thread::yield_now();
                }
            }
            let path = observed.expect("sonda precisa alcançar a operação real");
            if mode.ends_with("control") {
                if mode == "rollback-control" {
                    assert_eq!(fs::read(path).unwrap(), b"original");
                } else {
                    assert!(!path.exists(), "limpeza própria não ocorreu");
                }
            } else if mode == "prune-refresh" {
                assert_eq!(fs::read(&path).unwrap(), b"original");
                assert_eq!(
                    fs::read_to_string(fixture.path().join("count")).unwrap(),
                    "0"
                );
            } else if mode == "cleanup-occupied" {
                assert!(occupied, "devolução precisa ser interceptada");
                assert_eq!(fs::read(&path).unwrap(), b"ocupante");
                let mut preserved = Vec::new();
                for directory in fs::read_dir(fixture.path().join("vault"))
                    .unwrap()
                    .flatten()
                {
                    if directory
                        .file_name()
                        .to_string_lossy()
                        .starts_with(".upload-")
                    {
                        for file in fs::read_dir(directory.path()).unwrap().flatten() {
                            preserved.push(fs::read(file.path()).unwrap());
                        }
                    }
                }
                assert_eq!(
                    preserved,
                    vec![b"substituto".to_vec()],
                    "substituto capturado deve sobreviver sem overwrite"
                );
            } else {
                assert_eq!(
                    fs::read(&path).unwrap(),
                    b"substituto",
                    "remoção não pode apagar identidade divergente: {mode}"
                );
                assert_eq!(
                    fs::read(path.parent().unwrap().join("original-moved.bin")).unwrap(),
                    b"original"
                );
                if mode == "prune" {
                    assert_eq!(
                        fs::read_to_string(fixture.path().join("count")).unwrap(),
                        "0"
                    );
                }
            }
            eprintln!("Controle nativo de remoção: {mode} passou");
        });
        if result.is_err() {
            failures.push(mode);
        }
    }
    assert!(
        failures.is_empty(),
        "controles de preservação falharam: {failures:?}"
    );
}

#[cfg(windows)]
fn create_directory_junction(alias: &std::path::Path, target: &std::path::Path) {
    let result = std::process::Command::new("cmd")
        .args(["/C", "mklink", "/J"])
        .arg(alias)
        .arg(target)
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "junção nativa não foi criada: {}",
        String::from_utf8_lossy(&result.stderr)
    );
}

#[cfg(windows)]
#[test]
fn windows_project_junction_preserves_missing_target_identity() {
    let fixture = tempfile::tempdir().unwrap();
    let target = fixture.path().join("missing-target");
    let alias = fixture.path().join("alias");
    create_directory_junction(&alias, &target);
    assert_eq!(
        project_key(&alias.join("child")).unwrap(),
        project_key(&target.join("child")).unwrap()
    );
    fs::remove_dir(alias).unwrap();
}

#[cfg(windows)]
#[test]
fn windows_audio_accepts_trusted_junction_and_keeps_transcript_identity() {
    let fixture = tempfile::tempdir().unwrap();
    let vault = fixture.path().join("vault");
    let alias = fixture.path().join("alias");
    let audio = vault.join("project/previous/audio.webm");
    fs::create_dir_all(audio.parent().unwrap()).unwrap();
    fs::write(&audio, b"audio").unwrap();
    create_directory_junction(&alias, &vault);
    let store = UploadStore::new(&alias).unwrap();
    let reference = alias.join("project/previous/audio.webm");
    assert_eq!(
        store
            .resolve_audio("project", "after-clear", reference.to_str().unwrap(), true)
            .unwrap(),
        audio
    );
    assert_eq!(
        store
            .resolve_audio("project", "after-clear", audio.to_str().unwrap(), true)
            .unwrap(),
        audio
    );
    assert_eq!(
        store
            .resolve_audio("project", "after-clear", reference.to_str().unwrap(), false)
            .unwrap_err()
            .kind(),
        io::ErrorKind::PermissionDenied
    );
    assert_eq!(
        store
            .resolve_audio("other", "after-clear", reference.to_str().unwrap(), true)
            .unwrap_err()
            .kind(),
        io::ErrorKind::InvalidInput
    );
    assert_eq!(
        store
            .resolve_audio("project", "after-clear", "audio.webm", true)
            .unwrap_err()
            .kind(),
        io::ErrorKind::NotFound
    );
    let before = windows_directory_identity(audio.parent().unwrap());
    let outside = fixture.path().join("outside");
    fs::create_dir_all(outside.join("project/previous")).unwrap();
    fs::write(outside.join("project/previous/audio.webm"), b"alheio").unwrap();
    fs::remove_dir(&alias).unwrap();
    create_directory_junction(&alias, &outside);
    assert_eq!(
        store
            .resolve_audio("project", "after-clear", reference.to_str().unwrap(), true)
            .unwrap_err()
            .kind(),
        io::ErrorKind::InvalidInput
    );
    assert_eq!(
        fs::read(outside.join("project/previous/audio.webm")).unwrap(),
        b"alheio"
    );
    drop(store);
    assert_eq!(windows_directory_identity(audio.parent().unwrap()), before);
    assert_eq!(fs::read(audio).unwrap(), b"audio");
    fs::remove_dir(alias).unwrap();
}

#[cfg(unix)]
#[test]
fn long_symbolic_link_chain_keeps_python_project_identity() {
    if let Ok(root) = std::env::var("HANGAR_LONG_LINK_FIXTURE") {
        let root = std::path::PathBuf::from(root);
        assert_eq!(
            project_key(&root.join("link-0")).unwrap(),
            project_key(&root.join("target")).unwrap()
        );
        return;
    }
    let fixture = tempfile::tempdir().unwrap();
    fs::create_dir(fixture.path().join("target")).unwrap();
    for index in (0..2048).rev() {
        let target = if index == 2047 {
            "target".to_string()
        } else {
            format!("link-{}", index + 1)
        };
        std::os::unix::fs::symlink(target, fixture.path().join(format!("link-{index}"))).unwrap();
    }
    let result = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "long_symbolic_link_chain_keeps_python_project_identity",
            "--nocapture",
        ])
        .env("HANGAR_LONG_LINK_FIXTURE", fixture.path())
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "resolver precisa preservar a identidade sem esgotar a pilha: {}",
        String::from_utf8_lossy(&result.stderr)
    );
}

#[test]
fn project_identity_hashes_real_path_not_alias() {
    let fixture = tempfile::tempdir().unwrap();
    let real = fixture.path().join("ação");
    fs::create_dir(&real).unwrap();
    let key = project_key(&real).unwrap();
    let canonical = real.canonicalize().unwrap();
    let text = canonical.to_string_lossy();
    #[cfg(windows)]
    let text = text.strip_prefix(r"\\?\").unwrap_or(&text);
    let digest = ring::digest::digest(&ring::digest::SHA256, text.as_bytes());
    let expected = format!(
        "acao-{:02x}{:02x}{:02x}",
        digest.as_ref()[0],
        digest.as_ref()[1],
        digest.as_ref()[2]
    );
    assert_eq!(key, expected);
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(&real, fixture.path().join("alias")).unwrap();
        assert_eq!(project_key(&fixture.path().join("alias")).unwrap(), key);
    }
}

#[tokio::test]
async fn filename_extension_preserves_percent_encoded_python_behavior() {
    let fixture = tempfile::tempdir().unwrap();
    let store = UploadStore::new(fixture.path()).unwrap();
    let pattern = regex::Regex::new(r"^-?\d+-[0-9a-f]{6}\.[a-z0-9]{1,8}$").unwrap();
    for (name, suffix) in [
        ("..%2F..%2Famea%C3%A7a%252Etxt", "2fameac3"),
        ("foto.PNG", "png"),
        ("sem-extensão", "bin"),
        ("foo.ÁÇ", "bin"),
        (".hidden", "bin"),
    ] {
        let p = store
            .publish(
                "project",
                "session",
                name,
                None,
                stream::iter([Ok(Bytes::from_static(b"x"))]),
            )
            .await
            .unwrap();
        assert_eq!(p.extension().unwrap(), suffix);
        let filename = p.file_name().unwrap().to_str().unwrap();
        assert!(pattern.is_match(filename));
    }
}

#[test]
fn frozen_linux_and_windows_galleries_read_old_files_without_rewriting_goldens() {
    use base64::Engine;
    for text in [
        include_str!(
            "../../../backend/tests/fixtures/uploads_contract/linux/binary-and-gallery.json"
        ),
        include_str!(
            "../../../backend/tests/fixtures/uploads_contract/win32/binary-and-gallery.json"
        ),
    ] {
        let golden: serde_json::Value = serde_json::from_str(text).unwrap();
        let fixture = tempfile::tempdir().unwrap();
        for row in golden["normalized_tree"].as_array().unwrap() {
            let relative = row["path"]
                .as_str()
                .unwrap()
                .strip_prefix("<root>/vault/")
                .unwrap()
                .replace("<project>", "project");
            let path = fixture.path().join(relative);
            if row["kind"] == "directory" {
                fs::create_dir_all(path).unwrap();
                continue;
            }
            let data = base64::engine::general_purpose::STANDARD
                .decode(row["content_base64"].as_str().unwrap())
                .unwrap();
            fs::write(&path, &data).unwrap();
            let time = UNIX_EPOCH + Duration::from_secs_f64(row["mtime"].as_f64().unwrap());
            fs::File::options()
                .write(true)
                .open(path)
                .unwrap()
                .set_times(fs::FileTimes::new().set_modified(time))
                .unwrap();
        }
        let store = UploadStore::new(fixture.path()).unwrap();
        let entries = store
            .list("project", "transcript", 0, 1_700_000_000.0)
            .unwrap();
        let expected = &golden["normalized_response"][3]["json"]["files"];
        // Empates mantêm a ordem da enumeração, que varia entre filesystems.
        let mut actual = serde_json::to_value(entries)
            .unwrap()
            .as_array()
            .unwrap()
            .clone();
        let mut expected = expected.as_array().unwrap().clone();
        actual.sort_by_key(|r| r["filename"].as_str().unwrap().to_string());
        expected.sort_by_key(|r| r["filename"].as_str().unwrap().to_string());
        assert_eq!(actual, expected);
        for row in golden["normalized_tree"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|r| r["kind"] == "file")
        {
            let name = row["path"].as_str().unwrap().rsplit('/').next().unwrap();
            let path = store.resolve("project", "transcript", name).unwrap();
            assert_eq!(
                base64::engine::general_purpose::STANDARD.encode(fs::read(path).unwrap()),
                row["content_base64"].as_str().unwrap()
            );
        }
    }
}

#[test]
fn gallery_and_pruning_preserve_project_scope_and_exact_cutoff() {
    let fixture = tempfile::tempdir().unwrap();
    for folder in ["project/old", "project/current", "other/old"] {
        fs::create_dir_all(fixture.path().join(folder)).unwrap();
    }
    let now = 1_700_000_000.0;
    for (path, age) in [
        ("project/old/expired.webm", 8),
        ("project/old/boundary.bin", 7),
        ("project/current/new.bin", 0),
        ("other/old/expired.webm", 8),
    ] {
        let p = fixture.path().join(path);
        fs::write(&p, b"audio").unwrap();
        fs::File::options()
            .write(true)
            .open(&p)
            .unwrap()
            .set_times(
                fs::FileTimes::new()
                    .set_modified(UNIX_EPOCH + Duration::from_secs(now as u64 - age * 86400)),
            )
            .unwrap();
    }
    let store = UploadStore::new(fixture.path()).unwrap();
    let entries = store.list("project", "old", 7, now).unwrap();
    assert_eq!(
        entries
            .iter()
            .map(|e| e.filename.as_str())
            .collect::<Vec<_>>(),
        ["boundary.bin", "expired.webm"]
    );
    assert_eq!(entries[1].expires_in_days, Some(-1.0));
    assert_eq!(entries[1].size, 5);
    assert_eq!(entries[1].mtime, 1_699_308_800.0);
    assert_eq!(store.prune("project", 7, now).unwrap(), 1);
    assert!(fixture.path().join("project/old/boundary.bin").is_file());
    assert!(fixture.path().join("other/old/expired.webm").is_file());
    assert_eq!(store.prune("project", 0, now).unwrap(), 0);
    assert_eq!(store.list("project", "absent", 0, now).unwrap().len(), 0);
    assert!(
        store.list("project", "current", 0, now).unwrap()[0]
            .expires_in_days
            .is_none()
    );
}

#[test]
fn previous_transcript_audio_is_readable_only_within_same_project() {
    let fixture = tempfile::tempdir().unwrap();
    let previous = fixture.path().join("project/previous/audio.webm");
    fs::create_dir_all(previous.parent().unwrap()).unwrap();
    fs::write(&previous, b"audio").unwrap();
    let store = UploadStore::new(fixture.path()).unwrap();
    assert_eq!(
        fs::read(
            store
                .resolve_audio("project", "after-clear", previous.to_str().unwrap(), true)
                .unwrap()
        )
        .unwrap(),
        b"audio"
    );
    assert_eq!(
        store
            .resolve_audio("project", "after-clear", previous.to_str().unwrap(), false)
            .unwrap_err()
            .kind(),
        io::ErrorKind::PermissionDenied
    );
    assert_eq!(
        store
            .resolve_audio("other", "after-clear", previous.to_str().unwrap(), true)
            .unwrap_err()
            .kind(),
        io::ErrorKind::InvalidInput
    );
    assert_eq!(
        store
            .resolve_audio("project", "after-clear", "audio.webm", true)
            .unwrap_err()
            .kind(),
        io::ErrorKind::NotFound
    );
    for reference in [
        "//server/secret",
        "\\\\server\\secret",
        "nul\0",
        "../audio.webm",
    ] {
        assert_eq!(
            store
                .resolve_audio("project", "after-clear", reference, true)
                .unwrap_err()
                .kind(),
            io::ErrorKind::InvalidInput
        );
    }
    for name in ["a/b", "a\\b", "a..b", ""] {
        assert_eq!(
            store
                .resolve("project", "previous", name)
                .unwrap_err()
                .kind(),
            io::ErrorKind::InvalidInput
        );
    }
}

#[tokio::test]
async fn published_audio_keeps_real_project_and_previous_transcript_identity() {
    let fixture = tempfile::tempdir().unwrap();
    let root = std::env::var_os("HANGAR_UPLOAD_EXPORT_ROOT")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| fixture.path().to_path_buf());
    let cwd = root.join("ação");
    fs::create_dir_all(&cwd).unwrap();
    let vault = root.join("vault");
    let project = project_key(&cwd).unwrap();
    let store = UploadStore::new(&vault).unwrap();
    let path = store
        .publish(
            &project,
            "previous-transcript",
            "audio.webm",
            None,
            stream::iter([Ok(Bytes::from_static(b"audio"))]),
        )
        .await
        .unwrap();
    let resolved = store
        .resolve_audio(&project, "after-clear", path.to_str().unwrap(), true)
        .unwrap();
    assert_eq!(fs::read(&resolved).unwrap(), b"audio");
    assert_eq!(
        resolved.parent().unwrap().file_name().unwrap(),
        "previous-transcript"
    );
    if std::env::var_os("HANGAR_UPLOAD_EXPORT_ROOT").is_some() {
        fs::write(
            root.join("audio-export.json"),
            serde_json::to_vec(
                &serde_json::json!({"cwd":cwd,"vault":vault,"path":path,"project":project}),
            )
            .unwrap(),
        )
        .unwrap();
    }
}

#[cfg(unix)]
#[tokio::test]
async fn symlink_swap_during_stream_never_publishes_or_writes_outside() {
    let fixture = tempfile::tempdir().unwrap();
    let folder = fixture.path().join("project/session");
    fs::create_dir_all(&folder).unwrap();
    let outside = tempfile::tempdir().unwrap();
    fs::write(outside.path().join("previous.bin"), b"externo").unwrap();
    let store = UploadStore::new(fixture.path()).unwrap();
    let mut step = 0;
    let input = stream::poll_fn(|_| {
        step += 1;
        if step == 1 {
            return std::task::Poll::Ready(Some(Ok(Bytes::from_static(b"primeiro"))));
        }
        if step == 2 {
            fs::rename(&folder, fixture.path().join("project/moved")).unwrap();
            std::os::unix::fs::symlink(outside.path(), &folder).unwrap();
            return std::task::Poll::Ready(Some(Ok(Bytes::from_static(b"segundo"))));
        }
        std::task::Poll::Ready(None)
    });
    assert!(
        store
            .publish("project", "session", "previous.bin", None, input)
            .await
            .is_err()
    );
    assert_eq!(fs::read_dir(outside.path()).unwrap().count(), 1);
    assert_eq!(
        fs::read(outside.path().join("previous.bin")).unwrap(),
        b"externo"
    );
    assert_eq!(
        fs::read_dir(fixture.path().join("project/moved"))
            .unwrap()
            .count(),
        0
    );
}

#[cfg(unix)]
#[test]
fn resolve_gallery_and_prune_do_not_follow_escaping_symlinks() {
    let fixture = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    fs::write(outside.path().join("audio.webm"), b"externo").unwrap();
    fs::create_dir_all(fixture.path().join("project/session")).unwrap();
    std::os::unix::fs::symlink(
        outside.path().join("audio.webm"),
        fixture.path().join("project/session/link.webm"),
    )
    .unwrap();
    std::os::unix::fs::symlink(outside.path(), fixture.path().join("project/linked")).unwrap();
    let store = UploadStore::new(fixture.path()).unwrap();
    assert!(store.resolve("project", "session", "link.webm").is_err());
    assert!(
        store
            .list("project", "session", 7, 1_800_000_000.0)
            .unwrap()
            .is_empty()
    );
    assert_eq!(store.prune("project", 7, 1_800_000_000.0).unwrap(), 0);
    assert_eq!(
        fs::read(outside.path().join("audio.webm")).unwrap(),
        b"externo"
    );
}

#[cfg(target_os = "linux")]
#[test]
fn disk_write_failure_cleans_partial_in_dedicated_process() {
    if std::env::var_os("UPLOAD_DISK_PROBE").is_none() {
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "disk_write_failure_cleans_partial_in_dedicated_process",
                "--nocapture",
            ])
            .env("UPLOAD_DISK_PROBE", "1")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stdout)
        );
        return;
    }
    tokio::runtime::Runtime::new().unwrap().block_on(async {
        let fixture = tempfile::tempdir().unwrap();
        let folder = fixture.path().join("project/session");
        fs::create_dir_all(&folder).unwrap();
        fs::write(folder.join("previous.bin"), b"anterior").unwrap();
        let store = UploadStore::new(fixture.path()).unwrap();
        // O limite atinge o write real sem esgotar o disco compartilhado.
        unsafe {
            libc::signal(libc::SIGXFSZ, libc::SIG_IGN);
            let limit = libc::rlimit {
                rlim_cur: 4,
                rlim_max: 4,
            };
            assert_eq!(libc::setrlimit(libc::RLIMIT_FSIZE, &limit), 0);
        }
        let error = store
            .publish(
                "project",
                "session",
                "previous.bin",
                None,
                stream::iter([Ok(Bytes::from_static("conteúdo maior".as_bytes()))]),
            )
            .await
            .unwrap_err();
        assert_eq!(error.raw_os_error(), Some(libc::EFBIG));
        assert_eq!(fs::read_dir(folder).unwrap().count(), 1);
    });
}

#[cfg(target_os = "linux")]
#[test]
fn real_enospc_write_cleans_partial_and_preserves_previous() {
    if std::env::var_os("UPLOAD_FULL_PROBE").is_none() {
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "real_enospc_write_cleans_partial_and_preserves_previous",
                "--nocapture",
            ])
            .env("UPLOAD_FULL_PROBE", "1")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stdout)
        );
        return;
    }
    tokio::runtime::Runtime::new().unwrap().block_on(async {
        use std::os::fd::AsRawFd;
        let fixture = tempfile::tempdir().unwrap();
        let folder = fixture.path().join("project/session");
        fs::create_dir_all(&folder).unwrap();
        fs::write(folder.join("previous.bin"), b"anterior").unwrap();
        let store = UploadStore::new(fixture.path()).unwrap();
        let full = fs::OpenOptions::new()
            .write(true)
            .open("/dev/full")
            .unwrap();
        let mut stage = 0;
        let input = stream::poll_fn(|_| {
            stage += 1;
            if stage == 1 {
                return std::task::Poll::Ready(Some(Ok(Bytes::from_static(b"primeiro"))));
            }
            if stage == 2 {
                // Troca somente o descritor do temporário da fixture neste processo filho.
                let fd = fs::read_dir("/proc/self/fd")
                    .unwrap()
                    .find_map(|e| {
                        let e = e.unwrap();
                        let target = fs::read_link(e.path()).ok()?;
                        if target.parent() == Some(folder.as_path())
                            && target.file_name()?.to_str()?.starts_with(".upload-")
                        {
                            e.file_name().to_str()?.parse::<i32>().ok()
                        } else {
                            None
                        }
                    })
                    .expect("descritor do temporário aberto");
                assert!(unsafe { libc::dup2(full.as_raw_fd(), fd) } >= 0);
                return std::task::Poll::Ready(Some(Ok(Bytes::from_static(b"segundo"))));
            }
            std::task::Poll::Ready(None)
        });
        let error = store
            .publish("project", "session", "previous.bin", None, input)
            .await
            .unwrap_err();
        assert_eq!(error.raw_os_error(), Some(libc::ENOSPC));
        assert_eq!(fs::read_dir(&folder).unwrap().count(), 1);
        assert_eq!(fs::read(folder.join("previous.bin")).unwrap(), b"anterior");
    });
}

#[cfg(unix)]
#[tokio::test]
async fn replacing_temporary_with_symlink_is_rejected_without_deleting_replacement() {
    let fixture = tempfile::tempdir().unwrap();
    let folder = fixture.path().join("project/session");
    fs::create_dir_all(&folder).unwrap();
    let outside = tempfile::tempdir().unwrap();
    fs::write(outside.path().join("external.bin"), b"externo").unwrap();
    let store = UploadStore::new(fixture.path()).unwrap();
    let mut stage = 0;
    let mut replaced = None;
    let input = stream::poll_fn(|_| {
        stage += 1;
        if stage == 1 {
            return std::task::Poll::Ready(Some(Ok(Bytes::from_static(b"primeiro"))));
        }
        if stage == 2 {
            let temporary = fs::read_dir(&folder)
                .unwrap()
                .next()
                .unwrap()
                .unwrap()
                .path();
            fs::rename(&temporary, folder.join("moved.bin")).unwrap();
            std::os::unix::fs::symlink(outside.path().join("external.bin"), &temporary).unwrap();
            replaced = Some(temporary);
        }
        std::task::Poll::Ready(None)
    });
    assert!(
        store
            .publish("project", "session", "new.bin", None, input)
            .await
            .is_err(),
        "a identidade do temporário foi trocada"
    );
    assert!(
        fs::symlink_metadata(replaced.unwrap())
            .unwrap()
            .file_type()
            .is_symlink(),
        "a limpeza não pode apagar o substituto alheio"
    );
    assert_eq!(fs::read_dir(&folder).unwrap().count(), 2);
    assert_eq!(
        fs::read(outside.path().join("external.bin")).unwrap(),
        b"externo"
    );
}

#[cfg(windows)]
fn windows_directory_identity(path: &Path) -> (u32, u64) {
    use std::os::windows::{fs::OpenOptionsExt, io::AsRawHandle};
    #[repr(C)]
    #[derive(Default)]
    struct FileInformation {
        attributes: u32,
        creation_time: [u32; 2],
        access_time: [u32; 2],
        write_time: [u32; 2],
        volume_serial: u32,
        size_high: u32,
        size_low: u32,
        links: u32,
        index_high: u32,
        index_low: u32,
    }
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetFileInformationByHandle(
            handle: *mut std::ffi::c_void,
            information: *mut FileInformation,
        ) -> i32;
    }
    let handle = fs::OpenOptions::new()
        .read(true)
        .custom_flags(0x02000000)
        .open(path)
        .unwrap();
    let mut information = FileInformation::default();
    assert_ne!(
        unsafe { GetFileInformationByHandle(handle.as_raw_handle(), &mut information) },
        0
    );
    (
        information.volume_serial,
        ((information.index_high as u64) << 32) | information.index_low as u64,
    )
}

#[cfg(windows)]
#[tokio::test]
async fn directory_handle_prevents_windows_swap_during_stream() {
    let fixture = tempfile::tempdir().unwrap();
    let folder = fixture.path().join("project/session");
    fs::create_dir_all(&folder).unwrap();
    fs::write(folder.join("previous.bin"), b"anterior").unwrap();
    let identity = windows_directory_identity(&folder);
    let moved = fixture.path().join("moved");
    fs::rename(&folder, &moved).unwrap();
    assert_eq!(windows_directory_identity(&moved), identity);
    assert_eq!(fs::read(moved.join("previous.bin")).unwrap(), b"anterior");
    fs::rename(&moved, &folder).unwrap();
    let store = UploadStore::new(fixture.path()).unwrap();
    let mut stage = 0;
    let mut rename_error = None;
    let input = stream::poll_fn(|_| {
        stage += 1;
        if stage == 1 {
            return std::task::Poll::Ready(Some(Ok(Bytes::from_static(b"primeiro"))));
        }
        let error = fs::rename(&folder, &moved).unwrap_err();
        assert_eq!(windows_directory_identity(&folder), identity);
        assert!(folder.is_dir());
        assert!(!moved.exists());
        assert_eq!(fs::read(folder.join("previous.bin")).unwrap(), b"anterior");
        eprintln!(
            "Rename com handle: raw_os_error={:?}, kind={:?}; identidade={identity:?}",
            error.raw_os_error(),
            error.kind()
        );
        rename_error = Some(error);
        std::task::Poll::Ready(None)
    });
    let path = store
        .publish("project", "session", "safe.bin", None, input)
        .await
        .unwrap();
    assert_eq!(fs::read(&path).unwrap(), b"primeiro");
    let filename = path.file_name().unwrap().to_owned();
    drop(store);
    fs::rename(&folder, &moved).unwrap();
    assert_eq!(windows_directory_identity(&moved), identity);
    assert_eq!(fs::read(moved.join("previous.bin")).unwrap(), b"anterior");
    assert_eq!(fs::read(moved.join(filename)).unwrap(), b"primeiro");
    eprintln!("Controles de rename sem handle e após liberação passaram");
    let error = rename_error.unwrap();
    assert_eq!(error.raw_os_error(), Some(32), "{error:?}");
}

#[cfg(windows)]
#[tokio::test]
async fn hostile_windows_session_keeps_frozen_failure_and_directory() {
    let fixture = tempfile::tempdir().unwrap();
    let store = UploadStore::new(fixture.path()).unwrap();
    assert_eq!(
        store
            .publish(
                "project",
                "../..",
                "safe.bin",
                None,
                stream::iter([Ok(Bytes::from_static(b"x"))])
            )
            .await
            .unwrap_err()
            .kind(),
        io::ErrorKind::InvalidInput
    );
    assert!(fixture.path().join("project/..-").is_dir());
    assert_eq!(count_files(fixture.path()), 0);
}

#[cfg(windows)]
#[test]
fn unicode_windows_root_preserves_absolute_audio_identity() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path().join("İ");
    let audio = root.join("project/previous/audio.webm");
    fs::create_dir_all(audio.parent().unwrap()).unwrap();
    fs::write(&audio, b"audio").unwrap();
    let store = UploadStore::new(&root).unwrap();
    assert_eq!(
        fs::read(
            store
                .resolve_audio("project", "after-clear", audio.to_str().unwrap(), true)
                .unwrap()
        )
        .unwrap(),
        b"audio"
    );
}
