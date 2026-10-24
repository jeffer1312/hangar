use crate::common;
use hangar_server::groups::model::{Fed, GroupRecord, Member, OutsideRef, Sidecar};
use hangar_server::groups::store::{IdentityChanged, PairDir, file_stem};
use serde_json::Value;

fn golden_dir() -> std::path::PathBuf { common::contract().join("golden/pair_sidecars") }

fn fresh_dir(tmp: &tempfile::TempDir) -> PairDir {
    PairDir::new(tmp.path().to_path_buf(), tmp.path().join("arquivo"))
}

#[test]
fn reads_what_python_wrote() {
    let tmp = tempfile::tempdir().unwrap();
    for f in std::fs::read_dir(golden_dir()).unwrap() {
        let f = f.unwrap().path();
        if f.extension().is_some_and(|e| e == "json") && f.file_name().unwrap() != "expected.json" {
            std::fs::copy(&f, tmp.path().join(f.file_name().unwrap())).unwrap();
        }
    }
    let dir = PairDir::new(tmp.path().to_path_buf(), tmp.path().join("arquivo"));
    let expected: Value = serde_json::from_str(&std::fs::read_to_string(golden_dir().join("expected.json")).unwrap()).unwrap();
    assert!(expected.as_object().unwrap().len() >= 6, "o golden perdeu casos");
    for (name, want) in expected.as_object().unwrap() {
        let got = dir.sidecar(name).unwrap();
        match want {
            Value::Null => assert!(got.is_none(), "{name}"),
            w => {
                let got = got.unwrap();
                let peers: Vec<String> = w["peers"].as_array().unwrap().iter().map(|p| p.as_str().unwrap().to_owned()).collect();
                assert_eq!(got.peers, peers, "{name}");
                assert_eq!(got.gid, w["gid"].as_str().unwrap(), "{name}: gid legado igual ao do Python");
                assert_eq!(got.orq, w["orq"].as_bool().unwrap(), "{name}");
                assert_eq!(got.task.as_deref(), w["task"].as_str(), "{name}");
                let harness: std::collections::BTreeMap<String, String> =
                    w["harness"].as_object().unwrap().iter().map(|(k, v)| (k.clone(), v.as_str().unwrap().to_owned())).collect();
                assert_eq!(got.harness, harness, "{name}");
            }
        }
    }
}

#[test]
fn sidecar_with_fed_is_read_and_python_formats_are_not_confused_for_it() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::copy(golden_dir().join("fed.json"), tmp.path().join("fed.json")).unwrap();
    std::fs::copy(golden_dir().join("a.json"), tmp.path().join("a.json")).unwrap();
    let dir = fresh_dir(&tmp);
    let fed = dir.sidecar("fed").unwrap().unwrap();
    assert_eq!(fed.fed, Some(Fed { owner: "casa".into(), local: true, version: 7 }));
    assert_eq!(fed.peers, ["b", "lab::c"]);
    assert_eq!(dir.sidecar("a").unwrap().unwrap().fed, None, "o Python de hoje não grava fed");
}

#[test]
fn fed_round_trip_and_python_shape() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = PairDir::new(tmp.path().to_path_buf(), tmp.path().join("arquivo"));
    let s = Sidecar {
        peers: vec!["b".into(), "lab::c".into()], task: Some("t".into()), gid: "ab12cd34".into(),
        harness: [("a".into(), "claude".into()), ("fora".into(), "pi".into())].into(),
        orq: false, fed: Some(Fed { owner: "casa".into(), local: true, version: 7 }), linked_by: Some("b".into()), lead: Some("a".into()),
    };
    dir.write_sidecar("a", &s).unwrap();
    let raw: Value = serde_json::from_str(&std::fs::read_to_string(tmp.path().join("a.json")).unwrap()).unwrap();
    assert!(raw.get("orq").is_none(), "orq só aparece quando é true, como o PairLink.set");
    assert!(raw["harness"].get("fora").is_none(), "harness só com membros do grupo, como o PairLink.set");
    assert_eq!(dir.sidecar("a").unwrap().unwrap().fed, s.fed);
    assert_eq!(raw["linked_by"], "b");
    assert_eq!(dir.sidecar("a").unwrap().unwrap().linked_by, s.linked_by, "linked_by volta igual da leitura");
    assert_eq!(raw["lead"], "a");
    assert_eq!(dir.sidecar("a").unwrap().unwrap().lead, s.lead, "lead volta igual da leitura");
    assert!(!tmp.path().join("a.json.tmp").exists(), "o temporário não fica");
}

