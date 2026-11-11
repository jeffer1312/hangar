use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock, Mutex};

use regex::Regex;
use serde_json::{Map, Value, json};

use hangar_core::pycompat::py;

pub type Home = Arc<dyn Fn() -> io::Result<PathBuf> + Send + Sync>;
pub type Pointer = Arc<dyn Fn() -> io::Result<String> + Send + Sync>;
pub type WarningSink = Arc<dyn Fn(Warning) + Send + Sync>;

const TOKENS: &[&str] = &[
    "background",
    "surface",
    "surfaceContainerLow",
    "surfaceContainer",
    "surfaceContainerHigh",
    "onSurface",
    "onSurfaceVariant",
    "outline",
    "outlineVariant",
    "primary",
    "onPrimary",
];
static DECLARATION: LazyLock<Regex> =
    LazyLock::new(|| py::py_re(r"(?m)^\$([A-Za-z_][A-Za-z0-9_]*)\s*:\s*([^;]+);"));

#[derive(Clone, Debug)]
pub struct Warning {
    pub code: &'static str,
    pub detail: String,
}

pub struct DesktopTheme {
    home: Home,
    pointer: Pointer,
    warnings: WarningSink,
    last_warning: Mutex<Option<String>>,
}

impl DesktopTheme {
    /// Fontes externas explícitas isolam arquivos, registro e diagnóstico.
    pub fn new(home: Home, pointer: Pointer, warnings: WarningSink) -> Self {
        Self {
            home,
            pointer,
            warnings,
            last_warning: Mutex::new(None),
        }
    }

    pub fn system() -> Self {
        let home: Home = Arc::new(crate::system::home);
        let pointer = crate::system::pointer(home.clone());
        Self::new(
            home,
            pointer,
            Arc::new(|warning| {
                tracing::warn!(code=warning.code, detail=%warning.detail, "Não foi possível ler a aparência do desktop.");
            }),
        )
    }

    /// Recusa a paleta parcial para conservar o contraste do aplicativo.
    pub fn parse(&self, text: &str) -> Option<Value> {
        if text.is_empty() {
            return None;
        }
        let text = text.strip_prefix('\u{feff}').unwrap_or(text);
        let mut raw = Map::new();
        for found in DECLARATION.captures_iter(text) {
            raw.insert(found[1].to_owned(), json!(py::strip(&found[2])));
        }
        let mut colors = Map::new();
        for (name, value) in &raw {
            let color = value.as_str().unwrap_or_default();
            if color.len() == 7
                && color.starts_with('#')
                && color.as_bytes()[1..].iter().all(u8::is_ascii_hexdigit)
            {
                colors.insert(name.clone(), value.clone());
            }
        }
        let missing = TOKENS
            .iter()
            .filter(|token| !colors.contains_key(**token))
            .copied()
            .collect::<Vec<_>>();
        if !missing.is_empty() {
            self.warn(
                "desktop_palette_incomplete",
                format!("tokens ausentes: {missing:?}"),
            );
            return None;
        }
        self.clear_warning();
        let dark = raw
            .get("darkmode")
            .and_then(Value::as_str)
            .unwrap_or("True");
        Some(json!({"escuro":py::strip(dark).to_lowercase() != "false", "cores":colors}))
    }

    pub fn palette(&self) -> Option<Value> {
        let result = (self.home)().and_then(|home| {
            read_text(&home.join(".local/state/quickshell/user/generated/material_colors.scss"))
        });
        match result {
            Ok(text) => self.parse(&text),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                self.clear_warning();
                None
            }
            Err(error) => {
                self.warn("desktop_palette_unreadable", error.to_string());
                None
            }
        }
    }

    /// Resolve a imagem atual, sem validar sua assinatura nem guardar o conteúdo.
    ///
    /// # Erros
    /// A expansão de usuário não resolvível conserva o erro do contrato original.
    pub fn wallpaper(&self) -> io::Result<Option<PathBuf>> {
        let raw = match (self.pointer)() {
            Ok(raw) => raw,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => {
                self.warn("desktop_wallpaper_pointer_unreadable", error.to_string());
                return Ok(None);
            }
        };
        let raw = py::strip(&raw);
        if raw.is_empty() {
            return Ok(None);
        }
        let path = crate::system::expand_user(raw, &self.home)?;
        let extension = path
            .extension()
            .and_then(|value| value.to_str())
            .unwrap_or_default()
            .to_lowercase();
        if !["png", "jpg", "jpeg", "webp", "avif", "bmp", "gif"].contains(&extension.as_str())
            || !path.is_file()
        {
            self.warn("desktop_wallpaper_unusable", raw.to_owned());
            return Ok(None);
        }
        Ok(Some(path))
    }

    fn clear_warning(&self) {
        *self
            .last_warning
            .lock()
            .unwrap_or_else(|error| error.into_inner()) = None;
    }

    fn warn(&self, code: &'static str, detail: String) {
        let key = format!("{code}:{detail}");
        let changed = {
            let mut last = self
                .last_warning
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            if last.as_deref() == Some(&key) {
                false
            } else {
                *last = Some(key);
                true
            }
        };
        if changed {
            (self.warnings)(Warning { code, detail });
        }
    }
}

pub(crate) fn read_text(path: &Path) -> io::Result<String> {
    std::fs::read(path).map(|bytes| {
        String::from_utf8_lossy(&bytes)
            .replace("\r\n", "\n")
            .replace('\r', "\n")
    })
}
