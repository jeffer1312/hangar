use super::*;
use super::settings::{section_head, settings_box};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
enum Command {
    FocusComposer, OpenSettings, CopyLastReply, Find, NextSession, PreviousSession, NewChat, NewChatNoProject, NewSession, CloseSession, RenameSession, Costs,
    Search, ProjectFile, ProjectText, Sidebar, Worktrees, Dictation, Permission, ToolLook, Navigation,
    CloseFile, PreviousFile, NextFile, SaveFile, FindFile, FileLine, CopyTerminal, PasteTerminal,
}

impl Command {
    const ALL: [Self; 29] = [Self::FocusComposer, Self::OpenSettings, Self::CopyLastReply, Self::Find,
        Self::NextSession, Self::PreviousSession, Self::NewChat, Self::NewChatNoProject, Self::NewSession, Self::CloseSession, Self::RenameSession, Self::Costs, Self::Search, Self::ProjectFile,
        Self::ProjectText, Self::Sidebar, Self::Worktrees, Self::Dictation, Self::Permission, Self::ToolLook, Self::Navigation,
        Self::CloseFile, Self::PreviousFile, Self::NextFile, Self::SaveFile, Self::FindFile, Self::FileLine,
        Self::CopyTerminal, Self::PasteTerminal];

    fn key(&self) -> &'static str {
        match self {
            Self::FocusComposer => "keyboard_focus_composer", Self::OpenSettings => "keyboard_open_settings",
            Self::CopyLastReply => "keyboard_copy_reply", Self::Find => "keyboard_find", Self::NextSession => "keyboard_next_session",
            Self::PreviousSession => "keyboard_previous_session", Self::NewChat => "keyboard_new_chat", Self::NewChatNoProject => "keyboard_new_chat_no_project", Self::NewSession => "keyboard_new_session", Self::CloseSession => "keyboard_close_session",
            Self::RenameSession => "keyboard_rename_session", Self::Costs => "keyboard_costs",
            Self::Search => "keyboard_search", Self::ProjectFile => "keyboard_project_file", Self::ProjectText => "keyboard_project_text",
            Self::Sidebar => "keyboard_sidebar", Self::Worktrees => "keyboard_worktrees", Self::Dictation => "keyboard_dictation",
            Self::Permission => "keyboard_permission", Self::ToolLook => "keyboard_tool_look", Self::Navigation => "keyboard_navigation",
            Self::CloseFile => "keyboard_close_file", Self::PreviousFile => "keyboard_previous_file",
            Self::NextFile => "keyboard_next_file", Self::SaveFile => "keyboard_save_file", Self::FindFile => "keyboard_find_file",
            Self::FileLine => "keyboard_file_line", Self::CopyTerminal => "keyboard_copy_terminal", Self::PasteTerminal => "keyboard_paste_terminal",
        }
    }

    fn context(&self) -> &'static str {
        match self {
            Self::CloseFile | Self::PreviousFile | Self::NextFile | Self::SaveFile | Self::FindFile | Self::FileLine => "FileViewer",
            Self::CopyTerminal | Self::PasteTerminal => "Terminal",
            _ => "!Terminal",
        }
    }

    /// Onde a tecla dispara; `context` continua sendo o grupo da tela e a chave de conflito.
    fn predicate(&self) -> &'static str {
        match self {
            // Ctrl+W num campo de texto apaga a palavra, e na página do navegador é da página. No macOS a tecla é Cmd+W,
            // que não edita texto: fecha a sessão de qualquer lugar fora do terminal.
            Self::CloseSession if cfg!(target_os = "macos") => "!Terminal",
            Self::CloseSession => "!Terminal && !Input && !BrowserPage",
            // F2 na página do navegador é da página.
            Self::RenameSession => "!Terminal && !BrowserPage",
            // No painel do navegador, Ctrl+L vai ao endereço, como em todo navegador.
            Self::FocusComposer => "!Terminal && !BrowserPanel",
            // Ditar não depende de onde está o foco: o texto vai para o Composer, mesmo saindo do terminal.
            Self::Dictation => "Terminal || !Terminal",
            _ => self.context(),
        }
    }

    fn default_key(&self) -> &'static str {
        match self {
            Self::FocusComposer => "secondary-l", Self::OpenSettings => "secondary-,", Self::CopyLastReply => "secondary-shift-c",
            Self::Find => "secondary-f", Self::NextSession => "secondary-down", Self::PreviousSession => "secondary-up",
            Self::NewChat => "secondary-n", Self::NewChatNoProject => "secondary-shift-n", Self::NewSession => "secondary-shift-t", Self::CloseSession => "secondary-w", Self::RenameSession => "f2", Self::Costs => "secondary-alt-c", Self::Search => "secondary-k",
            Self::ProjectFile => "secondary-p", Self::ProjectText => "secondary-shift-f", Self::Sidebar => "secondary-b",
            Self::Worktrees => "secondary-alt-w", Self::Dictation => "ctrl-space", Self::Permission => "alt-shift-p",
            Self::ToolLook => "secondary-alt-v", Self::Navigation => "secondary-alt-b",
            Self::CloseFile => "alt-w", Self::PreviousFile => "ctrl-pageup", Self::NextFile => "ctrl-pagedown",
            Self::SaveFile => "secondary-s", Self::FindFile => "secondary-f", Self::FileLine => "secondary-g",
            Self::CopyTerminal => "ctrl-shift-c", Self::PasteTerminal => "ctrl-shift-v",
        }
    }

    fn action(&self) -> Box<dyn gpui_kit::Action> {
        match self {
            Self::FocusComposer => Box::new(FocusComposer), Self::OpenSettings => Box::new(OpenSettings),
            Self::CopyLastReply => Box::new(CopyLastReply), Self::Find => Box::new(FocusSettingsSearch),
            Self::NextSession => Box::new(NextSession), Self::PreviousSession => Box::new(PreviousSession), Self::NewChat => Box::new(NewChat), Self::NewChatNoProject => Box::new(NewChatNoProject), Self::NewSession => Box::new(OpenNewSession), Self::CloseSession => Box::new(CloseSession),
            Self::RenameSession => Box::new(RenameSession),
            Self::Costs => Box::new(OpenCosts), Self::Search => Box::new(OpenSearch), Self::ProjectFile => Box::new(FindProjectFile),
            Self::ProjectText => Box::new(FindProjectText), Self::Sidebar => Box::new(ToggleSidebar), Self::Worktrees => Box::new(OpenWorktrees),
            Self::Dictation => Box::new(ToggleDictation), Self::Permission => Box::new(CyclePermission),
            Self::ToolLook => Box::new(CycleToolLook), Self::Navigation => Box::new(CycleNavigation),
            Self::CloseFile => Box::new(files::CloseFile), Self::PreviousFile => Box::new(files::PreviousFile),
            Self::NextFile => Box::new(files::NextFile), Self::SaveFile => Box::new(files::SaveFile), Self::FindFile => Box::new(files::FindFile),
            Self::FileLine => Box::new(files::GoToFileLine), Self::CopyTerminal => Box::new(terminal::CopyTerminal),
            Self::PasteTerminal => Box::new(terminal::PasteTerminal),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
struct ShortcutBinding { server: String, project: Option<String>, id: String, label: String, key: String }

impl ShortcutBinding {
    fn same_target(&self, other: &Self) -> bool { self.server == other.server && self.project == other.project && self.id == other.id }
    fn action(&self) -> RunShortcut { RunShortcut { server: self.server.clone(), project: self.project.clone(), id: self.id.clone() } }
}

struct BindingRow { binding: ShortcutBinding, listed: bool }

fn binding_rows(listed: &[ShortcutBinding], saved: &[ShortcutBinding]) -> Vec<BindingRow> {
    let mut rows: Vec<_> = listed.iter().cloned().map(|mut binding| {
        if let Some(saved) = saved.iter().find(|item| item.same_target(&binding)) { binding.key = saved.key.clone(); }
        BindingRow { binding, listed: true }
    }).collect();
    for binding in saved {
        if !listed.iter().any(|item| item.same_target(binding)) { rows.push(BindingRow { binding: binding.clone(), listed: false }); }
    }
    rows
}

#[derive(Clone, PartialEq, gpui_kit::Action)]
#[action(namespace = hangar, no_json)]
pub(super) struct RunShortcut { pub(super) server: String, pub(super) project: Option<String>, pub(super) id: String }

#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
struct Config {
    overrides: BTreeMap<Command, String>, shortcuts: Vec<ShortcutBinding>, hold: Modifiers, physical: BTreeMap<String, Modifiers>,
    // Padrões cedidos na leitura: fora do arquivo, para o padrão voltar quando a tecla da pessoa sair.
    #[serde(skip)] yielded: BTreeSet<Command>,
}

impl Default for Config {
    fn default() -> Self { Self { overrides: BTreeMap::new(), shortcuts: Vec::new(), hold: Modifiers { control: true, shift: true, ..Modifiers::none() }, physical: BTreeMap::new(), yielded: BTreeSet::new() } }
}

// Quem lê e grava atalho sem `App` à mão (a config, os testes) precisa do sistema para resolver `secondary-`.
const HOST_OS: gpui_kit::OperatingSystem = if cfg!(target_os = "macos") {
    gpui_kit::OperatingSystem::Mac
} else if cfg!(target_os = "windows") {
    gpui_kit::OperatingSystem::Windows
} else {
    gpui_kit::OperatingSystem::Linux
};

fn parse_key(source: &str) -> Result<Keystroke, gpui_kit::InvalidKeystrokeError> {
    Keystroke::parse_with_os(source, HOST_OS)
}

fn canonical_key(source: &str) -> Result<String, String> {
    if source.split_whitespace().count() != 1 { return Err(tr("keyboard_invalid_key")); }
    let key = parse_key(source).map_err(|_| tr("keyboard_invalid_key"))?;
    let function = key.key.strip_prefix('f').and_then(|n| n.parse::<u8>().ok()).is_some_and(|n| (1..=24).contains(&n));
    let named = matches!(key.key.as_str(), "backspace" | "delete" | "escape" | "enter" | "tab" | "space" | "left" | "right" | "up" | "down"
        | "home" | "end" | "pageup" | "pagedown" | "insert");
    if (!function && !named && key.key.chars().count() != 1)
        || (!function && !key.modifiers.control && !key.modifiers.alt && !key.modifiers.platform) { return Err(tr("keyboard_invalid_key")); }
    Ok(key.unparse(HOST_OS))
}

fn captured_stroke(event: &KeyDownEvent, mapper: &dyn PlatformKeyboardMapper) -> Result<KeybindingKeystroke, String> {
    if event.prefer_character_input { return Err(tr("keyboard_invalid_key")); }
    Ok(KeybindingKeystroke::new_with_mapper(event.keystroke.clone(), false, mapper))
}

fn number_key(key: &str) -> bool { key.len() == 1 && key.as_bytes()[0].is_ascii_digit() }

fn reserved_number_key(key: &str, physical: Modifiers, hold: Modifiers, layouts: [&str; 2]) -> bool {
    physical == hold && (super::session_numbers::layout_digit(key, layouts).is_some() || matches!(key, "enter" | "escape" | "backspace"))
}

fn key_label(source: &str) -> String {
    let Ok(key) = parse_key(source) else { return source.to_owned(); };
    let mut parts = modifier_labels(key.modifiers);
    let text = match key.key.as_str() {
        "space" => tr("keyboard_space"), "up" => "↑".into(), "down" => "↓".into(), "left" => "←".into(), "right" => "→".into(),
        "escape" => "Esc".into(), "pageup" => "PageUp".into(), "pagedown" => "PageDown".into(),
        other if other.chars().count() == 1 || other.strip_prefix('f').is_some_and(|n| n.parse::<u8>().is_ok()) => other.to_uppercase(),
        other => other.to_owned(),
    };
    parts.push(text);
    parts.join("+")
}

fn display_key(source: &str, physical: Option<Modifiers>, cx: &App) -> String {
    let Ok(key) = parse_key(source) else { return source.to_owned(); };
    let mapped = KeybindingKeystroke::new_with_mapper(key, false, cx.keyboard_mapper().as_ref());
    let mut shown = mapped.inner().clone();
    shown.modifiers = physical.unwrap_or(*mapped.modifiers());
    shown.key = mapped.key().to_owned();
    if shown.modifiers.shift {
        let layout = cx.keyboard_layout();
        if let Some(digit) = super::session_numbers::layout_digit(&shown.key, [layout.id(), layout.name()]) { shown.key = digit.to_string(); }
    }
    key_label(&shown.unparse(HOST_OS))
}

fn modifier_labels(modifiers: Modifiers) -> Vec<String> {
    let mut parts = Vec::new();
    if modifiers.control { parts.push("Ctrl".into()); }
    if modifiers.alt { parts.push("Alt".into()); }
    if modifiers.shift { parts.push("Shift".into()); }
    if modifiers.platform { parts.push(if cfg!(target_os = "macos") { "Cmd" } else if cfg!(target_os = "windows") { "Win" } else { "Super" }.into()); }
    parts
}

impl Config {
    fn key(&self, command: &Command) -> &str { self.overrides.get(command).map_or(command.default_key(), String::as_str) }

    fn keys(&self, command: &Command) -> Vec<&str> {
        let key = self.key(command);
        if key.is_empty() || (!self.overrides.contains_key(command) && self.yielded.contains(command)) { return Vec::new(); }
        let mut keys = vec![key];
        if cfg!(target_os = "macos") && !self.overrides.contains_key(command) {
            match command { Command::CopyTerminal => keys.push("cmd-c"), Command::PasteTerminal => keys.push("cmd-v"), _ => {} }
        }
        keys
    }

    /// Na leitura, o padrão que cai numa tecla que a pessoa já usa no mesmo contexto fica sem tecla: um atalho padrão novo
    /// não pode invalidar a configuração inteira. Na edição o conflito continua sendo erro.
    fn yield_defaults(&mut self) {
        for command in Command::ALL {
            if !self.overrides.contains_key(&command) && self.default_taken(&command) { self.yielded.insert(command); }
        }
    }

    /// Ao salvar, o cedido cujo conflito saiu volta a ter o padrão; nenhum padrão novo é cedido fora da leitura.
    fn release_yielded(&mut self) {
        self.yielded = self.yielded.iter().filter(|command| self.default_taken(command)).cloned().collect();
    }

    fn default_taken(&self, command: &Command) -> bool {
        let Ok(default) = canonical_key(command.default_key()) else { return false; };
        let same = |key: &str| canonical_key(key).is_ok_and(|key| key == default);
        self.overrides.iter().any(|(other, key)| other != command && other.context() == command.context() && same(key))
            || (command.context() == "!Terminal" && self.shortcuts.iter().any(|shortcut| same(&shortcut.key)))
    }

    fn validate(&self) -> Result<(), String> {
        if self.hold.function || self.hold.number_of_modifiers() != 2 { return Err(tr("keyboard_hold_invalid")); }
        let mut seen = HashMap::new();
        let mut check = |context: &str, key: &str, label: String| -> Result<(), String> {
            if key.is_empty() { return Ok(()); }
            let key = canonical_key(key)?;
            let stroke = parse_key(&key).map_err(|_| tr("keyboard_invalid_key"))?;
            if stroke.modifiers == self.hold && (number_key(&stroke.key) || matches!(stroke.key.as_str(), "enter" | "escape" | "backspace")) {
                return Err(tr("keyboard_number_reserved").replace("{key}", &key_label(&key)));
            }
            if let Some(previous) = seen.insert((context.to_owned(), key.clone()), label.clone()) {
                return Err(tr("keyboard_conflict").replace("{key}", &key_label(&key)).replace("{action}", &previous).replace("{other_action}", &label));
            }
            Ok(())
        };
        for command in Command::ALL { for key in self.keys(&command) { check(command.context(), key, tr(command.key()))?; } }
        let mut targets = HashSet::new();
        for shortcut in &self.shortcuts {
            if shortcut.server.is_empty() || shortcut.id.is_empty() || shortcut.key.is_empty()
                || shortcut.project.as_deref() == Some("") || !targets.insert((shortcut.server.clone(), shortcut.project.clone(), shortcut.id.clone())) {
                return Err(tr("keyboard_invalid_config"));
            }
            check("!Terminal", &shortcut.key, shortcut.label.clone())?;
        }
        Ok(())
    }

    fn validate_runtime(&self, cx: &App) -> Result<(), String> {
        self.validate()?;
        let layout = cx.keyboard_layout();
        let mut seen = HashMap::new();
        let mut check = |context: &str, source: &str, label: String| -> Result<(), String> {
            if source.is_empty() { return Ok(()); }
            let parsed = parse_key(source).map_err(|_| tr("keyboard_invalid_key"))?;
            let key = KeybindingKeystroke::new_with_mapper(parsed, false, cx.keyboard_mapper().as_ref());
            let physical = self.physical.get(&canonical_key(source)?).copied().unwrap_or_else(|| {
                let mut modifiers = *key.modifiers();
                if !number_key(key.key()) && super::session_numbers::layout_digit(key.key(), [layout.id(), layout.name()]).is_some() { modifiers.shift = true; }
                modifiers
            });
            if reserved_number_key(key.key(), physical, self.hold, [layout.id(), layout.name()]) {
                return Err(tr("keyboard_number_reserved").replace("{key}", &display_key(source, Some(physical), cx)));
            }
            if let Some(previous) = seen.insert((context.to_owned(), key.inner().unparse(HOST_OS)), label.clone()) {
                return Err(tr("keyboard_conflict").replace("{key}", &key_label(source)).replace("{action}", &previous).replace("{other_action}", &label));
            }
            Ok(())
        };
        for command in Command::ALL { for key in self.keys(&command) { check(command.context(), key, tr(command.key()))?; } }
        for shortcut in &self.shortcuts { check("!Terminal", &shortcut.key, shortcut.label.clone())?; }
        let current: Vec<_> = cx.key_bindings().borrow().bindings().cloned().collect();
        validate_effective_bindings(self, &current, cx.keyboard_mapper().as_ref())
    }

    fn prune_physical(&mut self) {
        let used: HashSet<String> = Command::ALL.iter().flat_map(|command| self.keys(command)).chain(self.shortcuts.iter().map(|shortcut| shortcut.key.as_str()))
            .filter_map(|key| canonical_key(key).ok()).collect();
        self.physical.retain(|key, _| used.contains(key));
    }

    fn bindings(&self, mapper: &dyn PlatformKeyboardMapper) -> Result<Vec<KeyBinding>, String> {
        self.validate()?;
        let mut bindings = Vec::new();
        for command in Command::ALL {
            for key in self.keys(&command) { bindings.push(load_binding(key, command.action(), command.predicate(), mapper)?); }
        }
        for shortcut in &self.shortcuts { bindings.push(load_binding(&shortcut.key, Box::new(shortcut.action()), "!Terminal", mapper)?); }
        Ok(bindings)
    }
}

fn load_binding(key: &str, action: Box<dyn gpui_kit::Action>, context: &str, mapper: &dyn PlatformKeyboardMapper) -> Result<KeyBinding, String> {
    let predicate = KeyBindingContextPredicate::parse(context).map_err(|error| error.to_string())?;
    KeyBinding::load_with_mapper(key, action, Some(predicate.into()), false, None, HOST_OS, mapper).map_err(|error| error.to_string())
}

fn managed(action: &dyn gpui_kit::Action) -> bool {
    action.as_any().is::<RunShortcut>() || Command::ALL.iter().any(|command| command.action().as_any().type_id() == action.as_any().type_id())
}

fn scope_contexts() -> Vec<Vec<KeyContext>> {
    let root = KeyContext::new_with_defaults(HOST_OS);
    let mut file = root.clone();
    file.add("FileViewer");
    let mut terminal = root.clone();
    terminal.add("Terminal");
    vec![vec![root], vec![file], vec![terminal]]
}

fn binding_applies(binding: &KeyBinding, contexts: &[KeyContext]) -> bool {
    binding.predicate().is_none_or(|predicate| predicate.depth_of(contexts).is_some())
}

fn keys_match(a: &KeyBinding, b: &KeyBinding) -> bool {
    a.keystrokes().len() == b.keystrokes().len() && a.keystrokes().iter().zip(b.keystrokes())
        .all(|(a, b)| a.inner().key == b.inner().key && a.inner().modifiers == b.inner().modifiers)
}

fn original_binding(binding: &KeyBinding, defaults: &[KeyBinding]) -> bool {
    defaults.iter().any(|original| original.action().partial_eq(binding.action()) && original.predicate() == binding.predicate() && keys_match(original, binding))
}

fn binding_label(binding: &KeyBinding, config: &Config) -> String {
    for command in Command::ALL {
        if command.action().as_any().type_id() == binding.action().as_any().type_id() { return tr(command.key()); }
    }
    if let Some(action) = binding.action().as_any().downcast_ref::<RunShortcut>() {
        if let Some(shortcut) = config.shortcuts.iter().find(|shortcut| shortcut.server == action.server && shortcut.project == action.project && shortcut.id == action.id) {
            return if shortcut.label.is_empty() { shortcut.id.clone() } else { shortcut.label.clone() };
        }
    }
    tr("keyboard_actions")
}

fn validate_effective_bindings(config: &Config, current: &[KeyBinding], mapper: &dyn PlatformKeyboardMapper) -> Result<(), String> {
    let owned = config.bindings(mapper)?;
    let defaults = Config::default().bindings(mapper)?;
    let original: Vec<_> = owned.iter().map(|binding| original_binding(binding, &defaults)).collect();
    let scopes = scope_contexts();
    let mut seen: HashMap<String, Vec<usize>> = HashMap::new();
    for (ix, binding) in owned.iter().enumerate() {
        let signature = binding.keystrokes().iter().map(|key| key.inner().unparse(HOST_OS)).collect::<Vec<_>>().join(" ");
        let matching = seen.entry(signature.clone()).or_default();
        for &other in matching.iter() {
            if scopes.iter().any(|contexts| binding_applies(binding, contexts) && binding_applies(&owned[other], contexts))
                && !(original[ix] && original[other]) {
                return Err(tr("keyboard_conflict").replace("{key}", &key_label(&signature)).replace("{action}", &binding_label(&owned[other], config))
                    .replace("{other_action}", &binding_label(binding, config)));
            }
        }
        matching.push(ix);
    }
    let library = Keymap::new(current.iter().filter(|binding| !managed(binding.action())).cloned().collect());
    for (ix, binding) in owned.iter().enumerate().filter(|(ix, _)| !original[*ix]) {
        let input: Vec<_> = binding.keystrokes().iter().map(|key| key.inner().clone()).collect();
        for mut contexts in scopes.iter().filter(|contexts| binding_applies(binding, contexts)).cloned() {
            if contexts.iter().any(|context| context.contains("Terminal")) { continue; }
            let mut editing = KeyContext::new_with_defaults(HOST_OS);
            editing.add("Input");
            contexts.push(editing);
            let (matches, pending) = library.bindings_for_input(&input, &contexts);
            if !matches.is_empty() || pending {
                let key = owned[ix].keystrokes().iter().map(|key| key.unparse(HOST_OS)).collect::<Vec<_>>().join(" ");
                return Err(tr("keyboard_editing_conflict").replace("{key}", &key_label(&key)));
            }
        }
    }
    Ok(())
}

fn replacement_bindings(current: &[KeyBinding], config: &Config, mapper: &dyn PlatformKeyboardMapper) -> Result<Vec<KeyBinding>, String> {
    let mut bindings: Vec<_> = current.iter().filter(|binding| !managed(binding.action())).cloned().collect();
    bindings.extend(config.bindings(mapper)?);
    Ok(bindings)
}

fn config_path() -> Result<PathBuf, String> {
    appearance::dir().map(|dir| dir.join("keyboard.json")).ok_or_else(|| tr("keyboard_no_directory"))
}

fn load_config() -> Result<Config, String> {
    let path = config_path()?;
    let mut config: Config = match std::fs::read(&path) {
        Ok(bytes) => serde_json::from_slice(&bytes).map_err(|error| format!("{}: {error}", path.display()))?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Config::default(),
        Err(error) => return Err(format!("{}: {error}", path.display())),
    };
    config.yield_defaults();
    config.validate()?;
    Ok(config)
}

/// Tira do caminho um keyboard.json que não valida, guardando-o ao lado para o usuário conferir.
fn set_aside_config(path: &std::path::Path, stamp: u64) -> Result<Option<PathBuf>, String> {
    let aside = path.with_file_name(format!("keyboard.json.bad-{stamp}"));
    match std::fs::rename(path, &aside) {
        Ok(()) => Ok(Some(aside)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(format!("{}: {error}", path.display())),
    }
}

fn save_config(config: &Config) -> Result<(), String> {
    config.validate()?;
    let path = config_path()?;
    let dir = path.parent().ok_or_else(|| tr("keyboard_no_directory"))?;
    let bytes = serde_json::to_vec_pretty(config).map_err(|error| error.to_string())?;
    std::fs::create_dir_all(dir).map_err(|error| error.to_string())?;
    let temporary = path.with_extension("json.tmp");
    std::fs::write(&temporary, bytes).and_then(|_| std::fs::rename(&temporary, &path)).map_err(|error| error.to_string())
}

#[derive(Clone)]
enum Target { Command(Command), Shortcut(ShortcutBinding), Hold }

struct Edit { target: Target, key: Option<String>, physical: Option<Modifiers>, hold: Option<Modifiers>, origin: Option<WeakFocusHandle> }

pub(super) struct Keyboard {
    config: Config,
    loading: bool,
    load_error: Option<String>,
    save_error: Option<String>,
    saving: bool,
    edit: Option<Edit>,
    shortcut_running: bool,
}

impl Keyboard {
    pub(super) fn new(window: &mut Window, cx: &mut Context<Hangar>) -> Self {
        let mut state = Self { config: Config::default(), loading: true, load_error: None, save_error: None, saving: false, edit: None, shortcut_running: false };
        if let Err(error) = state.apply(cx) { state.load_error = Some(error); }
        begin_load(window, cx);
        state
    }

    pub(super) fn is_editing(&self) -> bool { self.edit.is_some() }
    pub(super) fn cancel_edit(&mut self) { self.edit = None; }
    pub(super) fn hold_modifiers(&self) -> Modifiers { self.config.hold }

    fn apply(&self, cx: &mut App) -> Result<(), String> {
        self.config.validate_runtime(cx)?;
        let current: Vec<_> = cx.key_bindings().borrow().bindings().cloned().collect();
        let bindings = replacement_bindings(&current, &self.config, cx.keyboard_mapper().as_ref())?;
        cx.clear_key_bindings();
        cx.bind_keys(bindings);
        Ok(())
    }

    fn busy(&self) -> bool { self.loading || self.saving || self.load_error.is_some() }

    fn key_label(&self, source: &str, cx: &App) -> String {
        let physical = canonical_key(source).ok().and_then(|key| self.config.physical.get(&key).copied());
        display_key(source, physical, cx)
    }

    fn candidate(&self) -> Result<Config, String> {
        let edit = self.edit.as_ref().ok_or_else(|| tr("keyboard_press_keys"))?;
        let mut config = self.config.clone();
        match &edit.target {
            Target::Command(command) => { config.overrides.insert(command.clone(), edit.key.clone().ok_or_else(|| tr("keyboard_press_keys"))?); }
            Target::Shortcut(shortcut) => {
                let mut shortcut = shortcut.clone();
                shortcut.key = edit.key.clone().ok_or_else(|| tr("keyboard_press_keys"))?;
                config.shortcuts.retain(|item| !item.same_target(&shortcut));
                config.shortcuts.push(shortcut);
            }
            Target::Hold => config.hold = edit.hold.ok_or_else(|| tr("keyboard_hold_press"))?,
        }
        if let (Some(key), Some(physical)) = (&edit.key, edit.physical) { config.physical.insert(key.clone(), physical); }
        config.prune_physical();
        config.validate()?;
        Ok(config)
    }
}

fn begin_load(window: &mut Window, cx: &mut Context<Hangar>) {
    let read = cx.background_executor().spawn(async move { load_config() });
    cx.spawn_in(window, async move |this, cx| {
        let result = read.await;
        let _ = this.update_in(cx, |this, window, cx| {
            this.keyboard.loading = false;
            match result.and_then(|config| config.validate_runtime(cx).map(|_| config)) {
                Ok(config) => { this.keyboard.config = config; this.keyboard.load_error = this.keyboard.apply(cx).err(); }
                Err(error) => this.keyboard.load_error = Some(error),
            }
            if let Some(error) = &this.keyboard.load_error {
                window.push_notification(Notification::warning(tr("keyboard_load_failed").replace("{error}", error)), cx);
            }
            cx.notify();
        });
    }).detach();
}

impl Hangar {
    fn open_keyboard_edit(&mut self, target: Target, window: &mut Window, cx: &mut Context<Self>) {
        if self.keyboard.busy() { return; }
        self.keyboard.save_error = None;
        self.keyboard.edit = Some(Edit { target, key: None, physical: None, hold: None, origin: window.focused(cx).map(|focus| focus.downgrade()) });
        self.root_focus.focus(window, cx);
        cx.notify();
    }

    fn save_keyboard(&mut self, mut config: Config, window: &mut Window, cx: &mut Context<Self>) {
        if self.keyboard.busy() { return; }
        config.prune_physical();
        config.release_yielded();
        if let Err(error) = config.validate_runtime(cx) { self.keyboard.save_error = Some(error); cx.notify(); return; }
        self.keyboard.saving = true;
        self.keyboard.save_error = None;
        let saved = config.clone();
        let write = cx.background_executor().spawn(async move { save_config(&saved) });
        cx.spawn_in(window, async move |this, cx| {
            let result = write.await;
            let _ = this.update_in(cx, |this, window, cx| {
                this.keyboard.saving = false;
                match result {
                    Ok(()) => {
                        this.keyboard.config = config;
                        match this.keyboard.apply(cx) {
                            Ok(()) => { this.keyboard_escape(window, cx); }
                            Err(error) => this.keyboard.save_error = Some(error),
                        }
                    }
                    Err(error) => this.keyboard.save_error = Some(tr("keyboard_save_failed").replace("{error}", &error)),
                }
                if let Some(error) = &this.keyboard.save_error {
                    window.push_notification(Notification::warning(error.clone()), cx);
                }
                cx.notify();
            });
        }).detach();
        cx.notify();
    }

    fn reset_keyboard_config(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.keyboard.loading || self.keyboard.saving { return; }
        self.keyboard.saving = true;
        self.keyboard.save_error = None;
        let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |elapsed| elapsed.as_secs());
        let job = cx.background_executor().spawn(async move { set_aside_config(&config_path()?, stamp) });
        cx.spawn_in(window, async move |this, cx| {
            let result = job.await;
            let _ = this.update_in(cx, |this, window, cx| {
                this.keyboard.saving = false;
                match result {
                    Ok(aside) => {
                        if let Some(aside) = aside {
                            window.push_notification(Notification::success(tr("keyboard_reset_done").replace("{path}", &aside.display().to_string())), cx);
                        }
                        this.keyboard.loading = true;
                        begin_load(window, cx);
                    }
                    Err(error) => {
                        let error = tr("keyboard_reset_failed").replace("{error}", &error);
                        window.push_notification(Notification::warning(error.clone()), cx);
                        this.keyboard.save_error = Some(error);
                    }
                }
                cx.notify();
            });
        }).detach();
        cx.notify();
    }

    fn restore_keyboard_target(&mut self, target: Target, window: &mut Window, cx: &mut Context<Self>) {
        let mut config = self.keyboard.config.clone();
        match target {
            Target::Command(command) => { config.yielded.remove(&command); config.overrides.remove(&command); }
            Target::Shortcut(shortcut) => config.shortcuts.retain(|item| !item.same_target(&shortcut)),
            Target::Hold => config.hold = Config::default().hold,
        }
        self.save_keyboard(config, window, cx);
    }

    fn disable_keyboard_target(&mut self, target: Target, window: &mut Window, cx: &mut Context<Self>) {
        let mut config = self.keyboard.config.clone();
        match target {
            Target::Command(command) => { config.overrides.insert(command, String::new()); }
            Target::Shortcut(shortcut) => config.shortcuts.retain(|item| !item.same_target(&shortcut)),
            Target::Hold => return,
        }
        self.save_keyboard(config, window, cx);
    }

    pub(super) fn keyboard_escape(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if self.keyboard.edit.is_none() { return false; }
        if self.keyboard.saving { return true; }
        let edit = self.keyboard.edit.take();
        self.keyboard.save_error = None;
        if let Some(focus) = edit.and_then(|edit| edit.origin).and_then(|origin| origin.upgrade()) { focus.focus(window, cx); }
        cx.notify();
        true
    }

    pub(super) fn keyboard_key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if self.keyboard.edit.is_none() {
            if !event.is_held || event.prefer_character_input || window.context_stack().iter().any(|context| context.contains("Terminal")) { return false; }
            // A repetição do Ctrl+V (no RDP o soltar da tecla chega atrasado) colaria tudo de novo.
            if is_paste(&event.keystroke) { return true; }
            let event = KeybindingKeystroke::new_with_mapper(event.keystroke.clone(), false, cx.keyboard_mapper().as_ref());
            return self.keyboard.config.shortcuts.iter().any(|binding| parse_key(&binding.key).ok().is_some_and(|key| {
                let key = KeybindingKeystroke::new_with_mapper(key, false, cx.keyboard_mapper().as_ref());
                key.inner().key == event.inner().key && key.inner().modifiers == event.inner().modifiers
            }));
        }
        if event.keystroke.key == "escape" { self.keyboard_escape(window, cx); return true; }
        if self.keyboard.saving { return true; }
        let stroke = match captured_stroke(event, cx.keyboard_mapper().as_ref()) {
            Ok(stroke) => stroke,
            Err(error) => {
                if let Some(edit) = &mut self.keyboard.edit { edit.key = None; edit.physical = None; edit.hold = None; }
                self.keyboard.save_error = Some(error);
                cx.notify();
                return true;
            }
        };
        if event.keystroke.key == "enter" && !event.keystroke.modifiers.modified() {
            match self.keyboard.candidate() {
                Ok(config) => self.save_keyboard(config, window, cx),
                Err(error) => { self.keyboard.save_error = Some(error); cx.notify(); }
            }
            return true;
        }
        if event.is_held { return true; }
        let edit = self.keyboard.edit.as_mut().expect("editor checked");
        if matches!(edit.target, Target::Hold) {
            if !matches!(event.keystroke.key.as_str(), "shift" | "control" | "alt" | "platform" | "super" | "cmd" | "win") {
                self.keyboard.save_error = Some(tr("keyboard_hold_invalid"));
                cx.notify();
            }
            return true;
        }
        if matches!(event.keystroke.key.as_str(), "shift" | "control" | "alt" | "platform" | "super" | "cmd" | "win") { return true; }
        let layout = cx.keyboard_layout();
        let physical = window.modifiers();
        if reserved_number_key(stroke.key(), physical, self.keyboard.config.hold, [layout.id(), layout.name()]) {
            edit.key = None;
            edit.physical = None;
            self.keyboard.save_error = Some(tr("keyboard_number_reserved").replace("{key}", &display_key(&stroke.unparse(HOST_OS), Some(physical), cx)));
        } else {
            match canonical_key(&stroke.unparse(HOST_OS)) {
                Ok(key) => { edit.key = Some(key); edit.physical = Some(physical); self.keyboard.save_error = None; }
                Err(error) => { edit.key = None; edit.physical = None; self.keyboard.save_error = Some(error); }
            }
        }
        cx.notify();
        true
    }

    pub(super) fn keyboard_modifiers_changed(&mut self, event: &ModifiersChangedEvent, _: &mut Window, cx: &mut Context<Self>) {
        if self.keyboard.saving { return; }
        let Some(edit) = self.keyboard.edit.as_mut().filter(|edit| matches!(edit.target, Target::Hold)) else { return; };
        if !event.modifiers.function && event.modifiers.number_of_modifiers() == 2 {
            edit.hold = Some(event.modifiers);
            self.keyboard.save_error = None;
            cx.notify();
        } else if event.modifiers.number_of_modifiers() > 2 || event.modifiers.function {
            edit.hold = None;
            self.keyboard.save_error = Some(tr("keyboard_hold_invalid"));
            cx.notify();
        }
    }

    pub(super) fn run_keyboard_shortcut(&mut self, action: &RunShortcut, window: &mut Window, cx: &mut Context<Self>) {
        if self.keyboard.shortcut_running || self.keyboard.is_editing() || self.connection_dialog || window.has_active_dialog(cx)
            || self.page_open() { return; }
        let (Some(key), Some(api)) = (self.selected_key(), self.session_api()) else { return; };
        if servers::norm(&key.server) != action.server {
            window.push_notification(Notification::warning(tr("keyboard_wrong_server")), cx);
            return;
        }
        self.keyboard.shortcut_running = true;
        let (request, selection) = (action.clone(), self.selection);
        let name = key.name.clone();
        let job = self.runtime.spawn(async move {
            if let Some(project) = request.project.as_deref() {
                let value = api.read(&name, &["project-shortcuts"], &[], 15).await.map_err(|error| Hangar::fetch_failure(&error))?;
                let current = side::Project::parse(&value);
                if current.key != project { return Err(tr("keyboard_wrong_project")); }
                current.items.iter().find(|item| item.id() == request.id).and_then(|item| keyboard_shortcut(item, Some(project)))
                    .ok_or_else(|| tr("keyboard_shortcut_missing"))
            } else {
                let value = api.config().await.map_err(|error| Hangar::fetch_failure(&error))?;
                let items = shortcuts::resolve(value.pointer("/campos/shortcuts/valor").and_then(Value::as_str).unwrap_or(""));
                items.iter().find(|item| item.id() == request.id).and_then(|item| keyboard_shortcut(item, None))
                    .ok_or_else(|| tr("keyboard_shortcut_missing"))
            }
        });
        cx.spawn_in(window, async move |this, cx| {
            let result = job.await.map_err(|error| error.to_string()).and_then(|result| result);
            let _ = this.update_in(cx, |this, window, cx| {
                this.keyboard.shortcut_running = false;
                if this.selection != selection || this.selected_key().as_ref() != Some(&key) { return; }
                match result {
                    Ok(side::Shortcut::External) if !this.external_terminal_available() => {
                        window.push_notification(Notification::warning(tr("keyboard_external_unavailable")), cx);
                    }
                    Ok(shortcut) => this.run_shortcut(shortcut, false, window, cx),
                    Err(error) => window.push_notification(Notification::warning(error), cx),
                }
            });
        }).detach();
    }

    fn render_keyboard_edit(&self, target: &Target, cx: &mut Context<Self>) -> Option<AnyElement> {
        let edit = self.keyboard.edit.as_ref().filter(|edit| same_edit_target(&edit.target, target))?;
        let candidate = self.keyboard.candidate().and_then(|config| config.validate_runtime(cx).map(|_| config));
        let error = self.keyboard.save_error.clone().or_else(|| match &candidate { Err(error) if edit.key.is_some() || edit.hold.is_some() => Some(error.clone()), _ => None });
        let title = if matches!(target, Target::Hold) { tr("keyboard_hold_press") } else { tr("keyboard_press_keys") };
        let value = if matches!(target, Target::Hold) { edit.hold.map(|hold| modifier_labels(hold).join("+")) } else { edit.key.as_deref().map(|key| display_key(key, edit.physical, cx)) };
        let body = div().px_4().py_3().flex().flex_col().gap_2().border_t_1().border_color(theme::border())
            .child(div().text_sm().text_color(theme::muted()).whitespace_normal().child(title))
            .children(value.map(|value| div().font_family(theme::MONO).text_sm().font_weight(FontWeight::SEMIBOLD).child(value)))
            .children(error.map(|error| div().id("keyboard-edit-error").role(Role::Alert).text_sm().text_color(theme::danger()).whitespace_normal().child(error)))
            .child(div().flex().items_center().gap_2()
                .child(Button::new("keyboard-save").primary().small().label(tr("keyboard_save"))
                    .disabled(candidate.is_err() || self.keyboard.busy()).loading(self.keyboard.saving)
                    .on_click(cx.listener(|this, _, window, cx| {
                        if let Ok(config) = this.keyboard.candidate() { this.save_keyboard(config, window, cx); }
                    })))
                .child(Button::new("keyboard-cancel").outline().small().label(tr("keyboard_cancel")).disabled(self.keyboard.saving)
                    .on_click(cx.listener(|this, _, window, cx| { this.keyboard_escape(window, cx); }))));
        Some(body.into_any_element())
    }

    fn keyboard_row(&self, target: Target, label: String, hint: Option<String>, cx: &mut Context<Self>) -> AnyElement {
        let (id, keys, changed) = match &target {
            Target::Command(command) => (format!("command-{command:?}"), self.keyboard.config.keys(command).iter().map(|key| self.keyboard.key_label(key, cx)).collect::<Vec<_>>(), self.keyboard.config.overrides.contains_key(command) || self.keyboard.config.yielded.contains(command)),
            Target::Shortcut(shortcut) => {
                let saved = self.keyboard.config.shortcuts.iter().find(|item| item.same_target(shortcut));
                (format!("shortcut-{}-{}-{}", shortcut.server, shortcut.project.as_deref().unwrap_or(""), shortcut.id), saved.map(|item| vec![self.keyboard.key_label(&item.key, cx)]).unwrap_or_default(), saved.is_some())
            }
            Target::Hold => ("hold".into(), vec![modifier_labels(self.keyboard.config.hold).join("+")], self.keyboard.config.hold != Config::default().hold),
        };
        let editing = self.keyboard.edit.as_ref().is_some_and(|edit| same_edit_target(&edit.target, &target));
        let configure = target.clone();
        let restore = target.clone();
        let remove = target.clone();
        let row = div().px_4().py_3().border_t_1().border_color(theme::border()).flex().flex_wrap().items_center().gap_3()
            .child(div().flex_1().min_w_0().flex().flex_col().gap_1()
                .child(div().text_sm().font_weight(FontWeight::MEDIUM).child(label.clone()))
                .children(hint.map(|hint| div().text_xs().text_color(theme::muted()).whitespace_normal().child(hint))))
            .child(div().flex().items_center().gap_2()
                .child(Button::new(SharedString::from(format!("keyboard-configure-{id}"))).outline().small()
                    .label(if editing { tr("keyboard_listening") } else if keys.is_empty() { tr("keyboard_configure") } else { keys.join(" / ") })
                    .accessibility_label(tr("keyboard_configure_action").replace("{action}", &label))
                    .disabled(self.keyboard.busy()).selected(editing)
                    .on_click(cx.listener(move |this, _, window, cx| this.open_keyboard_edit(configure.clone(), window, cx))))
                .when(changed, |el| el.child(Button::new(SharedString::from(format!("keyboard-restore-{id}"))).ghost().small()
                    .label(tr("keyboard_restore")).disabled(self.keyboard.busy())
                    .on_click(cx.listener(move |this, _, window, cx| this.restore_keyboard_target(restore.clone(), window, cx)))))
                .when(!keys.is_empty() && !matches!(target, Target::Hold), |el| el.child(Button::new(SharedString::from(format!("keyboard-remove-{id}"))).ghost().small()
                    .icon(IconName::Close).accessibility_label(tr("keyboard_remove_action").replace("{action}", &label)).disabled(self.keyboard.busy())
                    .on_click(cx.listener(move |this, _, window, cx| this.disable_keyboard_target(remove.clone(), window, cx))))));
        div().child(row).children(self.render_keyboard_edit(&target, cx)).into_any_element()
    }

    pub(super) fn render_keyboard_settings(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let header = section_head(IconName::Keyboard, tr("keyboard_title"), Some(tr("keyboard_lead")), None, px(16.));
        let mut section = div().id("keyboard-settings").flex().flex_col().gap_3().child(header);
        if self.keyboard.loading { return section.child(settings_box().child(div().p_4().text_sm().text_color(theme::muted()).child(tr("keyboard_loading")))).into_any_element(); }
        if let Some(error) = &self.keyboard.load_error {
            return section.child(settings_box().child(div().p_4().flex().flex_col().gap_3()
                .child(div().id("keyboard-load-error").role(Role::Alert).text_sm().text_color(theme::danger()).whitespace_normal().child(tr("keyboard_load_failed").replace("{error}", error)))
                .children(self.keyboard.save_error.clone().map(|error| div().id("keyboard-reset-error").role(Role::Alert).text_sm().text_color(theme::danger()).whitespace_normal().child(error)))
                .child(div().flex().items_center().gap_2()
                    .child(Button::new("keyboard-reload").outline().small().label(tr("keyboard_reload")).disabled(self.keyboard.saving)
                        .on_click(cx.listener(|this, _, window, cx| { this.keyboard.loading = true; begin_load(window, cx); cx.notify(); })))
                    .child(Button::new("keyboard-reset").outline().small().label(tr("keyboard_reset_defaults")).loading(self.keyboard.saving)
                        .on_click(cx.listener(|this, _, window, cx| this.reset_keyboard_config(window, cx))))))).into_any_element();
        }
        section = section.child(settings_box().child(self.keyboard_row(Target::Hold, tr("keyboard_hold_title"), Some(tr("keyboard_hold_help")), cx)));
        for (context, title) in [("!Terminal", "keyboard_global"), ("FileViewer", "keyboard_files"), ("Terminal", "keyboard_terminal")] {
            let mut list = settings_box().child(div().px_4().py_3().font_weight(FontWeight::SEMIBOLD).text_sm().child(tr(title)));
            for command in Command::ALL.into_iter().filter(|command| command.context() == context) {
                list = list.child(self.keyboard_row(Target::Command(command.clone()), tr(command.key()), None, cx));
            }
            section = section.child(list);
        }
        let server = self.active_key();
        let own = self.selected_key().and_then(|key| self.side.project.of(Some(&key)).cloned());
        let mut custom = settings_box().child(div().px_4().py_3().font_weight(FontWeight::SEMIBOLD).text_sm().child(tr("keyboard_actions")));
        let mut listed = Vec::new();
        for item in self.shortcuts.keyboard_items() {
            let label = if item.kind() == "internal" { tr(&format!("shortcuts_native_{}", item.action())) } else { item.label().to_owned() };
            listed.push(ShortcutBinding { server: server.clone(), project: None, id: item.id().to_owned(), label, key: String::new() });
        }
        if let Some(project) = &own {
            let project_server = self.open_server();
            for item in &project.items {
                listed.push(ShortcutBinding { server: project_server.clone(), project: Some(project.key.clone()), id: item.id().to_owned(), label: item.label().to_owned(), key: String::new() });
            }
        }
        let rows = binding_rows(&listed, &self.keyboard.config.shortcuts);
        if rows.is_empty() { custom = custom.child(div().p_4().text_sm().text_color(theme::muted()).whitespace_normal().child(tr("keyboard_actions_empty"))); }
        let mut outside_started = false;
        for row in rows {
            let binding = row.binding;
            if !row.listed && !outside_started {
                outside_started = true;
                custom = custom.child(div().px_4().py_3().border_t_1().border_color(theme::border()).font_weight(FontWeight::SEMIBOLD).text_sm().child(tr("keyboard_other_bindings")));
            }
            let scope = match &binding.project {
                Some(project) => tr("keyboard_project_action").replace("{project}", &own.as_ref().filter(|current| binding.server == self.open_server() && current.key == *project).map_or_else(|| project.clone(), |current| current.name.clone())),
                None => tr("keyboard_global_action"),
            };
            let hint = if row.listed { scope } else { tr("keyboard_unlisted_binding").replace("{server}", &binding.server).replace("{scope}", &scope) };
            let label = if binding.label.is_empty() { binding.id.clone() } else { binding.label.clone() };
            custom = custom.child(self.keyboard_row(Target::Shortcut(binding), label, Some(hint), cx));
        }
        section = section.child(custom);
        if let Some(error) = self.keyboard.save_error.as_ref().filter(|_| !self.keyboard.is_editing()) {
            section = section.child(div().id("keyboard-save-error").role(Role::Alert).text_sm().text_color(theme::danger()).whitespace_normal().child(error.clone()));
        }
        section.child(settings_box().child(div().p_4().flex().flex_col().gap_3()
            .child(div().text_sm().font_weight(FontWeight::SEMIBOLD).child(tr("keyboard_contextual")))
            .child(div().text_sm().text_color(theme::muted()).whitespace_normal().child(tr("keyboard_chat_notes")))
            .child(div().text_sm().text_color(theme::muted()).whitespace_normal().child(tr("keyboard_other_notes")))
            .child(div().text_sm().text_color(theme::muted()).whitespace_normal().child(tr("keyboard_terminal_notes"))))).into_any_element()
    }
}

fn same_edit_target(a: &Target, b: &Target) -> bool {
    match (a, b) { (Target::Command(a), Target::Command(b)) => a == b, (Target::Shortcut(a), Target::Shortcut(b)) => a.same_target(b),
        (Target::Hold, Target::Hold) => true, _ => false }
}

fn keyboard_shortcut(item: &shortcuts::Item, project: Option<&str>) -> Option<side::Shortcut> {
    if item.kind() != "internal" { return side::Shortcut::from_item(item, project); }
    match item.action() { "terminal" => Some(side::Shortcut::Terminal), "modo" => Some(side::Shortcut::Mode), "navegador" => Some(side::Shortcut::Browser),
        "anexos" => Some(side::Shortcut::Attach), "rodar" => Some(side::Shortcut::Run), "externo" => Some(side::Shortcut::External), _ => None }
}

fn is_paste(stroke: &Keystroke) -> bool {
    let m = &stroke.modifiers;
    match stroke.key.as_str() {
        "v" => !m.alt && !m.shift && (m.control != m.platform),
        "insert" => m.shift && !m.control && !m.alt && !m.platform,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::prelude::v1::test;

    #[test]
    fn paste_shortcuts_are_recognized_and_others_are_not() {
        let stroke = |s: &str| parse_key(s).unwrap();
        assert!(is_paste(&stroke("ctrl-v")));
        assert!(is_paste(&stroke("cmd-v")));
        assert!(is_paste(&stroke("shift-insert")));
        assert!(!is_paste(&stroke("v")));
        assert!(!is_paste(&stroke("ctrl-shift-v")));
        assert!(!is_paste(&stroke("ctrl-c")));
    }

    #[test]
    fn cancelling_key_capture_preserves_the_last_persistence_error() {
        let mut keyboard = Keyboard { config: Config::default(), loading: false, load_error: None,
            save_error: Some("disk failure".into()), saving: false, edit: None, shortcut_running: false };
        keyboard.cancel_edit();
        assert_eq!(keyboard.save_error.as_deref(), Some("disk failure"));
    }

    #[test]
    fn edited_binding_replaces_default_without_removing_other_keymaps() {
        let mut config = Config::default();
        config.overrides.insert(Command::FocusComposer, "ctrl-alt-l".into());
        let original = vec![load_binding("secondary-l", Box::new(FocusComposer), "!Terminal", &DummyKeyboardMapper).unwrap(),
            KeyBinding::new("tab", NoAction, Some("Terminal"))];
        let updated = replacement_bindings(&original, &config, &DummyKeyboardMapper).unwrap();
        let composer: Vec<_> = updated.iter().filter(|b| b.action().as_any().is::<FocusComposer>()).collect();
        assert_eq!(composer.len(), 1);
        assert_eq!(composer[0].keystrokes()[0].unparse(HOST_OS), "ctrl-alt-l");
        assert!(updated.iter().any(|b| b.action().as_any().is::<NoAction>() && b.keystrokes()[0].unparse(HOST_OS) == "tab"));
        config.overrides.insert(Command::FocusComposer, String::new());
        let disabled = replacement_bindings(&updated, &config, &DummyKeyboardMapper).unwrap();
        assert!(!disabled.iter().any(|b| b.action().as_any().is::<FocusComposer>()));
    }

    #[test]
    fn edited_bindings_cannot_replace_live_input_keys_or_file_commands() {
        let editing = vec![load_binding("secondary-c", Box::new(gpui_kit::base::input::Copy), "Input", &DummyKeyboardMapper).unwrap(),
            load_binding("secondary-f", Box::new(gpui_kit::base::input::Search), "Input", &DummyKeyboardMapper).unwrap()];
        assert!(validate_effective_bindings(&Config::default(), &editing, &DummyKeyboardMapper).is_ok());
        let mut config = Config::default();
        config.overrides.insert(Command::FocusComposer, "secondary-c".into());
        assert!(validate_effective_bindings(&config, &editing, &DummyKeyboardMapper).is_err());
        config.overrides.insert(Command::FocusComposer, "secondary-s".into());
        assert!(validate_effective_bindings(&config, &editing, &DummyKeyboardMapper).is_err());
        config.overrides.insert(Command::FocusComposer, "ctrl-alt-l".into());
        assert!(validate_effective_bindings(&config, &editing, &DummyKeyboardMapper).is_ok());
    }

    #[test]
    fn close_session_key_stays_out_of_text_fields_and_the_browser_page() {
        let bindings = Config::default().bindings(&DummyKeyboardMapper).unwrap();
        let close = bindings.iter().find(|b| b.action().as_any().is::<CloseSession>()).unwrap();
        let root = KeyContext::new_with_defaults(HOST_OS);
        let inside = |name: &str| { let mut context = KeyContext::default(); context.add(name); vec![root.clone(), context] };
        assert!(binding_applies(close, std::slice::from_ref(&root)));
        assert!(binding_applies(close, &inside("FileViewer")));
        assert!(!binding_applies(close, &inside("Terminal")));
        for name in ["Input", "BrowserPage"] { assert_eq!(binding_applies(close, &inside(name)), cfg!(target_os = "macos"), "{name}"); }
        assert_eq!(Command::CloseSession.context(), "!Terminal");
    }

    #[test]
    fn dictation_key_fires_with_the_focus_anywhere() {
        let bindings = Config::default().bindings(&DummyKeyboardMapper).unwrap();
        let dictation = bindings.iter().find(|b| b.action().as_any().is::<ToggleDictation>()).unwrap();
        let root = KeyContext::new_with_defaults(HOST_OS);
        let inside = |name: &str| { let mut context = KeyContext::default(); context.add(name); vec![root.clone(), context] };
        assert!(binding_applies(dictation, std::slice::from_ref(&root)));
        for name in ["Terminal", "FileViewer", "Input", "BrowserPage"] { assert!(binding_applies(dictation, &inside(name)), "{name}"); }
    }

    #[test]
    fn new_session_is_ctrl_shift_t_outside_the_terminal() {
        let bindings = Config::default().bindings(&DummyKeyboardMapper).unwrap();
        let open = bindings.iter().find(|b| b.action().as_any().is::<OpenNewSession>()).unwrap();
        let expected = if cfg!(target_os = "macos") { "cmd-shift-t" } else { "ctrl-shift-t" };
        assert_eq!(open.keystrokes().iter().map(|k| k.inner().unparse(HOST_OS)).collect::<Vec<_>>(), [expected]);
        let root = KeyContext::new_with_defaults(HOST_OS);
        let inside = |name: &str| { let mut context = KeyContext::default(); context.add(name); vec![root.clone(), context] };
        assert!(binding_applies(open, std::slice::from_ref(&root)));
        assert!(binding_applies(open, &inside("Input")));
        assert!(!binding_applies(open, &inside("Terminal")));
        let saved: Config = serde_json::from_str(r#"{"overrides":{"new_session":"ctrl-alt-t"}}"#).unwrap();
        assert_eq!(saved.overrides.get(&Command::NewSession).map(String::as_str), Some("ctrl-alt-t"));
    }

    #[test]
    fn saved_binding_on_a_new_default_key_drops_the_default_on_load() {
        let mut config = Config::default();
        config.shortcuts.push(ShortcutBinding { server: "http://host:8765".into(), project: None,
            id: "custom".into(), label: "custom".into(), key: "secondary-shift-t".into() });
        assert!(config.validate().is_err());
        config.yield_defaults();
        assert!(config.validate().is_ok());
        assert!(config.keys(&Command::NewSession).is_empty());
        let bindings = config.bindings(&DummyKeyboardMapper).unwrap();
        assert!(!bindings.iter().any(|b| b.action().as_any().is::<OpenNewSession>()));
        assert!(bindings.iter().any(|b| b.action().as_any().is::<RunShortcut>()));
        let mut config = Config::default();
        config.overrides.insert(Command::Costs, "secondary-shift-t".into());
        config.yield_defaults();
        assert!(config.validate().is_ok());
        assert!(config.keys(&Command::NewSession).is_empty());
        assert_eq!(config.keys(&Command::Costs), ["secondary-shift-t"]);
        // O cedido não vai para o arquivo: sem o conflito, o padrão volta na próxima leitura.
        let saved: Config = serde_json::from_str(&serde_json::to_string(&config).unwrap()).unwrap();
        assert!(!saved.overrides.contains_key(&Command::NewSession) && saved.yielded.is_empty());
        // Na mesma sessão: com o conflito ainda lá o cedido fica; trocada a tecla da pessoa, o padrão volta ao salvar.
        config.release_yielded();
        assert!(config.keys(&Command::NewSession).is_empty());
        config.overrides.insert(Command::Costs, "ctrl-alt-c".into());
        config.release_yielded();
        assert!(!config.keys(&Command::NewSession).is_empty());
        assert!(config.validate().is_ok());
        // Sem conflito nada muda; a tecla escolhida pela pessoa para o próprio comando não é tocada.
        let mut config = Config::default();
        config.overrides.insert(Command::NewSession, "ctrl-alt-t".into());
        config.yield_defaults();
        assert_eq!(config, { let mut c = Config::default(); c.overrides.insert(Command::NewSession, "ctrl-alt-t".into()); c });
    }

    #[test]
    fn rename_session_is_f2_everywhere_but_the_terminal_and_the_browser_page() {
        let bindings = Config::default().bindings(&DummyKeyboardMapper).unwrap();
        let rename = bindings.iter().find(|b| b.action().as_any().is::<RenameSession>()).unwrap();
        assert_eq!(rename.keystrokes().iter().map(|k| k.inner().unparse(HOST_OS)).collect::<Vec<_>>(), ["f2"]);
        let root = KeyContext::new_with_defaults(HOST_OS);
        let inside = |name: &str| { let mut context = KeyContext::default(); context.add(name); vec![root.clone(), context] };
        assert!(binding_applies(rename, std::slice::from_ref(&root)));
        assert!(binding_applies(rename, &inside("Input")));
        for name in ["BrowserPage", "Terminal"] { assert!(!binding_applies(rename, &inside(name)), "{name}"); }
        assert_eq!(key_label("f2"), "F2");
    }

    #[test]
    fn retained_shortcut_bindings_remain_listed_outside_the_current_sources() {
        let listed = ShortcutBinding { server: "http://one:8765".into(), project: None, id: "listed".into(), label: "listed".into(), key: String::new() };
        let saved = vec![ShortcutBinding { key: "ctrl-alt-l".into(), ..listed.clone() },
            ShortcutBinding { server: "http://two:8765".into(), project: Some("/other".into()), id: "retained".into(), label: "retained".into(), key: "ctrl-alt-r".into() }];
        let rows = binding_rows(&[listed], &saved);
        assert_eq!(rows.len(), 2);
        assert!(rows[0].listed);
        assert_eq!(rows[0].binding.id, "listed");
        assert!(!rows[1].listed);
        assert_eq!((rows[1].binding.server.as_str(), rows[1].binding.project.as_deref(), rows[1].binding.id.as_str(), rows[1].binding.key.as_str()),
            ("http://two:8765", Some("/other"), "retained", "ctrl-alt-r"));
        assert_eq!(binding_rows(&[], &saved).len(), 2);
    }

    #[test]
    fn duplicate_keys_are_rejected_within_the_same_context() {
        let mut config = Config::default();
        config.overrides.insert(Command::FocusComposer, "secondary-k".into());
        assert!(config.validate().is_err());
        config.overrides.insert(Command::FocusComposer, "secondary-g".into());
        assert!(config.validate().is_ok());
        config.shortcuts.push(ShortcutBinding { server: "http://host:8765".into(), project: None,
            id: "custom".into(), label: "custom".into(), key: "secondary-g".into() });
        assert!(config.validate().is_err());
    }

    #[test]
    fn hold_requires_two_modifiers_and_reserves_its_number_keys() {
        let mut config = Config::default();
        config.hold = Modifiers { control: true, ..Modifiers::none() };
        assert!(config.validate().is_err());
        config.hold = Modifiers { control: true, alt: true, ..Modifiers::none() };
        assert!(config.validate().is_ok());
        config.overrides.insert(Command::FocusComposer, "ctrl-alt-1".into());
        assert!(config.validate().is_err());
    }

    #[test]
    fn configuration_round_trips_and_absent_fields_keep_defaults() {
        let old: Config = serde_json::from_str("{}").unwrap();
        assert_eq!(old.hold, Modifiers { control: true, shift: true, ..Modifiers::none() });
        let mut saved = old;
        saved.overrides.insert(Command::OpenSettings, "ctrl-alt-,".into());
        saved.shortcuts.push(ShortcutBinding { server: "http://host:8765".into(), project: Some("/repo".into()),
            id: "run".into(), label: "run".into(), key: "ctrl-alt-r".into() });
        let loaded: Config = serde_json::from_str(&serde_json::to_string(&saved).unwrap()).unwrap();
        assert_eq!(loaded, saved);
    }

    #[test]
    fn shifted_number_capture_uses_physical_modifiers_and_known_layout() {
        let hold = Modifiers { control: true, shift: true, ..Modifiers::none() };
        assert!(reserved_number_key("!", hold, hold, ["English (US)"; 2]));
        assert!(reserved_number_key("dead_diaeresis", hold, hold, ["Portuguese (Brazil)"; 2]));
        assert!(!reserved_number_key("!", Modifiers::control(), hold, ["English (US)"; 2]));
        assert!(!reserved_number_key("&", hold, hold, ["German"; 2]));
    }

    #[test]
    fn native_button_mapping_does_not_require_a_panel_tile() {
        let items = shortcuts::resolve(r#"[{"id":"terminal","type":"internal","action":"terminal"}]"#);
        assert!(matches!(keyboard_shortcut(&items[0], None), Some(side::Shortcut::Terminal)));
        let items = shortcuts::resolve(r#"[{"id":"review","type":"send_text","label":"review","text":"/review","confirm":true,"send_direct":false}]"#);
        assert!(matches!(keyboard_shortcut(&items[0], None), Some(side::Shortcut::Send { text, direct: false, confirm: true, .. }) if text == "/review"));
    }

    #[test]
    fn text_preferred_key_events_are_not_captured_as_shortcuts() {
        let mut event = KeyDownEvent { keystroke: parse_key("ctrl-alt-q").unwrap(), is_held: false, prefer_character_input: true, physical_digit: None };
        assert!(captured_stroke(&event, &DummyKeyboardMapper).is_err());
        event.prefer_character_input = false;
        assert!(captured_stroke(&event, &DummyKeyboardMapper).is_ok());
    }

    #[test]
    fn broken_config_is_set_aside_so_defaults_load() {
        let dir = std::env::temp_dir().join(format!("hangar-keyboard-reset-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("keyboard.json");
        std::fs::write(&path, br#"{"hold":{"control":true}}"#).unwrap();
        let aside = set_aside_config(&path, 42).unwrap().unwrap();
        assert_eq!(aside, dir.join("keyboard.json.bad-42"));
        assert!(!path.exists());
        assert_eq!(std::fs::read(&aside).unwrap(), br#"{"hold":{"control":true}}"#);
        assert_eq!(set_aside_config(&path, 43).unwrap(), None);
        std::fs::remove_dir_all(&dir).unwrap();
        assert!(set_aside_config(&dir.join("missing-dir/keyboard.json"), 44).unwrap().is_none());
    }

    #[test]
    fn capture_does_not_accept_bare_text_or_modifier_keys() {
        for key in ["j", "shift-j", "enter", "ctrl-shift"] { assert!(canonical_key(key).is_err(), "{key}"); }
        assert_eq!(canonical_key("CTRL-alt-j").unwrap(), "ctrl-alt-j");
        assert_eq!(canonical_key("f8").unwrap(), "f8");
    }
}
