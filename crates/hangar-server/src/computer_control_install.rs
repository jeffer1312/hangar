//! O hcu (`hangar-computer-control-setup`) sem pedido: instala na subida se faltar e, de hora em hora,
//! instala de novo se falhou ou atualiza sob os portões do Hangar. Relógio próprio: o `tick_inner` da
//! atualização do Hangar sai cedo quando ele está em dia.
use std::sync::Arc;

use hangar_computer_control_setup as setup;

use crate::routes::AppState;
use crate::update::periodic::INTERVAL;

/// Só a subida liga; nunca segura a subida. Falha já foi ao diário pelo crate.
pub(crate) fn spawn(state: Arc<AppState>) {
    tokio::spawn(async move {
        let http = match crate::http_client().build() {
            Ok(http) => http,
            Err(error) => return tracing::warn!(code = "computer_control_http_failed", %error, "Computer Use: sem cliente http"),
        };
        let github = setup::Github::default();
        let paths = setup::Paths::new(state.accounts.env.home.clone(), Vec::new());
        let diary = crate::diag::diary_dir_in(&state.accounts.env);
        let _ = setup::install_if_missing(&paths, &http, &github, diary.as_deref()).await;
        loop {
            tokio::time::sleep(INTERVAL).await;
            if let Ok(setup::Swapped::No) = setup::install_if_missing(&paths, &http, &github, diary.as_deref()).await {
                let _ = setup::update_tick(&paths, &http, &github, &policy(&state), diary.as_deref()).await;
            }
        }
    });
}

/// Os portões da atualização automática do Hangar, lidos a cada tique.
fn policy(state: &AppState) -> setup::Policy {
    let home = &state.accounts.env.home;
    let package_channel = matches!(crate::update::http::mode(home), crate::update::http::Mode::Package(_))
        .then(|| crate::update::channel(&crate::update::Layout::new(home)));
    let automations = match crate::config_sync::runtime::RuntimeConfig::from_state(state) {
        Ok(runtime) => {
            let value = runtime.get("automations");
            let enabled = crate::orchestration::truthy(&value);
            crate::orchestration::codec::discard(value);
            enabled
        }
        Err(error) => {
            tracing::warn!(code = "computer_control_config_failed", %error, "Computer Use: sem configuração; não atualiza");
            false
        }
    };
    setup::Policy { automations, package_channel }
}
