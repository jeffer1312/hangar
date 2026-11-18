//! O que a sessão abre, decidido dos argumentos e do arquivo da sessão antes de qualquer efeito:
//! retomada importada, relançamento da própria sessão, pasta, conta, `CODEX_HOME` e modo.
use std::collections::BTreeMap;
use std::path::{Component, Path, PathBuf};

use serde_json::{Map, Value};

use super::args::{Args, Refusal};
use crate::runtime::codex_launch::{mode_of, policy_of};

/// O padrão sem escolha é Full Access: `workspace-write` corta a rede, inclusive o loopback, e o
/// `hangar-send` de dentro do Codex não falava com o servidor.
pub(crate) const APPROVAL: &str = "never";
pub(crate) const SANDBOX: &str = "danger-full-access";
/// A checagem de atualização da TUI travava a abertura antes da thread; quem atualiza é o lançador.
pub(crate) const NO_UPDATE_CHECK: [&str; 2] = ["-c", "check_for_update_on_startup=false"];

/// O que vem do pane e da máquina.
pub(crate) struct Facts<'a> {
    /// `CP_SESSION_KEY` do pane.
    pub session_key: Option<&'a str>,
    /// A pasta do processo.
    pub here: &'a Path,
    /// `CODEX_HOME` herdado.
    pub inherited_codex_home: Option<&'a str>,
    pub home: &'a Path,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Plan {
    pub name: String,
    pub cwd: String,
    pub codex_home: PathBuf,
    pub account: String,
    pub approval: &'static str,
    pub sandbox: &'static str,
    /// O modo do app do par; fora dos três, o gravado na sessão fica como estava.
    pub mode: Option<&'static str>,
    /// Retomada de conversa importada: a chave da sessão vem do arquivo dela.
    pub imported_key: Option<String>,
    pub resume: Option<String>,
    pub model: Option<String>,
    pub effort: Option<String>,
    pub service_tier: Option<String>,
    pub limit: Option<u64>,
    pub prompt: Option<String>,
    pub config: Vec<String>,
}

impl Plan {
    pub fn imported(&self) -> bool { self.imported_key.is_some() }

    pub fn mode_warning(&self) -> Option<String> {
        self.mode.is_none().then(|| format!("aprovação {} com sandbox {} não é um modo do app; o modo gravado da sessão fica como estava",
            self.approval, self.sandbox))
    }

    /// A conta adicional prepara sempre; a padrão só com a sincronização ligada; a importada nunca.
    pub fn prepares(&self, sync_enabled: bool) -> bool {
        !self.imported() && (self.account != "default" || sync_enabled)
    }
}

/// O modo pelo nome exato, como o dicionário do Python (sem caixa nem espaço de folga).
fn exact_policy(mode: &str) -> Option<(&'static str, &'static str)> {
    policy_of(mode).filter(|&(approval, sandbox)| mode_of(approval, sandbox) == Some(mode))
}

fn text<'a>(meta: &'a Map<String, Value>, field: &str) -> Option<&'a str> {
    meta.get(field).and_then(Value::as_str)
}

/// `thread_id` do arquivo comparado ao `--resume` como o Python compara: ausente e nulo são nenhum,
/// e outro tipo nunca é igual.
fn same_conversation(meta: &Map<String, Value>, resume: Option<&str>) -> bool {
    match meta.get("thread_id") {
        None | Some(Value::Null) => resume.is_none(),
        Some(Value::String(thread)) => resume == Some(thread.as_str()),
        Some(_) => false,
    }
}

fn truthy(meta: &Map<String, Value>, field: &str) -> bool {
    meta.get(field).is_some_and(hangar_core::pycompat::truthy)
}

fn usage(message: &str) -> Refusal { Refusal::Usage(message.into()) }

