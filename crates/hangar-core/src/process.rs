//! Erros de processo que não são falha.
use std::io;

/// Processo que morreu entre a varredura e a leitura: não é falha, é corrida.
pub fn vanished(error: &io::Error) -> bool {
    // ESRCH: o `/proc/<pid>` ainda existe, o processo já não.
    error.kind() == io::ErrorKind::NotFound || error.raw_os_error() == Some(3)
}
