//! O `vault_fill`: a senha só entra numa página do site da entrada, e só por ref de snapshot.
use super::{host_of, ref_ok, same_site};

#[test]
fn the_page_host_comes_without_scheme_user_port_or_path() {
    assert_eq!(host_of("https://Login.GitHub.com:443/session?x=1#f").as_deref(), Some("login.github.com"));
    assert_eq!(host_of("http://user:pw@exemplo.com.br./a").as_deref(), Some("exemplo.com.br"));
    assert_eq!(host_of("github.com/login").as_deref(), Some("github.com"), "entrada cadastrada sem esquema");
    assert_eq!(host_of("http://[::1]:8080/").as_deref(), Some("::1"));
    assert_eq!(host_of(""), None);
}

#[test]
fn a_page_of_another_domain_is_refused() {
    assert!(same_site("https://github.com", "github.com"));
    assert!(same_site("https://github.com/login", "gist.github.com"), "subdomínio do site da entrada");
    assert!(same_site("https://www.github.com", "github.com"), "o www. da entrada não conta");
    assert!(!same_site("https://github.com", "github.com.evil.io"));
    assert!(!same_site("https://github.com", "evilgithub.com"));
    assert!(!same_site("https://gist.github.com", "github.com"), "o site pai não é o subdomínio cadastrado");
    assert!(!same_site("", "github.com"));
}

#[test]
fn only_a_snapshot_ref_is_a_target() {
    assert!(ref_ok("@e12"));
    for bad in ["@e", "e12", "@e1x", "#senha", "@e1 @e2"] { assert!(!ref_ok(bad), "{bad}"); }
}
