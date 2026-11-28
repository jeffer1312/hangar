//! A ordem da escolha: o que prende a sessão na máquina local, as regras do dono e a carga.

use std::cmp::Reverse;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use hangar_core::project::ProjectId;

use crate::rules::PlacementConfig;

/// O que uma máquina responde em `GET /api/placement/facts`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct MachineFacts {
    pub server_id: String,
    pub version: String,
    /// Entende o campo `machine` e a filha ligada pela rede.
    pub capable: bool,
    pub mem_total: u64,
    pub mem_available: u64,
    pub working: u32,
    /// `None` quando a máquina não sabe medir: não reprova.
    pub cpu_pct: Option<f32>,
    pub repos: Vec<KnownRepo>,
    /// Onde a máquina clonaria um repositório que não tem.
    pub clone_dir: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct KnownRepo {
    pub project: ProjectId,
    pub root: String,
}

/// O pedido de abertura visto da máquina que o recebeu. Máquina explícita nunca chega aqui.
#[derive(Clone, Debug, Default)]
pub struct OpenContext {
    pub local_id: String,
    pub cwd_project: Option<ProjectId>,
    pub cwd_root: Option<String>,
    pub branch: Option<String>,
    pub unpublished: Option<Unpublished>,
    /// Máquina de uma sessão aberta na mesma raiz git (a local) ou da reserva recente da pasta.
    pub same_folder_server: Option<String>,
    /// Pasta sem git que não é a pessoal: só existe aqui.
    pub plain_dir_not_home: bool,
    pub agent: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Unpublished {
    Ahead,
    Dirty,
    NoUpstream,
}

/// Por que uma máquina saiu da escolha.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Exclusion {
    /// Desligada ou não respondeu a tempo.
    Missing,
    NotCapable,
    Schedule,
    Memory,
    Working,
    Cpu,
    /// Sem o repositório e sem pasta onde cloná-lo.
    NoCloneDir,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "code", rename_all = "snake_case")]
pub enum ReasonCode {
    SameFolder,
    Unpublished { why: Unpublished },
    PlainDir,
    ProjectRule { project: ProjectId },
    LocalWithinLimits,
    LeastLoaded { local_out: Exclusion },
    NoCandidate,
    /// O pedido ficou fora do balanceamento e abre na local; `choose` nunca devolve. `why` é o código
    /// do servidor (`guest`, `local_field`, `link`, `orq`, `mode_off` ou o da leitura que falhou).
    NotBalanced { why: String },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "code", rename_all = "snake_case")]
pub enum Warning {
    /// A máquina da regra de projeto ficou fora; a escolha seguiu sem ela.
    RuleServerOut { server: String, why: Exclusion },
    /// Mesma pasta ou trabalho não publicado venceu a regra de projeto.
    RuleOverridden { server: String },
    AllOverLimit,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CloneTarget {
    pub dir: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Placement {
    pub server: String,
    pub reason: ReasonCode,
    /// A máquina escolhida não tem o repositório: clonar nesta pasta antes de abrir.
    pub clone: Option<CloneTarget>,
    pub warnings: Vec<Warning>,
}

fn project_rule<'a>(ctx: &OpenContext, config: &'a PlacementConfig) -> Option<&'a crate::rules::ProjectRule> {
    ctx.cwd_project.as_ref().and_then(|p| config.project_rules.iter().find(|r| &r.project == p))
}

/// Mesma pasta, trabalho não publicado e pasta sem git: a escolha sai sem fato nenhum. `None`: nada prende.
pub fn pinned(ctx: &OpenContext, config: &PlacementConfig) -> Option<Placement> {
    let reason = if ctx.same_folder_server.is_some() {
        ReasonCode::SameFolder
    } else if let Some(why) = ctx.unpublished {
        ReasonCode::Unpublished { why }
    } else if ctx.plain_dir_not_home {
        ReasonCode::PlainDir
    } else {
        return None;
    };
    let server = ctx.same_folder_server.clone().unwrap_or_else(|| ctx.local_id.clone());
    let warnings = project_rule(ctx, config)
        .filter(|r| r.server != server)
        .map(|r| Warning::RuleOverridden { server: r.server.clone() })
        .into_iter()
        .collect();
    Some(Placement { server, reason, clone: None, warnings })
}

