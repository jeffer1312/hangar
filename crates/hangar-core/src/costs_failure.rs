//! Motivos fixos de falha de custos: código para o app e frase para o diário, nunca o erro cru.

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FailureReason {
    NoScopes,
    NoDisk,
    ReaderPanic,
    Sqlite,
    Json,
    WorkerJoin,
    InfoUnavailable,
    Io,
    NonFinite,
}

impl FailureReason {
    pub fn code(self) -> &'static str {
        match self {
            Self::NoScopes => "costs_no_scopes",
            Self::NoDisk => "costs_no_disk",
            Self::ReaderPanic => "costs_reader_panic",
            Self::Sqlite => "costs_sqlite",
            Self::Json => "costs_json",
            Self::WorkerJoin => "costs_worker_join",
            Self::InfoUnavailable => "internal_info",
            Self::Io => "costs_io",
            Self::NonFinite => "costs_non_finite",
        }
    }

    pub fn message(self) -> &'static str {
        match self {
            Self::NoScopes => "escopos de custos indisponíveis",
            Self::NoDisk => "índice de custos indisponível",
            Self::ReaderPanic => "falha no leitor de custos",
            Self::Sqlite => "falha no índice de custos",
            Self::Json => "falha ao preparar a resposta de custos",
            Self::WorkerJoin => "falha no processamento de custos",
            Self::InfoUnavailable => "o backend não devolveu os dados da sessão",
            Self::Io => "falha na leitura dos dados de custos",
            Self::NonFinite => "valor de custos ou uso inválido",
        }
    }
}
