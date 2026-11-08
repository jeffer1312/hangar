//! Fatos brutos da mesma descoberta da lista; a factory não retém estado ao vivo.
use super::*;
use crate::list::{bridge::ListBridge, procs::ProcessView};
use crate::session_open::{
    self as open,
    terminal::{Mux as _, ProcessMux},
};
fn provider(value: &str) -> Option<hangar_transcript::Provider> {
    match value {
        "pi" => Some(hangar_transcript::Provider::Pi),
        "omp" => Some(hangar_transcript::Provider::Omp),
        "kimi" => Some(hangar_transcript::Provider::Kimi),
        _ => None,
    }
}
fn key_valid(key: &str) -> bool {
    key.len() == 32 && key.bytes().all(|byte| byte.is_ascii_hexdigit())
}
fn spec_by_name(home: &std::path::Path, name: &str) -> Option<open::SessionSpec> {
    std::fs::read_dir(open::record::dir(home))
        .ok()?
        .flatten()
        .filter_map(|entry| {
            let value: Value =
                serde_json::from_slice(&super::super::queue::read_replaced(&entry.path()).ok()?)
                    .ok()?;
            let spec: open::SessionSpec = serde_json::from_value(value).ok()?;
            (spec.name == name).then_some(spec)
        })
        .max_by(|a, b| a.created_at.total_cmp(&b.created_at))
}
async fn env_key(program: &str, name: &str) -> Result<Option<String>, RuntimeError> {
    let mux = ProcessMux::default();
    let target = super::super::claude_terminal::session_target(name);
    let output = mux
        .run(vec![
            program.into(),
            "show-environment".into(),
            "-t".into(),
            target,
            "CP_SESSION_KEY".into(),
        ])
        .await;
    if output.code == Some(0) {
        return output
            .stdout
            .lines()
            .find_map(|line| line.strip_prefix("CP_SESSION_KEY="))
            .map(|key| {
                if key_valid(key) {
                    Ok(Some(key.into()))
                } else {
                    Err(failed("other_terminal_key_invalid"))
                }
            })
            .unwrap_or(Ok(None));
    }
    if output.code.is_some() && output.stderr.contains("unknown variable: CP_SESSION_KEY") {
        return Ok(None);
    }
    Err(failed("other_terminal_key_unavailable"))
}
pub fn discovery(list: Arc<ListBridge>) -> OtherDiscovery {
    let find = Arc::downgrade(&list);
    let names = Arc::downgrade(&list);
    OtherDiscovery {
        find: Arc::new(move |name| {
            let weak = find.clone();
            Box::pin(async move {
                let list = weak.upgrade()?;
                let home = list.env().dirs.as_ref()?.home.clone();
                let context = match list.runtime_context(&name).await {
                    Ok(context) => context,
                    Err(error) => {
                        let owned = name.clone();
                        let known =
                            tokio::task::spawn_blocking(move || spec_by_name(&home, &owned))
                                .await
                                .ok()
                                .flatten();
                        return known
                            .filter(|spec| provider(spec.provider.name()).is_some())
                            .map(|_| Err(failed(error.code)));
                    }
                };
                let kind = context
                    .row
                    .as_ref()
                    .and_then(|row| provider(&row.provider))
                    .or_else(|| {
                        context
                            .panes
                            .iter()
                            .find_map(|pane| pane.provider.as_deref().and_then(provider))
                    });
                let Some(kind) = kind else {
                    let known = tokio::task::spawn_blocking(move || spec_by_name(&home, &name))
                        .await
                        .ok()
                        .flatten();
                    return known
                        .filter(|spec| provider(spec.provider.name()).is_some())
                        .map(|_| Err(failed("other_terminal_dead")));
                };
                Some(
                    resolve(
                        name,
                        kind,
                        context,
                        list.env().procs.clone(),
                        list.env().capture_program.to_string_lossy().into_owned(),
                    )
                    .await,
                )
            })
        }),
        names: Arc::new(move || {
            let weak = names.clone();
            Box::pin(async move {
                let list = weak
                    .upgrade()
                    .ok_or_else(|| failed("other_terminal_discovery_unavailable"))?;
                let context = list
                    .runtime_context("")
                    .await
                    .map_err(|error| failed(error.code))?;
                let rows = list
                    .discover(None)
                    .await
                    .map_err(|error| failed(error.code))?;
                let mut names = rows
                    .iter()
                    .filter(|row| provider(&row.provider).is_some())
                    .map(|row| row.name.clone())
                    .collect::<Vec<_>>();
                names.extend(
                    context
                        .panes
                        .iter()
                        .filter(|pane| pane.provider.as_deref().and_then(provider).is_some())
                        .map(|pane| pane.session.clone()),
                );
                names.sort();
                names.dedup();
                Ok(names)
            })
        }),
    }
}
async fn resolve(
    name: String,
    kind: hangar_transcript::Provider,
    context: crate::list::bridge::RuntimeContext,
    processes: Arc<dyn ProcessView>,
    program: String,
) -> Result<OtherDiscoveryState, RuntimeError> {
    let pid = context.agent_pid.map(i64::from);
    let panes = context.panes.clone();
    let children = context.children.clone();
    let selected = match pid {
        Some(pid) => panes
            .iter()
            .find(|pane| {
                pane.pid.is_some_and(|shell| {
                    crate::list::discover::descendants(i64::from(shell), &children).contains(&pid)
                })
            })
            .cloned(),
        None => {
            let mut candidates = panes
                .iter()
                .filter(|pane| !pane.dead && pane.provider.as_deref() == Some(kind.as_str()));
            match (candidates.next(), candidates.next()) {
                (Some(pane), None) => Some(pane.clone()),
                _ => None,
            }
        }
    }
    .ok_or_else(|| failed("other_terminal_pane_unproved"))?;
    if selected.dead {
        return Err(failed("other_terminal_dead"));
    }
    let created = selected
        .session_created
        .ok_or_else(|| failed("other_terminal_pane_unproved"))?;
    let address = selected
        .target()
        .ok_or_else(|| failed("other_terminal_pane_unproved"))?;
    let key = env_key(&program, &name).await?;
    let (home, read_key) = (context.dirs.home.clone(), key.clone());
    let recorded = tokio::task::spawn_blocking(move || match read_key {
        Some(key) => open::record::read(&home, &key),
        None => Ok(None),
    })
    .await
    .map_err(|_| failed("other_terminal_spec_worker"))?
    .map_err(|_| failed("other_terminal_spec_invalid"))?;
    if recorded.as_ref().is_some_and(|spec| {
        spec.name != name
            || spec.provider.name() != kind.as_str()
            || spec.surface != open::Surface::Terminal
    }) {
        return Err(failed("other_terminal_spec_changed"));
    }
    let (dirs, pane, cwd, reader) = (
        context.dirs.clone(),
        selected.clone(),
        context
            .row
            .as_ref()
            .and_then(|row| row.cwd.clone())
            .unwrap_or(selected.cwd.clone()),
        processes.clone(),
    );
    let (agent, cwd, source) = tokio::task::spawn_blocking(move || -> Result<_, RuntimeError> {
        let agent = match pid {
            Some(pid) => {
                let argv = reader
                    .argv_checked(pid)
                    .map_err(|_| failed("other_terminal_process_unreadable"))?;
                if crate::list::discover::provider_from_argv(&argv) != Some(kind.as_str()) {
                    return Err(failed("other_terminal_process_changed"));
                }
                Some(OtherProcess {
                    pid,
                    birth: reader
                        .start_time(pid)
                        .filter(|birth| birth.is_finite() && *birth > 0.0)
                        .ok_or_else(|| failed("other_terminal_process_unreadable"))?,
                })
            }
            None => None,
        };
        let cwd = agent
            .as_ref()
            .and_then(|agent| reader.cwd(agent.pid))
            .map(|cwd| cwd.to_string_lossy().into_owned())
            .unwrap_or(cwd);
        let source = crate::list::discover_other::ticket_transcript(
            kind.as_str(),
            &pane.pane_id,
            pane.pid.map(i64::from),
            &cwd,
            &*reader,
            &dirs,
            &mut Default::default(),
        );
        Ok((agent, cwd, source))
    })
    .await
    .map_err(|_| failed("other_terminal_process_worker"))??;
    let key = match key {
        Some(key) => key,
        None if agent.is_some() => crate::mods::state::random_hex(16),
        None => return Err(failed("other_terminal_opening_unproved")),
    };
    let spec = match recorded {
        Some(spec) => spec,
        None if agent.is_some() => {
            let mut value:open::SessionSpec=serde_json::from_value(json!({"version":open::SPEC_VERSION,"key":key,"name":name,"cwd":cwd,"provider":kind.as_str(),"surface":"terminal","account":{"kind":"default"},"account_source":"default","jev":false,"read_only":false,"created_at":created as f64})).map_err(|_|failed("other_terminal_spec_invalid"))?;
            value.conversation = source.as_deref().map(crate::locate::session_key);
            let (home, saved) = (context.dirs.home.clone(), value.clone());
            tokio::task::spawn_blocking(move || open::record::write(&home, &saved))
                .await
                .map_err(|_| failed("other_terminal_spec_worker"))?
                .map_err(|_| failed("other_terminal_spec_unwritable"))?;
            let output = ProcessMux::default()
                .run(vec![
                    program.clone(),
                    "set-environment".into(),
                    "-t".into(),
                    super::super::claude_terminal::session_target(&name),
                    "CP_SESSION_KEY".into(),
                    key.clone(),
                ])
                .await;
            if output.code != Some(0) {
                return Err(failed("other_terminal_key_unwritable"));
            }
            value
        }
        None => return Err(failed("other_terminal_opening_unproved")),
    };
    let projection = context.dirs.claude.join(".hangar-queue");
    let owned_projection = projection.clone();
    let owned_key = key.clone();
    let (mut generation, state_path, lease_path) =
        tokio::task::spawn_blocking(move || queue_identity(&owned_projection, &owned_key))
            .await
            .map_err(|_| failed("queue_worker"))??;
    let conversation = source
        .as_deref()
        .map(crate::locate::session_key)
        .unwrap_or_default();
    let mut binding = TerminalBinding {
        name: name.clone(),
        pane: selected.pane_id.clone(),
        conversation,
        generation,
        created,
        mux_argv: vec![program],
        windows: cfg!(windows),
        clipboard_lock_path: Some(projection.join("runtime/clipboard.lock")),
    };
    if let (Some(agent), Some(source)) = (agent.clone(), source) {
        let mut target = OtherTarget {
            key,
            generation,
            name,
            provider: kind,
            binding,
            pane_address: address,
            dirs: context.dirs,
            cwd,
            pane_pid: selected
                .pid
                .map(i64::from)
                .ok_or_else(|| failed("other_terminal_pane_unproved"))?,
            agent_pid: agent.pid,
            agent_birth: agent.birth,
            transcript: source.into(),
            state_path,
            lease_path,
            projection_dir: projection,
            config_dirs: (*context.config_dirs).clone(),
            created: agent.birth.max(created as f64),
        };
        let check = target.clone();
        generation =
            tokio::task::spawn_blocking(move || super::storage::desired_generation(&check))
                .await
                .map_err(|_| failed("queue_worker"))??;
        target.generation = generation;
        target.binding.generation = generation;
        Ok(OtherDiscoveryState::Ready(target))
    } else {
        binding.conversation.clear();
        Ok(OtherDiscoveryState::Opening(OtherOpening {
            spec,
            binding,
            pane_address: address,
            projection_dir: projection,
            dirs: context.dirs,
            config_dirs: (*context.config_dirs).clone(),
            pane_pid: selected.pid.map(i64::from),
            agent,
        }))
    }
}

