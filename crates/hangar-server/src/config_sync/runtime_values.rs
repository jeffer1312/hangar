//! Arquivos e corpos externos podem guardar subárvores ignoradas pelas preferências.
use crate::orchestration::codec;
use serde_json::{Map, Value};
use std::ops::{Deref, DerefMut};

pub(super) struct Values(pub Map<String, Value>);

pub(crate) struct ValueGuard(pub Value);
impl Deref for ValueGuard {
    type Target = Value;
    fn deref(&self) -> &Value { &self.0 }
}
impl DerefMut for ValueGuard {
    fn deref_mut(&mut self) -> &mut Value { &mut self.0 }
}
impl Drop for ValueGuard {
    fn drop(&mut self) { codec::discard(std::mem::replace(&mut self.0, Value::Null)); }
}

impl Values {
    pub fn read(bytes: &[u8]) -> Option<Self> {
        match codec::loads(std::str::from_utf8(bytes).ok()?)? {
            Value::Object(map) => Some(Self(map)),
            other => { codec::discard(other); None }
        }
    }
    pub fn copied(map: &Map<String, Value>) -> Self {
        Self(map.iter().map(|(key, value)| (key.clone(), codec::clone_value(value))).collect())
    }
    pub fn into_map(mut self) -> Map<String, Value> { std::mem::take(&mut self.0) }
    pub fn insert(&mut self, key: String, value: Value) {
        if let Some(old) = self.0.insert(key, value) { codec::discard(old); }
    }
    pub fn extend(&mut self, values: impl IntoIterator<Item = (String, Value)>) {
        for (key, value) in values { self.insert(key, value); }
    }
}
impl Clone for Values {
    fn clone(&self) -> Self { Self::copied(&self.0) }
}
impl Deref for Values {
    type Target = Map<String, Value>;
    fn deref(&self) -> &Self::Target { &self.0 }
}
impl DerefMut for Values {
    fn deref_mut(&mut self) -> &mut Self::Target { &mut self.0 }
}
impl Drop for Values {
    fn drop(&mut self) {
        for (_, value) in std::mem::take(&mut self.0) { codec::discard(value); }
    }
}