pub(crate) fn decide(mut args: Args, existing: Option<&Map<String, Value>>, facts: &Facts) -> Result<Plan, Refusal> {
    let mut imported_key = None;
    let imported = existing.is_some_and(|meta| truthy(meta, "transfer_id") && same_conversation(meta, args.resume.as_deref()));
    if let (true, Some(meta)) = (imported, existing) {
        let required = ["codex_home", "codex_account", "key", "cwd", "permission_mode"];
        let valid = text(meta, "name") == Some(args.name.as_str())
            && required.iter().all(|field| text(meta, field).is_some_and(|value| !value.is_empty()))
            && text(meta, "codex_home").is_some_and(|home| Path::new(home).is_absolute())
            && text(meta, "cwd").is_some_and(|cwd| Path::new(cwd).is_absolute())
            && text(meta, "permission_mode").and_then(exact_policy).is_some();
        if !valid {
            return Err(usage("metadados da conversa importada inválidos; nenhuma sessão foi aberta"));
        }
        let field = |name: &str| text(meta, name).map(str::to_owned);
        args.codex_home = args.codex_home.or_else(|| field("codex_home"));
        args.codex_account = args.codex_account.or_else(|| field("codex_account"));
        args.cwd = args.cwd.or_else(|| field("cwd"));
        let (approval, sandbox) = text(meta, "permission_mode").and_then(exact_policy).unwrap_or((APPROVAL, SANDBOX));
        args.approval = args.approval.or(Some(approval));
        args.sandbox = args.sandbox.or(Some(sandbox));
        imported_key = field("key");
    }
    if let Some(meta) = existing.filter(|meta| same_conversation(meta, args.resume.as_deref())) {
        if args.limit.is_none() {
            // Inteiro positivo, como o Python aceitava; outro valor gravado o derrubava ao subir o app-server.
            args.limit = match meta.get("tool_output_token_limit") {
                None | Some(Value::Null) => None,
                Some(value) => Some(value.as_u64().filter(|limit| *limit > 0)
                    .ok_or_else(|| usage("limite de resultado de ferramenta inválido no arquivo da sessão; nenhuma sessão foi aberta"))?),
            };
        }
        // Relançar a própria sessão (o servidor manda só a pasta, a conversa e a chave): o resto vem do arquivo.
        let key = imported_key.as_deref().or(facts.session_key);
        let own = args.resume.is_some() && text(meta, "key").is_some_and(|saved| !saved.is_empty() && Some(saved) == key);
        let from_file = |current: Option<String>, field: &str| current.or_else(|| text(meta, field).map(str::to_owned));
        if truthy(meta, "transfer_id") || own {
            args.model = from_file(args.model, "model");
            args.effort = from_file(args.effort, "effort");
        }
        if own {
            args.service_tier = from_file(args.service_tier, "service_tier");
            args.codex_home = from_file(args.codex_home, "codex_home");
            args.codex_account = from_file(args.codex_account, "codex_account");
            if args.approval.is_none() && args.sandbox.is_none() {
                // Sem modo conhecido não relança: o padrão do lançador é Full Access.
                let (approval, sandbox) = text(meta, "permission_mode").and_then(exact_policy)
                    .ok_or_else(|| usage("modo de permissão da sessão desconhecido; nenhuma sessão foi aberta"))?;
                args.approval = Some(approval);
                args.sandbox = Some(sandbox);
            }
        }
    }
    let account = args.codex_account.unwrap_or_else(|| "default".into());
    let cwd = absolute(facts.here, args.cwd.as_deref().map_or(facts.here, Path::new));
    let codex_home = match args.codex_home.as_deref().filter(|home| !home.is_empty()) {
        Some(home) => std::path::absolute(hangar_core::account_env::expand(home, facts.home)).unwrap_or_else(|_| home.into()),
        None => match facts.inherited_codex_home.filter(|home| !home.is_empty()) {
            Some(home) => std::path::absolute(home).unwrap_or_else(|_| home.into()),
            None => facts.home.join(".codex"),
        },
    };
    let approval = args.approval.unwrap_or(APPROVAL);
    let sandbox = args.sandbox.unwrap_or(SANDBOX);
    let mode = mode_of(approval, sandbox);
    if imported && mode.is_none() {
        return Err(usage("combinação de aprovação e sandbox não suportada para conversa importada; sessão preservada"));
    }
    Ok(Plan {
        name: args.name, cwd: cwd.to_string_lossy().into_owned(), codex_home, account, approval, sandbox, mode, imported_key,
        resume: args.resume, model: args.model, effort: args.effort, service_tier: args.service_tier, limit: args.limit,
        prompt: args.prompt, config: args.config,
    })
}

