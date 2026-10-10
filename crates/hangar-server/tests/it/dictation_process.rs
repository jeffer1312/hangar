use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, BufReader};

#[tokio::test]
async fn dictation_timeout_and_cancellation_kill_descendants_holding_stdout() {
    for cancel in [false, true] {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().to_owned();
        let mut command =
            tokio::process::Command::new(env!("CARGO_BIN_EXE_hangar-dictation-fixture"));
        command.args(["--pipe-child", &address.to_string()]);
        let task = tokio::spawn(hangar_server::dictation::process::limited_output(
            command,
            directory,
            "transcrição",
            Duration::from_secs(2),
        ));
        let mut sockets = Vec::new();
        let mut pids = Vec::new();
        for _ in 0..2 {
            let (socket, _) = tokio::time::timeout(Duration::from_secs(5), listener.accept())
                .await
                .unwrap()
                .unwrap();
            let mut socket = BufReader::new(socket);
            let mut pid = String::new();
            socket.read_line(&mut pid).await.unwrap();
            pids.push(sysinfo::Pid::from_u32(pid.trim().parse::<u32>().unwrap()));
            sockets.push(socket);
        }
        if cancel {
            task.abort();
            assert!(task.await.unwrap_err().is_cancelled());
        } else {
            assert_eq!(task.await.unwrap(), Err("dictation_organization_timeout"));
        }
        tokio::time::timeout(Duration::from_secs(5), async {
            for mut socket in sockets {
                let mut buffer = [0];
                match socket.read(&mut buffer).await {
                    Ok(0) => {}
                    Err(error)
                        if cfg!(windows)
                            && error.kind() == std::io::ErrorKind::ConnectionReset
                            && error.raw_os_error() == Some(10054) => {}
                    outcome => panic!("a conexão do descendente não encerrou: {outcome:?}"),
                }
            }
            let mut processes = sysinfo::System::new();
            loop {
                processes.refresh_processes(sysinfo::ProcessesToUpdate::Some(&pids), true);
                if pids.iter().all(|pid| processes.process(*pid).is_none()) && !path.exists() {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("a limpeza deve encerrar os PIDs anunciados e liberar a pasta dentro do prazo");
    }
}

#[tokio::test]
async fn dictation_spawn_failure_and_empty_nonzero_output_are_errors() {
    let missing = tokio::process::Command::new("hangar-inexistente-fixture");
    assert_eq!(
        hangar_server::dictation::process::limited_output(
            missing,
            tempfile::tempdir().unwrap(),
            "",
            Duration::from_secs(1)
        )
        .await,
        Err("dictation_cli_missing")
    );
}
