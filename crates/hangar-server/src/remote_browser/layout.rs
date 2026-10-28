//! Posse do tamanho imposto: o último espectador conserva o original.
use super::{input, shell, ws::Failure};
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    path::Path,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};
struct Record {
    owner: u64,
    original: String,
    imposed: String,
}
type Slot = Arc<tokio::sync::Mutex<Option<Record>>>;
pub(super) struct LayoutChange {
    pub event: Value,
    pub dimensions: Option<(i64, i64)>,
}

pub struct Layouts {
    slots: Mutex<HashMap<String, Slot>>,
    next: AtomicU64,
    pub(super) http: reqwest::Client,
}
impl Default for Layouts {
    fn default() -> Self {
        Self {
            slots: Mutex::default(),
            next: AtomicU64::new(1),
            http: reqwest::Client::builder()
                .no_proxy()
                .connect_timeout(Duration::from_secs(45))
                .read_timeout(Duration::from_secs(45))
                .build()
                .expect("cliente HTTP local"),
        }
    }
}
pub(super) fn label(answer: &str) -> Option<&str> {
    answer.strip_prefix("layout:").map(str::trim)
}
impl Layouts {
    fn slot(&self, name: &str) -> Slot {
        self.slots
            .lock()
            .unwrap()
            .entry(name.into())
            .or_default()
            .clone()
    }
    pub(super) async fn apply(
        &self,
        dir: &Path,
        name: &str,
        message: &Value,
        owner: &mut Option<u64>,
    ) -> Result<Option<LayoutChange>, Failure> {
        if message.get("w").is_some() || message.get("h").is_some() {
            let dimension = |key| {
                input::integer(&message[key])
                    .ok()
                    .filter(|v| (1..=8192).contains(v))
            };
            let (Some(width), Some(height)) = (dimension("w"), dimension("h")) else {
                return Ok(None);
            };
            let id = *owner.get_or_insert_with(|| self.next.fetch_add(1, Ordering::Relaxed));
            let slot = self.slot(name);
            let mut record = slot.lock().await;
            let original = match record.as_ref() {
                Some(record) => record.original.clone(),
                None => {
                    let answer = shell::verb(&self.http, dir, name, &[]).await?;
                    let Some(label) = label(&answer) else {
                        return Ok(Some(LayoutChange {
                            event: json!({"t":"erro","m":answer}),
                            dimensions: None,
                        }));
                    };
                    label.to_owned()
                }
            };
            let answer = shell::verb(
                &self.http,
                dir,
                name,
                &[width.to_string(), height.to_string()],
            )
            .await?;
            let dimensions = if label(&answer).is_some() {
                *record = Some(Record {
                    owner: id,
                    original,
                    imposed: format!("{width}x{height}"),
                });
                Some((width, height))
            } else {
                None
            };
            Ok(Some(LayoutChange {
                event: json!({"t":"layout","resposta":answer}),
                dimensions,
            }))
        } else {
            let mode = message["modo"].as_str().unwrap_or("");
            if !["mobile", "desktop"].contains(&mode) {
                return Ok(None);
            }
            let answer = shell::verb(&self.http, dir, name, &[mode.to_owned()]).await?;
            Ok(Some(LayoutChange {
                event: json!({"t":"layout","modo":mode,"resposta":answer}),
                dimensions: None,
            }))
        }
    }
    pub(super) async fn restore(&self, dir: &Path, name: &str, owner: Option<u64>) {
        let slot = self.slot(name);
        let mut record = slot.lock().await;
        if record.as_ref().is_none_or(|r| Some(r.owner) != owner) {
            return;
        }
        let record = record.take().unwrap();
        let result = async {
            let current = shell::verb(&self.http, dir, name, &[]).await?;
            if label(&current) != Some(record.imposed.as_str()) {
                return Ok::<_, Failure>(());
            }
            let args = if ["desktop", "mobile"].contains(&record.original.as_str()) {
                vec![record.original.clone()]
            } else {
                record.original.split('x').map(str::to_owned).collect()
            };
            let answer = shell::verb(&self.http, dir, name, &args).await?;
            if label(&answer).is_none() {
                tracing::warn!(target: "hangar.navsock",
                    "navsock {name}: layout de antes ({}) não voltou: {answer}", record.original);
            }
            Ok(())
        }.await;
        if let Err(error) = result {
            tracing::warn!(target:"hangar.navsock","navsock {name}: layout de antes não voltou: {}",error.message);
        }
    }
}
