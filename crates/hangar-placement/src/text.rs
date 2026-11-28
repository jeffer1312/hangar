//! O texto do motivo da escolha, para a tela e para o agente.
use crate::{Exclusion, Placement, ReasonCode, Unpublished, Warning};

/// Onde e por quê, numa frase.
pub fn text(p: &Placement) -> String {
    let mut text = match &p.reason {
        ReasonCode::SameFolder => format!("mesma pasta de uma sessão aberta em {}", p.server),
        ReasonCode::Unpublished { why } => format!("{}: fica nesta máquina", match why {
            Unpublished::Ahead => "commit não enviado",
            Unpublished::Dirty => "mudança não commitada",
            Unpublished::NoUpstream => "branch sem upstream",
        }),
        ReasonCode::PlainDir => "pasta sem git (ou sem origin) que só existe nesta máquina".to_owned(),
        ReasonCode::ProjectRule { project } => format!("a regra do projeto {} manda para {}", project.0, p.server),
        ReasonCode::LocalWithinLimits => "esta máquina está dentro dos limites".to_owned(),
        ReasonCode::LeastLoaded { local_out } => format!("esta máquina saiu da escolha ({}); {} tem menos sessões abertas",
            exclusion(*local_out), p.server),
        ReasonCode::NoCandidate => "nenhuma máquina dentro dos limites: fica nesta".to_owned(),
        ReasonCode::NotBalanced { why } => format!("fora do balanceamento ({})", why_text(why)),
    };
    for warning in &p.warnings {
        text.push_str("; ");
        text.push_str(&match warning {
            Warning::RuleServerOut { server, why } => format!("a máquina da regra, {server}, ficou fora ({})", exclusion(*why)),
            Warning::RuleOverridden { server } => format!("a regra do projeto mandaria para {server}"),
            Warning::AllOverLimit => "todas as máquinas passaram do limite".to_owned(),
        });
    }
    if let Some(clone) = &p.clone { text.push_str(&format!("; {} não tem o repositório: clonar em {}", p.server, clone.dir)); }
    text
}

fn exclusion(why: Exclusion) -> &'static str {
    match why {
        Exclusion::Missing => "desligada ou sem resposta",
        Exclusion::NotCapable => "versão sem a escolha de máquina",
        Exclusion::Schedule => "regra de horário",
        Exclusion::Memory => "pouca memória livre",
        Exclusion::Working => "muitas sessões trabalhando",
        Exclusion::Open => "muitas sessões abertas",
        Exclusion::Cpu => "processador acima do limite",
        Exclusion::NoCloneDir => "sem o repositório e sem pasta de clone",
    }
}

/// Por que o pedido ficou fora do balanceamento; código desconhecido sai como veio.
pub fn why_text(why: &str) -> &str {
    match why {
        "guest" => "pedido de convidado",
        "prewarm" => "reserva pré-aquecida",
        "local_field" => "conta, motor ou perfil desta máquina",
        "orq" => "orquestração",
        "mode_off" => "balanceamento desligado",
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_text_says_where_and_why_with_the_warnings() {
        let p = Placement { server: "nb".into(), reason: ReasonCode::LeastLoaded { local_out: Exclusion::Working }, clone: None,
            warnings: vec![Warning::RuleServerOut { server: "vm".into(), why: Exclusion::Missing }] };
        assert_eq!(text(&p), "esta máquina saiu da escolha (muitas sessões trabalhando); nb tem menos sessões abertas; \
            a máquina da regra, vm, ficou fora (desligada ou sem resposta)");
        let kept = Placement { server: "pc".into(), reason: ReasonCode::NotBalanced { why: "orq".into() }, clone: None, warnings: Vec::new() };
        assert_eq!(text(&kept), "fora do balanceamento (orquestração)");
        assert_eq!(why_text("placement_config_unreadable"), "placement_config_unreadable");
    }
}
