//! Reconciliação dos comandos instalados, preservando a ordem dos hooks pessoais.
use crate::accounts::{AccountService, Provider};
use serde_json::{Value, json};
use std::{
    collections::BTreeSet,
    fs,
    path::Path,
    sync::Mutex,
};
static LOCK: Mutex<()> = Mutex::new(());

pub(super) fn tokens(command: &str, windows: bool) -> Vec<String> {
    let fallback = || command.split_whitespace().map(str::to_owned).collect();
    let mut result = Vec::new();
    let mut current = String::new();
    let mut quote = None;
    let mut chars = command.chars().peekable();
    while let Some(c) = chars.next() {
        if let Some(q) = quote {
            if c == q {
                quote = None;
                if windows {
                    result.push(std::mem::take(&mut current));
                }
            } else if !windows && q == '"' && c == '\\' {
                let Some(next) = chars.next() else {
                    return fallback();
                };
                if next != '"' && next != '\\' {
                    current.push('\\');
                }
                current.push(next);
            } else {
                current.push(c);
            }
        } else if !windows && c == '\\' {
            let Some(next) = chars.next() else {
                return fallback();
            };
            current.push(next);
        } else if (c == '\'' || c == '"') && (!windows || current.is_empty()) {
            quote = Some(c);
        } else if c.is_whitespace() {
            if !current.is_empty() {
                result.push(std::mem::take(&mut current));
            }
        } else {
            current.push(c);
        }
    }
    if quote.is_some() {
        return fallback();
    }
    if !current.is_empty() {
        result.push(current);
    }
    result
}
pub fn refers(command: &Value, script: &str, basename: bool) -> bool {
    refers_platform(command, script, basename, cfg!(windows))
}
pub fn refers_platform(command: &Value, script: &str, basename: bool, windows: bool) -> bool {
    let basename_of = |text: &str| -> String {
        if windows {
            text.rsplit(['/', '\\']).next().unwrap_or("").to_owned()
        } else {
            text.rsplit('/').next().unwrap_or("").to_owned()
        }
    };
    let target = if basename {
        basename_of(script)
    } else {
        script.to_owned()
    };
    command.as_str().is_some_and(|c| {
        tokens(c, windows).iter().any(|token| {
            let token = token.trim_matches(['"', '\'']);
            (if basename {
                basename_of(token)
            } else {
                token.to_owned()
            }) == target
        })
    })
}
pub fn sync_hook(
    data: &mut Value,
    event: &str,
    command: Option<&str>,
    matcher: Option<&str>,
    script: &str,
) -> bool {
    sync_hook_any(data, event, command, matcher, &[script], cfg!(windows))
}

/// Programa do Hangar num token: o servidor, o script que o acha ou o lançador estável.
pub fn hangar_program(token: &str) -> bool {
    let name = token.trim_matches(['"', '\'']).rsplit(['/', '\\']).next().unwrap_or_default();
    matches!(name, "hangar-server" | "hangar-server.exe" | "hangar-server-command" | "hangar-launcher" | "hangar-launcher.exe")
}

/// O comando roda `mode` de um programa do Hangar (`"<…>/hangar-launcher" state-hook …`). Só o nome do
/// subcomando não basta: um hook pessoal pode ter um argumento igual.
pub fn runs_subcommand(command: &Value, mode: &str, windows: bool) -> bool {
    command.as_str().is_some_and(|text| {
        tokens(text, windows).windows(2).any(|pair| hangar_program(&pair[0]) && pair[1] == mode)
    })
}

/// Script Python de antes pelo nome do arquivo; subcomando, só atrás de um programa do Hangar.
fn refers_any(command: &Value, names: &[&str], windows: bool) -> bool {
    names.iter().any(|name| {
        if name.ends_with(".py") { refers_platform(command, name, true, windows) } else { runs_subcommand(command, name, windows) }
    })
}