impl OtherDiscoveryState {
    pub fn key(&self) -> &str {
        match self {
            Self::Ready(target) => &target.key,
            Self::Opening(opening) => &opening.spec.key,
        }
    }
    pub fn generation(&self) -> u64 {
        self.binding().generation
    }
    pub fn name(&self) -> &str {
        &self.binding().name
    }
    pub fn provider(&self) -> &str {
        match self {
            Self::Ready(target) => target.provider.as_str(),
            Self::Opening(opening) => opening.spec.provider.name(),
        }
    }
    pub fn binding(&self) -> &TerminalBinding {
        match self {
            Self::Ready(target) => &target.binding,
            Self::Opening(opening) => &opening.binding,
        }
    }
    pub fn dirs(&self) -> &crate::list::discover_other::Dirs {
        match self {
            Self::Ready(target) => &target.dirs,
            Self::Opening(opening) => &opening.dirs,
        }
    }
    pub fn transcript(&self) -> Option<&std::path::Path> {
        match self {
            Self::Ready(target) => Some(&target.transcript),
            Self::Opening(_) => None,
        }
    }
    pub fn agent(&self) -> Option<OtherProcess> {
        match self {
            Self::Ready(target) => Some(OtherProcess {
                pid: target.agent_pid,
                birth: target.agent_birth,
            }),
            Self::Opening(opening) => opening.agent.clone(),
        }
    }
    pub fn lifecycle_id(&self) -> String {
        lifecycle_id(self.key(), self.generation())
    }
    pub fn projection_dir(&self) -> &std::path::Path {
        match self {
            Self::Ready(target) => &target.projection_dir,
            Self::Opening(opening) => &opening.projection_dir,
        }
    }
}

