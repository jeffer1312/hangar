//! Posse das sessões Pi/omp/Kimi e reconciliação sem relançar o CLI.
use super::super::other_terminal::*;
use super::*;
use crate::session_open::terminal::{Mux as _, ProcessMux};
pub(super) struct OtherConfiguration {
    pub options: OtherOptions,
    pub discovery: OtherDiscovery,
    pub opening: Mutex<BTreeMap<String, OtherOpening>>,
    wake: Arc<tokio::sync::Notify>,
    watch: std::sync::Mutex<Option<crate::AbortOnDrop>>,
}
impl RuntimeRegistry {
    pub fn bind_other(
        self: &Arc<Self>,
        mut options: OtherOptions,
        discovery: OtherDiscovery,
    ) -> Result<(), RuntimeError> {
        if options.tick.is_zero() {
            return Err(failure("other_terminal_tick"));
        }
        self.bind_lifecycle();
        options.registry = Some(Arc::downgrade(self));
        self.other
            .set(OtherConfiguration {
                options,
                discovery,
                opening: Mutex::new(BTreeMap::new()),
                wake: Arc::new(tokio::sync::Notify::new()),
                watch: Default::default(),
            })
            .map_err(|_| failure("other_runtime_already_bound"))?;
        let weak = Arc::downgrade(self);
        let ready = self.startup.clone();
        let source = self.other.get().unwrap().discovery.clone();
        let wake = self.other.get().unwrap().wake.clone();
        let task = tokio::spawn(async move {
            ready.wait().await;
            if let Ok(names) = (source.names)().await {
                for name in names {
                    if let Some(registry) = weak.upgrade() {
                        let _ = registry.adopt_other(&name).await;
                    } else {
                        return;
                    }
                }
            }
            loop {
                let names = match weak.upgrade() {
                    Some(registry) => registry
                        .other
                        .get()
                        .unwrap()
                        .opening
                        .lock()
                        .await
                        .keys()
                        .cloned()
                        .collect::<Vec<_>>(),
                    None => return,
                };
                if names.is_empty() {
                    wake.notified().await;
                    continue;
                }
                tokio::select! {_=tokio::time::sleep(Duration::from_millis(500))=>{},_=wake.notified()=>{}}
                for name in names {
                    if let Some(registry) = weak.upgrade() {
                        let _ = registry.adopt_other(&name).await;
                    } else {
                        return;
                    }
                }
            }
        });
        *self
            .other
            .get()
            .unwrap()
            .watch
            .lock()
            .unwrap_or_else(|error| error.into_inner()) = Some(crate::AbortOnDrop(task));
        Ok(())
    }
    pub async fn open_other_terminal(&self, target: OtherTarget) -> Result<Value, RuntimeError> {
        if !matches!(
            target.provider,
            hangar_transcript::Provider::Pi
                | hangar_transcript::Provider::Omp
                | hangar_transcript::Provider::Kimi
        ) || target.generation == 0
            || target.binding.generation != target.generation
            || target.binding.name != target.name
            || target.binding.conversation.is_empty()
            || target.agent_pid <= 0
            || target.pane_pid <= 0
            || !target.agent_birth.is_finite()
            || target.agent_birth <= 0.0
        {
            return Err(failure("runtime_binding"));
        }
        let _name = crate::session_open::open::NameClaim::take(&target.dirs.home, &target.name)
            .map_err(|_| failure("other_terminal_adoption_busy"))?;
        let barrier = self.barrier(&target.key).await;
        let _guard = barrier.lock().await;
        self.open_other_terminal_locked(target).await
    }
    pub(super) async fn open_other_terminal_locked(
        &self,
        target: OtherTarget,
    ) -> Result<Value, RuntimeError> {
        let configuration = self
            .other
            .get()
            .ok_or_else(|| failure("other_runtime_unavailable"))?;
        let io = configuration
            .options
            .io_override
            .clone()
            .unwrap_or_else(|| Arc::new(ProcessOtherIo::new(&target, &configuration.options)));
        if !io.prove(&target).await? {
            return Err(failure("other_terminal_life_changed"));
        }
        let legacy = if configuration.options.io_override.is_some() {
            Some(target.name.clone())
        } else {
            let owned = target.clone();
            let reader = configuration.options.processes.clone();
            tokio::task::spawn_blocking(move || {
                super::super::other_terminal::targets::legacy_line(&owned, &*reader)
            })
            .await
            .map_err(|_| failure("other_terminal_process_worker"))??
        };
        configuration
            .options
            .inbox
            .bind_session(&target.name, &target.key, legacy.as_deref());
        let existing = {
            let entries = self.entries.lock().await;
            if entries
                .iter()
                .any(|(key, entry)| entry.name == target.name && *key != target.key)
            {
                return Err(failure("runtime_binding"));
            }
            entries
                .get(&target.key)
                .map(|entry| (entry.generation, entry.handle.clone()))
        };
        let handle = match existing {
            Some((
                generation,
                EntryHandle::OtherTerminal {
                    target: old,
                    handle,
                },
            )) if generation == target.generation
                && old.same_life(&target)
                && old.transcript == target.transcript =>
            {
                handle
            }
            Some(_) => return Err(failure("runtime_generation")),
            None => {
                configuration
                    .options
                    .interactions
                    .bind(&crate::interactions::InteractionBinding {
                        key: target.key.clone(),
                        generation: target.generation,
                        conversation: target.binding.conversation.clone(),
                    })
                    .map_err(crate::interactions::runtime_error)?;
                let queue = super::super::other_terminal::open_queue(&target).await?;
                let live = self.live_sender(&target.name);
                let handle = OtherHandle::spawn(
                    target.clone(),
                    queue,
                    configuration.options.clone(),
                    live.clone(),
                );
                self.entries.lock().await.insert(
                    target.key.clone(),
                    Entry {
                        generation: target.generation,
                        handle: EntryHandle::OtherTerminal {
                            target: Box::new(target.clone()),
                            handle: handle.clone(),
                        },
                        lease_path: target.lease_path.clone(),
                        name: target.name.clone(),
                        mods_life: 0,
                        provider: target.provider.as_str().into(),
                        account_key: None,
                        relaunch: None,
                        settings_owner: None,
                        mode_source: crate::terminal_facts::ModeSource {
                            binding: crate::interactions::InteractionBinding {
                                key: target.key.clone(),
                                generation: target.generation,
                                conversation: target.binding.conversation.clone(),
                            },
                            transcript: target.transcript.clone(),
                            confirmed: None,
                        },
                        live,
                        runtime_target: None,
                        cano_birth: None,
                        auto_respawn: None,
                        respawn_inflight: false,
                    },
                );
                handle
            }
        };
        let snapshot = handle.snapshot().await?;
        if let Some(code) = snapshot["error"].as_str().filter(|code| !code.is_empty()) {
            return Err(failure(code));
        }
        Ok(
            json!({"opened":true,"instance":self.instance,"key":target.key,"generation":target.generation,"state":snapshot}),
        )
    }
    async fn retire_claude_before_other(&self, target: &OtherTarget) -> Result<(), RuntimeError> {
        let stale = self
            .entries
            .lock()
            .await
            .iter()
            .filter_map(|(key, entry)| match &entry.handle {
                EntryHandle::Terminal { target: old, .. }
                    if entry.name == target.name
                        && (key != &target.key
                            || old.binding.created != target.binding.created) =>
                {
                    Some((key.clone(), entry.generation, old.binding.clone()))
                }
                _ => None,
            })
            .collect::<Vec<_>>();
        if stale.is_empty() {
            return Ok(());
        }
        let _name = crate::session_open::open::NameClaim::take(&target.dirs.home, &target.name)
            .map_err(|_| failure("other_terminal_adoption_busy"))?;
        let config = self
            .other
            .get()
            .ok_or_else(|| failure("other_runtime_unavailable"))?;
        let io = config
            .options
            .io_override
            .clone()
            .unwrap_or_else(|| Arc::new(ProcessOtherIo::new(target, &config.options)));
        if !io.prove(target).await? {
            return Err(failure("other_terminal_life_changed"));
        }
        let shown = mux(
            &target.binding,
            vec![
                "show-environment".into(),
                "-t".into(),
                super::super::claude_terminal::session_target(&target.name),
                "CP_SESSION_KEY".into(),
            ],
        )
        .await;
        if shown.code != Some(0)
            || super::super::claude_terminal::session_key(&shown.stdout).as_deref()
                != Some(target.key.as_str())
        {
            return Err(failure("other_terminal_life_changed"));
        }
        for (key, generation, binding) in stale {
            let barrier = self.barrier(&key).await;
            let _guard = barrier.lock().await;
            let unchanged=self.entries.lock().await.get(&key).is_some_and(|entry|entry.generation==generation && entry.name==target.name && matches!(&entry.handle,EntryHandle::Terminal {target:old,..} if old.binding==binding));
            if unchanged {
                self.close_locked(&key, generation, false).await?;
            }
        }
        Ok(())
    }

