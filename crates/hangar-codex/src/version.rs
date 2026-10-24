//! Versão do Codex de onde saiu o recorte do schema guardado (`schema/<versão>.json`). Não é
//! comparada com o Codex instalado: quem confere o Codex novo é o workflow semanal `codex-schema`.
#[macro_export]
#[doc(hidden)]
macro_rules! checked_version { () => { "0.162.1" } }

pub const CHECKED:&str = checked_version!();
