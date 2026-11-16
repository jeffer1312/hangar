//! A qual navegador vai um pedido do servidor (evento `nav`, `/cdp/browser/<nome>`): ao da máquina dona da sessão,
//! qualquer que seja o servidor ativo. O ativo só vale em Configurações e na criação de sessão, nunca na conversa.

/// O que fazer com o `nav` que a lista da máquina `server` trouxe.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum Nav {
    /// `key` é a mesma chave de `browser_key` com essa sessão aberta.
    Open { key: String, name: String, url: String },
    /// Não abre neste app; o código vai ao diário.
    Skip(&'static str),
}

/// `owner_loopback`: a conexão guardada da máquina `server` aponta para este computador (`None` = sem conexão). O
/// endereço que o servidor resolve é o 127.0.0.1 dele e o CLI que pediu roda nele: só a máquina deste app o abre.
pub(super) fn nav_target(server: &str, owner_loopback: Option<bool>, data: &serde_json::Value) -> Nav {
    let field = |name: &str| data[name].as_str().filter(|v| !v.is_empty());
    let (Some(name), Some(url)) = (field("name"), field("url")) else { return Nav::Skip("nav_missing_fields") };
    match owner_loopback {
        None => Nav::Skip("nav_no_connection"),
        Some(false) => Nav::Skip("nav_remote_server"),
        Some(true) => Nav::Open { key: format!("{server}::{name}"), name: name.to_owned(), url: url.to_owned() },
    }
}

/// Dona da sessão `name` pedida por `/cdp/browser/<nome>`, que chega pelo loopback sem dizer a máquina: a primeira
/// máquina em loopback que tem a sessão na lista. `machines`: (chave, conexão em loopback, sessão na lista).
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub(super) fn cdp_owner(name: &str, machines: impl IntoIterator<Item = (String, bool, bool)>) -> Result<String, (u16, String)> {
    let mut local = false;
    for (key, loopback, listed) in machines {
        if !loopback { continue; }
        local = true;
        if listed { return Ok(format!("{key}::{name}")); }
    }
    if local { Err((404, format!("erro: session_not_found: a sessao {name} nao existe no servidor desta maquina"))) }
    else { Err((503, "erro: no_local_server: nenhum servidor desta maquina esta ligado no app".into())) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const LOCAL: &str = "http://127.0.0.1:8765";

    #[test]
    fn nav_opens_for_the_owner_machine_even_when_another_server_is_active() {
        let data = json!({"name": "web-app", "url": "http://127.0.0.1:8765/api/sessions/web-app/pages/a?page_ticket=1.x"});
        assert_eq!(nav_target(LOCAL, Some(true), &data), Nav::Open {
            key: format!("{LOCAL}::web-app"), name: "web-app".into(), url: data["url"].as_str().unwrap().into(),
        });
    }

    #[test]
    fn nav_of_another_machine_or_without_fields_is_skipped_with_a_code() {
        let data = json!({"name": "s", "url": "http://localhost:3000"});
        assert_eq!(nav_target("https://casa.ts.net", Some(false), &data), Nav::Skip("nav_remote_server"));
        assert_eq!(nav_target(LOCAL, None, &data), Nav::Skip("nav_no_connection"));
        for bad in [json!({"name": "s"}), json!({"url": "http://x"}), json!({"name": "", "url": "http://x"}), json!(null)] {
            assert_eq!(nav_target(LOCAL, Some(true), &bad), Nav::Skip("nav_missing_fields"), "{bad}");
        }
    }

    #[test]
    fn cdp_goes_to_the_local_machine_that_lists_the_session() {
        let machines = || vec![
            ("https://casa.ts.net".to_owned(), false, true),
            (LOCAL.to_owned(), true, true),
        ];
        assert_eq!(cdp_owner("api", machines()), Ok(format!("{LOCAL}::api")), "a ativa remota não tira a sessão local");
        assert_eq!(cdp_owner("api", vec![(LOCAL.to_owned(), true, false)]).unwrap_err().0, 404);
        assert_eq!(cdp_owner("api", vec![("https://casa.ts.net".to_owned(), false, true)]).unwrap_err().0, 503);
        assert_eq!(cdp_owner("api", Vec::new()).unwrap_err().0, 503);
    }
}
