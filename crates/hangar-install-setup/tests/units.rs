//! Os casos que as capturas não cobrem: formato do par, token, `export`, fim de linha, link, pasta sem
//! escrita, de onde vem o `.env` e os argumentos.
use base64::Engine;
use hangar_install_setup::vapid::{self, Outcome, Request, with_keys};
use hangar_install_setup::{tailscale, token, utf8_error};
use ring::signature::{self, ECDSA_P256_SHA256_FIXED_SIGNING, EcdsaKeyPair, KeyPair};

const B64: base64::engine::GeneralPurpose = base64::engine::general_purpose::URL_SAFE_NO_PAD;

fn fixed() -> Result<(String, String), &'static str> {
    Ok(("PUB".to_owned(), "PRIV".to_owned()))
}

fn run(request: &Request) -> (u8, String, String) {
    let (mut out, mut err) = (Vec::new(), Vec::new());
    let code = vapid::run(request, &mut out, &mut err);
    (code, String::from_utf8(out).unwrap(), String::from_utf8(err).unwrap())
}

#[test]
fn the_pair_is_p256_and_the_private_key_signs_for_the_public_one() {
    let (public, private) = vapid::new_pair_b64().unwrap();
    assert!(!public.contains('=') && !private.contains('=') && !public.contains('+') && !public.contains('/'));
    let (public, private) = (B64.decode(public).unwrap(), B64.decode(private).unwrap());
    assert_eq!((public.len(), public[0], private.len()), (65, 0x04, 32));
    let rng = ring::rand::SystemRandom::new();
    let pair = EcdsaKeyPair::from_private_key_and_public_key(&ECDSA_P256_SHA256_FIXED_SIGNING, &private, &public, &rng).unwrap();
    assert_eq!(pair.public_key().as_ref(), &public[..]);
    let signed = pair.sign(&rng, b"hangar").unwrap();
    signature::UnparsedPublicKey::new(&signature::ECDSA_P256_SHA256_FIXED, &public).verify(b"hangar", signed.as_ref()).unwrap();
    assert_ne!(vapid::new_pair().unwrap().1, private, "cada chamada gera um par novo");
}

#[test]
fn the_token_is_48_lowercase_hex_digits() {
    let (mut out, mut err) = (Vec::new(), Vec::new());
    assert_eq!(token::run(&[], &mut out, &mut err), 0);
    let text = String::from_utf8(out).unwrap();
    let line = text.strip_suffix('\n').unwrap();
    assert!(line.len() == 48 && line.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)), "{text:?}");
    assert!(err.is_empty());
    assert_ne!(token::generate().unwrap(), token::generate().unwrap());
    let (mut out, mut err) = (Vec::new(), Vec::new());
    assert_eq!(token::run(&["x".to_owned()], &mut out, &mut err), 2);
    assert!(out.is_empty() && !err.is_empty());
}

#[test]
fn export_lines_lose_export_and_keep_the_rest() {
    let text = "A=1\r\n  export CP_VAPID_PUBLIC=a b\r\nCP_VAPID_PRIVATE=b\r\nexport OTHER=x\r\n";
    assert_eq!(with_keys(text, fixed).unwrap(), Outcome::Unexported(
        "A=1\r\n  CP_VAPID_PUBLIC=a b\r\nCP_VAPID_PRIVATE=b\r\nexport OTHER=x\r\n".to_owned(), vec!["CP_VAPID_PUBLIC"]));
    assert_eq!(with_keys("CP_VAPID_PUBLIC=a\nCP_VAPID_PRIVATE=b", fixed).unwrap(), Outcome::Kept);
    assert_eq!(with_keys("export CP_VAPID_PUBLIC=a\n", fixed).unwrap_err(),
        "tem só CP_VAPID_PUBLIC; apague a linha ou complete o par à mão");
}

