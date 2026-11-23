//! Escolhas marcadas no painel e ainda não enviadas, guardadas por ticket no disco do servidor: sair da sessão,
//! fechar o app ou reiniciar o servidor não as perde. Valem só para a rodada (`review_id`) em que foram marcadas.
use hangar_connectors::work_review::{MAX_REASON, Review};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::io;
use std::path::PathBuf;

/// Teto de decisões guardadas e dos ids; o painel nunca chega perto.
const MAX_PICKS: usize = 200;
const MAX_ID: usize = 256;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Pick { pub choice: String, #[serde(default)] pub reason: String }

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Draft { pub review_id: String, pub choices: BTreeMap<String, Pick> }

impl Draft {
    pub(crate) fn check(&self) -> Result<(), String> {
        let id = |field: &str, value: &str| if value.is_empty() || value.len() > MAX_ID { Err(format!("{field} vazio ou acima de {MAX_ID} bytes")) } else { Ok(()) };
        id("review_id", &self.review_id)?;
        if self.choices.len() > MAX_PICKS { return Err(format!("choices acima de {MAX_PICKS}")); }
        for (decision, pick) in &self.choices {
            id("choices.id", decision)?;
            id("choices.choice", &pick.choice)?;
            if pick.reason.chars().count() > MAX_REASON { return Err(format!("motivo acima de {MAX_REASON} caracteres")); }
        }
        Ok(())
    }

    /// Só as escolhas que a rodada lida ainda mostra, com escolha que existe; nada sobrando é `None`.
    pub(crate) fn for_review(self, review: &Review) -> Option<BTreeMap<String, Pick>> {
        if self.review_id != review.id { return None; }
        let kept: BTreeMap<String, Pick> = self.choices.into_iter().filter(|(id, pick)| review.decisions.iter()
            .any(|decision| &decision.id == id && decision.choices.iter().any(|choice| choice.id == pick.choice))).collect();
        (!kept.is_empty()).then_some(kept)
    }
}

pub(crate) struct Drafts { dir: PathBuf }

impl Drafts {
    pub(crate) fn new(dir: PathBuf) -> Self { Self { dir } }

    /// Um arquivo por conector e ticket; o nome é o resumo dos dois, que podem ter qualquer caractere.
    fn path(&self, connector: &str, ticket: &str) -> PathBuf {
        let mut sha = sha1_smol::Sha1::new();
        sha.update(connector.as_bytes());
        sha.update(b"\n");
        sha.update(ticket.as_bytes());
        self.dir.join(format!("{}.json", sha.digest()))
    }

    pub(crate) async fn read(&self, connector: &str, ticket: &str) -> io::Result<Option<Draft>> {
        let path = self.path(connector, ticket);
        tokio::task::spawn_blocking(move || match std::fs::read(&path) {
            Ok(bytes) => serde_json::from_slice(&bytes).map(Some).map_err(io::Error::other),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error),
        }).await.unwrap_or_else(|_| Err(io::Error::other("leitura do rascunho interrompida")))
    }

    pub(crate) async fn write(&self, connector: &str, ticket: &str, draft: &Draft) -> io::Result<()> {
        let (dir, path, bytes) = (self.dir.clone(), self.path(connector, ticket), serde_json::to_vec(draft)?);
        tokio::task::spawn_blocking(move || {
            std::fs::create_dir_all(&dir)?;
            hangar_core::fs::atomic_write(&path, &bytes)
        }).await.unwrap_or_else(|_| Err(io::Error::other("gravação do rascunho interrompida")))
    }
}