#[test]
fn write_replaces_the_whole_file_and_clear_removes_it() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = fresh_dir(&tmp);
    let mut s = Sidecar { peers: vec!["b".into()], task: Some("um".into()), gid: "ab12cd34".into(), ..Default::default() };
    dir.write_sidecar("nome com espaço", &s).unwrap();
    assert!(tmp.path().join("nome-com-espa-o.json").is_file(), "o arquivo leva o nome saneado");
    s.task = Some("dois".into());
    dir.write_sidecar("nome com espaço", &s).unwrap();
    assert_eq!(dir.sidecar("nome com espaço").unwrap().unwrap().task.as_deref(), Some("dois"));
    // Resto de uma escrita interrompida some junto com o sidecar, como no PairLink.clear.
    std::fs::write(tmp.path().join("nome-com-espa-o.json.tmp"), "{").unwrap();
    dir.clear_sidecar("nome com espaço").unwrap();
    assert!(dir.sidecar("nome com espaço").unwrap().is_none());
    assert!(!tmp.path().join("nome-com-espa-o.json.tmp").exists());
    dir.clear_sidecar("nome com espaço").unwrap();
}

#[test]
fn missing_is_none_and_broken_syntax_is_an_error_for_the_caller_to_log() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = fresh_dir(&tmp);
    assert!(dir.sidecar("nao-existe").unwrap().is_none());
    std::fs::write(tmp.path().join("torto.json"), "{").unwrap();
    assert!(dir.sidecar("torto").is_err(), "JSON torto chega ao chamador, que o trata como sem grupo");
}

/// PAIR-07/08/09: harness que não é objeto vale vazio, `task: null` fica nulo, `orq` só com `true` literal.
#[test]
fn parse_edge_values_like_python() {
    let parse = |v: Value| Sidecar::parse("a", v.as_object().unwrap());
    let st = parse(serde_json::json!({"peers": ["b"], "task": null, "harness": "claude"})).unwrap();
    assert_eq!((st.task, st.harness.len()), (None, 0));
    assert_eq!(parse(serde_json::json!({"peers": ["b"]})).unwrap().task.as_deref(), Some(""));
    for orq in [serde_json::json!("true"), serde_json::json!(1)] {
        assert!(parse(serde_json::json!({"peers": [], "orq": orq})).is_none(), "{orq}");
    }
    assert!(parse(serde_json::json!({"peers": [], "orq": true})).unwrap().orq);
    // Diferença aceita (I-1A): valor de harness que não é texto sai.
    let st = parse(serde_json::json!({"peers": ["b"], "harness": {"a": "claude", "b": 1}})).unwrap();
    assert_eq!(st.harness.into_iter().collect::<Vec<_>>(), [("a".to_owned(), "claude".to_owned())]);
}

/// PAIR-05: nome vazio na lista de peers some; o resto fica.
#[test]
fn empty_peer_names_are_dropped() {
    let st = Sidecar::parse("a", serde_json::json!({"peers": ["", "b"]}).as_object().unwrap()).unwrap();
    assert_eq!(st.peers, ["b"]);
}

