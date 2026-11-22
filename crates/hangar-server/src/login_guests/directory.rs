//! Diretório de convidados no serviço do Connect: depois de cada gravação, na subida e quando a conta passa a
//! ser lida (Connect ligado, PC liberado), a lista de hashes
//! das contas desta máquina vai ao serviço. Falha não volta à tela: vai ao log e ao diário.
use std::path::PathBuf;
use std::sync::Arc;

use hangar_guest_accounts::Store;
use tokio::sync::watch;

use crate::connect::supervisor::Supervisor;

#[derive(Clone, Default)]
pub struct Publisher {
    accounts: Option<Arc<Store>>,
    supervisor: Option<Arc<Supervisor>>,
    diary: Option<PathBuf>,
    // Uma publicação por vez: a de depois lê o arquivo depois da anterior e chega por último ao serviço.
    turn: Arc<tokio::sync::Mutex<()>>,
}

impl Publisher {
    pub(super) fn bind(&mut self, accounts: Arc<Store>, supervisor: Arc<Supervisor>, diary: Option<PathBuf>) {
        let (read, changes) = (supervisor.account_read(), accounts.changes());
        self.accounts = Some(accounts);
        self.supervisor = Some(supervisor);
        self.diary = diary;
        self.follow_account(read);
        self.follow_changes(changes);
    }

    /// Connect ligado depois da subida, ou PC liberado na conta depois de recusado: a lista vai de novo.
    fn follow_account(&self, mut read: watch::Receiver<bool>) {
        let me = self.clone();
        tokio::spawn(async move {
            while read.changed().await.is_ok() {
                if *read.borrow_and_update() {
                    me.publish();
                }
            }
        });
    }

    /// Conta gravada ou apagada, por qualquer caminho: a lista vai de novo.
    fn follow_changes(&self, mut changes: watch::Receiver<u64>) {
        let me = self.clone();
        tokio::spawn(async move {
            while changes.changed().await.is_ok() {
                me.publish();
            }
        });
    }

    /// Sem Connect ligado não há o que publicar; arquivo que nunca existiu também não.
    pub fn publish(&self) {
        let Some(supervisor) = self.supervisor.clone() else { return };
        let me = self.clone();
        tokio::spawn(async move {
            let outcome = me.run(&supervisor).await;
            me.record(outcome).await;
        });
    }

    async fn run(&self, supervisor: &Supervisor) -> Option<Result<usize, &'static str>> {
        let accounts = self.accounts.clone()?;
        let _turn = self.turn.lock().await;
        let linked = match supervisor.read_linked().await {
            Ok(Some(linked)) => linked,
            Ok(None) => return None,
            Err(error) => return Some(Err(error.code)),
        };
        let reader = accounts.clone();
        let hashes = match tokio::task::spawn_blocking(move || reader.user_hashes()).await {
            Ok(Ok(Some(hashes))) => hashes,
            Ok(Ok(None)) => return None,
            _ => return Some(Err("guest_accounts_unreadable")),
        };
        let taken = match hangar_connect::guests::publish(&linked, &hashes).await {
            Ok(taken) => taken,
            Err(error) => return Some(Err(error.code)),
        };
        let count = taken.len();
        match tokio::task::spawn_blocking(move || accounts.mark_taken(&taken)).await {
            Ok(Ok(())) => Some(Ok(count)),
            _ => Some(Err("guest_accounts_io_failed")),
        }
    }

    async fn record(&self, outcome: Option<Result<usize, &'static str>>) {
        let event = match outcome {
            None => return,
            Some(Ok(taken)) => serde_json::json!({"evento":"convidados.diretorio","nivel":"ok","nomes_em_uso":taken}),
            Some(Err(code)) => {
                tracing::warn!(code, "convidados não publicados no serviço do Connect");
                serde_json::json!({"evento":"convidados.diretorio","nivel":"aviso","codigo":code})
            }
        };
        let Some(dir) = self.diary.clone() else { return };
        let _ = tokio::task::spawn_blocking(move || crate::diag::record_blocking(Some(&dir), &[event], "convidados")).await;
    }
}
