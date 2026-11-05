use hangar_server::codex_setup::files;
use serde_json::json;
#[test]
fn codex_compared_write_preserves_external_changes_and_first_backup() {
    let root = crate::physical_tempdir();
    let path = root.path().join("config");
    let backup = root.path().join("backups");
    std::fs::write(&path, b"externo").unwrap();
    assert!(matches!(
        files::write(&path, b"nosso", Some(b"anterior"), Some(&backup)),
        Err(files::Error::Changed(_))
    ));
    assert_eq!(std::fs::read(&path).unwrap(), b"externo");
    assert!(files::write(&path, b"novo", Some(b"externo"), Some(&backup)).unwrap());
    let modified = path.metadata().unwrap().modified().unwrap();
    assert!(!files::write(&path, b"novo", Some(b"novo"), Some(&backup)).unwrap());
    assert_eq!(path.metadata().unwrap().modified().unwrap(), modified);
    assert!(files::write(&path, b"outro", Some(b"novo"), Some(&backup)).unwrap());
    let entries: Vec<_> = std::fs::read_dir(&backup).unwrap().collect();
    assert_eq!(entries.len(), 1);
    let record = files::object(&entries[0].as_ref().unwrap().path()).unwrap();
    assert_eq!(record["conteudo_hex"], "65787465726e6f");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            entries[0]
                .as_ref()
                .unwrap()
                .metadata()
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
    }
}
#[test]
fn codex_transformation_retries_current_snapshot() {
    let root = crate::physical_tempdir();
    let path = root.path().join("config");
    std::fs::write(&path, b"a").unwrap();
    let mut seen = Vec::new();
    files::transform(&path, &root.path().join("backups"), |raw| {
        let raw = raw.unwrap();
        seen.push(raw.to_vec());
        if seen.len() == 1 {
            std::fs::write(&path, b"externo")?;
        }
        Ok([raw, b" atualizado"].concat())
    })
    .unwrap();
    assert_eq!(seen, vec![b"a".to_vec(), b"externo".to_vec()]);
    assert_eq!(std::fs::read(&path).unwrap(), b"externo atualizado");
}
#[tokio::test]
async fn codex_file_lock_cancel_releases() {
    let root = crate::physical_tempdir();
    let path = root.path().join("lock");
    let first = files::exclusive(&path).await.unwrap();
    let target = path.clone();
    let waiting = tokio::spawn(async move { files::exclusive(&target).await.unwrap() });
    tokio::task::yield_now().await;
    assert!(!waiting.is_finished());
    waiting.abort();
    let _ = waiting.await;
    drop(first);
    assert!(
        tokio::time::timeout(std::time::Duration::from_secs(1), files::exclusive(&path))
            .await
            .unwrap()
            .is_ok()
    );
}

#[tokio::test]
#[cfg(unix)]
async fn codex_native_edit_uses_isolated_copy_and_retries_races() {
    use hangar_server::accounts::environment::AccountEnvironment;
    use std::os::unix::fs::PermissionsExt;
    let root = crate::physical_tempdir();
    let binary = root.path().join("bin");
    std::fs::create_dir(&binary).unwrap();
    let script = binary.join("codex");
    std::fs::write(
        &script,
        r##"#!/bin/sh
while IFS= read -r message; do
  id=$(printf '%s' "$message" | /usr/bin/sed -n 's/.*"id":\([0-9]*\).*/\1/p')
  case "$message" in
    *'"method":"config/batchWrite"'*)
      printf '%s\n' "$HOME" "$CODEX_HOME" "$XDG_CONFIG_HOME" "$XDG_DATA_HOME" "$XDG_STATE_HOME" "$XDG_CACHE_HOME" >> "$FIXTURE_OBSERVATIONS"
      printf 'model = "novo"\n' > "$CODEX_HOME/config.toml"
      printf '{"id":%s,"result":{}}\n' "$id";;
    *'"method":"initialize"'*) printf '{"id":%s,"result":{}}\n' "$id";;
  esac
