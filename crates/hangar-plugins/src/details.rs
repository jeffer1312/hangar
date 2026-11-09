//! Leitura do texto de `claude plugin details`, que não tem saída em JSON.
use crate::model::{Inventory, PluginDetails, PluginId};

/// Linha que não casa vira `None`: o texto é da CLI e pode mudar sem aviso.
pub(crate) fn parse(id: &PluginId, text: &str) -> PluginDetails {
    let lines: Vec<&str> = text.lines().map(str::trim).collect();
    let count = |label: &str| lines.iter().find_map(|line| inventory_count(line, label));
    PluginDetails {
        id: id.as_str().to_owned(),
        inventory: Inventory {
            skills: count("Skills"),
            agents: count("Agents"),
            hooks: count("Hooks"),
            mcp: count("MCP servers"),
            lsp: count("LSP servers"),
        },
        always_on_tokens: lines.iter().find_map(|line| always_on(line)),
    }
}

/// `Skills (23)  cavecrew, …` → 23.
fn inventory_count(line: &str, label: &str) -> Option<u32> {
    let (digits, _) = line.strip_prefix(label)?.strip_prefix(" (")?.split_once(')')?;
    digits.parse().ok()
}

/// `Always-on:   ~1,991 tok   added to every session` → 1991.
fn always_on(line: &str) -> Option<u64> {
    let (number, _) = line.strip_prefix("Always-on:")?.trim_start().strip_prefix('~')?.split_once(" tok")?;
    number.replace(',', "").parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Saída real de `claude plugin details caveman@caveman` (CLI 2.1.319, caveman 4.1.0).
    const CAVEMAN: &str = "caveman 4.1.0
  Description: Talk like caveman. Cut filler. Keep technical details explicit.
  Source: caveman@caveman

Component inventory
  Skills (23)  cavecrew, caveman, caveman-commit, caveman-compress, caveman-discover, caveman-evidence-review, caveman-explore, caveman-help, caveman-init, caveman-learn, caveman-manage, caveman-optimize, caveman-review, caveman-setup, caveman-stats, investigate-first, lean-build, megacave, migration, safe-refactor, surgical-patch, ultracave, verify-and-stop
  Agents (3)  cavecrew-builder, cavecrew-investigator, cavecrew-reviewer
  Hooks (4)  SessionStart, SubagentStart, UserPromptSubmit, SessionEnd  (harness-only — no model context cost)
  MCP servers (0)
  LSP servers (0)

Projected token cost
  Always-on:   ~1,991 tok   added to every session

Per-component (rounded)
  component                always-on  on-invoke
  cavecrew                       ~90      ~1.4k
  caveman-commit                 ~60       ~820
  caveman                        ~80      ~1.7k
  cavecrew-builder              ~120       ~410

  On-invoke cost is paid each time a skill or agent fires.
  Token counts are estimates and may differ from actual usage.
";

    fn caveman() -> PluginId {
        PluginId::parse("caveman@caveman").expect("id real")
    }

    #[test]
    fn reads_real_caveman_details() {
        let details = parse(&caveman(), CAVEMAN);
        assert_eq!(details.id, "caveman@caveman");
        assert_eq!(
            details.inventory,
            Inventory { skills: Some(23), agents: Some(3), hooks: Some(4), mcp: Some(0), lsp: Some(0) }
        );
        assert_eq!(details.always_on_tokens, Some(1991));
    }

    #[test]
    fn unknown_text_gives_none_not_error() {
        let details = parse(&caveman(), "Plugin \"x\" not found.\n  Skills (muitas)\n  Always-on: ~? tok\n");
        assert_eq!(details.inventory, Inventory { skills: None, agents: None, hooks: None, mcp: None, lsp: None });
        assert_eq!(details.always_on_tokens, None);
    }
}
