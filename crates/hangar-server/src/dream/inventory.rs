//! Sessões desta máquina com conversa nova, tiradas do índice da Busca (todos os agentes e contas),
//! e as pastas de memória de cada raiz.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use hangar_dream::project::{canonical_root, memory_dir, project_of, sanitize};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SessionRef {
    pub path: String,
    pub provider: String,
    pub cwd: String,
    pub mtime: DateTime<Utc>,
    /// Raiz do checkout principal e o projeto; ausentes quando a pasta já sumiu desta máquina.
    pub root: Option<String>,
    pub project: Option<String>,
}

/// Leitura bloqueante (SQLite e git): chamar dentro de `spawn_blocking`.
pub fn local_sessions(index: &Path, since: Option<DateTime<Utc>>) -> Result<Vec<SessionRef>, String> {
    let conn = rusqlite::Connection::open_with_flags(index, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(|e| format!("dream_index_unavailable: {e}"))?;
    // Sem a espera a leitura só desiste antes, e isso já sai como `dream_index_unavailable`.
    let _ = conn.busy_timeout(std::time::Duration::from_secs(10));
    let since_ns = since.and_then(|t| t.timestamp_nanos_opt()).unwrap_or(0);
    let mut stmt = conn
        .prepare("SELECT path, provider, cwd, mtime_ns FROM files WHERE has_user=1 AND COALESCE(internal,0)=0 AND mtime_ns>?1 AND cwd IS NOT NULL AND cwd<>''")
        .map_err(|e| format!("dream_index_unavailable: {e}"))?;
    let rows = stmt
        .query_map([since_ns], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?, r.get::<_, i64>(3)?)))
        .map_err(|e| format!("dream_index_unavailable: {e}"))?;
    let mut roots: HashMap<String, Option<(String, String)>> = HashMap::new();
    let mut out = Vec::new();
    for row in rows {
        let (path, provider, cwd, mtime_ns) = row.map_err(|e| format!("dream_index_unavailable: {e}"))?;
        if hangar_transcript::Provider::parse(&provider).is_none() {
            continue;
        }
        let resolved = roots
            .entry(cwd.clone())
            .or_insert_with(|| canonical_root(Path::new(&cwd)).map(|root| (root.to_string_lossy().into_owned(), project_of(&root).0)))
            .clone();
        out.push(SessionRef {
            path,
            provider,
            cwd,
            mtime: DateTime::<Utc>::from_timestamp_nanos(mtime_ns),
            root: resolved.as_ref().map(|r| r.0.clone()),
            project: resolved.map(|r| r.1),
        });
    }
    Ok(out)
}

/// A pasta principal (`~/.claude/projects/<raiz>/memory`) e as de contas que não são atalho para
/// ela: `link_memory` pula o projeto que não tinha pasta na principal.
pub fn memory_dirs(claude_base: &Path, account_dirs: &[PathBuf], root: &Path) -> Vec<PathBuf> {
    let main = memory_dir(claude_base, root);
    let name = sanitize(&root.to_string_lossy());
    let mut dirs = vec![main];
    for account in account_dirs.iter().filter(|d| d.as_path() != claude_base) {
        let dir = account.join("projects").join(&name).join("memory");
        if std::fs::symlink_metadata(&dir).is_ok_and(|m| m.is_dir()) {
            dirs.push(dir);
        }
    }
    dirs
}