done
"##,
    )
    .unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o700)).unwrap();
    let path = root.path().join("config.toml");
    std::fs::write(&path, "model = \"antigo\"\n").unwrap();
    let observations = root.path().join("observations");
    let env = AccountEnvironment::from_map(
        [
            ("HOME".into(), root.path().to_string_lossy().into_owned()),
            ("PATH".into(), binary.to_string_lossy().into_owned()),
            ("XDG_CONFIG_HOME".into(), "/dados-reais/config".into()),
            ("XDG_DATA_HOME".into(), "/dados-reais/data".into()),
            ("XDG_STATE_HOME".into(), "/dados-reais/state".into()),
            ("XDG_CACHE_HOME".into(), "/dados-reais/cache".into()),
            (
                "FIXTURE_OBSERVATIONS".into(),
                observations.to_string_lossy().into_owned(),
            ),
        ]
        .into(),
    );
    let work = root.path().join("work");
    let backups = root.path().join("backups");
    let mut calls = 0;
    files::edit_config(&env, &path, &backups, &work, |current| {
        calls += 1;
        if calls == 1 {
            assert_eq!(current["model"], "antigo");
            std::fs::write(&path, "model = \"externo\"\n").unwrap();
        } else {
            assert_eq!(current["model"], "externo");
        }
        Ok(vec![
            json!({"keyPath":"model","value":"novo","mergeStrategy":"replace"}),
        ])
    })
    .await
    .unwrap();
    assert_eq!(calls, 2);
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        "model = \"novo\"\n"
    );
    let recorded = std::fs::read_to_string(observations).unwrap();
    let lines: Vec<_> = recorded.lines().collect();
    assert_eq!(lines.len(), 12);
    assert!(
        lines
            .iter()
            .all(|p| std::path::Path::new(p).starts_with(&work))
    );
    assert_eq!(std::fs::read_dir(work).unwrap().count(), 0);
}

#[tokio::test]
#[cfg(unix)]
async fn codex_native_cleanup_failure_blocks_config_write() {
    use hangar_server::accounts::environment::AccountEnvironment;
    use std::os::unix::fs::PermissionsExt;
    let root = crate::physical_tempdir();
    let bin = root.path().join("bin");
    std::fs::create_dir(&bin).unwrap();
    let script = bin.join("codex");
    std::fs::write(
        &script,
        r##"#!/bin/sh
while IFS= read -r message; do
 id=$(printf '%s' "$message" | /usr/bin/sed -n 's/.*"id":\([0-9]*\).*/\1/p')
 case "$message" in
 *'"method":"config/batchWrite"'*)
   printf 'model = "novo"\n' > "$CODEX_HOME/config.toml"
   /bin/mv "$FIXTURE_WORK" "$FIXTURE_WORK.saved"
   printf blocked > "$FIXTURE_WORK"
   printf '{"id":%s,"result":{}}\n' "$id";;
 *'"method":"initialize"'*) printf '{"id":%s,"result":{}}\n' "$id";;
 esac
done
"##,
    )
    .unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o700)).unwrap();
    let work = root.path().join("work");
    let path = root.path().join("config.toml");
    std::fs::write(&path, "model = \"antigo\"\n").unwrap();
    let env = AccountEnvironment::from_map(
        [
            ("HOME".into(), root.path().to_string_lossy().into_owned()),
            ("PATH".into(), bin.to_string_lossy().into_owned()),
            ("FIXTURE_WORK".into(), work.to_string_lossy().into_owned()),
        ]
        .into(),
    );
    let result = files::edit_config(&env, &path, &root.path().join("backups"), &work, |_| {
        Ok(vec![
            json!({"keyPath":"model","value":"novo","mergeStrategy":"replace"}),
        ])
    })
    .await;
    std::fs::set_permissions(&work, std::fs::Permissions::from_mode(0o700)).unwrap();
    assert!(result.is_err(), "falha de limpeza precisa impedir sucesso");
    assert_eq!(
        std::fs::read_to_string(path).unwrap(),
        "model = \"antigo\"\n"
    );
}
