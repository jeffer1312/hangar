//! Antigravity sem terminal no registro: abrir pelo sidecar (`antigravity-sessions/`) e adotar na subida o
//! `agy acp` que ficou vivo. O processo sobe e religa pelo mesmo caminho do Codex sem terminal (`open_with_launch`).
use super::*;

/// Conversa e caminhos da sessão a partir do sidecar, como o `headless_target` do Codex.
fn target(queue_dir:&std::path::Path,name:&str,key:&str,generation:u64,meta:&super::super::codex_sidecar::Meta) -> RuntimeTarget {
    let (state_path,lease_path) = super::super::codex_terminal::queue_files(queue_dir,key);
    let mut metadata = Value::Object(meta.clone());
    metadata["key"] = json!(key);
    let text = |field:&str|meta.get(field).and_then(Value::as_str).filter(|s|!s.is_empty());
    let transcript = match (text("brain_dir"),text("conversation_id")) {
        (Some(dir),Some(id)) if hangar_antigravity::paths::valid_conversation_id(id) =>
            std::path::Path::new(dir).join(id).join(".system_generated").join("logs").join("transcript_full.jsonl"),
        _ => std::path::PathBuf::new(),
    };
    let binding = meta.get("cano").cloned().and_then(|cano|serde_json::from_value::<process::Cano>(cano).ok())
        .map_or(CanoBinding { pid:0,escuta:String::new(),token:String::new(),versao:2 },|cano|CanoBinding { pid:cano.pid,escuta:cano.escuta,token:cano.token,versao:cano.versao });
    RuntimeTarget { key:key.into(),generation,name:name.into(),provider:"antigravity".into(),binding,transcript,
        created:meta.get("created").and_then(Value::as_f64).unwrap_or(0.0),metadata,lease_path,state_path,projection_dir:queue_dir.to_owned() }
}

impl RuntimeRegistry {
    fn antigravity_dirs(&self) -> (std::path::PathBuf,std::path::PathBuf) {
        let env = self.policy.codex_environment();
        (super::super::antigravity_sidecar::dir(&env.home),env.claude_base.join(".hangar-queue"))
    }

    /// Entrada sem terminal do Antigravity aberta aqui para `name`.
    pub(crate) async fn antigravity_headless(&self,name:&str) -> Option<(String,u64,RuntimeHandle)> {
        self.entries.lock().await.iter().find_map(|(key,entry)|match &entry.handle {
            EntryHandle::Headless(handle) if entry.name == name && entry.provider == "antigravity" => Some((key.clone(),entry.generation,handle.clone())),
            _ => None,
        })
    }

    /// O sidecar do Antigravity sem terminal, para quem só lê (as pílulas) sem subir o `agy acp`.
    pub async fn antigravity_headless_meta(&self,name:&str) -> Option<Value> {
        let (dir,_) = self.antigravity_dirs();
        let path = super::super::codex_sidecar::path(&dir,name);
        tokio::task::spawn_blocking(move||super::super::codex_sidecar::load(&path)).await.ok().flatten()
            .filter(|meta|meta.get("headless") == Some(&Value::Bool(true))).map(Value::Object)
    }

