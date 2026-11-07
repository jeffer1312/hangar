//! Destinos OAuth Pi/omp do login Codex, lidos e gravados dentro da guarda da conta.
use super::{codex_device_login::atomic_json_linked, environment::AccountEnvironment};
use rusqlite::{Connection, OpenFlags, TransactionBehavior};
use serde_json::{Map, Value, json};
use std::{path::Path, time::Duration};

const PROVIDER: &str = "openai-codex";
const BUSY: Duration = Duration::from_secs(5);
const BUSY_STEP: Duration = Duration::from_millis(10);

/// Espera o banco ocupado até `BUSY`. Não o `busy_timeout` do SQLite: o `nanosleep` dele volta no
/// primeiro sinal (o SIGCHLD de qualquer filho do servidor) e a espera acaba antes do prazo.
/// O prazo é do relógio, não da contagem de passos: cada `sleep` dura mais que o pedido (relógio de
/// ~16 ms do Windows, máquina carregada) e 500 passos passariam muito de `BUSY`.
fn wait_busy(connection: &Connection) -> rusqlite::Result<()> {
    thread_local! { static STARTED: std::cell::Cell<std::time::Instant> = std::cell::Cell::new(std::time::Instant::now()); }
    fn step(count: i32) -> bool {
        // O SQLite chama com 0 a cada nova espera pela trava, na linha que espera.
        if count == 0 { STARTED.set(std::time::Instant::now()); }
        let Some(left) = BUSY.checked_sub(STARTED.get().elapsed()).filter(|left| !left.is_zero()) else { return false };
        // O `sleep` da std retoma depois de um sinal até completar o passo.
        std::thread::sleep(BUSY_STEP.min(left));
        true
    }
    connection.busy_handler(Some(step))
}

pub use hangar_core::account_env::{OmpDirectories, omp_directories};

fn barrier(env: &AccountEnvironment, path: &Path) {
    if let Some(barrier) = &env.secondary_barrier {
        barrier(path);
    }
}

fn pi_logged(path: &Path) -> bool {
    std::fs::read(path)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
        .and_then(|value| value.get(PROVIDER).cloned())
        .is_some_and(|entry| entry.is_object() && entry["type"] == "oauth" && crate::costs::accumulator::truthy(entry.get("refresh")))
}

fn write_pi(env: &AccountEnvironment, tokens: &Value) -> (bool, String) {
    let dir = env.home.join(".pi").join("agent");
    if !dir.is_dir() {
        return (false, "nao-instalado".into());
    }
    let path = dir.join("auth.json");
    barrier(env, &path);
    if pi_logged(&path) {
        return (true, "ja-logado".into());
    }
    let mut data = Map::new();
    if path.exists() {
        let Ok(bytes) = std::fs::read(&path) else {
            return (false, "armazenamento-indisponivel".into());
        };
        match serde_json::from_slice(&bytes) {
            Ok(Value::Object(existing)) => data = existing,
            _ => return (false, "auth-invalido".into()),
        }
    }
    data.insert(
        PROVIDER.into(),
        json!({"type":"oauth","access":tokens["access"],"refresh":tokens["refresh"],
            "expires":tokens["expires_ms"],"accountId":tokens["account_id"]}),
    );
    match atomic_json_linked(&path, &Value::Object(data)) {
        Ok(()) => (true, path.to_string_lossy().into()),
        Err(_) => (false, "armazenamento-indisponivel".into()),
    }
}

fn has_oauth(connection: &Connection) -> rusqlite::Result<bool> {
    connection.query_row(
        "select count(*) from auth_credentials where provider=?1 and credential_type='oauth'",
        [PROVIDER],
        |row| row.get::<_, i64>(0).map(|count| count > 0),
    )
}

/// Mesmo texto do `json.dumps` do Python, na ordem dada: separadores com espaço e só ASCII.
fn python_dumps(entries: &[(&str, &Value)]) -> String {
    let object: Map<String, Value> = entries
        .iter()
        .map(|(key, value)| ((*key).to_owned(), (*value).clone()))
        .collect();
    crate::transcript::pyjson::dumps(&Value::Object(object), false)
}

