use chrono::{DateTime, Datelike, Duration, LocalResult, NaiveTime, TimeZone, Utc};
use serde::{Deserialize, Serialize};

use crate::Error;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase", deny_unknown_fields)]
pub enum Schedule {
    Once { at: String },
    Weekly { time: String, weekdays: Vec<u32> },
    Interval { minutes: u32 },
}

pub fn next_run<T: TimeZone>(
    schedule: &Schedule,
    after: DateTime<Utc>,
    zone: &T,
) -> Result<Option<DateTime<Utc>>, Error> {
    validate_schedule(schedule)?;
    match schedule {
        Schedule::Once { at } => {
            let instant = parse_instant(at)?;
            Ok((instant > after).then_some(instant))
        }
        Schedule::Interval { minutes } => {
            Ok(after.checked_add_signed(Duration::minutes(i64::from(*minutes))))
        }
        Schedule::Weekly { time, weekdays } => {
            let time = parse_time(time)?;
            let date = after.with_timezone(zone).date_naive();
            for offset in 0..=14 {
                let Some(day) = date.checked_add_signed(Duration::days(offset)) else {
                    break;
                };
                if !weekdays.contains(&day.weekday().num_days_from_sunday()) {
                    continue;
                }
                // Um horário repetido tem uma ocorrência; o horário que não existe fica para outro dia.
                let candidate = match zone.from_local_datetime(&day.and_time(time)) {
                    LocalResult::Single(dt) => Some(dt.with_timezone(&Utc)),
                    LocalResult::Ambiguous(a, b) => {
                        Some(a.with_timezone(&Utc).min(b.with_timezone(&Utc)))
                    }
                    LocalResult::None => None,
                };
                if let Some(candidate) = candidate.filter(|at| *at > after) {
                    return Ok(Some(candidate));
                }
            }
            Err(Error::invalid(
                "Não foi possível calcular o próximo horário neste fuso.",
            ))
        }
    }
}

pub(crate) fn parse_instant(at: &str) -> Result<DateTime<Utc>, Error> {
    DateTime::parse_from_rfc3339(at)
        .map(|dt| dt.with_timezone(&Utc))
        .map_err(|_| Error::invalid("Informe uma data RFC3339 com o deslocamento de fuso."))
}

pub(crate) fn validate_schedule(schedule: &Schedule) -> Result<(), Error> {
    match schedule {
        Schedule::Once { at } => {
            parse_instant(at)?;
        }
        Schedule::Interval { minutes } if *minutes == 0 => {
            return Err(Error::invalid("O intervalo deve ter pelo menos 1 minuto."));
        }
        Schedule::Weekly { time, weekdays } => {
            parse_time(time)?;
            if weekdays.is_empty() || weekdays.len() > 7 || weekdays.iter().any(|day| *day > 6) {
                return Err(Error::invalid(
                    "Selecione dias da semana entre domingo e sábado.",
                ));
            }
            let mut unique = weekdays.clone();
            unique.sort_unstable();
            unique.dedup();
            if unique.len() != weekdays.len() {
                return Err(Error::invalid("Os dias da semana não podem se repetir."));
            }
        }
        _ => {}
    }
    Ok(())
}

fn parse_time(time: &str) -> Result<NaiveTime, Error> {
    if time.len() != 5
        || time.as_bytes()[2] != b':'
        || !time
            .bytes()
            .enumerate()
            .all(|(index, b)| index == 2 || b.is_ascii_digit())
    {
        return Err(Error::invalid("Informe o horário no formato HH:MM."));
    }
    NaiveTime::parse_from_str(time, "%H:%M")
        .map_err(|_| Error::invalid("Informe um horário válido no formato HH:MM."))
}