/// Uma entrada nossa por evento: a primeira que cita algum dos `names` (o script Python antigo ou o
/// subcomando) vira `command` NO LUGAR, as repetidas saem, bloco esvaziado sai, filtro trocado move o
/// hook para um bloco próprio, e a ausente entra num bloco novo no fim. `command` nulo só tira.
pub fn sync_hook_any(
    data: &mut Value,
    event: &str,
    command: Option<&str>,
    matcher: Option<&str>,
    names: &[&str],
    windows: bool,
) -> bool {
    let Some(root) = data.as_object_mut() else {
        return false;
    };
    let hooks = root.entry("hooks").or_insert_with(|| json!({}));
    let Some(hooks) = hooks.as_object_mut() else {
        return false;
    };
    let events = hooks.entry(event).or_insert_with(|| json!([]));
    let Some(events) = events.as_array_mut() else {
        return false;
    };
    let mut changed = false;
    let mut kept = command.is_none();
    events.retain_mut(|block| {
        // Filtro trocado: o hook sai deste bloco (que pode ser dividido com outro) e nasce num bloco próprio.
        let moved = command.is_some()
            && matcher.is_some()
            && block.get("matcher").and_then(Value::as_str) != matcher;
        let Some(entries) = block.get_mut("hooks").and_then(Value::as_array_mut) else {
            return true;
        };
        let before = entries.len();
        entries.retain_mut(|hook| {
            // O próprio comando que gravamos também é nosso, qualquer que seja o nome do programa.
            let ours = hook.is_object()
                && (refers_any(&hook["command"], names, windows)
                    || (command.is_some() && hook["command"].as_str() == command));
            if !ours {
                return true;
            }
            if kept || moved {
                changed = true;
                return false;
            }
            kept = true;
            if hook["command"].as_str() != command {
                hook["command"] = json!(command);
                changed = true;
            }
            hook.as_object_mut()
                .unwrap()
                .entry("type")
                .or_insert(json!("command"));
            true
        });
        before == entries.len() || !entries.is_empty()
    });
    if !kept {
        let mut block = json!({"hooks":[{"type":"command","command":command}]});
        if let Some(matcher) = matcher {
            block["matcher"] = json!(matcher);
        }
        events.push(block);
        changed = true;
    }
    changed
}

/// Tira as entradas que citam `names` sem criar o evento; evento esvaziado por isso sai inteiro.
pub fn remove_hook(data: &mut Value, event: &str, names: &[&str], windows: bool) -> bool {
    let Some(hooks) = data.get_mut("hooks").and_then(Value::as_object_mut) else {
        return false;
    };
    let Some(blocks) = hooks.get_mut(event).and_then(Value::as_array_mut) else {
        return false;
    };
    let mut changed = false;
    blocks.retain_mut(|block| {
        let Some(entries) = block.get_mut("hooks").and_then(Value::as_array_mut) else {
            return true;
        };
        let before = entries.len();
        entries.retain(|hook| !(hook.is_object() && refers_any(&hook["command"], names, windows)));
        changed |= entries.len() != before;
        before == entries.len() || !entries.is_empty()
    });
    if changed && blocks.is_empty() {
        hooks.shift_remove(event);
    }
    changed
}

/// Os hooks de marcador do Claude: chave do instalador, subcomando, nomes reconhecidos (o script
/// Python de antes e o subcomando), eventos e filtro.
pub const MARKER_HOOKS: &[(&str, &str, &[&str], &[&str], Option<&str>)] = &[
    ("state_hook.py", "state-hook", &["state_hook.py", "state-hook"],
     &["UserPromptSubmit", "PreToolUse", "PostToolUse", "Notification", "Stop", "SessionStart"], None),
    ("preview_hook.py", "preview-hook", &["preview_hook.py", "preview-hook"], &["MessageDisplay", "Stop"], None),
    ("nav_hook.py", "nav-hook", &["nav_hook.py", "nav-hook"], &["UserPromptSubmit"], None),
    ("askq_capture.py", "askq-capture", &["askq_capture.py", "askq-capture"],
     &["PreToolUse", "PostToolUse", "PostToolUseFailure"], Some("AskUserQuestion")),
];
const GUARD_MATCHER: &str = "Bash|PowerShell|Read|Grep";