pub fn choose(ctx: &OpenContext, facts: &[MachineFacts], config: &PlacementConfig, now: DateTime<Utc>) -> Placement {
    if let Some(placement) = pinned(ctx, config) {
        return placement;
    }
    let rule = project_rule(ctx, config);

    let eval = |id: &str| match facts.iter().find(|f| f.server_id == id) {
        Some(f) => check(ctx, f, config, now),
        None => Err(Exclusion::Missing),
    };
    let mut warnings = Vec::new();
    if let Some(rule) = rule {
        match eval(&rule.server) {
            Ok(clone) => {
                return Placement {
                    server: rule.server.clone(),
                    reason: ReasonCode::ProjectRule { project: rule.project.clone() },
                    clone,
                    warnings,
                };
            }
            Err(why) => warnings.push(Warning::RuleServerOut { server: rule.server.clone(), why }),
        }
    }
    let local_out = match eval(&ctx.local_id) {
        Ok(_) => return Placement { server: ctx.local_id.clone(), reason: ReasonCode::LocalWithinLimits, clone: None, warnings },
        Err(why) => why,
    };
    let candidates: Vec<_> = facts.iter().filter_map(|f| check(ctx, f, config, now).ok().map(|clone| (f, clone))).collect();
    // O agente só clona quando nenhuma candidata tem o repositório; a tela mostra o clone e a pessoa decide.
    let has_repo = candidates.iter().any(|(_, clone)| clone.is_none());
    let best = candidates
        .into_iter()
        .filter(|(_, clone)| !(ctx.agent && has_repo && clone.is_some()))
        .min_by_key(|(f, _)| (f.working, Reverse(f.mem_available)));
    match best {
        Some((f, clone)) => {
            Placement { server: f.server_id.clone(), reason: ReasonCode::LeastLoaded { local_out }, clone, warnings }
        }
        None => {
            warnings.push(Warning::AllOverLimit);
            Placement { server: ctx.local_id.clone(), reason: ReasonCode::NoCandidate, clone: None, warnings }
        }
    }
}

