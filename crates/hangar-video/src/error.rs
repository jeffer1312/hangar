use std::fmt;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum VideoError {
    /// A libmpv não está nesta máquina, ou é de uma versão sem a API 2 do cliente. O texto diz o que foi tentado.
    Unavailable(String),
    /// A libmpv recusou criar ou configurar o player.
    Init(String),
    /// Cabeçalho vazio ou com quebra de linha: viraria outro cabeçalho no pedido.
    InvalidHeader,
}

impl fmt::Display for VideoError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            VideoError::Unavailable(why) => write!(f, "libmpv unavailable: {why}"),
            VideoError::Init(why) => write!(f, "libmpv refused to start: {why}"),
            VideoError::InvalidHeader => f.write_str("invalid HTTP header for the video request"),
        }
    }
}

impl std::error::Error for VideoError {}
