//! Conserva os valores brutos antes de separar o envelope do corpo tipado.
use serde::{Deserialize, Deserializer, de::{self, DeserializeOwned, MapAccess, Visitor}};
use serde_json::value::RawValue;

/// Float que atravessa o buffer do serde (enum com tag, `flatten`): com `arbitrary_precision` o
/// número chega como mapa, que `f64` recusa e `Number` aceita.
pub(crate) fn float<'de, D: Deserializer<'de>>(deserializer: D) -> Result<f64, D::Error> {
    serde_json::Number::deserialize(deserializer)?
        .as_f64()
        .ok_or_else(|| de::Error::custom("número fora do alcance de f64"))
}

pub(crate) fn float32<'de, D: Deserializer<'de>>(deserializer: D) -> Result<f32, D::Error> {
    float(deserializer).map(|value| value as f32)
}

pub(crate) struct Fields(Vec<(String, Box<RawValue>)>);

impl<'de> Deserialize<'de> for Fields {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct Object;
        impl<'de> Visitor<'de> for Object {
            type Value = Fields;
            fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
                formatter.write_str("um objeto JSON")
            }
            fn visit_map<M: MapAccess<'de>>(self, mut map: M) -> Result<Fields, M::Error> {
                let mut fields = Vec::new();
                while let Some(field) = map.next_entry()? { fields.push(field); }
                Ok(Fields(fields))
            }
        }
        deserializer.deserialize_map(Object)
    }
}

impl Fields {
    pub(crate) fn take<T: DeserializeOwned>(&mut self, name: &str) -> Result<T, String> {
        let mut positions = self.0.iter().enumerate().filter(|(_, (key, _))| key == name);
        let index = positions.next().map(|(index, _)| index).ok_or_else(|| format!("campo ausente: {name}"))?;
        if positions.next().is_some() { return Err(format!("campo duplicado: {name}")); }
        let (_, raw) = self.0.remove(index);
        serde_json::from_str(raw.get()).map_err(|error| error.to_string())
    }

    pub(crate) fn decode<T: DeserializeOwned>(&self) -> Result<T, serde_json::Error> {
        // O Content do serde transforma floats em mapas com arbitrary_precision. Os pares brutos
        // conservam também duplicatas para o desserializador do corpo recusá-las como antes.
        let fields = self.0.iter().map(|(key, raw)| {
            format!("{}:{}", serde_json::to_string(key).expect("chave JSON"), raw.get())
        }).collect::<Vec<_>>().join(",");
        serde_json::from_str(&format!("{{{fields}}}"))
    }
}

pub(crate) struct Envelope<T> {
    pub(crate) sessao: String,
    pub(crate) token: String,
    pub(crate) body: T,
}

impl<'de, T: DeserializeOwned> Deserialize<'de> for Envelope<T> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let mut fields = Fields::deserialize(deserializer)?;
        Ok(Self {
            sessao: fields.take("sessao").map_err(de::Error::custom)?,
            token: fields.take("token").map_err(de::Error::custom)?,
            body: fields.decode().map_err(de::Error::custom)?,
        })
    }
}