/// PAIR-06: os dois lados de um par legado derivam o mesmo gid, e ler não regrava o arquivo.
#[test]
fn legacy_pair_derives_one_gid_and_reading_never_writes() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = fresh_dir(&tmp);
    std::fs::write(tmp.path().join("a.json"), r#"{"peer": "b"}"#).unwrap();
    std::fs::write(tmp.path().join("b.json"), r#"{"peer": "a"}"#).unwrap();
    let stamp = |n: &str| { let p = tmp.path().join(n); (std::fs::read(&p).unwrap(), std::fs::metadata(&p).unwrap().modified().unwrap()) };
    let before = (stamp("a.json"), stamp("b.json"));
    let (a, b) = (dir.sidecar("a").unwrap().unwrap(), dir.sidecar("b").unwrap().unwrap());
    assert_eq!(a.gid, b.gid);
    assert_eq!(a.gid.len(), 8);
    let listed = dir.sidecars().unwrap();
    assert!(listed.iter().all(|(_, s)| s.gid == a.gid), "{listed:?}");
    assert_eq!((stamp("a.json"), stamp("b.json")), before);
}

/// PAIR-11 no Windows: um leitor com o sidecar aberto (só FILE_SHARE_READ) não derruba a troca.
#[cfg(windows)]
#[test]
fn write_waits_for_a_reader_holding_the_sidecar_on_windows() {
    use std::os::windows::fs::OpenOptionsExt;
    let tmp = tempfile::tempdir().unwrap();
    let dir = fresh_dir(&tmp);
    let mut s = Sidecar { peers: vec!["b".into()], task: Some("um".into()), gid: "ab12cd34".into(), ..Default::default() };
    dir.write_sidecar("a", &s).unwrap();
    let reader = std::fs::OpenOptions::new().read(true).share_mode(0x1).open(tmp.path().join("a.json")).unwrap();
    let release = std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(60));
        drop(reader);
    });
    s.task = Some("dois".into());
    dir.write_sidecar("a", &s).unwrap();
    release.join().unwrap();
    assert_eq!(dir.sidecar("a").unwrap().unwrap().task.as_deref(), Some("dois"));
}

#[test]
fn file_stem_matches_python_sanitize() {
    assert_eq!(file_stem("nome com espaço"), "nome-com-espa-o");
    assert_eq!(file_stem("ok_1.2-x"), "ok_1.2-x");
}

#[test]
fn sidecars_skips_registry_files() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = fresh_dir(&tmp);
    let s = Sidecar { peers: vec!["b".into()], task: Some(String::new()), gid: "ab12cd34".into(), ..Default::default() };
    dir.write_sidecar("a", &s).unwrap();
    // O registro de pares externos é uma lista, não um membro; groups/ e os contratos não são sidecar.
    std::fs::write(tmp.path().join("external_pairs.json"), "[]").unwrap();
    std::fs::create_dir(tmp.path().join("groups")).unwrap();
    std::fs::write(tmp.path().join("groups/ab12cd34.json"), r#"{"peers":["x"]}"#).unwrap();
    std::fs::write(tmp.path().join("grupo-ab12cd34.md"), "contrato").unwrap();
    // Sem grupo (vazio, ou outro tipo de JSON) fica de fora, como no Python.
    std::fs::write(tmp.path().join("vazio.json"), r#"{"peers":[]}"#).unwrap();
    std::fs::write(tmp.path().join("quebrado.json"), "[1,2]").unwrap();
    std::fs::write(tmp.path().join("torto.json"), "{").unwrap();
    let found = dir.sidecars().unwrap();
    assert_eq!(found.iter().map(|(stem, _)| stem.as_str()).collect::<Vec<_>>(), ["a"]);
    assert_eq!(found[0].1.peers, ["b"]);
}

#[test]
fn sidecars_of_a_missing_dir_is_empty() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = PairDir::new(tmp.path().join("nao-existe"), tmp.path().join("arquivo"));
    assert!(dir.sidecars().unwrap().is_empty());
}

