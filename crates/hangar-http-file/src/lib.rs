//! Transporte compartilhado dos arquivos abertos, com os modos de cache dos consumidores.
#![forbid(unsafe_code)]

mod legacy;
mod opened;
mod ranges;
mod stream;

pub use legacy::legacy_file;
pub use opened::{html_escape, serve_open_file};
pub use ranges::{RangeError, ranges};