fn insert_omp(db: &Path, tokens: &Value) -> rusqlite::Result<bool> {
    // Sem CREATE: o banco e o esquema são do omp.
    let mut connection = Connection::open_with_flags(db, OpenFlags::SQLITE_OPEN_READ_WRITE)?;
    wait_busy(&connection)?;
    if has_oauth(&connection)? {
        return Ok(true);
    }
    let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    // Outro escritor pode ter entrado entre a leitura e a reserva.
    if has_oauth(&transaction)? {
        return Ok(true);
    }
    let data = python_dumps(&[
        ("access", &tokens["access"]),
        ("refresh", &tokens["refresh"]),
        ("expires", &tokens["expires_ms"]),
        ("accountId", &tokens["account_id"]),
    ]);
    let identity = tokens["account_id"].as_str().filter(|v| !v.is_empty());
    transaction.execute(
        "insert into auth_credentials (provider, credential_type, data, identity_key) values (?1, 'oauth', ?2, ?3)",
        rusqlite::params![PROVIDER, data, identity],
    )?;
    transaction.commit()?;
    Ok(false)
}

fn write_omp(env: &AccountEnvironment, tokens: &Value) -> (bool, String) {
    let Ok(directories) = env.omp_directories() else {
        return (false, "armazenamento-indisponivel".into());
    };
    let db = directories.agent_dir.join("agent.db");
    if !db.is_file() {
        return (false, "nao-instalado".into());
    }
    barrier(env, &db);
    match insert_omp(&db, tokens) {
        Ok(true) => (true, "ja-logado".into()),
        Ok(false) => (true, db.to_string_lossy().into()),
        Err(_) => (false, "sqlite-indisponivel".into()),
    }
}

/// Grava Pi e omp em separado, para conservar o sucesso parcial; só devolve códigos.
pub fn write(env: &AccountEnvironment, tokens: &Value) -> Value {
    let (pi_ok, pi_reason) = write_pi(env, tokens);
    let (omp_ok, omp_reason) = write_omp(env, tokens);
    json!({"pi":{"ok":pi_ok,"motivo":pi_reason},"omp":{"ok":omp_ok,"motivo":omp_reason}})
}

