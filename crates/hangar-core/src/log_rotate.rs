//! Giro de arquivo de log na subida do processo.

pub const LOG_MAX_BYTES: u64 = 4 * 1024 * 1024;
const LOG_BACKUPS: u32 = 3;

/// Mesmo teto dos logs privados do Python (4 MB, três cópias `.1`..`.3`), conferido ao subir.
// ponytail: só na subida; um processo que viva muito pode passar do teto até o próximo início.
// Girar em execução pede um writer próprio, se o arquivo crescer assim na prática.
pub fn rotate_log(path: &std::path::Path, max: u64) {
    if std::fs::metadata(path).map_or(true, |m| m.len() < max) {
        return;
    }
    let numbered = |i: u32| {
        let mut s = path.as_os_str().to_owned();
        s.push(format!(".{i}"));
        std::path::PathBuf::from(s)
    };
    // remove antes do rename: no Windows o rename não sobrescreve.
    let _ = std::fs::remove_file(numbered(LOG_BACKUPS));
    for i in (1..LOG_BACKUPS).rev() {
        let _ = std::fs::rename(numbered(i), numbered(i + 1));
    }
    let _ = std::fs::rename(path, numbered(1));
}