    pub(crate) async fn ensure_other(&self, name: &str) -> Option<Result<(), RuntimeError>> {
        self.adopt_other(name).await
    }
    pub(super) async fn adopt_other(&self, name: &str) -> Option<Result<(), RuntimeError>> {
        let config = self.other.get()?;
        let found = (config.discovery.find)(name.into()).await?;
        let _pass = match self.ingress.enter(name, Duration::ZERO).await {
            Ok(pass) => pass,
            Err(_) => return Some(Err(failure("session_transfer_busy"))),
        };
        Some(match found {
            Err(error) => Err(error),
            Ok(OtherDiscoveryState::Ready(target)) if target.name == name => {
                if let Err(error) = self.retire_claude_before_other(&target).await {
                    return Some(Err(error));
                }
                let existing = self
                    .entries
                    .lock()
                    .await
                    .values()
                    .find(|entry| entry.name == name)
                    .and_then(|entry| match &entry.handle {
                        EntryHandle::OtherTerminal { target, .. } => Some((**target).clone()),
                        _ => None,
                    });
                let result = if existing.as_ref().is_some_and(|old| {
                    !old.same_life(&target) || old.transcript != target.transcript
                }) {
                    self.reconcile_other_entered(name, Some(OtherDiscoveryState::Ready(target)))
                        .await
                } else {
                    self.open_other_terminal(target).await.map(|_| ())
                };
                if result.is_ok() {
                    config.opening.lock().await.remove(name);
                }
                result
            }
            Ok(OtherDiscoveryState::Opening(opening))
                if opening.spec.name == name
                    && opening.binding.name == name
                    && opening.spec.surface == crate::session_open::Surface::Terminal =>
            {
                config.opening.lock().await.insert(name.into(), opening);
                config.wake.notify_one();
                Err(failure("other_terminal_starting"))
            }
            _ => Err(failure("runtime_binding")),
        })
    }
    pub async fn other_info(&self, name: &str) -> Option<crate::transcript::InternalInfo> {
        let target = {
            let entries = self.entries.lock().await;
            let mut found = entries.values().filter(|entry| entry.name == name);
            match (found.next(), found.next()) {
                (
                    Some(Entry {
                        handle: EntryHandle::OtherTerminal { target, .. },
                        ..
                    }),
                    None,
                ) => Some(target.clone()),
                (None, None) => None,
                _ => return None,
            }
        };
        let life = match target {
            Some(target) => OtherDiscoveryState::Ready(*target),
            None => OtherDiscoveryState::Opening(
                self.other.get()?.opening.lock().await.get(name)?.clone(),
            ),
        };
        Some(crate::transcript::InternalInfo {
            provider: life.provider().into(),
            jsonl: life.transcript().map(PathBuf::from),
            session_key: life.binding().conversation.clone(),
            history: json!({"queue":super::super::queue::projection_path(life.projection_dir(),name)}),
            headless: false,
            lifecycle_id: Some(life.lifecycle_id()),
            cwd: Some(match life {
                OtherDiscoveryState::Ready(target) => target.cwd.into(),
                OtherDiscoveryState::Opening(opening) => opening.spec.cwd.into(),
            }),
        })
    }
    pub async fn other_opening_key(&self, name: &str) -> Option<String> {
        Some(
            self.other
                .get()?
                .opening
                .lock()
                .await
                .get(name)?
                .spec
                .key
                .clone(),
        )
    }
    pub async fn queue_other_opening(
        &self,
        name: &str,
        key: &str,
        text: &str,
        id: &str,
    ) -> Result<Value, RuntimeError> {
        if !super::super::queue::queue_text_ok(text) {
            return Err(failure("invalid_text"));
        }
        let config = self
            .other
            .get()
            .ok_or_else(|| failure("other_runtime_unavailable"))?;
        let found = (config.discovery.find)(name.into())
            .await
            .ok_or_else(|| failure("other_terminal_life_changed"))??;
        if found.key() != key {
            return Err(failure("other_terminal_life_changed"));
        }
        let _name = crate::session_open::open::NameClaim::take(&found.dirs().home, name)
            .map_err(|_| failure("other_terminal_adoption_busy"))?;
        let barrier = self.barrier(key).await;
        let _guard = barrier.lock().await;
        match found {
            OtherDiscoveryState::Ready(target) => {
                self.open_other_terminal_locked(target).await?;
                let handle = self
                    .entries
                    .lock()
                    .await
                    .get(key)
                    .ok_or_else(|| failure("runtime_binding"))?
                    .handle
                    .clone();
                handle
                    .queue(
                        format!("opening:{id}"),
                        Action::Append {
                            text: text.into(),
                            delivered: false,
                            ts: None,
                            pre_transcript: true,
                            entry_id: Some(id.into()),
                        },
                    )
                    .await?;
            }
            OtherDiscoveryState::Opening(opening) => {
                let (store, lease) =
                    super::super::other_terminal::storage::open((&opening).into()).await?;
                let mut store = store;
                store
                    .exec(
                        opening.binding.generation,
                        &format!("opening:{id}"),
                        clock_sample(),
                        Action::Append {
                            text: text.into(),
                            delivered: false,
                            ts: None,
                            pre_transcript: true,
                            entry_id: Some(id.into()),
                        },
                    )
                    .map_err(|_| failure("queue_io"))?;
                let queue = QueueActor::start(store, lease);
                let sent = super::super::other_terminal::send_opening(&opening, &queue, &config.options).await;
                queue.shutdown().await.map_err(|_| failure("queue_io"))?;
                sent?;
                config.opening.lock().await.insert(name.into(), opening);
                config.wake.notify_one();
            }
        }
        Ok(json!({"ok":true,"delivered":false,"steered":false,"native":false,"queue_id":id}))
    }
    pub async fn reconcile_other(&self, name: &str) -> Option<Result<(), RuntimeError>> {
        let config = self.other.get()?;
        let found = (config.discovery.find)(name.into()).await;
        if self.entries.lock().await.values().all(|entry| {
            entry.name != name || !matches!(entry.handle, EntryHandle::OtherTerminal { .. })
        }) {
            return found.map(|_| Err(failure("runtime_binding")));
        }
        let found = match found {
            Some(Ok(found)) => Some(found),
            Some(Err(error)) if error.code != "other_terminal_dead" => return Some(Err(error)),
            _ => None,
        };
        Some(self.reconcile_other_entered(name, found).await)
    }
    async fn reconcile_other_entered(
        &self,
        name: &str,
        next: Option<OtherDiscoveryState>,
    ) -> Result<(), RuntimeError> {
        let old = self
            .entries
            .lock()
            .await
            .values()
            .find_map(|entry| match &entry.handle {
                EntryHandle::OtherTerminal { target, .. } if entry.name == name => {
                    Some((**target).clone())
                }
                _ => None,
            })
            .ok_or_else(|| failure("runtime_binding"))?;
        if let Some(OtherDiscoveryState::Opening(opening)) = &next {
            if opening.agent.as_ref().is_some_and(|agent| {
                agent.pid == old.agent_pid
                    && crate::terminal_facts::same_birth(agent.birth, old.agent_birth)
            }) {
                return Ok(());
            }
        }
        if let Some(OtherDiscoveryState::Ready(target)) = &next {
            if old.same_life(target) && old.transcript == target.transcript {
                return Ok(());
            }
        }
        let _claim = crate::session_open::open::NameClaim::take(&old.dirs.home, name)
            .map_err(|_| failure("other_terminal_adoption_busy"))?;
        let barrier = self.barrier(&old.key).await;
        let _guard = barrier.lock().await;
        if next.is_none()
            && !self
                .other_life_ended(&OtherDiscoveryState::Ready(old.clone()))
                .await?
        {
            return Err(failure("other_terminal_life_uncertain"));
        }
        self.close_locked(&old.key, old.generation, false).await?;
        match next {
            Some(OtherDiscoveryState::Ready(mut target)) => {
                let check = target.clone();
                target.generation = tokio::task::spawn_blocking(move || {
                    super::super::other_terminal::storage::desired_generation(&check)
                })
                .await
                .map_err(|_| failure("queue_worker"))??;
                target.binding.generation = target.generation;
                self.open_other_terminal_locked(target).await?;
            }
            _ => {
                if let Some(config) = self.other.get() {
                    config.opening.lock().await.remove(name);
                    if let Some(after) = &config.options.after_closed {
                        after(OtherClosed {
                            life: OtherDiscoveryState::Ready(old),
                            reason: OtherEnd::Disappeared,
                        })
                        .await?;
                    }
                }
            }
        }
        Ok(())
    }
    pub async fn other_life_ended(&self, life: &OtherDiscoveryState) -> Result<bool, RuntimeError> {
        if !mux_present(life.binding()).await? {
            return Ok(true);
        }
        let current = (self
            .other
            .get()
            .ok_or_else(|| failure("other_runtime_unavailable"))?
            .discovery
            .find)(life.name().into())
        .await;
        match current {
            Some(Ok(current))
                if current.key() == life.key()
                    && current.binding().created == life.binding().created =>
            {
                match (life.agent(), current.agent()) {
                    (Some(old), Some(now)) => Ok(old.pid != now.pid
                        || !crate::terminal_facts::same_birth(old.birth, now.birth)),
                    (Some(old), None) => {
                        let procs = self.other.get().unwrap().options.processes.clone();
                        tokio::task::spawn_blocking(move || {
                            Ok(procs.start_time(old.pid).is_none_or(|birth| {
                                !crate::terminal_facts::same_birth(birth, old.birth)
                            }))
                        })
                        .await
                        .map_err(|_| failure("other_terminal_process_worker"))?
                    }
                    _ => Ok(false),
                }
            }
            Some(Err(error)) => Err(error),
            _ => Ok(false),
        }
    }
    pub async fn close_other(&self, name: &str) -> Option<Result<Value, RuntimeError>> {
        let config = self.other.get()?;
        let current = self
            .entries
            .lock()
            .await
            .values()
            .find_map(|entry| match &entry.handle {
                EntryHandle::OtherTerminal { target, .. } if entry.name == name => {
                    Some(OtherDiscoveryState::Ready((**target).clone()))
                }
                _ => None,
            });
        let life = match current.or_else(|| {
            config
                .opening
                .try_lock()
                .ok()
                .and_then(|map| map.get(name).cloned())
                .map(OtherDiscoveryState::Opening)
        }) {
            Some(life) => life,
            None => match (config.discovery.find)(name.into()).await? {
                Ok(life) => life,
                Err(error) => return Some(Err(error)),
            },
        };
        Some(self.close_other_life(life).await)
    }
    pub(super) async fn close_other_life(
        &self,
        life: OtherDiscoveryState,
    ) -> Result<Value, RuntimeError> {
        let _claim = crate::session_open::open::NameClaim::take(&life.dirs().home, life.name())
            .map_err(|_| failure("other_terminal_adoption_busy"))?;
        let _closed =
            super::super::lifecycle::ClosedIngress::acquire_exclusive(&self.ingress, life.name())
                .await?;
        let barrier = self.barrier(life.key()).await;
        let _guard = barrier.lock().await;
        self.close_locked(life.key(), life.generation(), false)
            .await?;
        let result = async {
            if mux_present(life.binding()).await? {
                let found = (self.other.get().unwrap().discovery.find)(life.name().into())
                    .await
                    .ok_or_else(|| failure("other_terminal_life_changed"))??;
                if found.key() != life.key()
                    || found.binding().created != life.binding().created
                    || found.agent().zip(life.agent()).is_some_and(|(new, old)| {
                        new.pid != old.pid
                            || !crate::terminal_facts::same_birth(new.birth, old.birth)
                    })
                {
                    return Err(failure("other_terminal_life_changed"));
                }
                let output = mux(
                    life.binding(),
                    vec![
                        "kill-session".into(),
                        "-t".into(),
                        super::super::claude_terminal::session_target(life.name()),
                    ],
                )
                .await;
                if output.code != Some(0) {
                    return Err(failure("other_terminal_kill_failed"));
                }
                let end = std::time::Instant::now() + Duration::from_secs(5);
                while mux_present(life.binding()).await? {
                    if std::time::Instant::now() >= end {
                        return Err(failure("other_terminal_kill_unconfirmed"));
                    }
                    tokio::time::sleep(Duration::from_millis(50)).await;
                }
            }
            if let Some(agent) = life.agent() {
                let processes = self.other.get().unwrap().options.processes.clone();
                if super::super::lifecycle::wait_absent(
                    &*processes,
                    agent.pid,
                    agent.birth,
                    "other_terminal_agent_still_alive",
                )
                .await
                .is_err()
                {
                    super::super::claude_reopen::stop_tree(vec![(agent.pid, agent.birth)]).await?;
                    super::super::lifecycle::wait_absent(
                        &*processes,
                        agent.pid,
                        agent.birth,
                        "other_terminal_agent_still_alive",
                    )
                    .await?;
                }
            }
            Ok::<_, RuntimeError>(())
        }
        .await;
        if let Err(error) = result {
            if let OtherDiscoveryState::Ready(target) = &life {
                let _ = self.open_other_terminal_locked(target.clone()).await;
            }
            return Err(error);
        }
        let config = self.other.get().unwrap();
        config.opening.lock().await.remove(life.name());
        let location = match &life {
            OtherDiscoveryState::Ready(target) => {
                super::super::other_terminal::storage::Location::from(target)
            }
            OtherDiscoveryState::Opening(opening) => {
                super::super::other_terminal::storage::Location::from(opening)
            }
        };
        let (mut store, lease) = super::super::other_terminal::storage::open(location).await?;
        store
            .exec(
                life.generation(),
                &format!("closed:{}:{}", life.key(), life.generation()),
                clock_sample(),
                Action::Clear,
            )
            .map_err(|_| failure("queue_io"))?;
        store.ensure_projection().map_err(|_| failure("queue_io"))?;
        drop(store);
        drop(lease);
        let warnings = match &config.options.after_closed {
            Some(after) => match after(OtherClosed {
                life: life.clone(),
                reason: OtherEnd::Requested,
            })
            .await
            {
                Ok(warnings) => warnings,
                Err(error) => vec![json!({"code":error.code})],
            },
            None => vec![],
        };
        Ok(json!({"closed":true,"killed":true,"warnings":warnings}))
    }
    pub(super) fn stop_other_watch(&self) {
        if let Some(config) = self.other.get() {
            config
                .watch
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .take();
        }
    }
}
fn clock_sample() -> ClockSample {
    ClockSample {
        monotonic_s: 0.0,
        epoch_s: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0.0, |time| time.as_secs_f64()),
    }
}
async fn mux(
    binding: &crate::terminal_input::TerminalBinding,
    args: Vec<String>,
) -> crate::session_open::terminal::MuxOutput {
    ProcessMux::default()
        .run(binding.mux_argv.iter().cloned().chain(args).collect())
        .await
}
async fn mux_present(
    binding: &crate::terminal_input::TerminalBinding,
) -> Result<bool, RuntimeError> {
    let output = mux(
        binding,
        vec![
            "has-session".into(),
            "-t".into(),
            super::super::claude_terminal::session_target(&binding.name),
        ],
    )
    .await;
    match output.code {
        Some(0) => Ok(true),
        Some(_) if crate::list::mux::is_absence(output.stderr.trim()) => Ok(false),
        _ => Err(failure("other_terminal_mux_unavailable")),
    }
}

impl RuntimeRegistry {
    pub async fn other_cleanup_allowed(
        &self,
        life: &OtherDiscoveryState,
    ) -> Result<bool, RuntimeError> {
        if !mux_present(life.binding()).await? {
            return Ok(true);
        }
        let current = (self
            .other
            .get()
            .ok_or_else(|| failure("other_runtime_unavailable"))?
            .discovery
            .find)(life.name().into())
        .await;
        match current {
            Some(Ok(OtherDiscoveryState::Opening(opening)))
                if opening.spec.key == life.key()
                    && opening.binding.created == life.binding().created
                    && opening.binding.generation == life.generation()
                    && opening.spec.provider.name() == life.provider()
                    && opening.agent.is_none() =>
            {
                self.other_life_ended(life).await
            }
            _ => Ok(false),
        }
    }
}