/// `"<programa>" <subcomando>` com o sufixo que impede o hook de barrar o agente: em `SessionStart` e
/// `UserPromptSubmit`, onde o stdout chega ao modelo, a falha vira um aviso para ele contar ao usuário.
/// O aviso não pode ter token terminado no subcomando (seria reconhecido como o hook).
pub fn safe_command(program: &Path, mode: &str, event: &str) -> String {
    let command = format!("\"{}\" {mode}", program.display());
    if ["SessionStart", "UserPromptSubmit"].contains(&event) {
        format!(
            "{command} || echo \"[hangar] O hook {mode} do Hangar falhou em {event} e foi ignorado. AVISE O USUARIO agora: parte do app pode estar sem estado, pareamento ou navegador. Para ver o erro, rode o comando do hook a mao.\""
        )
    } else {
        format!("{command} || exit 0")
    }
}
pub fn sync(service: &AccountService, repo: &Path) -> Value {
    sync_report(service, repo).0
}

fn installer_script(name: &str) -> Option<&'static str> {
    Some(match name {
        "ensure_askq_hook_installed" => "askq_capture.py",
        "ensure_state_hooks_installed" => "state_hook.py",
        "ensure_preview_hook_installed" => "preview_hook.py",
        // O hook de subagentes saiu (nada lia o arquivo dele): o instalador agora só o tira.
        "ensure_subagent_hook_installed" => "subagent_hook.py",
        "ensure_pair_hook_installed" => "pair-context",
        "ensure_nav_hook_installed" => "nav_hook.py",
        "ensure_guard_hooks_installed" => "guard_tmux.py",
        _ => return None,
    })
}

pub fn sync_installer(
    service: &AccountService,
    repo: &Path,
    name: &str,
) -> Result<Value, Vec<String>> {
    sync_installers(service, repo, &[name])
}

/// Vários instaladores numa passada: cada `settings.json` é lido e gravado uma vez.
pub fn sync_installers(
    service: &AccountService,
    repo: &Path,
    names: &[&str],
) -> Result<Value, Vec<String>> {
    let scripts = names
        .iter()
        .map(|name| installer_script(name).ok_or_else(|| vec![format!("instalador de hook desconhecido: {name}")]))
        .collect::<Result<Vec<_>, _>>()?;
    let (changes, errors) = sync_report_selected(service, repo, Some(&scripts));
    if errors.is_empty() {
        Ok(changes)
    } else {
        Err(errors)
    }
}

pub(super) fn sync_report(service: &AccountService, repo: &Path) -> (Value, Vec<String>) {
    sync_report_selected(service, repo, None)
}

/// Os hooks do Hangar num `settings.json` já lido. `program` é o binário dos hooks; `pair_dir`, a pasta
/// dos grupos. O guarda entra quando pedido ou quando o arquivo já tem um (o Python antigo ou o
/// subcomando com outro programa): a subida conserta o que existe, mas não instala o que não havia.
pub fn reconcile(data: &mut Value, program: &Path, pair_dir: &Path, enabled: &dyn Fn(&str) -> bool, windows: bool) -> bool {
    let mut changed = false;
    for &(key, mode, names, events, matcher) in MARKER_HOOKS {
        if !enabled(key) {
            continue;
        }
        for event in events {
            let command = safe_command(program, mode, event);
            changed |= sync_hook_any(data, event, Some(&command), matcher, names, windows);
        }
    }
    if enabled("subagent_hook.py") {
        for event in ["SubagentStart", "SubagentStop"] {
            changed |= remove_hook(data, event, &["subagent_hook.py"], windows);
        }
    }
    let guards = ["guard_tmux.py", "guard-tmux"];
    let has_guard = data["hooks"]["PreToolUse"].as_array().into_iter().flatten()
        .flat_map(|block| block["hooks"].as_array().into_iter().flatten())
        .any(|hook| refers_any(&hook["command"], &guards, windows));
    if enabled("guard_tmux.py") || has_guard {
        // Read, Grep e PowerShell: o guarda também barra a leitura do arquivo do `vault_file`.
        let command = format!("\"{}\" guard-tmux", program.display());
        changed |= sync_hook_any(data, "PreToolUse", Some(&command), Some(GUARD_MATCHER), &guards, windows);
    }
    if enabled("pair-context") {
        changed |= sync_hook_any(data, "SessionStart", None, None, &["pair_hook.py"], windows);
        let command = format!("\"{}\" pair-context \"{}\" || exit 0", program.display(), pair_dir.display());
        changed |= sync_hook_any(data, "SessionStart", Some(&command), Some("startup|resume|clear|compact"), &["pair-context"], windows);
    }
    changed
}

