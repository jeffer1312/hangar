//! O lançador contra as saídas do lançador Python capturadas (`tests/fixtures/codex_tui/`): recusas,
//! argv do app-server e da TUI, e o ambiente que eles recebem. `$T` vira uma pasta absoluta fixa.
use std::collections::BTreeMap;
use std::path::Path;

use serde_json::Value;

use super::args::{self, Parsed};
use super::{plan, session};

const T: &str = "/T";

fn fixture(name: &str) -> Value {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/codex_tui").join(format!("{name}.json"));
    serde_json::from_str(&std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))).unwrap()
}

fn placed(text: &str) -> String { text.replace("$T", T) }

/// O argv depois de `$PY -B -s $REF/scripts/hangar-codex-tui`.
fn argv(case: &Value) -> Vec<String> {
    case["input"]["argv"].as_array().unwrap().iter().skip(4).map(|a| placed(a.as_str().unwrap())).collect()
}

fn env(case: &Value) -> Vec<(String, String)> {
    case["input"]["env"].as_object().unwrap().iter().map(|(k, v)| (k.clone(), placed(v.as_str().unwrap()))).collect()
}

/// O arquivo da sessão que o caso tinha antes, numa pasta de sidecars própria.
fn sidecars(case: &Value) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    if let Some(file) = case["input"]["files_before"].get("home/.hangar/codex-sessions/codex-proj.json") {
        std::fs::write(dir.path().join("codex-proj.json"), placed(file["content"]["text"].as_str().unwrap())).unwrap();
    }
    dir
}

/// Argumentos e arquivo da sessão até o plano, como o `flow` faz; recusa sai no texto do pane.
fn decide(case: &Value, dir: &Path) -> Result<plan::Plan, String> {
    let env = env(case);
    let name = super::value_of(&env, "CP_SESSION_NAME");
    let args = match args::parse(&argv(case), name).map_err(|refusal| refusal.render())? {
        Parsed::Run(args) => args,
        Parsed::Help => return Err("help".into()),
    };
    let existing = crate::runtime::codex_sidecar::load(&crate::runtime::codex_sidecar::path(dir, &args.name));
    let here = placed(case["input"]["cwd"].as_str().unwrap());
    let home = format!("{T}/home");
    let facts = plan::Facts {
        session_key: super::value_of(&env, "CP_SESSION_KEY"),
        here: Path::new(&here),
        inherited_codex_home: super::value_of(&env, "CODEX_HOME"),
        home: Path::new(&home),
    };
    plan::decide(args, existing.as_ref(), &facts).map_err(|refusal| refusal.render())
}

/// R2-S01: toda recusa antes de qualquer efeito, com a frase e o código 2 do lançador Python.
#[cfg(unix)]
#[test]
fn every_refusal_matches_the_python_launcher() {
    for name in ["refuse-bad-approval", "refuse-bad-sandbox", "refuse-bad-service-tier", "refuse-empty-name-env", "refuse-imported-missing-key",
        "refuse-imported-pair-outside-modes", "refuse-imported-relative-home", "refuse-imported-unknown-mode", "refuse-limit-not-int",
        "refuse-limit-zero", "refuse-no-name", "refuse-own-relaunch-mode-absent", "refuse-own-relaunch-mode-unknown", "refuse-unknown-arg"] {
        let case = fixture(&format!("launcher__{name}"));
        let dir = sidecars(&case);
        assert_eq!(case["output"]["code"], 2, "{name}");
        assert_eq!(decide(&case, dir.path()).map(|plan| plan.name), Err(placed(case["output"]["stderr"]["text"].as_str().unwrap())), "{name}");
    }
}

fn spawned<'a>(case: &'a Value, role: &str) -> Vec<&'a Value> {
    case["spawns"].as_array().unwrap().iter().filter(|spawn| spawn["role"] == role).collect()
}

fn strings(value: &Value) -> Vec<String> { value.as_array().unwrap().iter().map(|a| placed(a.as_str().unwrap())).collect() }

/// R2-S02/S11: o argv do app-server e o da TUI saem iguais aos do lançador Python, na criação, na
/// retomada, no relançamento da própria sessão e na conversa importada.
#[cfg(unix)]
#[test]
fn the_app_server_and_tui_argv_match_the_python_launcher() {
    let endpoint = "ws://127.0.0.1:$WS_PORT";
    for name in ["new-session-sync-off-full-args", "new-session-sync-off-defaults", "new-session-codex-home-inherited", "new-session-no-thread",
        "resume-non-imported", "resume-legacy-pair", "resume-legacy-pair-keeps-mode", "resume-imported-ok", "resume-imported-partial-override",
        "relaunch-own-from-sidecar", "relaunch-other-key-not-own", "name-sanitized", "prepare-secondary-ready"] {
        let case = fixture(&format!("launcher__{name}"));
        let dir = sidecars(&case);
        let plan = decide(&case, dir.path()).unwrap_or_else(|refusal| panic!("{name}: {refusal}"));
        let session = session::Session::new(dir.path().to_path_buf(), &plan, endpoint.into(), None);
        let child = super::child_environment(&env(&case), &plan);
        let codex = format!("{T}/stubs/codex");
        let tier = session.selected_tier();
        let [server] = spawned(&case, "codex app-server")[..] else { panic!("{name}: um app-server") };
        assert_eq!(plan::server_argv(&codex, &plan, endpoint, tier.as_deref(), &child), strings(&server["argv"]), "{name}: app-server");
        // Retomando, a conversa já foi gravada antes da TUI: o Fast da TUI sai do arquivo dela.
        if let Some(resume) = plan.resume.as_deref()
            && let Some(rollout) = session::rollout_of(&plan.codex_home, resume) {
            session.save_thread(resume, &rollout.to_string_lossy(), None, &NoScreen);
        }
        let [tui] = spawned(&case, "codex tui")[..] else { panic!("{name}: uma TUI") };
        assert_eq!(plan::tui_argv(&codex, &plan, endpoint, session.selected_tier().as_deref()), strings(&tui["argv"]), "{name}: TUI");
    }
}

