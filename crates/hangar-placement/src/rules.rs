//! Regras, limites e modos do dono, como gravados em `~/.hangar/placement.json`.

use std::collections::BTreeMap;

use chrono::{DateTime, Datelike, Duration, NaiveTime, Utc};
use chrono_tz::Tz;
use serde::{Deserialize, Serialize};

use hangar_core::project::ProjectId;

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PlacementConfig {
    pub ui_mode: UiMode,
    pub agent_mode: AgentMode,
    pub limits: Limits,
    pub project_rules: Vec<ProjectRule>,
    pub schedule_rules: Vec<ScheduleRule>,
    /// Pasta de clone escolhida na tela, por máquina; vence a calculada pela própria máquina.
    pub clone_dirs: BTreeMap<String, String>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UiMode {
    Off,
    #[default]
    Suggest,
    Ask,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentMode {
    Off,
    #[default]
    Auto,
    Ask,
}

/// Máquina sai da escolha com memória livre abaixo de `mem_free_pct`, `working` ou mais sessões
/// trabalhando, ou processador acima de `cpu_pct`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Limits {
    pub mem_free_pct: u32,
    pub working: u32,
    pub cpu_pct: u32,
}

impl Default for Limits {
    fn default() -> Self {
        Self { mem_free_pct: 15, working: 6, cpu_pct: 80 }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectRule {
    pub project: ProjectId,
    pub server: String,
}

/// Evita `avoid` nos dias (1 = segunda … 7 = domingo; vazio = todos) entre `from` e `to` (`HH:MM`)
/// no fuso `tz`. `from` depois de `to` cruza a meia-noite e pertence ao dia em que começa;
/// `from` igual a `to` é o dia inteiro.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScheduleRule {
    #[serde(default)]
    pub days: Vec<u8>,
    pub from: String,
    pub to: String,
    pub tz: String,
    pub avoid: String,
}

impl ScheduleRule {
    /// Fuso ou hora ilegível não vale: quem grava (`PUT`) recusa antes.
    pub fn covers(&self, now: DateTime<Utc>) -> bool {
        let (Ok(tz), Some(from), Some(to)) = (self.tz.parse::<Tz>(), hhmm(&self.from), hhmm(&self.to)) else {
            return false;
        };
        let local = now.with_timezone(&tz);
        let day_on = |d: chrono::Weekday| {
            self.days.is_empty() || self.days.contains(&(d.number_from_monday() as u8))
        };
        let today = local.weekday();
        let t = local.time();
        if from == to {
            day_on(today)
        } else if from < to {
            day_on(today) && from <= t && t < to
        } else {
            (day_on(today) && t >= from) || (day_on((local - Duration::days(1)).weekday()) && t < to)
        }
    }
}

fn hhmm(s: &str) -> Option<NaiveTime> {
    NaiveTime::parse_from_str(s.trim(), "%H:%M").ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn rule(days: &[u8], from: &str, to: &str) -> ScheduleRule {
        ScheduleRule { days: days.to_vec(), from: from.into(), to: to.into(), tz: "America/Sao_Paulo".into(), avoid: "nb".into() }
    }

    /// 2026-11-27 é sexta; São Paulo é UTC-3.
    fn sp(day: u32, h: i64, m: i64) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 11, day, 0, 0, 0).unwrap() + Duration::hours(h + 3) + Duration::minutes(m)
    }

    #[test]
    fn interval_uses_the_rule_time_zone() {
        let r = rule(&[5], "09:00", "18:00");
        assert!(r.covers(sp(27, 9, 0)));
        assert!(!r.covers(sp(27, 18, 0)));
        // 07:00 em São Paulo é 10:00 UTC: dentro do intervalo se o fuso fosse ignorado.
        assert!(!r.covers(sp(27, 7, 0)));
        assert!(!r.covers(sp(26, 10, 0)), "quinta fora dos dias");
    }

    #[test]
    fn interval_crossing_midnight_belongs_to_the_start_day() {
        let r = rule(&[5], "22:00", "06:00");
        assert!(r.covers(sp(27, 23, 0)));
        assert!(r.covers(sp(28, 5, 59)), "madrugada de sábado vem da sexta");
        assert!(!r.covers(sp(28, 6, 0)));
        assert!(!r.covers(sp(27, 5, 0)), "madrugada de sexta seria da quinta");
    }

    #[test]
    fn empty_days_is_every_day_and_equal_ends_is_all_day() {
        assert!(rule(&[], "08:00", "09:00").covers(sp(25, 8, 30)));
        assert!(rule(&[5], "00:00", "00:00").covers(sp(27, 15, 0)));
    }

    #[test]
    fn unreadable_rule_never_covers() {
        let mut r = rule(&[], "08:00", "09:00");
        r.tz = "Lua/Base".into();
        assert!(!r.covers(sp(25, 8, 30)));
        assert!(!rule(&[], "8h", "09:00").covers(sp(25, 8, 30)));
    }

    #[test]
    fn missing_fields_take_the_defaults() {
        let c: PlacementConfig = serde_json::from_str(r#"{"limits":{"working":3}}"#).unwrap();
        assert_eq!(c.limits, Limits { mem_free_pct: 15, working: 3, cpu_pct: 80 });
        assert_eq!((c.ui_mode, c.agent_mode), (UiMode::Suggest, AgentMode::Auto));
    }
}
