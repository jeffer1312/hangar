//! Capacidade `work_review/v1`: a revisão do trabalho de um ticket, com as decisões que esperam a
//! pessoa. O host confere a resposta inteira e repassa só os campos do contrato; texto, rótulo e
//! escolha são do conector, nunca do Hangar.
use crate::protocol::ConnectorError;
use crate::work_tracking::GetParams;
use base64::Engine as _;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashSet;

pub const CAPABILITY: &str = "work_review";
pub const VERSION: u32 = 1;
pub const GET: &str = "work_review.get";
pub const ASSET: &str = "work_review.asset";
pub const DECIDE: &str = "work_review.decide";
/// Imagem decodificada: em base64 ela ainda cabe na mensagem de 4 MiB.
pub const MAX_ASSET_BYTES: usize = 2 * 1024 * 1024;
pub const MAX_REASON: usize = 2000;
/// Resultado inteiro do `work_review.get`, em JSON.
pub const MAX_REVIEW_BYTES: usize = 3 * 1024 * 1024;
pub const MAX_DECISIONS: usize = 50;
const SHORT: usize = 200;
const LONG: usize = 2000;
const MAX_ITEMS: usize = 200;
const MAX_SIDE: u32 = 16384;

/// IDs de revisão, decisão e asset: 1 a 128 bytes de `[A-Za-z0-9._:-]`.
pub fn valid_ref(id: &str) -> bool {
    (1..=128).contains(&id.len()) && id.bytes().all(|byte| byte.is_ascii_alphanumeric() || b"._:-".contains(&byte))
}

fn reference(field: &str, id: &str) -> Result<(), String> {
    if valid_ref(id) { Ok(()) } else { Err(format!("{field} fora de [A-Za-z0-9._:-]{{1,128}}")) }
}

fn optional(field: &str, value: Option<&str>, max: usize) -> Result<(), String> {
    let Some(value) = value else { return Ok(()) };
    if value.contains('\0') { return Err(format!("{field} com NUL")); }
    if value.chars().count() > max { return Err(format!("{field} acima de {max} caracteres")); }
    Ok(())
}

fn text(field: &str, value: &str, max: usize) -> Result<(), String> {
    if value.trim().is_empty() { return Err(format!("{field} vazio")); }
    optional(field, Some(value), max)
}

fn most<T>(field: &str, items: &[T], max: usize) -> Result<(), String> {
    if items.len() > max { Err(format!("{field} acima de {max} itens")) } else { Ok(()) }
}