fn sync_report_selected(
    service: &AccountService,
    repo: &Path,
    selected: Option<&[&str]>,
) -> (Value, Vec<String>) {
    let _ = repo;
    let enabled = |script: &str| selected.is_none_or(|selected| selected.contains(&script));
    let mut errors = Vec::new();
    let _lock = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    // O lançador estável: o comando não muda a cada versão (nem acende o Recarregar) e continua
    // existindo na volta de versão.
    let program = match crate::codex_setup::hook_command::hook_server(&service.env.home) {
        Ok(program) => program,
        Err(error) => return (json!({"claude": []}), vec![format!("programa dos hooks: {error}")]),
    };
    let pair_dir = service.env.claude_base.join(".hangar-pair");
    // Catálogo e contas marcadas juntos: conta antiga com login e `CP_CLAUDE_CONFIG_DIRS` (que no
    // catálogo substitui a varredura) só vêm dele; as `.claude-<id>` do Hangar, do `snapshot`.
    let catalog: Vec<std::path::PathBuf> = service
        .claude_catalog()
        .map(|rows| {
            rows.as_array()
                .into_iter()
                .flatten()
                .filter_map(|row| row["path"].as_str().map(std::path::PathBuf::from))
                .collect()
        })
        .unwrap_or_else(|error| {
            errors.push(format!("descoberta das contas Claude: {error:?}"));
            Vec::new()
        });
    let managed: Vec<std::path::PathBuf> = service
        .snapshot(Provider::Claude)
        .map(|accounts| accounts.into_iter().map(|account| account.home).collect())
        .unwrap_or_else(|error| {
            errors.push(format!("contas Claude do Hangar: {error:?}"));
            Vec::new()
        });
    let mut dirs: Vec<_> = catalog
        .into_iter()
        .chain(managed)
        .chain([service.env.claude_base.clone()])
        .collect();
    dirs.sort_by_key(|d| (d.file_name().is_none_or(|n| n != ".claude"), d.clone()));
    dirs.dedup();
    let mut seen = BTreeSet::new();
    let mut touched = Vec::new();
    for dir in dirs {
        if !dir.is_dir() {
            continue;
        }
        let settings = dir.join("settings.json");
        let path = fs::canonicalize(&settings).unwrap_or(settings);
        if !seen.insert(path.clone()) {
            continue;
        }
        let result = (|| -> Result<bool, String> {
            let text = match fs::read_to_string(&path) {
                Ok(v) => v,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
                Err(e) => return Err(e.to_string()),
            };
            let mut data: Value = if text.trim().is_empty() {
                json!({})
            } else {
                serde_json::from_str(&text).map_err(|e| e.to_string())?
            };
            if !data.is_object()
                || data
                    .get("hooks")
                    .is_some_and(|h| !h.is_object() && (selected.is_some() || !h.is_null()))
            {
                return Err("settings.json inválido; hooks preservados".into());
            }
            if data["hooks"].is_null() {
                data["hooks"] = json!({});
            }
            let changed = reconcile(&mut data, &program, &pair_dir, &enabled, cfg!(windows));
            if changed {
                super::files::write_preserving(
                    &path,
                    &format!(
                        "{}\n",
                        crate::transcript::pyjson::dumps_indent(&data, 2, false)
                    ),
                )?;
            }
            Ok(changed)
        })();
        match result {
            Ok(true) => touched.push(path),
            Err(error) => {
                tracing::warn!(%error,"Hooks não atualizados");
                errors.push(error);
            }
            _ => {}
        }
    }
    (json!({"claude":touched}), errors)
}
