use std::collections::BTreeMap;

use serde_json::Value;

pub const FULL_PCT: f64 = 99.0;
pub const LOGIN_SPENT_S: f64 = 3600.0;

pub type SpentAccounts = BTreeMap<String,SpentMark>;

#[derive(Clone,Copy,serde::Serialize)]
#[serde(untagged)]
pub enum SpentMark {
    Legacy(f64),
    Recorded { until:f64, at:Option<f64>, #[serde(skip_serializing_if="Option::is_none")] login:Option<f64> },
}

impl<'de> serde::Deserialize<'de> for SpentMark {
    fn deserialize<D:serde::Deserializer<'de>>(deserializer:D) -> Result<Self,D::Error> {
        // O serde_json do servidor conserva precisão arbitrária; o enum untagged não lê seus floats.
        let raw = <Value as serde::Deserialize>::deserialize(deserializer)?;
        if let Some(until) = raw.as_f64() { return Ok(Self::Legacy(until)); }
        let until = raw["until"].as_f64().ok_or_else(||serde::de::Error::custom("prazo de limite inválido"))?;
        let time = |field:&str| match raw.get(field) {
            None | Some(Value::Null)=>Ok(None),
            Some(value)=>value.as_f64().map(Some).ok_or_else(||serde::de::Error::custom("hora de limite inválida")),
        };
        Ok(Self::Recorded {until,at:time("at")?,login:time("login")?})
    }
}

impl From<f64> for SpentMark {
    fn from(until:f64) -> Self { Self::Legacy(until) }
}

impl SpentMark {
    pub fn blocking_until(self,row:Option<&Value>,now:f64) -> Option<f64> {
        let (until,at) = match self { Self::Legacy(until)=>(until,None),Self::Recorded{until,at,..}=>(until,at) };
        if until <= now { return None; }
        // A hora é da última leitura boa, inclusive quando uma falha posterior preservou o cache.
        let recovered = at.zip(row).is_some_and(|(at,row)| {
            row["estado"] == "lida"
                && row["ts"].as_f64().is_some_and(|ts|ts > at && ts <= now)
                && row["janelas"].as_array().is_some_and(|windows|!windows.is_empty()
                    && windows.iter().all(|w|w["pct"].as_f64().is_some_and(|pct| (0.0..FULL_PCT).contains(&pct))))
        });
        (!recovered).then_some(until)
    }

    /// Login refeito depois da recusa libera a marca que só ela pôs; limite que estendeu o prazo segue valendo.
    pub fn relogged(self,written:impl FnOnce()->Option<f64>) -> bool {
        let Self::Recorded{until,login:Some(refused),..} = self else { return false };
        until <= refused + LOGIN_SPENT_S && written().is_some_and(|written|written > refused)
    }
}
