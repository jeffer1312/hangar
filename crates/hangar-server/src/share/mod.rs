//! Convites, chaves de convidado, porteiro da 8766 e par externo: o Rust é o único dono.
pub mod store;
pub mod pairs;
pub mod tunnel;
pub mod life;
pub mod caller;
pub mod gate;
pub mod owner_routes;
pub mod guest_routes;
pub mod pair_routes;
pub mod forward;
pub mod bridge;

use std::sync::Arc;

/// O que o porteiro, as rotas do dono, do convidado e do par compartilham.
pub struct ShareState {
    pub store: store::ShareStore,
    pub pairs: pairs::PairStore,
    pub lives: life::Lives,
    pub tailscale: Arc<dyn tunnel::Tailscale>,
    pub tunnel: tunnel::Memo,
    pub connect: Option<Arc<crate::connect::supervisor::Supervisor>>,
}

impl ShareState {
    /// Nome desta máquina no Tailscale, lembrado por alguns minutos.
    pub async fn host(&self) -> Result<String, String> {
        tunnel::cached_host(&*self.tailscale, &self.tunnel).await
    }

    /// Liga o Funnel; a varredura passa a conferir o estado de novo.
    pub async fn ensure_on(&self) -> Result<String, tunnel::TunnelError> {
        let done = tunnel::ensure_on(&*self.tailscale).await;
        self.tunnel.set_applied(done.is_ok().then_some(true));
        if done.is_ok() { self.tunnel.forget_failure(); }
        done
    }

    /// A ligação, quando é o Connect que publica o convite: com Tailscale, o Funnel segue como era. Falha de um
    /// instante não conta, ou gastaria certificado em máquina com Tailscale; com o site já de pé não há o que gastar.
    pub async fn connect_entrance(&self) -> Option<crate::connect::state::Linked> {
        let linked = self.connect.as_ref()?.linked().await?;
        let fix = self.host().await.err()?;
        (tunnel::confirmed_absent(&fix) || linked.guest_published).then_some(linked)
    }

    /// Endereço público do convite, sem ligar nada: a página e os resgates são abertos a quem não tem token.
    /// Site nunca publicado não é endereço: `tailscale` mudo por um instante gravaria um servidor morto no convidado.
    pub async fn public_guest_url(&self) -> Result<String, tunnel::TunnelError> {
        if let Some(linked) = self.connect_entrance().await.filter(|linked| linked.guest_published) {
            return Ok(format!("https://{}", linked.guest_host));
        }
        let host = self.host().await.map_err(tunnel::TunnelError::plain)?;
        Ok(format!("https://{host}:{}", tunnel::FUNNEL_PORT))
    }

    /// `public_guest_url` com a entrada aberta: só para rota do dono.
    pub async fn open_guest_url(&self) -> Result<String, tunnel::TunnelError> {
        let (Some(supervisor), Some(linked)) = (self.connect.clone(), self.connect_entrance().await) else {
            return self.ensure_on().await;
        };
        // Tarefa própria: quem vem pelo Connect perde a conexão na troca dos filhos, e o pedido cancelado os deixaria parados.
        match tokio::spawn(async move { supervisor.publish_guest().await }).await {
            Ok(Ok(())) => Ok(format!("https://{}", linked.guest_host)),
            Ok(Err(error)) => Err(tunnel::TunnelError::plain(error.msg)),
            Err(error) => Err(tunnel::TunnelError::plain(error.to_string())),
        }
    }
}

/// Nome desta máquina para o outro lado: o id de servidor, senão o nome do sistema.
pub fn machine_name(server_id: &str) -> String {
    if server_id.is_empty() { sysinfo::System::host_name().unwrap_or_default() } else { server_id.to_owned() }
}

/// Estado de convites quando o Python indica o `shares.json`. O `external_pairs.json` mora no
/// `.hangar-pair` da conta padrão, como os grupos; sem as pastas da lista não há onde pôr.
pub fn from_env(shares_file: Option<std::path::PathBuf>, dirs: Option<&crate::list::discover_other::Dirs>,
                mux_program: std::ffi::OsString, connect: Option<Arc<crate::connect::supervisor::Supervisor>>) -> Option<Arc<ShareState>> {
    let shares_file = shares_file?;
    let Some(dirs) = dirs else {
        tracing::error!("HANGAR_SHARES_FILE sem HANGAR_LIST_DIRS: convites ficam fora do Rust");
        return None;
    };
    let clock: store::Clock = Arc::new(|| std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs_f64()).unwrap_or_default());
    Some(Arc::new_cyclic(|me: &std::sync::Weak<ShareState>| {
        let me = me.clone();
        ShareState {
            store: store::ShareStore::open(shares_file, clock),
            pairs: pairs::PairStore::open(dirs.claude.join(".hangar-pair").join("external_pairs.json")),
            lives: life::Lives::new(Arc::new(life::MuxProbe::new(mux_program, dirs.home.clone())), Arc::default())
                .rereading(move |hash| me.upgrade()?.store.lookup_hash(hash)),
            tailscale: Arc::new(tunnel::Real),
            tunnel: tunnel::Memo::default(),
            connect,
        }
    }))
}
