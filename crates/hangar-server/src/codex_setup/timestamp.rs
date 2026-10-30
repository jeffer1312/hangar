//! Datas publicadas pela integração usam a precisão de microssegundos do contrato.
#[derive(Debug)]
pub struct Error { pub kind: &'static str, pub message: String }

pub fn iso(value: f64) -> Result<Option<String>, Error> {
    use chrono::Datelike;
    if value == 0.0 { return Ok(None); }
    if value.is_nan() {
        return Err(Error {kind:"ValueError", message:"Invalid value NaN (not a number)".into()});
    }
    if !value.is_finite() || value < i64::MIN as f64 || value >= -(i64::MIN as f64) {
        return Err(Error {kind:"OverflowError", message:"timestamp out of range for platform time_t".into()});
    }
    let seconds = value.trunc();
    let micros = ((value - seconds) * 1_000_000.0).round_ties_even() as i64;
    let seconds = (seconds as i64).checked_add(micros.div_euclid(1_000_000))
        .ok_or_else(|| Error {kind:"OverflowError", message:"timestamp out of range for platform time_t".into()})?;
    let nanos = micros.rem_euclid(1_000_000) as u32 * 1000;
    let date = chrono::DateTime::from_timestamp(seconds, nanos)
        .ok_or_else(|| Error {kind:"ValueError", message:"Data de atualização fora do intervalo".into()})?;
    if !(1..=9999).contains(&date.year()) {
        return Err(Error {kind:"ValueError", message:format!("year must be in 1..9999, not {}", date.year())});
    }
    let precision = if nanos == 0 { chrono::SecondsFormat::Secs } else { chrono::SecondsFormat::Micros };
    Ok(Some(date.to_rfc3339_opts(precision, false)))
}

#[cfg(test)]
mod tests {
    #[test]
    fn timestamps_match_original_precision_rounding_and_refusals() {
        let fixture: serde_json::Value = serde_json::from_str(include_str!("../../../../backend/tests/fixtures/contract/codex-integration/timestamps.json")).unwrap();
        let mut differences = Vec::new();
        for row in fixture["cases"].as_array().unwrap() {
            let input = row["value"].as_str().unwrap();
            let actual = match super::iso(input.parse().unwrap()) {
                Ok(value) => serde_json::json!({"value":input,"result":value}),
                Err(error) => serde_json::json!({"value":input,"error":error.kind,"message":error.message}),
            };
            if &actual != row { differences.push((row.clone(),actual)); }
        }
        assert!(differences.is_empty(), "Horários divergentes: {differences:#?}");
    }
}
