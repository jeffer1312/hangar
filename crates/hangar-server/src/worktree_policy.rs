//! Uma rodada por servidor; configuração desligada não consulta sessões nem percorre disco.
use crate::{
    routes::AppState,
    worktree_routes::{self, Context},
};
use hangar_workspace::{
    Result, error,
    worktree_cleanup::{self, Candidate},
    worktree_settings::WorktreeSettings,
};
use std::{
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

pub fn spawn(st: Arc<AppState>) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut ticks = tokio::time::interval(Duration::from_secs(60));
        ticks.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            ticks.tick().await;
            if cleanup_once(st.clone()).await.is_err() {
                st.diag.report(
                    "rust.worktrees_cleanup_failed",
                    "",
                    "worktrees_cleanup_unavailable",
                    "limpeza de worktrees indisponível; nenhuma exclusão foi presumida",
                );
            }
        }
    })
}
fn activity(ctx: &Context, path: &str, sources: &[(PathBuf, u64)]) -> Option<u64> {
    let root = hangar_workspace::real(Path::new(path));
    let mut newest = 0;
    for a in ctx.activities.as_ref()? {
        if !a.mtime.is_finite() || a.mtime < 0.0 || a.cwd.is_empty() {
            return None;
        }
        if hangar_workspace::real(Path::new(&a.cwd)).starts_with(&root) {
            newest = newest.max(a.mtime.ceil() as u64);
        }
    }
    let project = hangar_workspace::worktrees::sanitize_cwd(path);
    let resolved_project = hangar_workspace::worktrees::sanitize_cwd(&root.to_string_lossy());
    let bases = ctx
        .project_bases
        .iter()
        .map(|base| hangar_workspace::real(Path::new(base)))
        .collect::<Vec<_>>();
    for (file, mtime) in sources {
        if bases.iter().any(|base| {
            file.strip_prefix(base)
                .ok()
                .and_then(|p| p.components().next())
                .is_some_and(|component| {
                    let name = component.as_os_str().to_string_lossy();
                    [project.as_str(), resolved_project.as_str()]
                        .iter()
                        .any(|key| name == *key || name.starts_with(&format!("{key}-")))
                })
        }) {
            newest = newest.max(*mtime);
        }
    }
    Some(newest)
}
fn readable_sources(st: &AppState, ctx: &Context) -> Result<Vec<(PathBuf, u64)>> {
    let (claude, dirs) = st
        .list
        .state_dirs_blocking()
        .map_err(|_| error(503, "fontes de conversas desconhecidas"))?;
    let mut sources = ctx
        .project_bases
        .iter()
        .map(PathBuf::from)
        .collect::<Vec<_>>();
    sources.extend(claude.iter().map(|p| p.join("projects")));
    sources.extend([
        dirs.codex_home.join("sessions"),
        dirs.pi_sessions,
        dirs.omp_agent.join("sessions"),
        dirs.kimi_home.join("sessions"),
    ]);
    // Do omp só as sessões: o resto da pasta são extensões e perfis de navegador, não conversas.
    match std::fs::read_dir(dirs.omp_config.join("profiles")) {
        Ok(profiles) => {
            for profile in profiles {
                let profile = profile.map_err(|_| error(503, "fonte de conversas ilegível"))?;
                if profile.path().is_dir() {
                    sources.push(profile.path().join("agent/sessions"));
                }
            }
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(_) => return Err(error(503, "fonte de conversas ilegível")),
    }
    for account in st
        .accounts
        .snapshot(crate::accounts::Provider::Codex)
        .map_err(|_| error(503, "contas de conversas ilegíveis"))?
    {
        sources.push(account.home.join("sessions"));
    }
    // A existência dos provedores é opcional; arquivo que existe e não pode ser lido é incerteza.
    sources.sort();
    sources.dedup();
    let roots = sources
        .iter()
        .filter_map(|p| std::fs::canonicalize(p).ok())
        .collect::<Vec<_>>();
    let mut sources = sources.into_iter().map(|p| (p, true)).collect::<Vec<_>>();
    let start = Instant::now();
    let mut count = 0;
    let mut stamps = Vec::new();
    while let Some((mut path, optional)) = sources.pop() {
        if count > 100_000 || start.elapsed() > Duration::from_secs(5) {
            return Err(error(503, "leitura das fontes de conversas incompleta"));
        }
        let meta = match std::fs::symlink_metadata(&path) {
            Ok(m) => m,
            Err(e) if optional && e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(_) => return Err(error(503, "fonte de conversas ilegível")),
        };
        let meta = if meta.is_symlink() && optional {
            path = std::fs::canonicalize(&path)
                .map_err(|_| error(503, "fonte de conversas indireta ilegível"))?;
            std::fs::metadata(&path)
                .map_err(|_| error(503, "fonte de conversas indireta ilegível"))?
        } else {
            meta
        };
        if meta.is_symlink() {
            // Link quebrado não tem conversa; `memory` compartilhada só guarda notas e é lida na
            // fonte dela. Outro link pode levar conversa de uma worktree para a chave de outra.
            let memory = path.file_name().is_some_and(|name| name == "memory");
            match std::fs::canonicalize(&path) {
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
                Ok(target) if memory && roots.iter().any(|root| target.starts_with(root)) => continue,
                _ => return Err(error(503, "fonte de conversas indireta sem leitura segura")),
            }
        }
        let modified = meta
            .modified()
            .map_err(|_| error(503, "data da fonte de conversas ilegível"))?
            .duration_since(UNIX_EPOCH)
            .map_err(|_| error(503, "data da fonte de conversas inválida"))?
            .as_secs();
        stamps.push((path.clone(), modified));
        count += 1;
        if meta.is_dir() {
            for entry in
                std::fs::read_dir(&path).map_err(|_| error(503, "fonte de conversas ilegível"))?
            {
                sources.push((
                    entry
                        .map_err(|_| error(503, "fonte de conversas ilegível"))?
                        .path(),
                    false,
                ));
            }
        } else {
            std::fs::File::open(&path).map_err(|_| error(503, "conversa ilegível"))?;
        }
    }
    Ok(stamps)
}

/// A prova chama uma rodada diretamente, com `AppState` e diretórios isolados.
pub async fn cleanup_once(st: Arc<AppState>) -> Result<Vec<String>> {
    let home = st.origins_home.clone();
    let loaded = home.clone();
    let settings = tokio::task::spawn_blocking(move || {
        let s = WorktreeSettings::load_at(&loaded.join(".hangar/worktree-settings.json"))?;
        s.location_path(&loaded)?;
        Ok::<_, hangar_workspace::WorkspaceError>(s)
    })
    .await
    .map_err(|_| error(503, "configuração de worktrees indisponível"))??;
    if !settings.enabled() {
        return Ok(Vec::new());
    }
    let ctx = worktree_routes::fresh_context(&st).await?;
    let reader = st.clone();
    let scan_ctx = ctx.clone();
    let settings_scan = settings.clone();
    let scan_home = home.clone();
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| error(503, "relógio indisponível"))?
        .as_secs();
    let candidates = tokio::task::spawn_blocking(move || {
        let source_health = readable_sources(&reader, &scan_ctx);
        if source_health.is_err() {
            reader.diag.report(
                "rust.worktrees_inactivity_skipped",
                "",
                "worktree_activity_uncertain",
                "fontes de conversas ilegíveis ou incompletas; inatividade conservada",
            );
        }
        let mut cwds = scan_ctx.cwds.clone();
        cwds.extend(
            scan_ctx
                .activities
                .as_ref()
                .into_iter()
                .flatten()
                .map(|a| a.cwd.clone()),
        );
        let repos = hangar_workspace::worktrees::list_all(
            &cwds,
            &scan_ctx.sessions,
            Some(&scan_ctx.roots),
            None,
            false,
            &scan_ctx.project_bases,
        );
        let mut candidates = Vec::<Candidate>::new();
        for repo in repos.as_array().into_iter().flatten() {
            // A descoberta parcial pode omitir outros repos; a segurança deste é individual.
            for wt in repo["worktrees"].as_array().into_iter().flatten() {
                let Some(path) = wt["path"].as_str() else {
                    continue;
                };
                let activity = source_health
                    .as_ref()
                    .ok()
                    .and_then(|sources| activity(&scan_ctx, path, sources));
                match worktree_cleanup::candidate(
                    path,
                    &scan_ctx.sessions,
                    &scan_ctx.project_bases,
                    &scan_home,
                    &settings_scan,
                    now,
                    activity,
                ) {
                    Ok(Some(candidate)) => candidates.push(candidate),
                    Ok(None) => {}
                    Err(_) => reader.diag.report(
                        "rust.worktrees_cleanup_skipped",
                        "",
                        "worktree_read_uncertain",
                        "worktree protegida por leitura incerta",
                    ),
                }
            }
        }
        candidates
    })
    .await
    .map_err(|_| error(503, "leitura de worktrees indisponível"))?;
    let mut removed = Vec::new();
    for candidate in candidates {
        let ctx = worktree_routes::fresh_context(&st).await?;
        let home = home.clone();
        let reader = st.clone();
        let path = candidate.path.clone();
        let runtime = tokio::runtime::Handle::current();
        let done = tokio::task::spawn_blocking(move || {
            let sources = readable_sources(&reader, &ctx);
            let activity = sources
                .as_ref()
                .ok()
                .and_then(|sources| activity(&ctx, &candidate.path, sources));
            let guard = |path: &str| {
                let fresh = worktree_routes::final_context(&reader, &runtime, path)?;
                let fresh_sources = readable_sources(&reader, &fresh);
                let latest = fresh_sources
                    .as_ref()
                    .ok()
                    .and_then(|sources| self::activity(&fresh, path, sources));
                if activity.is_some() && latest.is_none_or(|time| time > activity.unwrap()) {
                    return Err(error(409, "atividade de conversa mudou durante a limpeza"));
                }
                Ok(())
            };
            worktree_cleanup::remove_candidate_with_guard(
                &candidate,
                &ctx.sessions,
                &ctx.project_bases,
                &home,
                now,
                activity,
                &guard,
            )
        })
        .await
        .map_err(|_| error(503, "remoção de worktrees indisponível"))?;
        match done {
            Ok(true) => removed.push(path),
            Ok(false) => {}
            Err(_) => st.diag.report(
                "rust.worktrees_cleanup_skipped",
                "",
                "worktree_changed",
                "worktree conservada na revalidação",
            ),
        }
    }
    if !removed.is_empty() {
        st.list.invalidate();
    }
    Ok(removed)
}