/// `os.path.abspath`: absoluta e sem `.`/`..`, sem resolver links.
fn absolute(here: &Path, path: &Path) -> PathBuf {
    let joined = if path.is_absolute() { path.to_path_buf() } else { here.join(path) };
    let mut out = PathBuf::new();
    for part in joined.components() {
        match part {
            Component::CurDir => {}
            Component::ParentDir => { out.pop(); }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

fn some(value: &Option<String>) -> Option<&str> { value.as_deref().filter(|value| !value.is_empty()) }

/// `service_tier=<json>`, o `-c` do Fast.
fn tier_override(tier: Option<&str>) -> Vec<String> {
    tier.map(|tier| vec!["-c".into(), format!("service_tier={}", Value::from(tier))]).unwrap_or_default()
}

/// O app-server leva a política: o `thread/resume` que a TUI faz ao reconectar não repassa as flags dela.
pub(crate) fn server_argv(codex: &str, plan: &Plan, endpoint: &str, tier: Option<&str>, env: &BTreeMap<String, String>) -> Vec<String> {
    let mut argv: Vec<String> = vec![codex.into(), "app-server".into(), "--listen".into(), endpoint.into(),
        "-c".into(), format!("sandbox_mode=\"{}\"", plan.sandbox), "-c".into(), format!("approval_policy=\"{}\"", plan.approval)];
    if let Some(limit) = plan.limit {
        argv.extend(["-c".into(), format!("tool_output_token_limit={limit}")]);
    }
    argv.extend(tier_override(tier));
    argv.extend(crate::runtime::codex_launch::identity(env));
    argv.extend(plan.config.iter().flat_map(|value| ["-c".to_owned(), value.clone()]));
    argv
}

/// O std do Rust recusa argumento de `.cmd`/`.bat` com quebra de linha (o `codex.cmd` do npm no Windows):
/// aí a primeira mensagem sai do argv e vai inteira pelo app-server, nunca cortada.
pub(crate) fn prompt_through_app_server(codex: &Path, plan: &Plan) -> Option<String> {
    let name = codex.file_name()?.to_string_lossy().to_ascii_lowercase();
    let batch = name.ends_with(".cmd") || name.ends_with(".bat");
    some(&plan.prompt).filter(|prompt| batch && prompt.contains(['\r', '\n'])).map(str::to_owned)
}

/// A TUI: o modelo é `-m`; o esforço é `-c` (o `--effort` matava o Codex no arranque), entre aspas
/// porque o `-c` é TOML. Retomando, sem `-C` nem política: o `resume --remote` as recusa.
pub(crate) fn tui_argv(codex: &str, plan: &Plan, endpoint: &str, tier: Option<&str>) -> Vec<String> {
    let mut choice: Vec<String> = NO_UPDATE_CHECK.iter().map(|s| (*s).to_owned()).collect();
    if let Some(model) = some(&plan.model) {
        choice.extend(["-m".into(), model.into()]);
    }
    if let Some(effort) = some(&plan.effort) {
        choice.extend(["-c".into(), format!("model_reasoning_effort=\"{effort}\"")]);
    }
    let mut argv: Vec<String> = match &plan.resume {
        Some(thread) => {
            let mut argv: Vec<String> = vec![codex.into(), "resume".into(), "--remote".into(), endpoint.into(), "--no-alt-screen".into()];
            argv.extend(tier_override(tier));
            argv.extend(choice);
            argv.push(thread.clone());
            argv
        }
        None => {
            let mut argv: Vec<String> = vec![codex.into(), "--remote".into(), endpoint.into(), "--no-alt-screen".into(), "-C".into(),
                plan.cwd.clone(), "--sandbox".into(), plan.sandbox.into(), "--ask-for-approval".into(), plan.approval.into()];
            argv.extend(tier_override(tier));
            argv.extend(choice);
            argv
        }
    };
    if let Some(prompt) = some(&plan.prompt) {
        argv.push(prompt.into());
    }
    argv
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn args(name: &str) -> Args { Args { name: name.into(), ..Args::default() } }

    /// Caminho absoluto no sistema do teste: `/w` não é absoluto no Windows.
    fn abs(path: &str) -> String { if cfg!(windows) { format!("C:{}", path.replace('/', "\\")) } else { path.to_owned() } }

    fn here() -> &'static Path { Path::new(if cfg!(windows) { r"C:\w" } else { "/w" }) }

    fn facts<'a>(home: &'a Path, key: Option<&'a str>) -> Facts<'a> {
        Facts { session_key: key, here: here(), inherited_codex_home: None, home }
    }

    fn meta(value: Value) -> Map<String, Value> { value.as_object().unwrap().clone() }

    #[test]
    fn without_choices_the_session_opens_in_full_access_with_the_default_account() {
        let home = abs("/h");
        let home = Path::new(&home);
        let plan = decide(args("cx"), None, &facts(home, None)).unwrap();
        assert_eq!((plan.approval, plan.sandbox, plan.mode), ("never", "danger-full-access", Some("Full Access")));
        assert_eq!((plan.account.as_str(), plan.codex_home.as_path(), plan.cwd.as_str()), ("default", Path::new(&abs("/h/.codex")), abs("/w").as_str()));
        assert!(plan.prepares(true) && !plan.prepares(false));
        // Pasta relativa vira absoluta sem `..`; `CODEX_HOME` herdado é a casa padrão, não outra conta.
        let mut relative = args("cx");
        relative.cwd = Some("a/../b".into());
        let old = abs("/velho");
        let inherited = Facts { inherited_codex_home: Some(&old), ..facts(home, None) };
        let plan = decide(relative, None, &inherited).unwrap();
        assert_eq!((plan.cwd.as_str(), plan.codex_home.as_path(), plan.account.as_str()), (abs("/w/b").as_str(), Path::new(&old), "default"));
        let mut tilde = args("cx");
        tilde.codex_home = Some("~/.codex-x".into());
        tilde.codex_account = Some("x".into());
        let plan = decide(tilde, None, &facts(home, None)).unwrap();
        assert_eq!(plan.codex_home, Path::new(&abs("/h/.codex-x")));
        assert!(plan.prepares(false), "a adicional prepara sempre");
    }

    #[test]
    fn a_pair_outside_the_modes_warns_and_keeps_the_recorded_mode() {
        let mut legacy = args("cx");
        (legacy.approval, legacy.sandbox) = (Some("never"), Some("read-only"));
        let plan = decide(legacy, None, &facts(Path::new("/h"), None)).unwrap();
        assert_eq!(plan.mode, None);
        assert_eq!(plan.mode_warning().unwrap(), "aprovação never com sandbox read-only não é um modo do app; o modo gravado da sessão fica como estava");
    }

    #[test]
    fn the_own_relaunch_completes_from_the_file_and_refuses_an_unknown_mode() {
        let saved = meta(json!({"name":"cx","thread_id":"t-own","key":"k1","model":"gpt-6","effort":"high","service_tier":"priority",
            "codex_home":abs("/h/.codex-x"),"codex_account":"x","permission_mode":"Ask for approval","tool_output_token_limit":144000}));
        let mut relaunch = args("cx");
        relaunch.resume = Some("t-own".into());
        let plan = decide(relaunch.clone(), Some(&saved), &facts(Path::new("/h"), Some("k1"))).unwrap();
        assert_eq!((plan.model.as_deref(), plan.effort.as_deref(), plan.service_tier.as_deref()), (Some("gpt-6"), Some("high"), Some("priority")));
        assert_eq!((plan.account.as_str(), plan.codex_home.as_path(), plan.limit), ("x", Path::new(&abs("/h/.codex-x")), Some(144000)));
        assert_eq!((plan.approval, plan.sandbox), ("on-request", "read-only"));
        // Outra chave não é a própria sessão: só o limite da mesma conversa vem do arquivo.
        let other = decide(relaunch.clone(), Some(&saved), &facts(Path::new("/h"), Some("k2"))).unwrap();
        assert_eq!((other.model, other.account.as_str(), other.approval, other.limit), (None, "default", "never", Some(144000)));
        // Limite gravado fora de forma recusa com frase, em vez de derrubar o lançador ao subir o app-server.
        for limit in [json!(0), json!(-1), json!(1.5), json!("9"), json!(true)] {
            let mut broken = saved.clone();
            broken.insert("tool_output_token_limit".into(), limit.clone());
            assert_eq!(decide(relaunch.clone(), Some(&broken), &facts(Path::new("/h"), Some("k1"))),
                Err(Refusal::Usage("limite de resultado de ferramenta inválido no arquivo da sessão; nenhuma sessão foi aberta".into())), "{limit}");
        }
        for mode in [json!(null), json!("turbo"), json!(" full access")] {
            let mut broken = saved.clone();
            broken.insert("permission_mode".into(), mode.clone());
            assert_eq!(decide(relaunch.clone(), Some(&broken), &facts(Path::new("/h"), Some("k1"))),
                Err(Refusal::Usage("modo de permissão da sessão desconhecido; nenhuma sessão foi aberta".into())), "{mode}");
        }
    }

    #[test]
    fn the_imported_resume_validates_the_file_and_swaps_the_identity() {
        let saved = meta(json!({"name":"cx","thread_id":"t-imp","transfer_id":"tr","key":"k-imp","cwd":abs("/proj"),"codex_home":abs("/h/.codex-x"),
            "codex_account":"x","permission_mode":"Ask for approval","model":"gpt-6"}));
        let mut resume = args("cx");
        resume.resume = Some("t-imp".into());
        let plan = decide(resume.clone(), Some(&saved), &facts(Path::new("/h"), Some("k-operador"))).unwrap();
        assert_eq!(plan.imported_key.as_deref(), Some("k-imp"));
        assert_eq!((plan.cwd.as_str(), plan.account.as_str(), plan.approval, plan.sandbox), (abs("/proj").as_str(), "x", "on-request", "read-only"));
        assert_eq!(plan.model.as_deref(), Some("gpt-6"));
        assert!(!plan.prepares(true), "a importada nunca prepara");
        // Override parcial que forma modo é aceito; par fora dos modos recusa sem tocar em nada.
        let mut partial = resume.clone();
        partial.sandbox = Some("workspace-write");
        assert_eq!(decide(partial, Some(&saved), &facts(Path::new("/h"), None)).unwrap().mode, Some("Approve for me"));
        let mut outside = resume.clone();
        (outside.approval, outside.sandbox) = (Some("never"), Some("read-only"));
        assert_eq!(decide(outside, Some(&saved), &facts(Path::new("/h"), None)),
            Err(Refusal::Usage("combinação de aprovação e sandbox não suportada para conversa importada; sessão preservada".into())));
        for (field, value) in [("key", json!(null)), ("codex_home", json!("rel/x")), ("cwd", json!("")), ("permission_mode", json!("x")),
            ("name", json!("outra"))] {
            let mut broken = saved.clone();
            broken.insert(field.into(), value);
            assert_eq!(decide(resume.clone(), Some(&broken), &facts(Path::new("/h"), None)),
                Err(Refusal::Usage("metadados da conversa importada inválidos; nenhuma sessão foi aberta".into())), "{field}");
        }
    }

    /// Só prompt com quebra de linha num `.cmd`/`.bat` sai do argv (o std a recusa ali); o resto fica.
    #[test]
    fn only_a_multiline_prompt_for_a_batch_codex_leaves_the_argv() {
        let mut plan = decide(args("cx"), None, &facts(Path::new("/h"), None)).unwrap();
        plan.prompt = Some("linha 1\r\nlinha 2".into());
        for (codex, moved) in [("C:/npm/codex.cmd", true), ("C:/npm/CODEX.CMD", true), ("C:/npm/codex.bat", true),
            ("C:/npm/codex.exe", false), ("/usr/bin/codex", false)] {
            assert_eq!(prompt_through_app_server(Path::new(codex), &plan).is_some(), moved, "{codex}");
        }
        assert_eq!(prompt_through_app_server(Path::new("C:/npm/codex.cmd"), &plan).as_deref(), Some("linha 1\r\nlinha 2"));
        for single in [Some("uma linha"), Some(""), None] {
            plan.prompt = single.map(Into::into);
            assert_eq!(prompt_through_app_server(Path::new("C:/npm/codex.cmd"), &plan), None, "{single:?}");
        }
        plan.prompt = None;
        assert!(!tui_argv("C:/npm/codex.cmd", &plan, "ws://127.0.0.1:9", None).iter().any(|arg| arg.contains("linha")));
    }

    #[test]
    fn the_tui_argv_differs_between_a_new_and_a_resumed_conversation() {
        let mut plan = decide(args("cx"), None, &facts(Path::new("/h"), None)).unwrap();
        (plan.model, plan.effort, plan.prompt) = (Some("gpt-6".into()), Some("high".into()), Some("olá".into()));
        assert_eq!(tui_argv("/b/codex", &plan, "ws://127.0.0.1:9", Some("priority")), ["/b/codex", "--remote", "ws://127.0.0.1:9", "--no-alt-screen",
            "-C", abs("/w").as_str(), "--sandbox", "danger-full-access", "--ask-for-approval", "never", "-c", "service_tier=\"priority\"",
            "-c", "check_for_update_on_startup=false", "-m", "gpt-6", "-c", "model_reasoning_effort=\"high\"", "olá"]);
        plan.resume = Some("t-1".into());
        (plan.model, plan.effort, plan.prompt) = (None, Some(String::new()), None);
        assert_eq!(tui_argv("/b/codex", &plan, "ws://127.0.0.1:9", None), ["/b/codex", "resume", "--remote", "ws://127.0.0.1:9", "--no-alt-screen",
            "-c", "check_for_update_on_startup=false", "t-1"]);
    }
}
