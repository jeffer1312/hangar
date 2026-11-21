//! Player de vídeo sobre a libmpv, carregada na hora (dlopen): o app abre sem o mpv instalado, e só o vídeo
//! avisa que ele falta. A FFI é escrita à mão sobre `client.h` e `render.h` (API 2 do cliente), com a
//! renderização por software: cada quadro sai num buffer RGBA ou BGRA reaproveitado, e quem pinta copia para
//! a textura dele.
mod error;
mod ffi;
mod frame;
mod library;
mod player;
mod text;

pub use error::VideoError;
pub use frame::FrameRef;
pub use library::Library;
pub use player::{Change, Options, PixelFormat, Player, Snapshot};
pub use text::{bearer, clock};