/// A origem antiga só é reutilizada quando o bilhete desta vida e o processo a distinguem.
pub(in crate::runtime) fn legacy_line(
    target: &OtherTarget,
    processes: &dyn ProcessView,
) -> Result<Option<String>, RuntimeError> {
    if !matches!(
        target.provider,
        hangar_transcript::Provider::Pi | hangar_transcript::Provider::Omp
    ) {
        return Ok(None);
    }
    let config = processes
        .env_var(target.agent_pid, "CLAUDE_CONFIG_DIR")
        .ok()
        .flatten()
        .map(PathBuf::from)
        .unwrap_or_else(|| target.dirs.default_claude());
    let stable = processes
        .env_var(target.agent_pid, "CP_SESSION_KEY")
        .ok()
        .flatten()
        .and_then(|value| value.into_string().ok());
    let fresh = |path: &std::path::Path, birth: f64| -> bool {
        super::super::queue::read_replaced(path)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
            .is_some_and(|value| {
                value["ts"].as_f64().is_some_and(|ts| ts >= birth - 2.0)
                    && value["file"].as_str().is_some_and(|file| !file.is_empty())
            })
    };
    if stable.as_ref().is_some_and(|key| {
        fresh(
            &config.join(".hangar-pi").join(format!("{key}.json")),
            target.agent_birth,
        )
    }) {
        return Ok(None);
    }
    let psmux = processes
        .env_var(target.agent_pid, "PSMUX_SESSION")
        .map_err(|_| failed("other_terminal_process_unreadable"))?
        .and_then(|value| value.into_string().ok());
    if let Some(psmux) = &psmux {
        let children = processes
            .children(Duration::ZERO)
            .map_err(|_| failed("other_terminal_process_unreadable"))?;
        let mut pids = children
            .values()
            .flatten()
            .copied()
            .collect::<std::collections::HashSet<_>>();
        pids.insert(target.agent_pid);
        let mut candidates = 0;
        for pid in pids {
            let argv = processes.argv(pid);
            if crate::list::discover::is_aux(&argv)
                || !matches!(
                    crate::list::discover::provider_from_argv(&argv),
                    Some("pi" | "omp")
                )
            {
                continue;
            }
            if processes
                .env_var(pid, "PSMUX_SESSION")
                .ok()
                .flatten()
                .is_none_or(|value| value.to_string_lossy() != *psmux)
            {
                continue;
            }
            let dir = processes
                .env_var(pid, "CLAUDE_CONFIG_DIR")
                .ok()
                .flatten()
                .map(PathBuf::from)
                .unwrap_or_else(|| target.dirs.default_claude());
            let modern = processes
                .env_var(pid, "CP_SESSION_KEY")
                .ok()
                .flatten()
                .and_then(|key| key.into_string().ok())
                .is_some_and(|key| {
                    processes.start_time(pid).is_some_and(|birth| {
                        fresh(&dir.join(".hangar-pi").join(format!("{key}.json")), birth)
                    })
                });
            if !modern {
                candidates += 1;
            }
        }
        if candidates > 1 {
            return Err(failed("other_terminal_inbox_ambiguous"));
        }
    }
    let key = psmux.unwrap_or_else(|| target.binding.pane.clone());
    let ticket = config
        .join(".hangar-pi")
        .join(format!("{}.json", key.trim_start_matches('%')));
    let value = super::super::queue::read_replaced(&ticket)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok());
    if value.as_ref().is_some_and(|value| {
        value["file"]
            .as_str()
            .is_some_and(|file| std::path::Path::new(file) == target.transcript)
            && value["ts"]
                .as_f64()
                .is_some_and(|ts| ts >= target.agent_birth - 2.0)
    }) {
        Ok(Some(key))
    } else {
        Ok(None)
    }
}