#[test]
fn merge_contract_appends_the_loser_and_removes_it() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = fresh_dir(&tmp);
    let at = |f: &str| tmp.path().join(f);
    std::fs::write(at("grupo-perde.md"), "  decisão A\n").unwrap();
    std::fs::write(at("grupo-fica.md"), "decisão B").unwrap();
    std::fs::write(at("regras-perde.md"), "regra A").unwrap();
    // `regras-fica.md` não existe: o perdedor vira o arquivo do sobrevivente.
    dir.merge_contract("perde", "fica");
    assert_eq!(
        std::fs::read_to_string(at("grupo-fica.md")).unwrap(),
        "decisão B\n\n## Contrato herdado do grupo perde (merge)\n\ndecisão A\n"
    );
    assert_eq!(
        std::fs::read_to_string(at("regras-fica.md")).unwrap(),
        "\n\n## Contrato herdado do grupo perde (merge)\n\nregra A\n"
    );
    assert!(!at("grupo-perde.md").exists() && !at("regras-perde.md").exists());
    // Sem contrato do perdedor não há o que fazer, e o do sobrevivente fica como está.
    let before = std::fs::read_to_string(at("grupo-fica.md")).unwrap();
    dir.merge_contract("outro", "fica");
    assert_eq!(std::fs::read_to_string(at("grupo-fica.md")).unwrap(), before);
    // Contrato só com espaço não é herdado, e o arquivo vazio do perdedor continua onde estava.
    std::fs::write(at("grupo-branco.md"), " \n").unwrap();
    dir.merge_contract("branco", "fica");
    assert_eq!(std::fs::read_to_string(at("grupo-fica.md")).unwrap(), before);
    assert!(at("grupo-branco.md").exists());
}

#[test]
fn archive_contracts_moves_both_files_with_a_local_timestamp() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = fresh_dir(&tmp);
    std::fs::write(dir.contract_path("ab12cd34"), "decisões").unwrap();
    std::fs::write(tmp.path().join("regras-ab12cd34.md"), "regras").unwrap();
    std::fs::write(tmp.path().join("grupo-outro.md"), "de outro grupo").unwrap();
    dir.archive_contracts("ab12cd34");
    assert!(!dir.contract_path("ab12cd34").exists() && !tmp.path().join("regras-ab12cd34.md").exists());
    assert!(tmp.path().join("grupo-outro.md").exists(), "só o contrato do grupo vai");
    let mut moved: Vec<String> = std::fs::read_dir(tmp.path().join("arquivo")).unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
    moved.sort();
    assert_eq!(moved.len(), 2);
    for (name, prefix) in moved.iter().zip(["grupo", "regras"]) {
        let stamp = name.strip_prefix(&format!("{prefix}-ab12cd34-")).and_then(|s| s.strip_suffix(".md")).unwrap_or_else(|| panic!("{name}"));
        // AAAAmmdd-HHMMSS
        assert_eq!(stamp.len(), 15, "{name}");
        assert_eq!(stamp.as_bytes()[8], b'-', "{name}");
        assert!(stamp.bytes().enumerate().all(|(i, b)| i == 8 || b.is_ascii_digit()), "{name}");
    }
    assert_eq!(std::fs::read_to_string(tmp.path().join("arquivo").join(&moved[0])).unwrap(), "decisões");
    // Sem contrato, nada a fazer e nem a pasta de arquivo nasce.
    let other = tempfile::tempdir().unwrap();
    fresh_dir(&other).archive_contracts("zzzz");
    assert!(!other.path().join("arquivo").exists());
}

fn record(gid: &str, version: u64) -> GroupRecord {
    GroupRecord {
        protocol: 1, gid: gid.into(), owner: "casa".into(), version, task: "t".into(),
        members: vec![Member { server: "casa".into(), name: "a".into(), provider: "claude".into() },
                      Member { server: "lab".into(), name: "a".into(), provider: String::new() }],
        outside: vec![OutsideRef { server: "casa".into(), address: "ana::x".into() }],
    }
}

