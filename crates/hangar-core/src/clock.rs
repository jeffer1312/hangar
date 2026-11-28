//! Relógio de parede.

/// Segundos desde a época Unix; relógio antes de 1970 vira 0.
pub fn wall_now() -> f64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0.0, |d| d.as_secs_f64())
}

#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClockSample {
    pub monotonic_s: f64,
    pub epoch_s: f64,
}
