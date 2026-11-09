use crate::{accounts::environment::AccountEnvironment, routes::AppState, worktree_routes::{Activity, Context}};
use hangar_workspace::worktrees::Session;
use std::{collections::HashSet, path::{Path, PathBuf}, sync::Arc};

pub(crate) fn expanded(raw: &str, env: &AccountEnvironment) -> PathBuf {
    let Some(rest) = raw.strip_prefix('~') else { return raw.into() };
    let end = rest.find(|c| c == '/' || cfg!(windows) && c == '\\').unwrap_or(rest.len());
    let user = &rest[..end];
    let own = user.is_empty() || cfg!(windows) && env.base.get("USERNAME").is_some_and(|name| name == user);
    let home = if own { Some(env.home.clone()) } else {
        #[cfg(unix)]
        { user_home(user) }
        #[cfg(windows)]
        { env.base.get("USERNAME").filter(|name| env.home.file_name().is_some_and(|part| part == name.as_str()))
            .and_then(|_| env.home.parent()).map(|parent| parent.join(user)) }
    };
    home.map_or_else(|| raw.into(), |home| home.join(rest[end..].trim_start_matches(['/', '\\'])))
}

#[cfg(unix)]
fn user_home(user: &str) -> Option<PathBuf> {
    use std::{ffi::{CStr, CString, OsStr}, os::unix::ffi::OsStrExt};
    let name = CString::new(user).ok()?;
    let mut size = 1024;
    loop {
        let mut storage = vec![0u8; size];
        let mut record = std::mem::MaybeUninit::<libc::passwd>::uninit();
        let mut result = std::ptr::null_mut();
        let code = unsafe { libc::getpwnam_r(name.as_ptr(), record.as_mut_ptr(), storage.as_mut_ptr().cast(), storage.len(), &mut result) };
        if code == libc::ERANGE { size = size.checked_mul(2)?; continue; }
        if code != 0 || result.is_null() { return None; }
        let record = unsafe { record.assume_init() };
        if record.pw_dir.is_null() { return None; }
        return Some(PathBuf::from(OsStr::from_bytes(unsafe { CStr::from_ptr(record.pw_dir) }.to_bytes())));
    }
}

pub(crate) fn roots(st: &AppState) -> Result<Vec<String>, &'static str> {
    let config = crate::config_sync::runtime::RuntimeConfig::from_state(st).map_err(|_| "configuracao")?;
    let value = config.get("scan_roots");
    let raw = value.as_str().filter(|raw| !raw.trim().is_empty()).map(str::to_owned)
        .or(config.settings.value("CP_SCAN_ROOTS").map_err(|_| "configuracao")?)
        .unwrap_or_else(|| if cfg!(windows) { "~,~/Documents,~/Desktop" } else { "~/pessoal,~/sistemas" }.into());
    let mut roots = Vec::new();
    for entry in raw.split(',').map(str::trim).filter(|entry| !entry.is_empty()) {
        let path = hangar_workspace::real(&expanded(entry, &st.accounts.env));
        let text = path.to_string_lossy().into_owned();
        if path.is_dir() && !roots.contains(&text) { roots.push(text); }
    }
    Ok(roots)
}

pub(crate) fn project_bases(st: &AppState) -> Result<Vec<PathBuf>, &'static str> {
    let env = &st.accounts.env;
    let active = crate::list::links::resolve_lenient(&env.claude_base);
    let mut accounts = Vec::new();
    if !env.claude_fixed.trim().is_empty() {
        for item in env.claude_fixed.split(',').map(str::trim).filter(|item| !item.is_empty()) {
            let pair = item.split_once(':').filter(|(label, tail)| !(cfg!(windows) && label.len() == 1
                && label.as_bytes()[0].is_ascii_alphabetic() && tail.starts_with(['/', '\\'])));
            let path = pair.map_or(item, |(_, path)| path);
            accounts.push(crate::list::links::resolve_lenient(&expanded(path, env)));
        }
    } else {
        if let Ok(entries) = std::fs::read_dir(&env.home) {
            for entry in entries.flatten() {
                let path = entry.path();
                if !entry.file_name().to_string_lossy().starts_with(".claude") || !path.is_dir()
                    || path.join(".hangar-account-pending").exists() { continue; }
                let marker = path.join(crate::accounts::storage::CLAUDE_MARKER);
                let accepted = if marker.is_file() { !path.is_symlink() && !marker.is_symlink() }
                    else { path.join(".credentials.json").is_file() && path.join("projects").is_dir() };
                if accepted { accounts.push(crate::list::links::resolve_lenient(&path)); }
            }
        }
        accounts.sort_by_cached_key(|path| std::cmp::Reverse(crate::accounts::catalog::projects_mtime(&path.join("projects"))));
        if !accounts.contains(&active) { accounts.insert(0, active); }
    }
    let mut seen = HashSet::new();
    let mut bases = Vec::new();
    for account in accounts {
        if account.join(".hangar-account-pending").exists() { continue; }
        let projects = account.join("projects");
        if seen.insert(crate::list::links::resolve_lenient(&projects)) { bases.push(projects); }
    }
    let snapshot = st.settings_snapshot().map_err(|_| "configuracao")?;
    let own = snapshot.value("CP_PROJECTS_DIR").map_err(|_| "configuracao")?
        .map(PathBuf::from).unwrap_or_else(|| env.claude_base.join("projects"));
    if seen.insert(crate::list::links::resolve_lenient(&own)) { bases.push(own); }
    Ok(bases)
}

pub(crate) async fn load(st: Arc<AppState>) -> Result<Context, &'static str> {
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_err(|_| "relogio")?.as_secs_f64();
    let rows = st.list.discover(Some(now)).await.map_err(|_| "sessoes")?;
    tokio::task::spawn_blocking(move || collect(&st, &rows, now)).await.map_err(|_| "leitura")?
}

fn collect(st: &AppState, rows: &[hangar_api::session::SessionRow], now: f64) -> Result<Context, &'static str> {
    let home = &st.list.env().dirs.as_ref().ok_or("sessoes")?.home;
    let (orq, failed) = crate::list::local_facts::orq_rows(home, now);
    if failed.is_some() {
        st.diag.report_with("rust.worktrees_failed", "", "worktree_orq_uncertain", "leitura de orquestrações incompleta", serde_json::json!({}));
        return Err("orquestracoes");
    }
    let sessions = rows.iter().chain(orq.iter()).map(|row| Session {
        name: row.name.clone(), cwd: row.cwd.clone(),
        git_cwd: row.git_cwd.clone().filter(|cwd| !cwd.is_empty()).or_else(|| row.cwd.clone()),
        worktree_path: row.worktree_path.clone(), jsonl: row.jsonl.clone(),
    }).collect::<Vec<_>>();
    let roots = roots(st)?;
    let bases = project_bases(st)?;
    let folders = crate::archive_list::worktree_activities(st, &bases, now);
    let cwds = sessions.iter().filter_map(|session| session.cwd.clone())
        .chain(folders.iter().filter(|(_, mtime)| *mtime >= now - 30.0 * 86400.0).map(|(cwd, _)| cwd.clone()))
        .filter(|cwd| !cwd.is_empty() && roots.iter().any(|root| hangar_workspace::real(Path::new(cwd)).starts_with(root)))
        .collect();
    Ok(Context { roots, cwds, sessions, project_bases: bases.into_iter().map(|path| path.to_string_lossy().into_owned()).collect(),
        activities: Some(folders.into_iter().map(|(cwd, mtime)| Activity { cwd, mtime }).collect()) })
}

#[cfg(test)]
#[path = "worktree_context_tests.rs"]
mod tests;