#[test]
fn group_record_round_trip_lives_under_groups() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = fresh_dir(&tmp);
    assert_eq!(dir.record("ab12cd34").unwrap(), None);
    dir.write_record(&record("ab12cd34", 3)).unwrap();
    dir.write_record(&record("ff00ff00", 1)).unwrap();
    assert!(tmp.path().join("groups/ab12cd34.json").is_file());
    assert_eq!(dir.record("ab12cd34").unwrap(), Some(record("ab12cd34", 3)));
    // A fila e a identidade moram na mesma pasta e não são registro.
    std::fs::write(tmp.path().join("groups/outbox.json"), "[]").unwrap();
    std::fs::write(tmp.path().join("groups/self.json"), r#"{"server_id":"casa"}"#).unwrap();
    std::fs::write(tmp.path().join("groups/torto.json"), "{").unwrap();
    let gids: Vec<String> = dir.records().unwrap().into_iter().map(|r| r.gid).collect();
    assert_eq!(gids, ["ab12cd34", "ff00ff00"]);
    dir.remove_record("ab12cd34").unwrap();
    dir.remove_record("ab12cd34").unwrap();
    assert_eq!(dir.record("ab12cd34").unwrap(), None);
    // Registro sem `outside` (protocolo 1 antes do campo) ainda lê.
    std::fs::write(tmp.path().join("groups/old.json"),
        r#"{"protocol":1,"gid":"old","owner":"casa","version":1,"members":[]}"#).unwrap();
    assert_eq!(dir.record("old").unwrap().unwrap().outside, []);
}

fn fed_sidecar(owner: &str, local: bool) -> Sidecar {
    Sidecar { peers: vec!["lab::b".into()], task: Some(String::new()), gid: "ab12cd34".into(),
              fed: Some(Fed { owner: owner.into(), local, version: 2 }), ..Default::default() }
}

#[test]
fn server_id_change_with_live_group_is_refused() {
    for (owner, local) in [("casa", true), ("lab", false)] {
        let tmp = tempfile::tempdir().unwrap();
        let dir = fresh_dir(&tmp);
        assert_eq!(dir.self_id().unwrap(), None);
        dir.remember_self("casa").unwrap();
        assert_eq!(dir.self_id().unwrap().as_deref(), Some("casa"));
        // Sem grupo entre máquinas vivo, trocar o id não prende ninguém.
        assert_eq!(dir.identity_check("casa2").unwrap(), None);
        dir.write_sidecar("a", &fed_sidecar(owner, local)).unwrap();
        assert_eq!(dir.identity_check("casa").unwrap(), None, "{owner}");
        assert_eq!(dir.identity_check("casa2").unwrap(),
                   Some(IdentityChanged { saved: "casa".into(), current: "casa2".into() }), "{owner}: participante incluída");
    }
    // Grupo vivo sem `self.json` (nunca gravado) não tem com o que comparar: não recusa.
    let tmp = tempfile::tempdir().unwrap();
    let dir = fresh_dir(&tmp);
    dir.write_sidecar("a", &fed_sidecar("casa", true)).unwrap();
    assert_eq!(dir.identity_check("outra").unwrap(), None);
}

mod disk_proptest {
    use super::*;
    use proptest::prelude::*;

    fn json_ish() -> impl Strategy<Value = Vec<u8>> {
        prop_oneof![
            proptest::collection::vec(any::<u8>(), 0..64),
            "[{}\\[\\]\":,a-z0-9 ]{0,64}".prop_map(String::into_bytes),
            Just(br#"{"server_id":7}"#.to_vec()),
            Just(br#"{"protocol":1,"gid":"g","owner":"o","version":-1,"members":[{}]}"#.to_vec()),
            Just(br#"{"protocol":1,"gid":"g","owner":"o","version":1,"members":[],"outside":[{"server":1}]}"#.to_vec()),
        ]
    }

    proptest! {
        #[test]
        fn record_and_self_from_disk_never_panic(raw in json_ish()) {
            let tmp = tempfile::tempdir().unwrap();
            let dir = fresh_dir(&tmp);
            std::fs::create_dir_all(tmp.path().join("groups")).unwrap();
            std::fs::write(tmp.path().join("groups/g.json"), &raw).unwrap();
            std::fs::write(tmp.path().join("groups/self.json"), &raw).unwrap();
            let _ = dir.record("g");
            let _ = dir.records();
            let _ = dir.self_id();
            let _ = dir.identity_check("x");
        }
    }
}
