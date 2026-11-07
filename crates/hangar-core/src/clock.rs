//! Relógio de parede.

/// Segundos desde a época Unix; relógio antes de 1970 vira 0.
pub fn wall_now() -> f64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0.0, |d| d.as_secs_f64())
}
