use hangar_server::machine_peers::{dotenv::{SettingsEnvironment,SettingsSnapshot},pairing};
proptest::proptest!{
 #[test]
 fn pairing_configuration_never_panics(raw in ".{0,120}", public in ".{0,120}", token in ".{0,120}"){
  let environment=SettingsEnvironment::from_pairs([("CP_PORT".to_owned(),raw),("CP_PUBLIC_URL".to_owned(),public),("CP_AUTH_TOKEN".to_owned(),token)]);
  if let Ok(settings)=pairing::Settings::from_snapshot(&SettingsSnapshot::from_parts(environment,String::new())){
   let mut output=Vec::new();let _=pairing::print(&settings,"192.0.2.10",false,&mut output);
  }
 }
}

fn settings(public_url: &str) -> pairing::Settings {
    pairing::Settings { public_url: public_url.into(), bind: "127.0.0.1".into(), token: "segredo-do-dono".into(), port: "8765".into(), front_port: "0".into() }
}

/// Sem terminal a saída vai ao journal ou ao log do Windows: os endereços saem, o token não.
#[test]
fn pairing_without_a_terminal_leaves_the_token_out() {
    for public in ["", "https://casa.ts.net/"] {
        let mut output = Vec::new();
        pairing::print(&settings(public), "192.0.2.10", false, &mut output).unwrap();
        let text = String::from_utf8(output).unwrap();
        assert!(!text.contains("segredo-do-dono") && !text.contains("token="), "{text}");
        assert!(text.contains("Pareamento (abra no celular): ") && text.contains("App nativo: http://192.0.2.10:8765/"), "{text}");
        let front = if public.is_empty() { "http://192.0.2.10:8765/" } else { "https://casa.ts.net/" };
        assert!(text.contains(front), "{text}");
    }
}

/// No terminal nada muda: os QR e as URLs com o token, para o celular entrar.
#[test]
fn pairing_on_a_terminal_keeps_the_token() {
    let mut output = Vec::new();
    pairing::print(&settings(""), "192.0.2.10", true, &mut output).unwrap();
    let text = String::from_utf8(output).unwrap();
    assert!(text.contains("Scan to pair, or open: http://192.0.2.10:8765/?token=segredo-do-dono"), "{text}");
    assert!(text.contains("App nativo: http://192.0.2.10:8765/?token=segredo-do-dono"), "{text}");
}
