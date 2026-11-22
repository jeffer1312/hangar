//! O que se grava no container do SideStore: arquivos de pareamento em `Documents/`, por versão do
//! iOS, e as preferências de anisette.
use plist::Dictionary;

use super::DeviceError;

pub const ALT_FILE: &str = "ALTPairingFile.mobiledevicepairing";
pub const REMOTE_FILE: &str = "PairingFile_RemoteRP.plist";
pub const LOCKDOWN_FILE: &str = "PairingFile_Lockdown.plist";

/// `(maior, menor)` de "26.5" ou "17.4.1"; pedaço ilegível vale 0.
pub fn version(ios: &str) -> (u32, u32) {
    let mut parts = ios.split('.').map(|x| x.parse::<u32>().unwrap_or(0));
    (parts.next().unwrap_or(0), parts.next().unwrap_or(0))
}

/// iOS 17.4 em diante precisa do RPPairing.
pub fn needs_rppairing(ios: &str) -> bool {
    version(ios) >= (17, 4)
}

/// Nome e conteúdo de cada arquivo. `rp` é obrigatório a partir do 17.4.
pub fn files(ios: &str, lockdown: &Dictionary, rp: Option<&Dictionary>) -> Result<Vec<(&'static str, Vec<u8>)>, DeviceError> {
    if ios.split('.').next().and_then(|major| major.parse::<u32>().ok()).is_none() {
        return Err(DeviceError::Other(format!("unreadable iOS version {ios:?}")));
    }
    if !needs_rppairing(ios) {
        let ld = xml(lockdown)?;
        return Ok(vec![(ALT_FILE, ld.clone()), (LOCKDOWN_FILE, ld)]);
    }
    let rp = rp.ok_or_else(|| DeviceError::Other(format!("rppairing missing for iOS {ios}")))?;
    // No 26.4+ o lockdown não passa pela VPN utun e, presente, o SideStore o carrega primeiro.
    if version(ios) >= (26, 4) {
        return Ok(vec![(REMOTE_FILE, xml(rp)?)]);
    }
    let mut both = lockdown.clone();
    both.extend(rp.clone());
    Ok(vec![(ALT_FILE, xml(&both)?), (REMOTE_FILE, xml(rp)?), (LOCKDOWN_FILE, xml(lockdown)?)])
}

fn xml(d: &Dictionary) -> Result<Vec<u8>, DeviceError> {
    let mut out = Vec::new();
    plist::to_writer_xml(&mut out, d).map_err(|e| DeviceError::Other(e.to_string()))?;
    Ok(out)
}