#[test]
fn generated_keys_follow_the_file_line_ending_and_fill_empty_lines() {
    assert_eq!(with_keys("A=1\r\nB=2", fixed).unwrap(),
        Outcome::Generated("A=1\r\nB=2\r\nCP_VAPID_PUBLIC=PUB\r\nCP_VAPID_PRIVATE=PRIV\r\n".to_owned()));
    assert_eq!(with_keys("export CP_VAPID_PUBLIC=\nX=1\nCP_VAPID_PUBLIC=  \n", fixed).unwrap(),
        Outcome::Generated("export CP_VAPID_PUBLIC=\nX=1\nCP_VAPID_PUBLIC=PUB\nCP_VAPID_PRIVATE=PRIV\n".to_owned()));
    assert_eq!(with_keys("CP_VAPID_PRIVATE=", fixed).unwrap(),
        Outcome::Generated("CP_VAPID_PRIVATE=PRIV\nCP_VAPID_PUBLIC=PUB\n".to_owned()));
    assert_eq!(with_keys("", fixed).unwrap(), Outcome::Generated("CP_VAPID_PUBLIC=PUB\nCP_VAPID_PRIVATE=PRIV\n".to_owned()));
    assert_eq!(with_keys("# CP_VAPID_PUBLIC=a\n", fixed).unwrap(),
        Outcome::Generated("# CP_VAPID_PUBLIC=a\nCP_VAPID_PUBLIC=PUB\nCP_VAPID_PRIVATE=PRIV\n".to_owned()));
}

#[test]
fn export_rewrite_is_written_and_warned() {
    let dir = tempfile::tempdir().unwrap();
    let env = dir.path().join(".env");
    std::fs::write(&env, "export CP_VAPID_PUBLIC=a\nexport CP_VAPID_PRIVATE=b\n").unwrap();
    let (code, out, err) = run(&Request { env: Some(env.clone()), ..Request::default() });
    assert_eq!(code, 0);
    assert_eq!(out, format!("ok: {} já tem as chaves VAPID (mantidas)\n", env.display()));
    assert!(err.starts_with(&format!("aviso: {}: `export` tirado de CP_VAPID_PUBLIC, CP_VAPID_PRIVATE;", env.display())), "{err}");
    assert_eq!(std::fs::read_to_string(&env).unwrap(), "CP_VAPID_PUBLIC=a\nCP_VAPID_PRIVATE=b\n");
}

#[cfg(unix)]
#[test]
fn a_linked_env_stays_a_link_and_its_target_keeps_the_mode() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let real = dir.path().join("real.env");
    std::fs::write(&real, "CP_AUTH_TOKEN=x\n").unwrap();
    std::fs::set_permissions(&real, std::fs::Permissions::from_mode(0o640)).unwrap();
    let link = dir.path().join(".env");
    std::os::unix::fs::symlink("real.env", &link).unwrap();
    let (code, ..) = run(&Request { env: Some(link.clone()), ..Request::default() });
    assert_eq!(code, 0);
    assert!(std::fs::symlink_metadata(&link).unwrap().file_type().is_symlink());
    assert!(std::fs::read_to_string(&real).unwrap().starts_with("CP_AUTH_TOKEN=x\nCP_VAPID_PUBLIC="));
    assert_eq!(std::fs::metadata(&real).unwrap().permissions().mode() & 0o777, 0o640);
    // Link quebrado: o arquivo nasce no alvo, 600, e o link fica.
    let dangling = dir.path().join("other.env");
    std::os::unix::fs::symlink("missing.env", &dangling).unwrap();
    assert_eq!(run(&Request { env: Some(dangling.clone()), ..Request::default() }).0, 0);
    assert!(std::fs::symlink_metadata(&dangling).unwrap().file_type().is_symlink());
    assert_eq!(std::fs::metadata(dir.path().join("missing.env")).unwrap().permissions().mode() & 0o777, 0o600);
}

#[cfg(unix)]
#[test]
fn an_unwritable_folder_is_an_error_line_without_leftovers() {
    use std::os::unix::fs::PermissionsExt;
    if unsafe { libc::geteuid() } == 0 { return; }
    let dir = tempfile::tempdir().unwrap();
    let folder = dir.path().join("cfg");
    std::fs::create_dir(&folder).unwrap();
    std::fs::write(folder.join(".env"), "CP_AUTH_TOKEN=x\n").unwrap();
    std::fs::set_permissions(&folder, std::fs::Permissions::from_mode(0o555)).unwrap();
    let env = folder.join(".env");
    let (code, out, err) = run(&Request { env: Some(env.clone()), ..Request::default() });
    let listed: Vec<_> = std::fs::read_dir(&folder).unwrap().map(|e| e.unwrap().file_name()).collect();
    std::fs::set_permissions(&folder, std::fs::Permissions::from_mode(0o755)).unwrap();
    assert_eq!((code, out.as_str()), (1, ""));
    assert_eq!(err, format!("erro: não consegui gravar {}: [Errno 13] Permission denied\n", env.display()));
    assert_eq!(listed, vec![std::ffi::OsString::from(".env")]);
    assert_eq!(std::fs::read_to_string(&env).unwrap(), "CP_AUTH_TOKEN=x\n");
}

