//! Persona e pacotes do instalador, preservando arquivos reais do usuário.
use serde_json::Value;
use std::{fs, path::Path};

fn link(path: &Path, target: &Path, directory: bool) -> std::io::Result<bool> {
    match fs::symlink_metadata(path) {
        Ok(meta) if meta.file_type().is_symlink() => {
            if fs::read_link(path)? == target {
                return Ok(true);
            }
            #[cfg(windows)]
            if meta.is_dir() {
                fs::remove_dir(path)?;
            } else {
                fs::remove_file(path)?;
            }
            #[cfg(not(windows))]
            fs::remove_file(path)?;
        }
        Ok(_) => return Ok(false),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    #[cfg(unix)]
    {
        let _ = directory;
        std::os::unix::fs::symlink(target, path)?;
    }
    #[cfg(windows)]
    if directory {
        std::os::windows::fs::symlink_dir(target, path)?;
    } else {
        std::os::windows::fs::symlink_file(target, path)?;
    }
    Ok(true)
}
/// O `agy` trunca cada arquivo de regras em 24.000 bytes.
const AGY_RULES_LIMIT: u64 = 24_000;

/// Só o `~/.gemini/config/AGENTS.md` -> `~/.claude/CLAUDE.md`, na subida do servidor e a cada abertura do `agy`:
/// sem o `harness-setup` manual o `agy` ficava sem as regras globais. Arquivo de verdade da pessoa fica; tudo
/// só vai ao log.
/// Marcas do bloco Sessões-irmãs que o instalador grava no `CLAUDE.md`.
const BLOCK_START: &str = "<!-- claude-pocket:sessoes-irmas:start -->";
const BLOCK_END: &str = "<!-- claude-pocket:sessoes-irmas:end -->";
/// Primeira linha do corpo da regra: arquivo sem ela é da pessoa e não é regravado.
const RULE_MARK: &str = "<!-- hangar: gerado pelo Hangar a partir do bloco Sessões-irmãs; não edite -->";

/// O protocolo do Hangar como regra própria do `agy` (`~/.gemini/config/rules/hangar.md`, sempre ativa): o
/// `AGENTS.md` ligado ao `CLAUDE.md` é cortado em 24.000 bytes e o bloco fica no fim dele.
pub fn agy_rule(claude_md: &str) -> Option<String> {
    let start = claude_md.find(BLOCK_START)?;
    let end = claude_md[start..].find(BLOCK_END)? + start + BLOCK_END.len();
    let block = &claude_md[start..end];
    Some(format!("---\ntrigger: always_on\ndescription: \"Hangar: sessões-irmãs, recados (hangar-send), MCP hangar e navegador embutido\"\n---\n\n\
{RULE_MARK}\n\n{block}\n\n\
- **Antigravity:** no sandbox do terminal o `~/.hangar` some e IP direto é recusado: `hangar-send` e `hangar-preview` só \
funcionam pelo run_command com BypassSandbox. Prefira as tools do MCP hangar (`sessions`, `send`, `group`, `browser_*`).\n"))
}

fn sync_agy_rule(dir: &Path, source: &Path) {
    let Some(rule) = fs::read_to_string(source).ok().as_deref().and_then(agy_rule) else { return };
    let path = dir.join("rules").join("hangar.md");
    match fs::read_to_string(&path) {
        Ok(current) if current == rule => return,
        Ok(current) if !current.contains(RULE_MARK) => {
            tracing::info!(file = %path.display(), "rules/hangar.md do agy é da pessoa; protocolo do Hangar não gravado");
            return;
        }
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        // Ilegível (fora de UTF-8, sem permissão) pode ser da pessoa: regravar apagaria o conteúdo dela.
        Err(error) => {
            tracing::warn!(code = "agy_rule_unreadable", file = %path.display(), %error, "rules/hangar.md do agy ilegível; protocolo do Hangar não gravado");
            return;
        }
    }
    let written = fs::create_dir_all(dir.join("rules")).and_then(|()| hangar_core::fs::atomic_write(&path, rule.as_bytes()));
    if let Err(error) = written {
        tracing::warn!(code = "agy_rule_failed", %error, "protocolo do Hangar não gravado nas regras do agy");
    }
}

pub fn sync_agy(home: &Path) {
    let agy = hangar_antigravity::Account::default();
    let source = home.join(".claude/CLAUDE.md");
    if !agy.data_dir(home).is_dir() || !source.is_file() {
        return;
    }
    sync_agy_rule(&agy.gemini_dir(home).join("config"), &source);
    let dir = agy.gemini_dir(home).join("config");
    match fs::create_dir_all(&dir).and_then(|()| link(&dir.join("AGENTS.md"), &source, false)) {
        Ok(true) => {}
        Ok(false) => tracing::info!(file = %dir.join("AGENTS.md").display(), "AGENTS.md do agy é um arquivo da pessoa; não ligado ao CLAUDE.md"),
        Err(error) => tracing::warn!(code = "agy_persona_failed", %error, "AGENTS.md do agy não ligado ao CLAUDE.md"),
    }
    if fs::metadata(&source).is_ok_and(|meta| meta.len() > AGY_RULES_LIMIT) {
        tracing::warn!(code = "agy_persona_truncated", bytes = fs::metadata(&source).map_or(0, |m| m.len()),
            "o CLAUDE.md passa de 24.000 bytes; o agy lê só o começo dele");
    }
}

pub fn sync(home: &Path) -> Result<Vec<String>, String> {
    let installed: Value = serde_json::from_slice(
        &fs::read(home.join(".claude/plugins/installed_plugins.json"))
            .map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    let source = home.join(".claude/CLAUDE.md");
    let mut done = Vec::new();
    let mut warnings = Vec::new();
    let agy = hangar_antigravity::Account::default();
    // `root` diz se o agente já foi usado; `dir` é onde ele lê as instruções globais. O `agy` lê as
    // regras globais de `~/.gemini/config/AGENTS.md` mesmo numa conta adicional.
    for (name, root, dir, persona) in [
        ("kimi", home.join(".kimi-code"), home.join(".kimi-code"), "AGENTS.md"),
        ("pi", home.join(".pi/agent"), home.join(".pi/agent"), "CLAUDE.md"),
        ("agy", agy.data_dir(home), agy.gemini_dir(home).join("config"), "AGENTS.md"),
    ] {
        if !root.is_dir() {
            continue;
        }
        if source.is_file() {
            fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
            if !link(&dir.join(persona), &source, false).map_err(|e| e.to_string())? {
                warnings.push(format!(
                    "  {name}: {persona} já existe como arquivo de verdade — não mexi"
                ));
            }
        }
        done.push(name);
        if name == "pi" {
            let packages = root.join("packages");
            fs::create_dir_all(&packages).map_err(|e| e.to_string())?;
            for (key, alias) in [
                ("superpowers@claude-plugins-official", "superpowers"),
                ("ponytail@ponytail", "ponytail"),
            ] {
                if let Some(path) = installed["plugins"][key]
                    .as_array()
                    .and_then(|a| a.first())
                    .and_then(|v| v["installPath"].as_str())
                {
                    link(&packages.join(alias), Path::new(path), true)
                        .map_err(|e| e.to_string())?;
                }
            }
        }
    }
    let label = if done.is_empty() {
        "nenhum agente instalado, nada a fazer".to_owned()
    } else {
        done.join(", ")
    };
    let mut messages = vec![format!("ponte: {label}")];
    messages.extend(warnings);
    Ok(messages)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn home() -> tempfile::TempDir {
        let home = tempfile::tempdir().unwrap();
        fs::create_dir_all(home.path().join(".claude/plugins")).unwrap();
        fs::write(home.path().join(".claude/plugins/installed_plugins.json"), "{}").unwrap();
        fs::write(home.path().join(".claude/CLAUDE.md"), "# regras\n").unwrap();
        home
    }

    #[test]
    fn agy_gets_the_global_instructions_only_where_it_was_used() {
        let home = home();
        let rules = home.path().join(".gemini/config/AGENTS.md");
        assert_eq!(sync(home.path()).unwrap(), ["ponte: nenhum agente instalado, nada a fazer"]);
        assert!(!home.path().join(".gemini").exists(), "sem uso do agy nada nasce");
        fs::create_dir_all(home.path().join(".gemini/antigravity-cli")).unwrap();
        assert_eq!(sync(home.path()).unwrap(), ["ponte: agy"]);
        assert_eq!(fs::read_link(&rules).unwrap(), home.path().join(".claude/CLAUDE.md"));
        assert_eq!(sync(home.path()).unwrap(), ["ponte: agy"], "segunda vez não muda nada");
        assert_eq!(fs::read_link(&rules).unwrap(), home.path().join(".claude/CLAUDE.md"));
    }

    #[test]
    fn the_startup_link_needs_no_plugin_list_and_keeps_a_file_of_the_person() {
        let home = tempfile::tempdir().unwrap();
        fs::create_dir_all(home.path().join(".claude")).unwrap();
        fs::write(home.path().join(".claude/CLAUDE.md"), "# regras\n").unwrap();
        let rules = home.path().join(".gemini/config/AGENTS.md");
        sync_agy(home.path());
        assert!(!home.path().join(".gemini").exists(), "sem uso do agy nada nasce");
        fs::create_dir_all(home.path().join(".gemini/antigravity-cli")).unwrap();
        sync_agy(home.path());
        assert_eq!(fs::read_link(&rules).unwrap(), home.path().join(".claude/CLAUDE.md"));
        fs::remove_file(&rules).unwrap();
        fs::write(&rules, "minhas regras").unwrap();
        sync_agy(home.path());
        assert_eq!(fs::read_to_string(&rules).unwrap(), "minhas regras");
    }

    #[test]
    fn the_hangar_block_becomes_an_always_on_rule_of_agy_and_a_file_of_the_person_stays() {
        let home = home();
        let block = format!("{BLOCK_START}\n# Sessões-irmãs (hangar)\n- hangar-send --list\n{BLOCK_END}");
        fs::write(home.path().join(".claude/CLAUDE.md"), format!("# regras\n{}\n{block}\n", "x".repeat(30_000))).unwrap();
        fs::create_dir_all(home.path().join(".gemini/antigravity-cli")).unwrap();
        sync_agy(home.path());
        let rule = home.path().join(".gemini/config/rules/hangar.md");
        let text = fs::read_to_string(&rule).unwrap();
        assert!(text.starts_with("---\ntrigger: always_on\n"), "{text}");
        assert!(text.contains(&block) && text.contains("BypassSandbox"), "{text}");
        assert!(text.len() < 24_000);
        fs::write(&rule, "minha regra").unwrap();
        sync_agy(home.path());
        assert_eq!(fs::read_to_string(&rule).unwrap(), "minha regra");
        // Regra da pessoa fora de UTF-8 (Latin-1) não é lida, e por isso também não é regravada.
        fs::write(&rule, b"minha regra \xe9").unwrap();
        sync_agy(home.path());
        assert_eq!(fs::read(&rule).unwrap(), b"minha regra \xe9");
        assert_eq!(agy_rule("sem bloco"), None);
    }

    #[test]
    fn agy_keeps_a_rules_file_of_the_person() {
        let home = home();
        let rules = home.path().join(".gemini/config/AGENTS.md");
        fs::create_dir_all(home.path().join(".gemini/antigravity-cli")).unwrap();
        fs::create_dir_all(rules.parent().unwrap()).unwrap();
        fs::write(&rules, "minhas regras").unwrap();
        let lines = sync(home.path()).unwrap();
        assert_eq!(lines[1], "  agy: AGENTS.md já existe como arquivo de verdade — não mexi");
        assert!(!rules.is_symlink());
        assert_eq!(fs::read_to_string(&rules).unwrap(), "minhas regras");
    }
}