/// Anisette que o SideStore usa no aparelho; sem isso o login dele cai no -80045 da Apple. O
/// `mac-ani` é o mesmo que o login do Hangar usa primeiro (`apple::ANISETTE`).
fn sidestore_prefs() -> [(&'static str, plist::Value); 3] {
    [
        ("useOnDeviceAnisette", false.into()),
        ("menuAnisetteURL", "https://mac-ani.jkcoxson.com".into()),
        ("disableAnisetteRotation", true.into()),
    ]
}

pub const ANISETTE_CONFIG_FILE: &str = "/Library/anisette-config.json";

/// Cabeçalho que o SideStore manda à Apple no login. Com `macOS;` a Apple responde 503 e, com a
/// versão antiga do sistema, -22410; o padrão do SideStore 0.7.0 cai nos dois.
pub fn sidestore_anisette_config() -> Vec<u8> {
    serde_json::to_vec_pretty(&serde_json::json!({
        "clientInfo": "<MacBookPro13,3> <Mac OS X;26.6;25F84> <com.apple.AuthKit/1 (com.apple.akd/1.0)>",
        "userAgent": "akd/1.0 CFNetwork/811.5.4 Darwin/16.7.0",
    }))
    .expect("json fixo")
}

/// Grava as três chaves por cima do plist de preferências que já existe; vazio vira dict novo.
pub fn merge_sidestore_prefs(existing: &[u8]) -> Result<Vec<u8>, DeviceError> {
    // Arquivo ilegível é erro, nunca sobrescrito: apagaria as outras preferências da pessoa.
    let mut prefs: Dictionary = if existing.is_empty() {
        Dictionary::new()
    } else {
        plist::from_bytes(existing).map_err(|e| DeviceError::Other(format!("sidestore prefs: {e}")))?
    };
    for (key, value) in sidestore_prefs() {
        prefs.insert(key.into(), value);
    }
    xml(&prefs)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn expected_prefs() -> Dictionary {
        let mut d = Dictionary::new();
        d.insert("useOnDeviceAnisette".into(), false.into());
        d.insert("menuAnisetteURL".into(), "https://mac-ani.jkcoxson.com".into());
        d.insert("disableAnisetteRotation".into(), true.into());
        d
    }

    #[test]
    fn prefs_on_empty_file_are_the_three_keys() {
        assert_eq!(parsed(&merge_sidestore_prefs(b"").unwrap()), expected_prefs());
    }

    #[test]
    fn prefs_override_old_values_and_keep_other_keys() {
        let mut old = Dictionary::new();
        old.insert("useOnDeviceAnisette".into(), true.into());
        old.insert("menuAnisetteURL".into(), "https://ani.sidestore.io".into());
        old.insert("preferredServerID".into(), "5".into());
        let mut bin = Vec::new();
        plist::to_writer_binary(&mut bin, &old).unwrap();
        let mut want = expected_prefs();
        want.insert("preferredServerID".into(), "5".into());
        assert_eq!(parsed(&merge_sidestore_prefs(&bin).unwrap()), want);
    }

    // A Apple responde 503 a `macOS;` no X-Mme-Client-Info; `Mac OS X;` passa.
    #[test]
    fn anisette_config_identifies_as_mac_os_x() {
        let v: serde_json::Value = serde_json::from_slice(&sidestore_anisette_config()).unwrap();
        let client = v["clientInfo"].as_str().unwrap();
        assert!(client.contains("<Mac OS X;") && !client.contains("macOS;"), "{client}");
        assert!(!v["userAgent"].as_str().unwrap().is_empty());
    }

    #[test]
    fn prefs_that_are_not_a_dict_are_an_error() {
        assert!(merge_sidestore_prefs(b"not a plist").is_err());
    }

    fn dict(pairs: &[(&str, &str)]) -> Dictionary {
        pairs.iter().map(|(k, v)| (k.to_string(), plist::Value::from(*v))).collect()
    }
    fn lockdown() -> Dictionary { dict(&[("HostID", "H"), ("UDID", "U")]) }
    fn rp() -> Dictionary { dict(&[("public_key", "P"), ("identifier", "I")]) }
    fn parsed(b: &[u8]) -> Dictionary { plist::from_bytes(b).unwrap() }

    fn names(v: &[(&'static str, Vec<u8>)]) -> Vec<&'static str> { v.iter().map(|(n, _)| *n).collect() }

    #[test]
    fn version_parses_major_minor() {
        assert_eq!(version("26.5"), (26, 5));
        assert_eq!(version("17.4.1"), (17, 4));
        assert_eq!(version("18"), (18, 0));
        assert_eq!(version("?"), (0, 0));
    }

    #[test]
    fn rppairing_from_17_4() {
        assert!(!needs_rppairing("17.3.1"));
        assert!(needs_rppairing("17.4"));
        assert!(needs_rppairing("26.5"));
    }

    // SideStore novo recusa o combinado como "ambiguous" e, com o lockdown presente, falha no 26.4+.
    #[test]
    fn ios_26_4_and_later_get_only_remote_rp() {
        for ios in ["26.4", "26.5", "27.0"] {
            let f = files(ios, &lockdown(), Some(&rp())).unwrap();
            assert_eq!(names(&f), vec![REMOTE_FILE], "{ios}");
            assert_eq!(parsed(&f[0].1), rp());
        }
    }

    #[test]
    fn ios_17_4_to_26_3_get_three_files() {
        for ios in ["17.4", "18.2", "26.3.1"] {
            let f = files(ios, &lockdown(), Some(&rp())).unwrap();
            assert_eq!(names(&f), vec![ALT_FILE, REMOTE_FILE, LOCKDOWN_FILE], "{ios}");
            let mut both = lockdown();
            both.extend(rp());
            assert_eq!(parsed(&f[0].1), both);
            assert_eq!(parsed(&f[1].1), rp());
            assert_eq!(parsed(&f[2].1), lockdown());
        }
    }

    #[test]
    fn before_17_4_gets_lockdown_only() {
        for ios in ["16.7", "17.3.1"] {
            let f = files(ios, &lockdown(), None).unwrap();
            assert_eq!(names(&f), vec![ALT_FILE, LOCKDOWN_FILE], "{ios}");
            assert_eq!(parsed(&f[0].1), lockdown());
            assert_eq!(parsed(&f[1].1), lockdown());
        }
    }

    // Versão ilegível cairia na faixa < 17.4 e gravaria o lockdown num iOS 26.
    #[test]
    fn unreadable_version_is_refused() {
        for ios in ["", "?", ".5"] {
            assert!(matches!(files(ios, &lockdown(), Some(&rp())), Err(DeviceError::Other(_))), "{ios:?}");
        }
    }

    #[test]
    fn missing_rppairing_is_an_error() {
        assert!(matches!(files("26.5", &lockdown(), None), Err(DeviceError::Other(_))));
    }
}