    /// Sobe (ou adota, com o cano gravado vivo) o Antigravity sem terminal do sidecar. `None`: não há sidecar
    /// sem terminal do Antigravity com esse nome.
    pub async fn open_antigravity_headless(&self,name:&str) -> Option<Result<(),RuntimeError>> {
        let (dir,queue_dir) = self.antigravity_dirs();
        let path = super::super::codex_sidecar::path(&dir,name);
        let meta = match tokio::task::spawn_blocking(move||super::super::codex_sidecar::read(&path)).await {
            Ok(Ok(Some(meta))) => meta,
            Ok(Ok(None)) => return None,
            Ok(Err(_)) => return Some(Err(RuntimeError::new("antigravity_sidecar_unreadable","o arquivo da sessão do Antigravity existe e não foi lido"))),
            Err(_) => return Some(Err(failure("sidecar_job"))),
        };
        if meta.get("headless") != Some(&Value::Bool(true)) { return None; }
        let key = meta.get("key").and_then(Value::as_str).filter(|k|!k.is_empty())?.to_owned();
        if self.antigravity_headless(name).await.is_some() { return Some(Ok(())); }
        if self.has_entry(Some(&key),name).await {
            return Some(Err(RuntimeError::new("antigravity_headless_entry_conflict","já há outra sessão aberta aqui com este nome ou chave")));
        }
        let (state_path,_) = super::super::codex_terminal::queue_files(&queue_dir,&key);
        let (q,k,n) = (queue_dir.clone(),key.clone(),name.to_owned());
        let generation = match tokio::task::spawn_blocking(move||super::super::queue::Store::open(&state_path,&q,super::super::queue::State::new(&k,1,&n,Vec::new()))
            .map(|store|store.state().generation)).await {
            Ok(Ok(generation)) => generation,
            Ok(Err(_)) => return Some(Err(failure("queue_state_read"))),
            Err(_) => return Some(Err(failure("queue_job"))),
        };
        let target = target(&queue_dir,name,&key,generation,&meta);
        Some(self.open_with_launch(target,dir).await.map(|_|()))
    }

    /// Na subida: abre cada Antigravity sem terminal com cano vivo da chave (adota) ou com mensagem esperando.
    pub async fn register_antigravity_headless(self:Arc<Self>) {
        self.startup.wait().await;
        let (dir,queue_dir) = self.antigravity_dirs();
        let listed = tokio::task::spawn_blocking(move || -> std::io::Result<Vec<(String,bool)>> {
            let entries = match std::fs::read_dir(&dir) { Ok(entries)=>entries,Err(error) if error.kind()==std::io::ErrorKind::NotFound=>return Ok(Vec::new()),Err(error)=>return Err(error) };
            let mut out = Vec::new();
            for path in entries.flatten().map(|e|e.path()).filter(|p|p.extension().is_some_and(|ext|ext == "json")) {
                let Ok(Some(meta)) = super::super::codex_sidecar::read(&path) else { continue };
                if meta.get("headless") != Some(&Value::Bool(true)) { continue; }
                let (Some(name),Some(key)) = (meta.get("name").and_then(Value::as_str),meta.get("key").and_then(Value::as_str).filter(|k|!k.is_empty())) else { continue };
                let alive = meta.get("cano").and_then(|cano|cano["pid"].as_u64()).and_then(|pid|u32::try_from(pid).ok())
                    .is_some_and(|pid|process::liveness(pid,key)==process::Liveness::Ours);
                if alive || pending(&super::super::codex_terminal::queue_files(&queue_dir,key).0) { out.push((name.to_owned(),alive)); }
            }
            Ok(out)
        }).await;
        let listed = match listed {
            Ok(Ok(listed)) => listed,
            Ok(Err(error)) => {
                tracing::warn!(kind=?error.kind(),"sidecars do Antigravity sem terminal ilegíveis na subida");
                self.diag.report("runtime.registration_failed","*","antigravity_sidecar_dir","a pasta das sessões do Antigravity sem terminal não foi lida");
                return;
            }
            Err(_) => return,
        };
        let opens = listed.into_iter().map(|(name,_)| {
            let registry = self.clone();
            async move {
                if let Some(Err(error)) = registry.open_antigravity_headless(&name).await {
                    tracing::warn!(session=%name,code=%error.code,"Antigravity sem terminal não abriu na subida");
                    registry.diag.report("runtime.registration_failed",&name,&error.code,"a sessão do Antigravity sem terminal não abriu na subida do Rust");
                }
            }
        });
        futures_util::future::join_all(opens).await;
    }
}

/// O que a saída do sem terminal deixa para a abertura do terminal (e para desfazer, se ela falhar).
pub struct HeadlessLeft { pub key:String, pub conversation:String }

fn refuse(message:&str) -> RuntimeError { RuntimeError::new("erro_troca_modo",message) }

/// Alguma linha ainda não entregue: trocar agora a mandaria para o processo que está saindo.
fn waiting(rows:&Value) -> bool { rows.as_array().is_some_and(|rows|rows.iter().any(|row|row["delivered"] == false)) }

