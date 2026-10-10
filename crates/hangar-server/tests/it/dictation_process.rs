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
        for _ in 0..2 {
            let (socket, _) = tokio::time::timeout(Duration::from_secs(5), listener.accept())
                .await
                .unwrap()
                .unwrap();
            let mut socket = BufReader::new(socket);
            let mut pid = String::new();
            socket.read_line(&mut pid).await.unwrap();
            assert!(pid.trim().parse::<u32>().is_ok());
            sockets.push(socket);
        }
        if cancel {
            task.abort();
            assert!(task.await.unwrap_err().is_cancelled());
        } else {
            assert_eq!(task.await.unwrap(), Err("dictation_organization_timeout"));
        }
        for mut socket in sockets {
            let mut buffer = [0];
            assert_eq!(
                tokio::time::timeout(Duration::from_secs(5), socket.read(&mut buffer))
                    .await
                    .unwrap()
                    .unwrap(),
                0,
                "descendente ainda vivo"
            );
        }
        tokio::time::timeout(Duration::from_secs(5), async {
            while path.exists() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("a limpeza deve liberar a pasta após terminar a árvore");
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