/// `Ok` com o clone que a máquina precisaria, ou o motivo de ela sair da escolha.
fn check(
    ctx: &OpenContext,
    f: &MachineFacts,
    config: &PlacementConfig,
    now: DateTime<Utc>,
) -> Result<Option<CloneTarget>, Exclusion> {
    let limits = &config.limits;
    if !f.capable {
        return Err(Exclusion::NotCapable);
    }
    if config.schedule_rules.iter().any(|r| r.avoid == f.server_id && r.covers(now)) {
        return Err(Exclusion::Schedule);
    }
    // Memória total desconhecida (0) não reprova, como o processador sem medida.
    if f.mem_total > 0 && u128::from(f.mem_available) * 100 < u128::from(f.mem_total) * u128::from(limits.mem_free_pct) {
        return Err(Exclusion::Memory);
    }
    if f.working >= limits.working {
        return Err(Exclusion::Working);
    }
    if f.cpu_pct.is_some_and(|c| c > limits.cpu_pct as f32) {
        return Err(Exclusion::Cpu);
    }
    // A pasta do pedido é desta máquina: aqui o repositório sempre existe.
    let Some(project) = ctx.cwd_project.as_ref().filter(|_| f.server_id != ctx.local_id) else {
        return Ok(None);
    };
    if f.repos.iter().any(|r| &r.project == project) {
        return Ok(None);
    }
    config
        .clone_dirs
        .get(&f.server_id)
        .or(f.clone_dir.as_ref())
        .map(|dir| Some(CloneTarget { dir: dir.clone() }))
        .ok_or(Exclusion::NoCloneDir)
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone;

    use super::*;
    use crate::rules::{ProjectRule, ScheduleRule};

    const GB: u64 = 1 << 30;

    fn hangar() -> ProjectId {
        ProjectId("git:github.com/jeffer1312/hangar".into())
    }

    fn machine(id: &str, working: u32, free_gb: u64) -> MachineFacts {
        MachineFacts {
            server_id: id.into(),
            version: "1".into(),
            capable: true,
            mem_total: 32 * GB,
            mem_available: free_gb * GB,
            working,
            cpu_pct: Some(10.0),
            repos: vec![KnownRepo { project: hangar(), root: format!("/home/{id}/hangar") }],
            clone_dir: Some(format!("/home/{id}/Projetos")),
        }
    }

    fn ctx() -> OpenContext {
        OpenContext { local_id: "pc".into(), cwd_project: Some(hangar()), ..Default::default() }
    }

    /// Sexta, 27/11/2026, 15:00 UTC.
    fn now() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 11, 27, 15, 0, 0).unwrap()
    }

    fn rule_to(server: &str) -> PlacementConfig {
        PlacementConfig {
            project_rules: vec![ProjectRule { project: hangar(), server: server.into() }],
            ..Default::default()
        }
    }

    #[test]
    fn same_folder_stays_with_its_session_and_beats_the_project_rule() {
        let facts = [machine("pc", 9, 1), machine("nb", 0, 20)];
        let c = OpenContext { same_folder_server: Some("pc".into()), ..ctx() };
        let p = choose(&c, &facts, &rule_to("nb"), now());
        assert_eq!((p.server.as_str(), &p.reason), ("pc", &ReasonCode::SameFolder));
        assert_eq!(p.warnings, vec![Warning::RuleOverridden { server: "nb".into() }]);
    }

    #[test]
    fn unpublished_work_stays_local() {
        let facts = [machine("pc", 9, 1), machine("nb", 0, 20)];
        let c = OpenContext { unpublished: Some(Unpublished::Ahead), ..ctx() };
        let p = choose(&c, &facts, &PlacementConfig::default(), now());
        assert_eq!((p.server.as_str(), p.reason), ("pc", ReasonCode::Unpublished { why: Unpublished::Ahead }));
        assert!(p.warnings.is_empty(), "sem regra, nada a avisar");
    }

    #[test]
    fn plain_dir_outside_home_stays_local() {
        let facts = [machine("pc", 9, 1), machine("nb", 0, 20)];
        let c = OpenContext { cwd_project: None, plain_dir_not_home: true, ..ctx() };
        let p = choose(&c, &facts, &PlacementConfig::default(), now());
        assert_eq!((p.server.as_str(), p.reason), ("pc", ReasonCode::PlainDir));
    }

    #[test]
    fn each_limit_takes_the_local_out() {
        let cases = [
            (machine("pc", 6, 20), Exclusion::Working),
            (machine("pc", 0, 4), Exclusion::Memory), // 4 de 32 GB = 12,5%
            (MachineFacts { cpu_pct: Some(80.5), ..machine("pc", 0, 20) }, Exclusion::Cpu),
            (MachineFacts { capable: false, ..machine("pc", 0, 20) }, Exclusion::NotCapable),
        ];
        for (local, why) in cases {
            let p = choose(&ctx(), &[local, machine("nb", 2, 20)], &PlacementConfig::default(), now());
            assert_eq!((p.server.as_str(), p.reason), ("nb", ReasonCode::LeastLoaded { local_out: why }));
        }
    }

    #[test]
    fn local_at_the_edge_and_without_cpu_reading_stays() {
        // 5 trabalhando, 15% livre e processador sem medida: dentro dos três limites.
        // 3 de 20 GB é 15% exato; 15% de 32 GB não é inteiro e o corte o deixaria abaixo.
        let local = MachineFacts { cpu_pct: None, mem_total: 20 * GB, mem_available: 3 * GB, ..machine("pc", 5, 0) };
        let p = choose(&ctx(), &[local, machine("nb", 0, 30)], &PlacementConfig::default(), now());
        assert_eq!((p.server.as_str(), p.reason), ("pc", ReasonCode::LocalWithinLimits));
    }

    #[test]
    fn offline_machine_is_missing() {
        let p = choose(&ctx(), &[machine("nb", 1, 20)], &PlacementConfig::default(), now());
        assert_eq!((p.server.as_str(), p.reason), ("nb", ReasonCode::LeastLoaded { local_out: Exclusion::Missing }));
    }

    #[test]
    fn schedule_rule_takes_the_machine_out_in_its_time_zone() {
        // 15:00 UTC é 12:00 em São Paulo.
        let rule = |from: &str, to: &str| PlacementConfig {
            schedule_rules: vec![ScheduleRule {
                days: vec![5],
                from: from.into(),
                to: to.into(),
                tz: "America/Sao_Paulo".into(),
                avoid: "pc".into(),
            }],
            ..Default::default()
        };
        let facts = [machine("pc", 0, 20), machine("nb", 3, 20)];
        let p = choose(&ctx(), &facts, &rule("11:00", "13:00"), now());
        assert_eq!((p.server.as_str(), p.reason), ("nb", ReasonCode::LeastLoaded { local_out: Exclusion::Schedule }));
        let p = choose(&ctx(), &facts, &rule("14:00", "16:00"), now());
        assert_eq!(p.server, "pc", "14h–16h em São Paulo não cobre 12h");
    }

    #[test]
    fn project_rule_picks_its_machine_over_the_lighter_local() {
        let facts = [machine("pc", 0, 30), machine("nb", 4, 10)];
        let p = choose(&ctx(), &facts, &rule_to("nb"), now());
        assert_eq!((p.server.as_str(), p.reason), ("nb", ReasonCode::ProjectRule { project: hangar() }));
        assert!(p.warnings.is_empty());
    }

    #[test]
    fn project_rule_machine_out_falls_through_with_a_warning() {
        let facts = [machine("pc", 0, 30), machine("nb", 7, 10)];
        let p = choose(&ctx(), &facts, &rule_to("nb"), now());
        assert_eq!((p.server.as_str(), p.reason), ("pc", ReasonCode::LocalWithinLimits));
        assert_eq!(p.warnings, vec![Warning::RuleServerOut { server: "nb".into(), why: Exclusion::Working }]);
    }

    #[test]
    fn without_rule_the_fewest_working_wins_then_more_memory() {
        let facts = [machine("pc", 8, 20), machine("vm", 2, 6), machine("nb", 1, 6), machine("srv", 1, 9)];
        let p = choose(&ctx(), &facts, &PlacementConfig::default(), now());
        assert_eq!(p.server, "srv");
    }

    #[test]
    fn no_candidate_stays_local_with_a_warning() {
        let facts = [machine("pc", 8, 20), machine("nb", 6, 20)];
        let p = choose(&ctx(), &facts, &PlacementConfig::default(), now());
        assert_eq!((p.server.as_str(), p.reason), ("pc", ReasonCode::NoCandidate));
        assert_eq!(p.warnings, vec![Warning::AllOverLimit]);
        assert_eq!(p.clone, None);
    }

    #[test]
    fn agent_prefers_a_machine_with_the_repo_and_clones_only_when_none_has_it() {
        let bare = |id: &str, working| MachineFacts { repos: vec![], ..machine(id, working, 20) };
        let facts = [machine("pc", 9, 20), bare("vm", 0), machine("nb", 3, 20)];
        let agent = OpenContext { agent: true, ..ctx() };
        let p = choose(&agent, &facts, &PlacementConfig::default(), now());
        assert_eq!((p.server.as_str(), p.clone), ("nb", None));

        // A tela fica com a mais leve e mostra o clone.
        let p = choose(&ctx(), &facts, &PlacementConfig::default(), now());
        assert_eq!((p.server.as_str(), p.clone), ("vm", Some(CloneTarget { dir: "/home/vm/Projetos".into() })));

        // Nenhuma com o repositório: clona, na pasta escolhida na tela se houver.
        let facts = [machine("pc", 9, 20), bare("vm", 0), MachineFacts { working: 7, ..machine("nb", 0, 20) }];
        let mut config = PlacementConfig::default();
        config.clone_dirs.insert("vm".into(), r"C:\src".into());
        let p = choose(&agent, &facts, &config, now());
        assert_eq!((p.server.as_str(), p.clone), ("vm", Some(CloneTarget { dir: r"C:\src".into() })));
    }

    #[test]
    fn machine_without_repo_nor_clone_dir_is_out() {
        let lost = MachineFacts { repos: vec![], clone_dir: None, ..machine("vm", 0, 20) };
        let p = choose(&ctx(), &[machine("pc", 9, 20), lost], &rule_to("vm"), now());
        assert_eq!(p.reason, ReasonCode::NoCandidate);
        assert_eq!(p.warnings[0], Warning::RuleServerOut { server: "vm".into(), why: Exclusion::NoCloneDir });
    }

    #[test]
    fn placement_json_is_the_route_contract() {
        let p = Placement {
            server: "nb".into(),
            reason: ReasonCode::LeastLoaded { local_out: Exclusion::Working },
            clone: None,
            warnings: vec![Warning::RuleServerOut { server: "vm".into(), why: Exclusion::Missing }],
        };
        assert_eq!(
            serde_json::to_value(&p).unwrap(),
            serde_json::json!({
                "server": "nb",
                "reason": {"code": "least_loaded", "local_out": "working"},
                "clone": null,
                "warnings": [{"code": "rule_server_out", "server": "vm", "why": "missing"}],
            })
        );
    }
}
