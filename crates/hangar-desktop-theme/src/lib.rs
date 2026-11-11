//! Paleta e papel de parede da máquina que atende o recurso.
#![forbid(unsafe_code)]

mod http;
mod reader;
mod system;

pub use http::{ROUTES, owns_path, router};
pub use reader::{DesktopTheme, Home, Pointer, Warning, WarningSink};