fn omp_logged(db: &Path) -> bool {
    Connection::open_with_flags(db, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .and_then(|connection| {
            wait_busy(&connection)?;
            has_oauth(&connection)
        })
        .unwrap_or(false)
}

/// Leitura do estado: nada é criado, gravado, propagado ou importado.
pub fn inspect(env: &AccountEnvironment) -> Result<Value, &'static str> {
    let pi = pi_logged(&env.home.join(".pi").join("agent").join("auth.json"));
    let db = env
        .omp_directories()
        .map_err(|_| "device_bridge_unavailable")?
        .agent_dir
        .join("agent.db");
    Ok(json!({"pi":pi,"omp":db.is_file() && omp_logged(&db)}))
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;
    use serde_json::json;
    use std::{collections::BTreeMap, path::PathBuf};
    use std::time::{Duration, Instant};

    fn env(home: &Path, vars: &[(&str, &str)]) -> AccountEnvironment {
        let mut base: BTreeMap<String, String> = vars
            .iter()
            .map(|(key, value)| (key.to_string(), value.to_string()))
            .collect();
        base.insert("HOME".into(), home.to_string_lossy().into());
        base.insert("USERPROFILE".into(), home.to_string_lossy().into());
        AccountEnvironment::from_map(base)
    }
    fn tokens(account: &str) -> Value {
        json!({"access":"synthetic-access","refresh":"synthetic-refresh","id_token":"",
            "expires_ms":4102444800000i64,"account_id":account,"plano":"plus"})
    }
    fn pi(home: &Path) -> PathBuf {
        let dir = home.join(".pi").join("agent");
        std::fs::create_dir_all(&dir).unwrap();
        dir.join("auth.json")
    }
    fn omp(home: &Path) -> PathBuf {
        let dir = home.join(".omp").join("agent");
        std::fs::create_dir_all(&dir).unwrap();
        let db = dir.join("agent.db");
        Connection::open(&db)
            .unwrap()
            .execute(
                "create table auth_credentials (id integer primary key autoincrement, provider text not null, \
                 credential_type text not null, data text not null, identity_key text)",
                [],
            )
            .unwrap();
        db
    }
    type Row = (i64, String, String, String, Option<String>);
    fn rows(db: &Path) -> Vec<Row> {
        let connection = Connection::open(db).unwrap();
        let mut statement = connection
            .prepare("select id, provider, credential_type, data, identity_key from auth_credentials order by id")
            .unwrap();
        statement
            .query_map([], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?))
            })
            .unwrap()
            .map(Result::unwrap)
            .collect()
    }
    fn insert(db: &Path, rows: &[(i64, &str, &str, &str, Option<&str>)]) {
        let connection = Connection::open(db).unwrap();
        for row in rows {
            connection
                .execute(
                    "insert into auth_credentials values (?1, ?2, ?3, ?4, ?5)",
                    rusqlite::params![row.0, row.1, row.2, row.3, row.4],
                )
                .unwrap();
        }
    }
    fn result(ok: bool, reason: &str) -> Value {
        json!({"ok":ok,"motivo":reason})
    }

    #[test]
    fn pi_without_agent_directory_is_not_installed_and_nothing_is_created() {
        let home = tempfile::tempdir().unwrap();
        let written = write(&env(home.path(), &[]), &tokens("acc"));
        assert_eq!(written["pi"], result(false, "nao-instalado"));
        assert!(!home.path().join(".pi").exists());
    }

    #[test]
    fn pi_existing_oauth_is_kept_byte_for_byte() {
        let home = tempfile::tempdir().unwrap();
        let path = pi(home.path());
        let original = b"{ \"b\": 1, \"openai-codex\": {\"type\": \"oauth\", \"refresh\": \"existing\"} }\n";
        std::fs::write(&path, original).unwrap();
        let written = write(&env(home.path(), &[]), &tokens("acc"));
        assert_eq!(written["pi"], result(true, "ja-logado"));
        assert_eq!(std::fs::read(&path).unwrap(), original);
    }

    #[test]
    fn pi_invalid_destination_is_reported_and_kept() {
        let home = tempfile::tempdir().unwrap();
        let path = pi(home.path());
        for original in [&b"{"[..], b"[]", b"null", b"\xff"] {
            std::fs::write(&path, original).unwrap();
            let written = write(&env(home.path(), &[]), &tokens("acc"));
            assert_eq!(written["pi"], result(false, "auth-invalido"));
            assert_eq!(std::fs::read(&path).unwrap(), original);
        }
    }

    #[test]
    fn pi_writes_exact_fields_and_preserves_other_keys() {
        let home = tempfile::tempdir().unwrap();
        let path = pi(home.path());
        std::fs::write(
            &path,
            r#"{"zeta":{"k":1},"alpha":[1,2.5],"openai-codex":{"type":"api_key","key":"synthetic","refresh":""}}"#,
        )
        .unwrap();
        let written = write(&env(home.path(), &[]), &tokens("acc"));
        assert_eq!(written["pi"], result(true, &path.to_string_lossy()));
        let value: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        let keys: Vec<&String> = value.as_object().unwrap().keys().collect();
        assert_eq!(keys, ["zeta", "alpha", "openai-codex"]);
        assert_eq!(value["zeta"], json!({"k":1}));
        assert_eq!(value["alpha"], json!([1, 2.5]));
        assert_eq!(
            value["openai-codex"],
            json!({"type":"oauth","access":"synthetic-access","refresh":"synthetic-refresh",
                "expires":4102444800000i64,"accountId":"acc"})
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
        }
        std::fs::remove_file(&path).unwrap();
        write(&env(home.path(), &[]), &tokens("acc"));
        let value: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(value.as_object().unwrap().len(), 1);
    }

    #[cfg(unix)]
    #[test]
    fn pi_linked_file_is_replaced_and_its_target_kept() {
        use std::os::unix::fs::PermissionsExt;
        let home = tempfile::tempdir().unwrap();
        let path = pi(home.path());
        let target = home.path().join("shared-auth.json");
        let original = br#"{"keep":{"k":1}}"#;
        std::fs::write(&target, original).unwrap();
        std::os::unix::fs::symlink(&target, &path).unwrap();
        let written = write(&env(home.path(), &[]), &tokens("acc"));
        assert_eq!(written["pi"], result(true, &path.to_string_lossy()), "o link de arquivo Pi passou a falhar");
        let metadata = std::fs::symlink_metadata(&path).unwrap();
        assert!(metadata.is_file() && !metadata.file_type().is_symlink(), "o link não foi substituído pelo arquivo");
        assert_eq!(metadata.permissions().mode() & 0o777, 0o600);
        assert_eq!(std::fs::read(&target).unwrap(), original, "a escrita atravessou o link");
        let value: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(value["keep"], json!({"k":1}));
        assert_eq!(value["openai-codex"]["refresh"], "synthetic-refresh");
    }

    #[cfg(unix)]
    #[test]
    fn pi_linked_directory_stays_linked() {
        let home = tempfile::tempdir().unwrap();
        let real = home.path().join("real-agent");
        std::fs::create_dir_all(&real).unwrap();
        std::fs::write(real.join("auth.json"), br#"{"keep":true}"#).unwrap();
        std::fs::create_dir_all(home.path().join(".pi")).unwrap();
        let linked = home.path().join(".pi").join("agent");
        std::os::unix::fs::symlink(&real, &linked).unwrap();
        let written = write(&env(home.path(), &[]), &tokens("acc"));
        assert_eq!(
            written["pi"],
            result(true, &linked.join("auth.json").to_string_lossy()),
            "o diretório Pi vinculado passou a falhar"
        );
        assert!(std::fs::symlink_metadata(&linked).unwrap().file_type().is_symlink());
        let value: Value = serde_json::from_slice(&std::fs::read(real.join("auth.json")).unwrap()).unwrap();
        assert_eq!(value["keep"], true);
        assert_eq!(value["openai-codex"]["refresh"], "synthetic-refresh");
    }

    #[test]
    fn pi_unreadable_destination_has_sanitized_code() {
        let home = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(pi(home.path())).unwrap();
        let written = write(&env(home.path(), &[]), &tokens("acc"));
        assert_eq!(written["pi"], result(false, "armazenamento-indisponivel"));
    }

    #[test]
    fn omp_without_database_is_not_installed_and_not_created() {
        let home = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(home.path().join(".omp/agent")).unwrap();
        let written = write(&env(home.path(), &[]), &tokens("acc"));
        assert_eq!(written["omp"], result(false, "nao-instalado"));
        assert!(!home.path().join(".omp/agent/agent.db").exists());
    }

    #[test]
    fn omp_keeps_every_previous_oauth_and_api_key_row() {
        let home = tempfile::tempdir().unwrap();
        let db = omp(home.path());
        insert(
            &db,
            &[
                (3, "openai-codex", "api_key", r#"{"key":"synthetic"}"#, None),
                (4, "anthropic", "oauth", "{}", Some("other")),
                (7, "openai-codex", "oauth", r#"{"refresh": "first"}"#, Some("first")),
                (9, "openai-codex", "oauth", r#"{"refresh": "second"}"#, None),
            ],
        );
        let before = std::fs::read(&db).unwrap();
        let written = write(&env(home.path(), &[]), &tokens("acc"));
        assert_eq!(written["omp"], result(true, "ja-logado"));
        assert_eq!(std::fs::read(&db).unwrap(), before);
    }

    #[test]
    fn omp_inserts_once_in_the_current_format() {
        let home = tempfile::tempdir().unwrap();
        let db = omp(home.path());
        insert(&db, &[(3, "openai-codex", "api_key", r#"{"key":"synthetic"}"#, None)]);
        let written = write(&env(home.path(), &[]), &tokens("acc"));
        assert_eq!(written["omp"], result(true, &db.to_string_lossy()));
        let after = rows(&db);
        assert_eq!(after.len(), 2);
        assert_eq!(after[0], (3, "openai-codex".into(), "api_key".into(), r#"{"key":"synthetic"}"#.into(), None));
        assert_eq!(
            after[1].3,
            r#"{"access": "synthetic-access", "refresh": "synthetic-refresh", "expires": 4102444800000, "accountId": "acc"}"#
        );
        assert_eq!((after[1].1.as_str(), after[1].2.as_str(), after[1].4.as_deref()), ("openai-codex", "oauth", Some("acc")));
        assert_eq!(write(&env(home.path(), &[]), &tokens("acc"))["omp"], result(true, "ja-logado"));
        assert_eq!(rows(&db).len(), 2);
        let other = tempfile::tempdir().unwrap();
        let db = omp(other.path());
        write(&env(other.path(), &[]), &tokens(""));
        assert_eq!(rows(&db)[0].4, None, "conta vazia deve gravar identity_key NULL");
    }

    #[test]
    fn python_format_escapes_like_json_dumps() {
        let text = json!("açaí\n\"/\u{1F600}");
        let number = json!(42);
        assert_eq!(
            python_dumps(&[("a", &text), ("b", &number)]),
            r#"{"a": "a\u00e7a\u00ed\n\"/\ud83d\ude00", "b": 42}"#
        );
    }

    #[test]
    fn omp_without_schema_is_not_created() {
        let home = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(home.path().join(".omp/agent")).unwrap();
        let db = home.path().join(".omp/agent/agent.db");
        std::fs::write(&db, b"").unwrap();
        let written = write(&env(home.path(), &[]), &tokens("acc"));
        assert_eq!(written["omp"], result(false, "sqlite-indisponivel"));
        assert_eq!(std::fs::read(&db).unwrap(), b"");
    }

    #[test]
    fn omp_error_content_never_leaves_and_pi_keeps_its_result() {
        let home = tempfile::tempdir().unwrap();
        let db = omp(home.path());
        pi(home.path());
        Connection::open(&db)
            .unwrap()
            .execute(
                "create trigger deny before insert on auth_credentials begin select raise(fail, 'synthetic-secret'); end",
                [],
            )
            .unwrap();
        let written = write(&env(home.path(), &[]), &tokens("acc"));
        assert_eq!(written["omp"], result(false, "sqlite-indisponivel"));
        assert!(written["pi"]["ok"].as_bool().unwrap());
        assert!(!written.to_string().contains("synthetic-secret"));
        assert!(rows(&db).is_empty());
    }

    #[test]
    fn omp_busy_database_expires_with_sanitized_code() {
        let home = tempfile::tempdir().unwrap();
        let db = omp(home.path());
        let lock = Connection::open(&db).unwrap();
        lock.execute_batch("begin exclusive").unwrap();
        let started = Instant::now();
        let written = write(&env(home.path(), &[]), &tokens("acc"));
        let elapsed = started.elapsed();
        assert_eq!(written["omp"], result(false, "sqlite-indisponivel"));
        assert!(elapsed >= Duration::from_millis(4500), "busy terminou antes do prazo: {elapsed:?}");
        assert!(elapsed < Duration::from_secs(15), "busy passou do prazo: {elapsed:?}");
        lock.execute_batch("rollback").unwrap();
        assert!(rows(&db).is_empty());
    }

    /// Sinal no meio da espera (o SIGCHLD de um filho que sai, no servidor): a espera continua até o prazo.
    #[cfg(unix)]
    #[test]
    fn omp_busy_wait_keeps_its_deadline_under_signals() {
        extern "C" fn ignore(_: libc::c_int) {}
        let home = tempfile::tempdir().unwrap();
        let db = omp(home.path());
        let lock = Connection::open(&db).unwrap();
        lock.execute_batch("begin exclusive").unwrap();
        // SAFETY: tratador vazio, instalado antes de a linha que espera receber o sinal.
        unsafe {
            let mut action: libc::sigaction = std::mem::zeroed();
            action.sa_sigaction = ignore as *const () as usize;
            libc::sigaction(libc::SIGUSR1, &action, std::ptr::null_mut());
        }
        let waiting = unsafe { libc::pthread_self() } as usize;
        let done = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let signals = { let done = done.clone(); std::thread::spawn(move || while !done.load(std::sync::atomic::Ordering::Acquire) {
            // SAFETY: a linha que espera só termina depois de `done`.
            unsafe { libc::pthread_kill(waiting as libc::pthread_t, libc::SIGUSR1) };
            std::thread::sleep(Duration::from_millis(20));
        }) };
        let started = Instant::now();
        let written = write(&env(home.path(), &[]), &tokens("acc"));
        let elapsed = started.elapsed();
        done.store(true, std::sync::atomic::Ordering::Release);
        signals.join().unwrap();
        assert_eq!(written["omp"], result(false, "sqlite-indisponivel"));
        assert!(elapsed >= Duration::from_millis(4500), "o sinal encurtou a espera: {elapsed:?}");
        lock.execute_batch("rollback").unwrap();
    }

    #[test]
    fn invalid_profile_fails_omp_but_keeps_pi_result() {
        let home = tempfile::tempdir().unwrap();
        let db = omp(home.path());
        pi(home.path());
        let written = write(&env(home.path(), &[("OMP_PROFILE", "../fora")]), &tokens("acc"));
        assert_eq!(written["omp"], result(false, "armazenamento-indisponivel"));
        assert!(written["pi"]["ok"].as_bool().unwrap());
        assert!(rows(&db).is_empty(), "perfil inválido caiu na raiz padrão");
    }

    #[test]
    fn inspect_reads_without_creating_or_writing() {
        let home = tempfile::tempdir().unwrap();
        let state = |vars: &[(&str, &str)]| inspect(&env(home.path(), vars));
        assert_eq!(state(&[]).unwrap(), json!({"pi":false,"omp":false}));
        assert!(!home.path().join(".pi").exists() && !home.path().join(".omp").exists());
        let path = pi(home.path());
        std::fs::write(&path, r#"{"openai-codex":{"type":"oauth","refresh":""}}"#).unwrap();
        let db = omp(home.path());
        insert(&db, &[(1, "openai-codex", "api_key", "{}", None)]);
        assert_eq!(state(&[]).unwrap(), json!({"pi":false,"omp":false}));
        std::fs::write(&path, r#"{"openai-codex":{"type":"oauth","refresh":"synthetic"}}"#).unwrap();
        insert(&db, &[(2, "openai-codex", "oauth", "{}", None)]);
        let (pi_bytes, db_bytes) = (std::fs::read(&path).unwrap(), std::fs::read(&db).unwrap());
        assert_eq!(state(&[]).unwrap(), json!({"pi":true,"omp":true}));
        assert_eq!((std::fs::read(&path).unwrap(), std::fs::read(&db).unwrap()), (pi_bytes, db_bytes));
        assert_eq!(state(&[("OMP_PROFILE", "Inválido")]), Err("device_bridge_unavailable"));
    }

    fn resolve(home: &Path, vars: &[(&str, &str)]) -> Result<OmpDirectories, &'static str> {
        omp_directories(home, &vars.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect())
    }

    #[test]
    fn omp_profile_precedence_and_names() {
        let home = tempfile::tempdir().unwrap();
        let home = home.path();
        let root = home.join(".omp");
        let plain = resolve(home, &[]).unwrap();
        assert_eq!(plain, OmpDirectories { config_root: root.clone(), agent_dir: root.join("agent"), data_root: root.clone() });
        let work = root.join("profiles").join("work");
        assert_eq!(resolve(home, &[("OMP_PROFILE", " work ")]).unwrap().agent_dir, work.join("agent"));
        assert_eq!(resolve(home, &[("PI_PROFILE", "work")]).unwrap().agent_dir, work.join("agent"));
        assert_eq!(resolve(home, &[("OMP_PROFILE", ""), ("PI_PROFILE", "work")]).unwrap(), plain);
        assert_eq!(resolve(home, &[("OMP_PROFILE", "default")]).unwrap(), plain);
        assert!(resolve(home, &[("OMP_PROFILE", "a1._-z")]).is_ok());
        let long = "a".repeat(65);
        for invalid in ["../x", "Work", "a.", ".", "..", "con", "Lpt1.txt", "aux", "a/b", "-a", long.as_str()] {
            assert!(resolve(home, &[("OMP_PROFILE", invalid)]).is_err(), "perfil aceito: {invalid}");
        }
        assert!(resolve(home, &[("OMP_PROFILE", "../x"), ("PI_PROFILE", "work")]).is_err());
    }

    #[test]
    fn omp_directory_overrides_are_lexical() {
        let home = tempfile::tempdir().unwrap();
        let home = home.path();
        assert_eq!(resolve(home, &[("PI_CONFIG_DIR", ".pi-omp")]).unwrap().agent_dir, home.join(".pi-omp").join("agent"));
        assert_eq!(
            resolve(home, &[("PI_CODING_AGENT_DIR", "custom/./x/../agent")]).unwrap().agent_dir,
            home.join("custom").join("agent")
        );
        assert_eq!(
            resolve(home, &[("PI_CODING_AGENT_DIR", "custom"), ("OMP_PROFILE", "work")]).unwrap().agent_dir,
            home.join(".omp").join("profiles").join("work").join("agent"),
            "perfil deve vencer o diretório explícito"
        );
        let legacy = home.join(".omp").join("profiles").join("old").join("agent");
        assert_eq!(
            resolve(home, &[("OMP_PROFILE", ""), ("PI_PROFILE", "old"), ("PI_CODING_AGENT_DIR", &legacy.to_string_lossy())])
                .unwrap()
                .agent_dir,
            home.join(".omp").join("agent"),
            "override legado igual ao perfil anterior deve ser ignorado"
        );
        assert!(resolve(home, &[("PI_CODING_AGENT_DIR", "a\0b")]).is_err());
        #[cfg(unix)]
        {
            assert_eq!(
                resolve(home, &[("PI_CODING_AGENT_DIR", "/opt/a/../b")]).unwrap().agent_dir,
                PathBuf::from("/opt/b")
            );
            assert_eq!(resolve(home, &[("PI_CONFIG_DIR", "/abs")]).unwrap().agent_dir, home.join("abs").join("agent"));
        }
    }

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[test]
    fn xdg_moves_only_data_root_when_it_exists() {
        let home = tempfile::tempdir().unwrap();
        let home = home.path();
        let xdg = home.join("xdg");
        let vars = [("XDG_DATA_HOME", xdg.to_str().unwrap())];
        assert_eq!(resolve(home, &vars).unwrap().data_root, home.join(".omp"));
        std::fs::create_dir_all(xdg.join("omp")).unwrap();
        let resolved = resolve(home, &vars).unwrap();
        assert_eq!(resolved.data_root, xdg.join("omp"));
        assert_eq!(resolved.agent_dir, home.join(".omp").join("agent"), "XDG não move o agent.db");
        let custom = [("XDG_DATA_HOME", xdg.to_str().unwrap()), ("PI_CODING_AGENT_DIR", "custom")];
        assert_eq!(resolve(home, &custom).unwrap().data_root, home.join(".omp"));
    }

    #[cfg(windows)]
    #[test]
    fn windows_drive_relative_paths_are_refused() {
        let home = tempfile::tempdir().unwrap();
        let home = home.path();
        assert!(resolve(home, &[("PI_CODING_AGENT_DIR", "D:relativo")]).is_err());
        assert!(resolve(home, &[("PI_CONFIG_DIR", "D:\\omp")]).is_err());
        assert!(resolve(home, &[("PI_CONFIG_DIR", "\\\\server\\share")]).is_err());
        assert_eq!(
            resolve(home, &[("PI_CODING_AGENT_DIR", "D:\\omp\\x\\..\\agent")]).unwrap().agent_dir,
            PathBuf::from("D:\\omp\\agent")
        );
        assert_eq!(
            resolve(home, &[("pi_coding_agent_dir", "custom")]).unwrap().agent_dir,
            home.join("custom"),
            "variáveis do Windows não diferenciam caixa"
        );
    }
}