impl RuntimeRegistry {
    /// Com terminal → sem terminal, sob a porta de entrada que quem chama segura: o pane sai sem a limpeza do
    /// fechamento (grupo, convites, fila e `then` ficam), o sidecar sem terminal nasce com a mesma chave e
    /// conversa, o spec troca de superfície e o ator ACP abre pelo `session/resume`. `None`: não é Antigravity
    /// com terminal aberto aqui.
    pub async fn antigravity_to_headless(&self,name:&str) -> Option<Result<Value,RuntimeError>> {
        let (handle,target) = self.entries.lock().await.values().find_map(|entry|match &entry.handle {
            EntryHandle::OtherTerminal { target,handle } if entry.name == name && target.provider == hangar_transcript::Provider::Antigravity =>
                Some((handle.clone(),(**target).clone())),
            _ => None,
        })?;
        Some(async {
            let snapshot = handle.snapshot().await?;
            let state = &snapshot["view"]["public_state"];
            if state["state"] != "idle" || !state["question"].is_null() { return Err(refuse("Espere a sessão ficar ociosa antes de continuar sem terminal.")); }
            // O shift+tab troca o modo só na tela: o rodapé vale mais que o spec da abertura.
            let shown = state["claude_permission_mode"].as_str().and_then(crate::session_open::PermissionMode::antigravity);
            let conversation = hangar_antigravity::paths::conversation_of_transcript(&target.transcript)
                .ok_or_else(||refuse("Envie a primeira mensagem antes de continuar sem terminal."))?;
            if waiting(&handle.queue(format!("agy-switch:{}",crate::mods::state::random_hex(8)),Action::Load).await?) {
                return Err(refuse("Espere a fila esvaziar antes de trocar de modo."));
            }
            let env = self.policy.codex_environment();
            let (home,key) = (env.home.clone(),target.key.clone());
            let spec = tokio::task::spawn_blocking(move||crate::session_open::record::read(&home,&key)).await.map_err(|_|failure("spec_job"))?
                .map_err(|_|refuse("O registro da sessão não foi lido; a sessão foi mantida."))?
                .ok_or_else(||refuse("A sessão não tem registro de abertura; a sessão foi mantida."))?;
            let gemini_home = match &spec.account { crate::session_open::Account::Antigravity { gemini_home } => gemini_home.clone(), _ => None };
            let account = gemini_home.as_ref().map_or_else(hangar_antigravity::Account::default,|dir|hangar_antigravity::Account::extra(dir.into()));
            self.stop_other_for_switch(name).await?;
            let created = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0.0,|d|d.as_secs_f64());
            let meta = json!({"name":name,"provider":"antigravity","headless":true,"key":target.key,"cwd":target.cwd,"gemini_home":gemini_home,
                "brain_dir":account.brain_dir(&env.home),"conversation_id":conversation,"model":spec.model,
                "permission_mode":shown.or(spec.permission_mode).unwrap_or(crate::session_open::PermissionMode::Manual).name(),"jev":spec.jev,"created":created});
            let mut next = spec.with(&crate::session_open::SessionChange { reason:crate::session_open::ChangeReason::ModeSwitch,
                surface:Some(crate::session_open::Surface::Headless),account:None,permission_mode:shown,model:None,effort:None,service_tier:None });
            next.conversation = Some(conversation);
            let (dir,home,owned) = (super::super::antigravity_sidecar::dir(&env.home),env.home.clone(),name.to_owned());
            tokio::task::spawn_blocking(move||-> std::io::Result<()> {
                super::super::codex_sidecar::put(&dir,&owned,meta.as_object().unwrap_or(&Default::default()))?;
                crate::session_open::record::write(&home,&next)
            }).await.map_err(|_|failure("sidecar_job"))?
                .map_err(|error|RuntimeError::new("antigravity_switch_io",&format!("o arquivo da sessão sem terminal não foi gravado: {:?}",error.kind())))?;
            self.open_antigravity_headless(name).await.unwrap_or_else(||Err(failure("antigravity_sidecar_missing")))?;
            Ok(json!({"ok":true,"terminal":false}))
        }.await)
    }

    /// Sem terminal → com terminal, primeira metade, sob a porta de entrada: confere que a sessão pode trocar,
    /// para o ator e o `agy acp` e deixa o sidecar sem o cano (a fila fica). Quem abre o terminal apaga o sidecar
    /// depois (`finish_leave_headless`) ou o reabre se falhar (`open_antigravity_headless`). `None`: não é
    /// Antigravity sem terminal.
    pub async fn antigravity_leave_headless(&self,name:&str) -> Option<Result<HeadlessLeft,RuntimeError>> {
        let (dir,_) = self.antigravity_dirs();
        let path = super::super::codex_sidecar::path(&dir,name);
        let meta = tokio::task::spawn_blocking(move||super::super::codex_sidecar::load(&path)).await.ok().flatten()
            .filter(|meta|meta.get("headless") == Some(&Value::Bool(true)))?;
        Some(async {
            let text = |field:&str|meta.get(field).and_then(Value::as_str).filter(|s|!s.is_empty()).map(str::to_owned);
            let key = text("key").ok_or_else(||refuse("O arquivo da sessão não tem a chave; a sessão foi mantida."))?;
            let conversation = text("conversation_id").filter(|id|hangar_antigravity::paths::valid_conversation_id(id))
                .ok_or_else(||refuse("Envie a primeira mensagem antes de abrir esta conversa no terminal."))?;
            // O `agy` com terminal não abre no modo que pergunta antes de agir.
            if super::super::antigravity_launch::stored_mode(&Value::Object(meta.clone()))? == crate::session_open::PermissionMode::Manual {
                return Err(RuntimeError::new("erro_permissao_manual_sem_terminal",
                    "o Antigravity com terminal não abre no modo que pergunta antes de agir; troque o modo de permissão antes"));
            }
            if let Some((open_key,generation,handle)) = self.antigravity_headless(name).await {
                let view = handle.view().await.map_err(|_|refuse("Espere a sessão ficar ociosa antes de abrir o terminal."))?;
                let pending = view["pending"].as_array().is_some_and(|p|!p.is_empty());
                if view["alive"] != true || view["ready"] != true || view["in_progress"] == true || pending {
                    return Err(refuse(if pending { "Responda às permissões antes de abrir o terminal." } else { "Espere a sessão ficar ociosa antes de abrir o terminal." }));
                }
                if waiting(&handle.queue(format!("agy-switch:{}",crate::mods::state::random_hex(8)),Action::Load).await?) {
                    return Err(refuse("Espere a fila esvaziar antes de trocar de modo."));
                }
                let barrier = self.barrier(&open_key).await;
                let _guard = barrier.lock().await;
                // Estacionada: o processo morre, mas a fila e o arquivo da sessão ficam para a vida com terminal.
                self.close_locked_as(&open_key,generation,true,true).await?;
            }
            if let Some(cano) = meta.get("cano").cloned().filter(|c|!c.is_null()).and_then(|c|serde_json::from_value::<process::Cano>(c).ok()) {
                process::kill(&cano,&key,&dir).await.map_err(|error|RuntimeError::new(error.code(),"o processo do Antigravity sem terminal não saiu"))?;
                let (d,n,k,pid) = (dir.clone(),name.to_owned(),key.clone(),cano.pid);
                let _ = tokio::task::spawn_blocking(move||super::super::antigravity_sidecar::clear_cano(&d,&n,&k,&json!({"pid":pid}))).await;
            }
            Ok(HeadlessLeft { key,conversation })
        }.await)
    }

    /// O terminal nasceu: o sidecar sem terminal desta chave sai.
    pub async fn finish_leave_headless(&self,name:&str,key:&str) {
        let (dir,_) = self.antigravity_dirs();
        let (owned,key) = (name.to_owned(),key.to_owned());
        let removed = tokio::task::spawn_blocking(move||super::super::codex_sidecar::locked(&dir,&owned,|path| {
            match super::super::codex_sidecar::load(path) {
                Some(meta) if meta.get("key").and_then(Value::as_str) == Some(key.as_str()) => std::fs::remove_file(path),
                _ => Ok(()),
            }
        })).await;
        if !matches!(removed,Ok(Ok(()))) { tracing::warn!(session=%name,code="antigravity_switch_sidecar_left","o arquivo sem terminal ficou depois de abrir o terminal"); }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CONVERSATION:&str = "904fd70e-847b-49ed-9999-a13d7a4d9a98";

    fn registry(home:&std::path::Path) -> RuntimeRegistry {
        RuntimeRegistry::new("instance".into()).with_codex_environment(crate::accounts::environment::AccountEnvironment::from_map(
            [("HOME".to_owned(),home.to_string_lossy().into_owned())].into()))
    }

    fn sidecar(home:&std::path::Path,fields:Value) {
        let dir = super::super::super::antigravity_sidecar::dir(home);
        std::fs::create_dir_all(&dir).unwrap();
        let mut meta = json!({"name":"ag","provider":"antigravity","headless":true,"key":"k1","cwd":"/w","conversation_id":CONVERSATION,
            "brain_dir":"/h/.gemini/antigravity-cli/brain","permission_mode":"auto"});
        for (k,v) in fields.as_object().unwrap() { meta[k] = v.clone(); }
        std::fs::write(super::super::super::codex_sidecar::path(&dir,"ag"),meta.to_string()).unwrap();
    }

    #[test]
    fn the_target_follows_the_conversation_and_the_recorded_cano() {
        let meta = json!({"name":"ag","key":"k1","brain_dir":"/h/b","conversation_id":CONVERSATION,"cano":{"pid":9,"escuta":"unix:/s","token":"t","ts":1.0,"versao":2}});
        let target = target(std::path::Path::new("/q"),"ag","k1",3,meta.as_object().unwrap());
        assert_eq!(target.transcript,std::path::Path::new("/h/b").join(CONVERSATION).join(".system_generated/logs/transcript_full.jsonl"));
        assert_eq!((target.binding.pid,target.binding.escuta.as_str(),target.generation),(9,"unix:/s",3));
        assert_eq!(target.provider,"antigravity");
        let fresh = target_of(json!({"name":"ag","key":"k1"}));
        assert_eq!((fresh.binding.pid,fresh.transcript.as_os_str().is_empty()),(0,true),"sem cano nem conversa: sobe um processo novo");
    }

    fn target_of(meta:Value) -> RuntimeTarget { target(std::path::Path::new("/q"),"ag","k1",1,meta.as_object().unwrap()) }

    /// Sem terminal → terminal recusa antes de mexer em processo: modo que pergunta, conversa que não existe.
    #[tokio::test]
    async fn leaving_the_headless_refuses_what_the_terminal_cannot_open() {
        let home = tempfile::tempdir().unwrap();
        let registry = registry(home.path());
        assert!(registry.antigravity_leave_headless("ag").await.is_none(),"sem sidecar não é dele");
        sidecar(home.path(),json!({"permission_mode":"manual"}));
        let refused = registry.antigravity_leave_headless("ag").await.unwrap().err().unwrap();
        assert_eq!(refused.code,"erro_permissao_manual_sem_terminal");
        sidecar(home.path(),json!({"conversation_id":null}));
        assert_eq!(registry.antigravity_leave_headless("ag").await.unwrap().err().unwrap().code,"erro_troca_modo");
        sidecar(home.path(),json!({}));
        let left = registry.antigravity_leave_headless("ag").await.unwrap().ok().unwrap();
        assert_eq!((left.key.as_str(),left.conversation.as_str()),("k1",CONVERSATION),"a mesma conversa segue para o terminal");
        registry.finish_leave_headless("ag","outra").await;
        assert!(super::super::super::codex_sidecar::path(&super::super::super::antigravity_sidecar::dir(home.path()),"ag").exists(),"chave de outra vida não apaga");
        registry.finish_leave_headless("ag","k1").await;
        assert!(!super::super::super::codex_sidecar::path(&super::super::super::antigravity_sidecar::dir(home.path()),"ag").exists());
    }

    #[test]
    fn a_row_still_waiting_blocks_the_switch() {
        assert!(waiting(&json!([{"id":"a","delivered":true},{"id":"b","delivered":false}])));
        assert!(!waiting(&json!([{"id":"a","delivered":true}])));
        assert!(!waiting(&json!([])));
    }
}