struct NoScreen;
impl super::Screen for NoScreen {
    fn say(&self, _: &str) {}
}

/// R2-S03 e a correção da decisão 15: a conta adicional não leva identidade nenhuma da principal. O
/// lançador Python deixava `CHATGPT_ACCOUNT_ID`, `OPENAI_ACCOUNT_ID`, `CODEX_ACCOUNT_ID` e
/// `CODEX_PROJECT_ID` chegarem ao app-server (`launcher__prepare-secondary-ready`).
#[cfg(unix)]
#[test]
fn the_additional_account_environment_has_no_identity_of_the_main_one() {
    let case = fixture("launcher__prepare-secondary-ready");
    let dir = sidecars(&case);
    let plan = decide(&case, dir.path()).unwrap();
    let child = super::child_environment(&env(&case), &plan);
    let [server] = spawned(&case, "codex app-server")[..] else { panic!() };
    let python: BTreeMap<String, String> = server["env"].as_object().unwrap().iter().map(|(k, v)| (k.clone(), placed(v.as_str().unwrap()))).collect();
    for leaked in ["CHATGPT_ACCOUNT_ID", "OPENAI_ACCOUNT_ID", "CODEX_ACCOUNT_ID", "CODEX_PROJECT_ID"] {
        assert!(python.contains_key(leaked), "o Python deixava {leaked} passar");
        assert!(!child.contains_key(leaked), "{leaked} vazou para a conta adicional");
    }
    for key in ["OPENAI_API_KEY", "OPENAI_BASE_URL", "CODEX_SQLITE_HOME", "CODEX_API_KEY", "OPENAI_ORG_ID", "CHATGPT_SESSION_TOKEN"] {
        assert_eq!(child.contains_key(key), python.contains_key(key), "{key}");
    }
    for key in ["HOME", "USERPROFILE", "CODEX_HOME", "CP_SESSION_NAME", "CP_SESSION_KEY", "TMUX", "TMUX_PANE", "PATH"] {
        assert_eq!(child.get(key), python.get(key), "{key}");
    }
}

/// O ambiente inteiro do app-server igual ao do Python, nas três contas (padrão, adicional e
/// importada). Diferenças aceitas: o `LC_CTYPE` que a captura ganhou do Python sem `LANG`, e as
/// identidades da principal que o Python deixava vazar para a adicional.
#[cfg(unix)]
#[test]
fn the_whole_app_server_environment_matches_the_capture() {
    const LEAKED: [&str; 4] = ["CHATGPT_ACCOUNT_ID", "OPENAI_ACCOUNT_ID", "CODEX_ACCOUNT_ID", "CODEX_PROJECT_ID"];
    for name in ["prepare-secondary-ready", "new-session-sync-off-full-args", "resume-imported-ok", "resume-non-imported"] {
        let case = fixture(&format!("launcher__{name}"));
        let dir = sidecars(&case);
        let plan = decide(&case, dir.path()).unwrap();
        let child = super::child_environment(&env(&case), &plan);
        let [server] = spawned(&case, "codex app-server")[..] else { panic!("{name}") };
        let python: BTreeMap<String, String> = server["env"].as_object().unwrap().iter()
            .filter(|(key, _)| key.as_str() != "LC_CTYPE" && !(plan.account != "default" && LEAKED.contains(&key.as_str())))
            .map(|(k, v)| (k.clone(), placed(v.as_str().unwrap()))).collect();
        assert_eq!(child, python, "{name}");
    }
}

/// A retomada importada troca a identidade: o app-server nasce sem o pane e a chave de quem abriu.
#[cfg(unix)]
#[test]
fn the_imported_resume_carries_the_conversation_identity_only() {
    let case = fixture("launcher__resume-imported-ok");
    let dir = sidecars(&case);
    let plan = decide(&case, dir.path()).unwrap();
    let child = super::child_environment(&env(&case), &plan);
    let [server] = spawned(&case, "codex app-server")[..] else { panic!() };
    let python = server["env"].as_object().unwrap();
    for key in ["CP_SESSION_KEY", "TMUX", "TMUX_PANE", "HANGAR_CANO_KEY", "CODEX_HOME", "HOME", "OPENAI_API_KEY"] {
        assert_eq!(child.get(key).map(String::as_str), python.get(key).and_then(Value::as_str).map(placed).as_deref(), "{key}");
    }
}

/// `hangar-codex-tui --help` (prova do passo de atualização antigo) sai com 0.
#[test]
fn help_is_not_a_refusal() {
    assert_eq!(args::parse(&["--help".to_owned()], None), Ok(Parsed::Help));
    assert!(args::HELP.starts_with(args::USAGE.trim_end()));
}