#[test]
fn the_env_comes_from_the_copy_the_binary_belongs_to() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    std::fs::create_dir_all(&home).unwrap();
    let new = home.join(".hangar").join("config").join(".env");
    // Pacote: `<versão>/bin/hangar-server` com `manifest.json` na versão.
    let version = dir.path().join("versions").join("1");
    std::fs::create_dir_all(version.join("bin")).unwrap();
    std::fs::write(version.join("manifest.json"), "{}").unwrap();
    let request = Request { home: home.clone(), exe: Some(version.join("bin").join("hangar-server")), ..Request::default() };
    assert_eq!(vapid::env_path(&request).unwrap(), new);
    // Checkout não migrado: o binário do build acha a `backend/` acima dele.
    let checkout = dir.path().join("repo");
    std::fs::create_dir_all(checkout.join("backend").join("app")).unwrap();
    std::fs::create_dir_all(checkout.join("crates").join("target").join("release")).unwrap();
    let exe = checkout.join("crates").join("target").join("release").join("hangar-server");
    let request = Request { home: home.clone(), exe: Some(exe), ..Request::default() };
    assert_eq!(vapid::env_path(&request).unwrap(), checkout.join("backend").join(".env"));
    // `--root` vale como o antigo cwd `backend/`; `--env` vence tudo.
    let request = Request { home: home.clone(), root: Some(checkout.clone()), ..Request::default() };
    assert_eq!(vapid::env_path(&request).unwrap(), checkout.join("backend").join(".env"));
    let request = Request { home: home.clone(), root: Some(checkout.clone()), env: Some(dir.path().join("x")), ..Request::default() };
    assert_eq!(vapid::env_path(&request).unwrap(), dir.path().join("x"));
    // Máquina migrada (`~/.hangar/current`): o lugar novo, mesmo sem o arquivo.
    std::fs::create_dir_all(home.join(".hangar").join("current")).unwrap();
    let request = Request { home: home.clone(), root: Some(checkout.clone()), ..Request::default() };
    assert_eq!(vapid::env_path(&request).unwrap(), new);
    // Sem raiz nenhuma: o lugar novo.
    assert_eq!(vapid::env_path(&Request { home, ..Request::default() }).unwrap(), new);
}

#[test]
fn arguments_are_env_and_root_only() {
    let args = |list: &[&str]| vapid::parse_args(&list.iter().map(|s| s.to_string()).collect::<Vec<_>>());
    assert_eq!(args(&["--env=/a", "--root", "/b"]).unwrap(), (Some("/a".into()), Some("/b".into())));
    assert_eq!(args(&[]).unwrap(), (None, None));
    assert!(args(&["--root"]).is_err());
    assert!(args(&["--env="]).is_err());
    assert!(args(&["--home", "/x"]).is_err());
}

#[test]
fn utf8_errors_read_like_python() {
    let check = |bytes: &[u8], want: &str| {
        let error = std::str::from_utf8(bytes).unwrap_err();
        assert_eq!(utf8_error(bytes, &error), want);
    };
    check(b"CP_AUTH_TOKEN=\xff\n", "'utf-8' codec can't decode byte 0xff in position 14: invalid start byte");
    check(b"a\xe2\x28", "'utf-8' codec can't decode byte 0xe2 in position 1: invalid continuation byte");
    check(b"a\xe2\x82", "'utf-8' codec can't decode bytes in position 1-2: unexpected end of data");
}

#[test]
fn the_tailscale_name_needs_a_text_dns_name() {
    let name = |text: &str| tailscale::dns_name(text.as_bytes());
    assert_eq!(name(r#"{"Self": {"DNSName": "m.ts.net.."}}"#).as_deref(), Some("m.ts.net"));
    assert_eq!(name(r#"{"Self": {"DNSName": "..."}}"#), None);
    assert_eq!(name(r#"{"Self": {"DNSName": 3}}"#), None);
    assert_eq!(name(r#"{"Self": []}"#), None);
    assert_eq!(name("\u{feff}{\"Self\": {\"DNSName\": \"m\"}}"), None);
    let (mut out, mut err) = (Vec::new(), Vec::new());
    assert_eq!(tailscale::run(&["--json".to_owned()], b"{}", &mut out, &mut err), 2);
    assert!(out.is_empty() && !err.is_empty());
}