fn counted(field: &str, count: Option<u64>, items: usize) -> Result<(), String> {
    if count.is_some_and(|count| count < items as u64) { Err(format!("{field}.count abaixo dos itens")) } else { Ok(()) }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReviewState { Running, Waiting, Applying, Done }

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Tone { Warning, Success, Info }

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Steps { pub labels: Vec<String>, pub current: u32 }

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Headline {
    pub value: String,
    pub text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Progress { pub label: String, pub done: u64, pub total: u64 }

/// Texto e tom do chip da sessão; a cor do painel sai do `state`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Badge { pub text: String, pub tone: Tone }

/// Um lado da comparação: a imagem, o tamanho dela e o trecho (`box`, em px) que o painel amplia.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Side {
    pub label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<String>,
    pub asset: String,
    pub width: u32,
    pub height: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub r#box: Option<[u32; 4]>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Choice {
    pub id: String,
    pub label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hint: Option<String>,
    #[serde(default)]
    pub requires_reason: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason_prompt: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Decision {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tag: Option<String>,
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason_label: Option<String>,
    /// Sem os dois lados, a decisão mostra só título, motivo e escolhas.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub left: Option<Side>,
    /// Ausente, o painel mostra só o lado esquerdo, na largura toda.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub right: Option<Side>,
    pub choices: Vec<Choice>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResolvedItem {
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub before: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct KeptItem { pub title: String, pub reason: String }

/// Lista com rótulo do fim da revisão: o que já foi resolvido e o que fica como está.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Group<T> {
    pub label: String,
    /// Total, quando passa dos itens mostrados.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub count: Option<u64>,
    pub items: Vec<T>,
}

pub type Resolved = Group<ResolvedItem>;
pub type Kept = Group<KeptItem>;

trait GroupItem { fn check(&self) -> Result<(), String>; }

impl GroupItem for ResolvedItem {
    fn check(&self) -> Result<(), String> {
        text("resolved.items.title", &self.title, SHORT)?;
        for asset in [&self.before, &self.after].into_iter().flatten() { reference("resolved.items.asset", asset)?; }
        Ok(())
    }
}

impl GroupItem for KeptItem {
    fn check(&self) -> Result<(), String> {
        text("kept.items.title", &self.title, SHORT)?;
        text("kept.items.reason", &self.reason, LONG)
    }
}

fn group<T: GroupItem>(field: &str, group: &Group<T>) -> Result<(), String> {
    text(&format!("{field}.label"), &group.label, SHORT)?;
    most(&format!("{field}.items"), &group.items, MAX_ITEMS)?;
    counted(field, group.count, group.items.len())?;
    group.items.iter().try_for_each(GroupItem::check)
}

/// `local`: o endereço só abre na máquina do conector.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Link {
    pub label: String,
    pub url: String,
    #[serde(default)]
    pub local: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Review {
    /// Opaco; muda a cada rodada nova da revisão.
    pub id: String,
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subject: Option<String>,
    pub updated_at: String,
    pub state: ReviewState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub steps: Option<Steps>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub headline: Option<Headline>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub progress: Option<Progress>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub badge: Option<Badge>,
    #[serde(default)]
    pub decisions: Vec<Decision>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolved: Option<Resolved>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kept: Option<Kept>,
    #[serde(default)]
    pub links: Vec<Link>,
}

/// Soma os bytes do JSON sem guardá-los; passou do teto, a escrita falha e a serialização para.
struct Counter(usize);

impl std::io::Write for Counter {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0 += bytes.len();
        if self.0 > MAX_REVIEW_BYTES { return Err(std::io::ErrorKind::FileTooLarge.into()); }
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> { Ok(()) }
}

impl Side {
    fn check(&self, field: &str) -> Result<(), String> {
        text(&format!("{field}.label"), &self.label, SHORT)?;
        optional(&format!("{field}.value"), self.value.as_deref(), LONG)?;
        reference(&format!("{field}.asset"), &self.asset)?;
        if !(1..=MAX_SIDE).contains(&self.width) || !(1..=MAX_SIDE).contains(&self.height) {
            return Err(format!("{field} com largura ou altura fora de 1 a {MAX_SIDE}"));
        }
        // Trecho vazio não tem o que ampliar; fora da imagem, o zoom mostraria outra coisa.
        if let Some([x, y, w, h]) = self.r#box && (w == 0 || h == 0 || x as u64 + w as u64 > self.width as u64 || y as u64 + h as u64 > self.height as u64) {
            return Err(format!("{field}.box fora da imagem"));
        }
        Ok(())
    }
}

impl Decision {
    fn check(&self) -> Result<(), String> {
        reference("decisions.id", &self.id)?;
        optional("decisions.tag", self.tag.as_deref(), SHORT)?;
        text("decisions.title", &self.title, SHORT)?;
        optional("decisions.reason", self.reason.as_deref(), LONG)?;
        optional("decisions.reason_label", self.reason_label.as_deref(), SHORT)?;
        if let Some(left) = &self.left { left.check("left")?; }
        if let Some(right) = &self.right {
            if self.left.is_none() { return Err("right sem left".into()); }
            right.check("right")?;
        }
        if !(1..=4).contains(&self.choices.len()) { return Err("choices fora de 1 a 4".into()); }
        let mut ids = HashSet::new();
        for choice in &self.choices {
            if !crate::manifest::valid_id(&choice.id) || !ids.insert(choice.id.as_str()) {
                return Err("choices.id fora de [a-z][a-z0-9-]{0,63} ou repetido".into());
            }
            text("choices.label", &choice.label, SHORT)?;
            optional("choices.hint", choice.hint.as_deref(), LONG)?;
            optional("choices.default_reason", choice.default_reason.as_deref(), MAX_REASON)?;
            optional("choices.reason_prompt", choice.reason_prompt.as_deref(), LONG)?;
        }
        Ok(())
    }
}

impl Review {
    /// Resultado de `work_review.get`: `{"review": null}` (sem revisão) ou a revisão conferida.
    pub fn from_connector(mut value: Value) -> Result<Option<Self>, ConnectorError> {
        // Os tetos dos campos somados passariam do transporte; o limite da resposta inteira é este.
        if serde_json::to_writer(Counter(0), &value).is_err() {
            return Err(ConnectorError::protocol("revisão acima de 3 MiB"));
        }
        let review = value.get_mut("review").map(Value::take).ok_or_else(|| ConnectorError::protocol("resposta sem review"))?;
        if review.is_null() { return Ok(None); }
        let review: Review = serde_json::from_value(review)
            .map_err(|error| ConnectorError::protocol(format!("revisão fora do contrato: {error}")))?;
        review.check().map_err(|reason| ConnectorError::protocol(format!("revisão fora do contrato: {reason}")))?;
        Ok(Some(review))
    }

    fn check(&self) -> Result<(), String> {
        reference("id", &self.id)?;
        text("title", &self.title, SHORT)?;
        optional("subject", self.subject.as_deref(), SHORT)?;
        if chrono::DateTime::parse_from_rfc3339(&self.updated_at).is_err() { return Err("updated_at sem RFC 3339 com fuso".into()); }
        if let Some(steps) = &self.steps {
            if !(1..=12).contains(&steps.labels.len()) { return Err("steps.labels fora de 1 a 12".into()); }
            for label in &steps.labels { text("steps.labels", label, SHORT)?; }
            if steps.current as usize > steps.labels.len() { return Err("steps.current além do total de etapas".into()); }
        }
        if let Some(headline) = &self.headline {
            text("headline.value", &headline.value, 16)?;
            text("headline.text", &headline.text, LONG)?;
            optional("headline.detail", headline.detail.as_deref(), LONG)?;
        }
        if let Some(progress) = &self.progress {
            if self.state != ReviewState::Running { return Err("progress fora do estado running".into()); }
            text("progress.label", &progress.label, SHORT)?;
            if progress.total == 0 || progress.done > progress.total { return Err("progress com total zero ou done acima de total".into()); }
        }
        if let Some(badge) = &self.badge { text("badge.text", &badge.text, SHORT)?; }
        match (self.state, self.decisions.is_empty()) {
            (ReviewState::Waiting, true) => return Err("waiting sem decisões".into()),
            (ReviewState::Waiting, false) | (_, true) => {}
            (_, false) => return Err("decisões fora do estado waiting".into()),
        }
        most("decisions", &self.decisions, MAX_DECISIONS)?;
        let mut ids = HashSet::new();
        for decision in &self.decisions {
            decision.check()?;
            if !ids.insert(decision.id.as_str()) { return Err("decisão repetida".into()); }
        }
        if let Some(resolved) = &self.resolved { group("resolved", resolved)?; }
        if let Some(kept) = &self.kept { group("kept", kept)?; }
        most("links", &self.links, 8)?;
        for link in &self.links {
            text("links.label", &link.label, SHORT)?;
            optional("links.url", Some(&link.url), LONG)?;
            if !crate::work_tracking::absolute_http(&link.url) { return Err("links.url não é HTTP/HTTPS absoluta".into()); }
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AssetParams { pub ticket_id: String, pub review_id: String, pub asset: String }

impl AssetParams {
    pub fn new(ticket_id: impl Into<String>, review_id: impl Into<String>, asset: impl Into<String>) -> Result<Self, ConnectorError> {
        let ticket_id = GetParams::new(ticket_id)?.ticket_id;
        let AssetRefs { review_id, asset } = AssetRefs::new(review_id, asset)?;
        Ok(Self { ticket_id, review_id, asset })
    }
}

/// `review_id` e `asset` já conferidos, para quem só descobre o ticket depois.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AssetRefs { pub review_id: String, pub asset: String }

impl AssetRefs {
    pub fn new(review_id: impl Into<String>, asset: impl Into<String>) -> Result<Self, ConnectorError> {
        let (review_id, asset) = (review_id.into(), asset.into());
        reference("review_id", &review_id).and_then(|()| reference("asset", &asset)).map_err(ConnectorError::invalid_params)?;
        Ok(Self { review_id, asset })
    }

    pub fn for_ticket(self, ticket_id: impl Into<String>) -> Result<AssetParams, ConnectorError> {
        Ok(AssetParams { ticket_id: GetParams::new(ticket_id)?.ticket_id, review_id: self.review_id, asset: self.asset })
    }
}

/// Tipo de imagem aceito, com a extensão do arquivo e os bytes iniciais: o `mime` sozinho não prova
/// o conteúdo.
pub struct ImageType {
    pub mime: &'static str,
    pub ext: &'static str,
    matches: fn(&[u8]) -> bool,
}

pub const IMAGE_TYPES: [ImageType; 3] = [
    ImageType { mime: "image/png", ext: "png", matches: |bytes| bytes.starts_with(b"\x89PNG\r\n\x1a\n") },
    ImageType { mime: "image/jpeg", ext: "jpg", matches: |bytes| bytes.starts_with(b"\xff\xd8\xff") },
    ImageType { mime: "image/webp", ext: "webp", matches: |bytes| bytes.len() >= 12 && bytes.starts_with(b"RIFF") && &bytes[8..12] == b"WEBP" },
];

/// Imagem de um lado ou de um antes e depois, já decodificada.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Asset { pub mime: String, pub bytes: Vec<u8> }

#[derive(Deserialize)]
struct RawAsset { mime: String, data: String }

impl Asset {
    pub fn from_connector(value: Value) -> Result<Self, ConnectorError> {
        let raw: RawAsset = serde_json::from_value(value)
            .map_err(|error| ConnectorError::protocol(format!("imagem fora do contrato: {error}")))?;
        let Some(kind) = IMAGE_TYPES.iter().find(|kind| kind.mime == raw.mime) else {
            return Err(ConnectorError::protocol("imagem fora de PNG, JPEG e WebP"));
        };
        // Conferido antes de decodificar: o tamanho em base64 já diz se passa do teto.
        if raw.data.len() > MAX_ASSET_BYTES.div_ceil(3) * 4 { return Err(ConnectorError::protocol("imagem acima de 2 MiB")); }
        let bytes = base64::engine::general_purpose::STANDARD.decode(raw.data.as_bytes())
            .map_err(|_| ConnectorError::protocol("imagem com base64 inválido"))?;
        if bytes.len() > MAX_ASSET_BYTES { return Err(ConnectorError::protocol("imagem acima de 2 MiB")); }
        if !(kind.matches)(&bytes) { return Err(ConnectorError::protocol("conteúdo da imagem não confere com o mime")); }
        Ok(Self { mime: raw.mime, bytes })
    }
}

/// A escolha da pessoa numa decisão.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Answer {
    pub id: String,
    pub choice: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DecideParams { pub ticket_id: String, pub review_id: String, pub decisions: Vec<Answer> }

impl DecideParams {
    pub fn new(ticket_id: impl Into<String>, review_id: impl Into<String>, decisions: Vec<Answer>) -> Result<Self, ConnectorError> {
        let params = Self { ticket_id: GetParams::new(ticket_id)?.ticket_id, review_id: review_id.into(), decisions };
        params.check().map_err(ConnectorError::invalid_params)?;
        Ok(params)
    }

    fn check(&self) -> Result<(), String> {
        reference("review_id", &self.review_id)?;
        if !(1..=MAX_DECISIONS).contains(&self.decisions.len()) { return Err(format!("decisions fora de 1 a {MAX_DECISIONS}")); }
        let mut ids = HashSet::new();
        for answer in &self.decisions {
            reference("decisions.id", &answer.id)?;
            if !ids.insert(answer.id.as_str()) { return Err("decisão repetida".into()); }
            if !crate::manifest::valid_id(&answer.choice) { return Err("choice fora de [a-z][a-z0-9-]{0,63}".into()); }
            optional("reason", answer.reason.as_deref(), MAX_REASON)?;
        }
        Ok(())
    }

    pub fn from_value(value: Value) -> Result<Self, ConnectorError> {
        let params: DecideParams = serde_json::from_value(value)
            .map_err(|error| ConnectorError::invalid_params(format!("pedido de decisão inválido: {error}")))?;
        Self::new(params.ticket_id, params.review_id, params.decisions)
    }
}

/// Resultado de `work_review.decide`: só `{"ok": true}` confirma.
pub fn decided(value: Value) -> Result<(), ConnectorError> {
    if value.get("ok") == Some(&Value::Bool(true)) { Ok(()) } else { Err(ConnectorError::protocol("decide sem {\"ok\": true}")) }
}
