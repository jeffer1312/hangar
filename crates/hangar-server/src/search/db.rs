// crates/hangar-server/src/search/db.rs
//! Abrir o banco do índice: PRAGMAs, esquema por `user_version`, arquivo corrompido apagado e refeito.

use std::path::Path;
use std::time::Duration;

use rusqlite::{Connection, ErrorCode};

use super::IndexError;

pub(super) const SCHEMA_VERSION: i64 = 2;

// O `_SCHEMA` do Python mais conta, agente e o que o Arquivo lê do arquivo.
const SCHEMA: &str = "
CREATE TABLE files(
    id INTEGER PRIMARY KEY,
    path TEXT UNIQUE NOT NULL,
    project TEXT NOT NULL,
    session_id TEXT NOT NULL,
    size INTEGER NOT NULL DEFAULT 0,
    mtime_ns INTEGER NOT NULL DEFAULT 0,
    ino INTEGER NOT NULL DEFAULT 0,
    offset INTEGER NOT NULL DEFAULT 0,
    lines INTEGER NOT NULL DEFAULT 0,
    cwd TEXT,
    preview TEXT NOT NULL DEFAULT '',
    head_done INTEGER NOT NULL DEFAULT 0,
    internal INTEGER,
    provider TEXT NOT NULL DEFAULT 'claude',
    config_dir TEXT,
    account TEXT,
    has_user INTEGER NOT NULL DEFAULT 0,
    last_text TEXT NOT NULL DEFAULT '',
    rewrite TEXT,
    head_len INTEGER NOT NULL DEFAULT 0,
    head_hash TEXT
);
CREATE TABLE msg(
    id INTEGER PRIMARY KEY,
    file_id INTEGER NOT NULL,
    event_id TEXT NOT NULL,
    role TEXT NOT NULL,
    ts REAL,
    text TEXT NOT NULL
);
CREATE INDEX msg_file ON msg(file_id);
CREATE VIRTUAL TABLE msg_fts USING fts5(
    text, content='msg', content_rowid='id', tokenize='trigram remove_diacritics 1');
CREATE TRIGGER msg_ai AFTER INSERT ON msg BEGIN
    INSERT INTO msg_fts(rowid, text) VALUES (new.id, new.text);
END;
CREATE TRIGGER msg_ad AFTER DELETE ON msg BEGIN
    INSERT INTO msg_fts(msg_fts, rowid, text) VALUES ('delete', old.id, old.text);
END;
CREATE TABLE meta(k TEXT PRIMARY KEY, v TEXT);
";

fn connect(path: &Path) -> rusqlite::Result<Connection> {
    let conn = Connection::open(path)?;
    conn.busy_timeout(Duration::from_secs(10))?;
    conn.pragma_update(None, "journal_mode", "WAL")?;
    conn.pragma_update(None, "synchronous", "NORMAL")?;
    // Sem teto, o WAL fica do tamanho do maior lote de escrita mesmo após o checkpoint.
    conn.pragma_update(None, "journal_size_limit", 8_388_608)?;
    let version: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
    if version != SCHEMA_VERSION {
        let old: Vec<(String, String)> = conn.prepare(
            "SELECT type, name FROM sqlite_master WHERE type IN ('table','trigger') \
             AND name NOT LIKE 'sqlite_%' AND name NOT LIKE 'msg_fts_%'")?
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<Result<_, _>>()?;
        for (kind, name) in old {
            conn.execute_batch(&format!("DROP {} IF EXISTS \"{}\"", kind.to_uppercase(), name.replace('"', "\"\"")))?;
        }
        conn.execute_batch(SCHEMA)?;
        conn.pragma_update(None, "user_version", SCHEMA_VERSION)?;
    }
    Ok(conn)
}

fn corrupt(e: &rusqlite::Error) -> bool {
    matches!(e.sqlite_error_code(), Some(ErrorCode::DatabaseCorrupt | ErrorCode::NotADatabase))
}

/// Índice é cache: arquivo ilegível se apaga (banco, `-wal`, `-shm`) e se refaz uma vez.
pub(super) fn open(path: &Path) -> Result<Connection, IndexError> {
    std::fs::create_dir_all(path.parent().unwrap_or(Path::new(".")))?;
    match connect(path) {
        Err(e) if corrupt(&e) => {
            tracing::warn!(error = %e, "índice da busca ilegível, reconstruindo");
            // `connect` já soltou a conexão: no Windows o arquivo aberto não sai.
            for suffix in ["", "-wal", "-shm"] {
                let mut p = path.as_os_str().to_os_string();
                p.push(suffix);
                match std::fs::remove_file(&p) {
                    Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(e.into()),
                    _ => {}
                }
            }
            Ok(connect(path)?)
        }
        other => Ok(other?),
    }
}
