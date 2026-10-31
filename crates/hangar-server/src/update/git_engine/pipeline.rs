//! Sequência do checkout e compensação antes de publicar sucesso.
use super::{
    Service,
    releases::{self, Source},
    tail,
};
use serde_json::{Value, json};
fn panic_cause(payload: &(dyn std::any::Any + Send)) -> String {
    payload.downcast_ref::<String>().cloned()
        .or_else(|| payload.downcast_ref::<&str>().map(|text| (*text).to_owned()))
        .unwrap_or_else(|| "a atualização foi interrompida por uma falha interna".into())
}
const STEPS: [(&str, &str); 6] = [
    ("resguardar", "Guardando o que estava aqui"),
    ("codigo", "Puxando o código novo"),
    ("passos", "Aplicando o que a versão pede"),
    ("instalar", "Instalando dependências"),
    ("reiniciar", "Reiniciando o servidor"),
    ("nativo", "Atualizando o app desta máquina"),
];
impl Service {
    pub fn shell_changed(&self, from: &str, to: &str) -> bool {
        if from.is_empty() || to.is_empty() || from == to || !self.root.join("shell/node_modules/electron").is_dir() { return false; }
        match self.git(&["diff", "--name-only", &format!("{from}..{to}"), "--", "shell/"], 30) {
            Ok(output) if output.code == 0 => output.stdout.lines().any(|line| !line.is_empty() && !line.ends_with(".test.cjs")),
            _ => true,
        }
    }
    fn step(&self, key: &str) -> Result<(),String> {
        let index = STEPS.iter().position(|(k, _)| *k == key).unwrap_or(0);
        self.state().try_write(json!({"fase":"rodando","etapa":key,"passo":index+1,"total":STEPS.len(),"texto":STEPS[index].1,"etapa_inicio":chrono::Local::now().format("%Y-%m-%dT%H:%M:%S%:z").to_string()}))
    }
    fn failed(&self, message: &str, port: u16, up: Option<bool>) -> super::super::StateSnapshot {
        if let Err(error)=self.state().try_write(json!({"fase":"pronto","ok":false,"erro":message,"voltou":false,"no_ar":up.unwrap_or_else(||self.up(port,5))})) {return super::super::StateSnapshot::failure(error);}
        self.state().read()
    }
    fn rollback(&self, commit: &str, message: &str, topology: &str, port: u16, branch: &str, source: &(dyn Source + Sync), platform: Option<&str>) -> super::super::StateSnapshot {
        if let Err(error)=self.state().try_write(json!({"fase":"rodando","texto":"Voltando para a versão anterior"})) {return super::super::StateSnapshot::failure(error);}
        let result = (|| -> Result<(), String> {
            if !branch.is_empty()
                && self
                    .git(&["rev-parse", "--abbrev-ref", "HEAD"], 30)?
                    .stdout
                    .trim()
                    != branch
            {
                let output = self.git(&["checkout", "-f", branch], 120)?;
                if output.code != 0 {
                    return Err(format!(
                        "{message}; e nao consegui voltar para a branch {branch}: {}",
                        tail(&output, 12)
                    ));
                }
                self.mark_channel(branch);
            }
            let output = self.git(&["reset", "--hard", commit], 120)?;
            if output.code != 0 {
                return Err(format!(
                    "{message}; e nao consegui voltar pra versao anterior: {}",
                    tail(&output, 12)
                ));
            }
            Ok(())
        })();
        if let Err(error) = result {
            return self.failed(&error, port, Some(false));
        }
        if let Some(platform) = platform {
            for warning in releases::restore(&self.home.join(".hangar/bin"), platform) {
                self.state().warn(&warning);
            }
        }
        if let Err(error) = self
            .restore_dist()
            .and_then(|_| self.prepare(topology, false, None, source, platform))
            .and_then(|_| self.restart_service(topology, port))
        {
            return self.failed(
                &format!("{message}; e a volta para a versao anterior tambem falhou: {error}"),
                port,
                None,
            );
        }
        if let Err(error)=self.state().try_write(json!({"fase":"pronto","ok":false,"erro":message,"voltou":true,"no_ar":self.up(port,30)})) {return super::super::StateSnapshot::failure(error);}
        self.state().read()
    }
    pub fn execute(
        &self,
        port: u16,
        source: &(dyn Source + Sync),
        platform: Option<&str>,
    ) -> super::super::StateSnapshot {
        let mut engine = self.clone();
        engine.writing = true;
        let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            engine.execute_inner(port, source, platform)
        }));
        if let Err(error)=self.release_turn(){tracing::warn!(code="update_lock_unavailable",%error,"trava da atualização não removida");}
        match outcome {
            Ok(value) => value,
            Err(payload) => engine.failed(
                &panic_cause(payload.as_ref()),
                port,
                None,
            ),
        }
    }
    fn execute_inner(
        &mut self,
        port: u16,
        source: &(dyn Source + Sync),
        platform: Option<&str>,
    ) -> super::super::StateSnapshot {
        let backend = self
            .environment
            .get("HANGAR_UPDATE_BACKEND_PID")
            .and_then(|v| v.parse::<u32>().ok());
        if let Err(error)=self.state().try_write(json!({"fase":"rodando","ok":null,"erro":null,"resgate":null,"voltou":null,"no_ar":null,"commit_de":"","commit_para":null,"pid":std::process::id(),"backend_pid":backend,"reiniciar_manual":false,"shell_mudou":false,"waiting_for_server_build":false,"request_id":null,"log":[],"avisos":[]})) {return super::super::StateSnapshot::failure(error);}
        let pre = match self.preflight() {
            Ok(pre) => pre,
            Err(e) => return self.failed(&e, port, None),
        };
        let from = pre["commit"].as_str().unwrap_or("");
        let target = pre["alvo"]
            .as_str()
            .filter(|s| !s.is_empty())
            .unwrap_or("main");
        let branch = pre["branch"].as_str().unwrap_or("");
        let topology = pre["topologia"].as_str().unwrap_or("manual");
        if let Err(error)=self.state().try_write(json!({"commit_de":from})) {return super::super::StateSnapshot::failure(error);}
        if pre["branch_de_trabalho"] == true {
            return self.failed(&format!("este checkout esta na branch '{branch}', e a atualizacao alinha o disco com origin/{target} — troque para a {target} antes de atualizar"),port,None);
        }
        if pre["pode"] != true {
            let missing = pre["faltando"]
                .as_array()
                .map(|v| {
                    v.iter()
                        .filter_map(Value::as_str)
                        .collect::<Vec<_>>()
                        .join(", ")
                })
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| pre["erro"].as_str().unwrap_or("desconhecido").into());
            return self.failed(
                &format!("falta o que a atualizacao precisa: {missing}"),
                port,
                None,
            );
        }
        if target != "main" {
            match self.git(
                &[
                    "ls-remote",
                    "--exit-code",
                    "origin",
                    &format!("refs/heads/{target}"),
                ],
                60,
            ) {
                Ok(output) if output.code == 0 => {}
                Ok(output) => {
                    let reason = if output.code == 2 {
                        "nao existe no origin".into()
                    } else {
                        format!("nao pode ser consultada no origin ({})", tail(&output, 4))
                    };
                    return self.failed(
                        &format!(
                            "a branch '{target}' (CP_UPDATE_BRANCH) {reason}; nada foi alterado"
                        ),
                        port,
                        None,
                    );
                }
                Err(e) => return self.failed(&e, port, None),
            }
        }
        self.environment
            .insert("HANGAR_UPDATE_OWNS_RUST".into(), "1".into());
        let prepared = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| -> Result<(), String> {
            if let Some(platform) = platform {
                let errors = releases::discard(&self.home.join(".hangar/bin"), platform);
                if !errors.is_empty() {
                    return Err(errors.join("; "));
                }
            }
            self.step("resguardar")?;
            if let Some(rescue) = self.rescue(&pre)? {
                self.state().try_write(json!({"resgate":rescue}))?;
            }
            self.step("codigo")?;
            let release = self.pull(&pre, source, platform)?;
            let to = self
                .git(&["rev-parse", "HEAD"], 30)?
                .stdout
                .trim()
                .to_owned();
            self.state().try_write(json!({"commit_para":to,"shell_mudou":self.shell_changed(from, &to)}))?;
            if let Some((published, protocol)) = release
                .as_ref()
                .and_then(|r| self.release_binary(r, platform.unwrap_or("")))
            {
                if self.protocol_at("HEAD") != Some(protocol) {
                    self.state().warn(&format!("o binário do Rust publicado para este sistema (commit {}) fala outro contrato que este código; o Python atende sozinho até sair o binário novo",&published[..8]));
                }
            }
            self.step("passos")?;
            let runner = super::super::ShellRunner {
                timeout: std::time::Duration::from_secs(600),
            };
            super::super::steps::apply_checkout_steps(
                &self.root,
                &self.home,
                &self.state_dir,
                &runner,
                &self.state(),
                true,
            )
            .map_err(|e| e.to_string())?;
            self.step("instalar")?;
            self.prepare(topology, true, release.as_ref(), source, platform)
        })).unwrap_or_else(|payload| Err(panic_cause(payload.as_ref())));
        if let Err(mut error) = prepared {
            if !from.is_empty()
                && self
                    .git(&["rev-parse", "HEAD"], 30)
                    .is_ok_and(|v| v.stdout.trim() != from)
            {
                return self.rollback(from, &error, topology, port, branch, source, platform);
            }
            let current = self
                .git(&["rev-parse", "--abbrev-ref", "HEAD"], 30)
                .map(|v| v.stdout.trim().to_owned())
                .unwrap_or_default();
            if !current.is_empty() && !branch.is_empty() && current != branch {
                error.push_str(&format!(" (o checkout ficou na branch {current})"));
            }
            return self.failed(&error, port, None);
        }
        let before = self.server_pid(topology, port);
        if let Err(error)=self.step("reiniciar") {return super::super::StateSnapshot::failure(error);}
        if let Err(error) = self.restart_service(topology, port) {
            return self.rollback(
                from,
                &format!("o servidor nao reiniciou: {error}"),
                topology,
                port,
                branch,
                source,
                platform,
            );
        }
        if self.state().read().get("reiniciar_manual") != Some(&Value::Bool(true)) {
            if !self.up(port, 30) {
                return self.rollback(
                    from,
                    "o servidor nao respondeu depois de reiniciar",
                    topology,
                    port,
                    branch,
                    source,
                    platform,
                );
            }
            if let Some(before) = before {
                if self.server_pid(topology, port) == Some(before) {
                    return self.rollback(from,&format!("o servidor nao reiniciou: o processo na porta e o mesmo de antes (pid {before})"),topology,port,branch,source,platform);
                }
            }
        }
        if let Err(error)=self.step("nativo") {return super::super::StateSnapshot::failure(error);}
        self.update_native(target, port, source);
        let _ = std::fs::remove_dir_all(self.root.join("frontend/.dist-velho"));
        if let Some(platform) = platform {
            for warning in releases::discard(&self.home.join(".hangar/bin"), platform) {
                self.state().warn(&warning);
            }
        }
        if let Err(error)=self.state().try_write(json!({"fase":"pronto","ok":true,"texto":"Atualizado"})) {return super::super::StateSnapshot::failure(error);}
        self.state().read()
    }
}
impl Service {
    pub fn execute_restart(&self,port:u16,request:&str,source:&dyn Source)->super::super::StateSnapshot {
        let outcome=self.execute_restart_inner(port,request,source);
        if let Err(error)=self.release_turn(){tracing::warn!(code="update_lock_unavailable",%error,"trava da atualização não removida");}
        outcome
    }
    fn execute_restart_inner(
        &self,
        port: u16,
        request: &str,
        source: &dyn Source,
    ) -> super::super::StateSnapshot {
        let mut service = self.clone();
        service.writing = true;
        if let Err(error)=service.state().try_write(json!({"fase":"rodando","ok":null,"erro":null,"voltou":null,"no_ar":null,"resgate":null,"pid":std::process::id(),"reinicio_erro":null,"avisos":[],"log":[],"commit_de":"","commit_para":null,"reiniciar_manual":false,"shell_mudou":false,"waiting_for_server_build":false,"request_id":request})) {return super::super::StateSnapshot::failure(error);}
        let result = (|| -> Result<(), String> {
            service.state().try_write(json!({"fase":"rodando","etapa":"tela","passo":1,"total":2,"texto":"Trocando a tela pelo build publicado","etapa_inicio":chrono::Local::now().format("%Y-%m-%dT%H:%M:%S%:z").to_string()}))?;
            if let Some(warning) = service.update_dist(source)? {
                service.state().warn(&warning);
            }
            service.state().try_write(json!({"fase":"rodando","etapa":"reiniciar","passo":2,"total":2,"texto":"Reiniciando o servidor","etapa_inicio":chrono::Local::now().format("%Y-%m-%dT%H:%M:%S%:z").to_string()}))?;
            let topology = service.topology();
            let before = service.server_pid(topology, port);
            service.restart_service(topology, port)?;
            if !service.up(port, 30) {
                return Err("o servidor nao respondeu depois de reiniciar".into());
            }
            if let Some(before) = before {
                if service.server_pid(topology, port) == Some(before) {
                    return Err(format!(
                        "o servidor nao reiniciou: o processo na porta e o mesmo de antes (pid {before})"
                    ));
                }
            }
            let _ = std::fs::remove_dir_all(service.root.join("frontend/.dist-velho"));
            Ok(())
        })();
        let outcome = match result {
            Ok(()) => service
                .state()
                .finish(json!({"fase":"pronto","ok":true,"texto":"Reiniciado"})),
            Err(error) => {
                let error = format!("RuntimeError: {error}");
                service.state().finish(json!({"fase":"pronto","ok":false,"erro":error,"reinicio_erro":error,"voltou":false}))
            }
        };
        outcome
    }
}
