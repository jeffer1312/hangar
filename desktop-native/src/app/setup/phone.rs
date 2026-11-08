//! Tela 5: o QR do pareamento, pedido ao backend como o diálogo de pareamento já faz (`machines/pair.rs`). A escolha do
//! endereço é regra do assistente: Tailscale se respondeu, senão Wi-Fi (o diálogo escolhe o mais rápido). E a conferência da
//! conta Tailscale pelo `tailscale status`, para a tela 4 marcar a conta sem esperar o script.
use super::*;
use super::super::machines::{parse_reach, Kind, Reach, Status};
#[cfg(test)]
use super::super::machines::Address;
use std::process::Command;

pub(crate) struct Pairing { pub url: String, pub tailscale: bool, pub svg: Vec<u8> }

/// `pairing: None`: nenhum endereço respondeu para o celular.
pub(crate) struct Offer { pub pairing: Option<Pairing>, pub connect: bool }

/// O que a tela 5 mostra. A imagem é feita na thread da janela, uma vez, e reaproveitada a cada quadro.
pub(crate) enum Qr { Idle, Loading, Shown { url: String, tailscale: bool, image: Arc<Image>, connect: bool }, NoAddress { connect: bool }, Failed(String) }

pub(crate) fn pick(reach: &Reach) -> Option<Kind> {
    let answered = |kind: Kind| reach.addresses.iter().any(|a| a.kind == kind && a.status == Status::Ok);
    [Kind::Tailscale, Kind::Lan].into_iter().find(|kind| answered(*kind))
}

/// Máquina ligada ao Connect: o celular entra pela conta, de qualquer rede.
pub(crate) fn offers_connect(reach: &Reach) -> bool { reach.connect.is_some() }

pub(crate) async fn load(api: Api) -> Result<Offer, String> {
    let value = api.server_read(&["alcance"], &[], 30).await.map_err(|e| Hangar::failure(&e))?;
    let reach = parse_reach(&value).ok_or_else(|| tr("invalid_response"))?;
    let connect = offers_connect(&reach);
    let Some(kind) = pick(&reach) else { return Ok(Offer { pairing: None, connect }) };
    let value = api.server_read(&["alcance", "pareamento"], &[("endereco", kind.raw())], 15).await.map_err(|e| Hangar::failure(&e))?;
    match (value.get("url").and_then(Value::as_str), value.get("qr_svg").and_then(Value::as_str)) {
        (Some(url), Some(svg)) => Ok(Offer { pairing: Some(Pairing { url: url.to_owned(), tailscale: kind == Kind::Tailscale, svg: svg.as_bytes().to_vec() }), connect }),
        _ => Err(tr("invalid_response")),
    }
}

pub(crate) fn tailscale_running(json: &str) -> bool {
    serde_json::from_str::<Value>(json).ok().and_then(|v| v.get("BackendState")?.as_str().map(|state| state == "Running")).unwrap_or(false)
}

pub(crate) fn tailscale_status() -> bool {
    let path = super::system::refreshed_path();
    let Some(program) = super::system::find_program("tailscale", &path) else { return false };
    super::system::hidden(&mut Command::new(program)).args(["status", "--json"]).env("PATH", &path).output()
        .is_ok_and(|out| tailscale_running(&String::from_utf8_lossy(&out.stdout)))
}

#[cfg(test)]
mod tests {
    use super::*;
    // O glob da gpui_kit (via `use super::*`) traz um `test` que colide com o atributo padrão.
    use core::prelude::v1::test;

    fn reach(list: &[(Kind, Status)]) -> Reach {
        Reach { loopback: false, bind: String::new(), connect: None,
            addresses: list.iter().map(|&(kind, status)| Address { kind, url: format!("https://{kind:?}"), status, ms: Some(10) }).collect() }
    }

    #[test]
    fn tailscale_wins_when_it_answered() {
        assert_eq!(pick(&reach(&[(Kind::Lan, Status::Ok), (Kind::Tailscale, Status::Ok)])), Some(Kind::Tailscale));
        assert_eq!(pick(&reach(&[(Kind::Lan, Status::Ok), (Kind::Tailscale, Status::Failed)])), Some(Kind::Lan));
        assert_eq!(pick(&reach(&[(Kind::Here, Status::Ok), (Kind::Public, Status::Ok), (Kind::Lan, Status::Unset)])), None);
    }

    #[test]
    fn phone_setup_offers_connect_when_linked() {
        let linked = parse_reach(&serde_json::json!({"enderecos": [{"tipo": "rede_local", "url": "http://10.0.0.2:8765", "estado": "ok"}],
            "connect": "https://mesa.conta.connect.hangar.dev.br"})).expect("formato do /api/alcance");
        assert!(offers_connect(&linked));
        assert_eq!(pick(&linked), Some(Kind::Lan), "o Connect não mexe nos endereços");
        let unlinked = parse_reach(&serde_json::json!({"enderecos": [], "connect": null})).expect("formato do /api/alcance");
        assert!(!offers_connect(&unlinked));
    }

    #[test]
    fn tailscale_status_needs_running_backend() {
        assert!(tailscale_running(r#"{"BackendState":"Running","Self":{}}"#));
        assert!(!tailscale_running(r#"{"BackendState":"NeedsLogin"}"#));
        assert!(!tailscale_running("não é json"));
    }
}
