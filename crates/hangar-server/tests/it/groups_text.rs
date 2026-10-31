//! Texto do protocolo por variante, mais grupo com par de fora e a linha do contrato por tipo de membro.
use std::collections::BTreeMap;
use std::path::PathBuf;

use hangar_server::groups::text::{ContractLine, Kind, OutsidePeer, TextArgs, render};

#[path = "groups_text_cases.rs"]
mod cases;
use cases::*;

/// `contract_remote` antigo não trazia dono: o caminho segue como contrato local (caso `group_contract_remote_ignored`).
fn args_of(case: &Case) -> TextArgs {
    let text = |s: &str| s.to_owned();
    TextArgs {
        me: text(case.me),
        others: case.others.iter().map(|x| text(x)).collect(),
        task: text(case.task),
        harness: case.harness.iter().map(|(k, v)| (text(k), text(v))).collect(),
        contract: ContractLine { path: case.contract.map(PathBuf::from), remote_owner: None },
        peer: text(case.peer),
        owner: text(case.owner),
        ..Default::default()
    }
}

fn case(name: &str) -> &'static Case {
    CASES.iter().find(|c| c.name == name).unwrap()
}

#[test]
fn render_matches_every_text() {
    assert_eq!(CASES.len(), 18);
    for c in CASES {
        assert_eq!(render(c.kind, &args_of(c)), c.text, "{}", c.name);
    }
}

/// Os separadores \x1c..\x1f também saem da tarefa, embora `char::is_whitespace` não os conheça.
#[test]
fn task_trim_includes_control_separators() {
    let mut args = args_of(case("group_with_task"));
    args.task = "\u{1f}\u{3000} T-1 filtro\u{a0}\u{1c}".into();
    assert_eq!(render(case("group_with_task").kind, &args), TEXT_GROUP_WITH_TASK);
}

/// Membro local de grupo que tem par de fora recebe o texto de grupo seguido do bloco externo.
#[test]
fn mixed_group_gets_group_and_external_text() {
    let c = case("group_with_contract");
    let mut args = args_of(c);
    args.outside = vec![OutsidePeer { address: "ana::rev".into(), owner: "Ana".into() }];
    assert_eq!(render(c.kind, &args), format!("{}\n{}", c.text, TEXT_EXTERNAL));
}

/// Na dona, o caminho; em outra máquina do dono, a dona pela tool `contract`/`hangar-send --contrato`;
/// membro de fora, sem linha.
#[test]
fn contract_line_by_member_kind() {
    let c = case("group_with_contract");
    let without = case("group_without_contract").text;
    let mut args = args_of(c);
    assert_eq!(render(c.kind, &args), c.text, "dona");

    args.contract = ContractLine { path: None, remote_owner: Some("casa".into()) };
    let remote = "- Decisões que o grupo consulta ficam com a máquina `casa`: leia e grave pela tool `contract` do MCP ou `hangar-send --contrato`.\n";
    let invite = "- Convite de par";
    assert_eq!(render(c.kind, &args), without.replacen(invite, &format!("{remote}{invite}"), 1), "máquina própria");

    args.contract = ContractLine::default();
    assert_eq!(render(c.kind, &args), without, "de fora");
}

/// Grupo entre máquinas: uma linha a mais para quem não lê o CLAUDE.md (Codex, Pi, omp, Kimi).
#[test]
fn across_machines_group_explains_relayed_outside_notice() {
    let c = case("group_with_contract");
    let mut args = args_of(c);
    args.across = true;
    let line = "- Aviso [de fora: <apelido>::<sessão> via <máquina>] veio de fora por outra máquina do grupo: se precisar responder, \
        só com um aviso de grupo, uma vez.\n";
    let invite = "- Decisões que o grupo consulta";
    assert_eq!(render(c.kind, &args), c.text.replacen(invite, &format!("{line}{invite}"), 1));
}

proptest::proptest! {
    #[test]
    fn render_never_panics(me in "\\PC*", others in proptest::collection::vec("\\PC*", 0..3), task in "\\PC*",
                           peer in "\\PC*", provider in "\\PC*") {
        let harness: BTreeMap<String, String> = [(me.clone(), provider)].into();
        let args = TextArgs { me, others, task, harness, peer: peer.clone(), owner: peer.clone(),
            outside: vec![OutsidePeer { address: peer.clone(), owner: peer }], ..Default::default() };
        for kind in [Kind::Group, Kind::Orq, Kind::External] {
            proptest::prop_assert!(render(kind, &args).starts_with("[painel: grupo de trabalho] "));
        }
    }
}
