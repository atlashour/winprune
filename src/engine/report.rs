use super::plan::Plan;
use crate::catalog::Level;
use crate::system::Outcome;
use serde::Serialize;
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, Serialize)]
pub struct OpResult {
    pub item: String,
    pub op: String,
    #[serde(serialize_with = "serialize_outcome")]
    pub outcome: Outcome,
}

#[derive(Debug, Clone, Copy, Default, Serialize)]
pub struct Tally {
    pub done: usize,
    pub skipped: usize,
    pub absent: usize,
    pub failed: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct Report {
    pub schema: u32,
    pub version: &'static str,
    pub started: String,
    pub dry_run: bool,
    pub build: u32,
    pub level: Level,
    pub items: Vec<String>,
    pub results: Vec<OpResult>,
    pub tally: Tally,
}

impl Report {
    pub fn new(plan: &Plan, dry_run: bool) -> Report {
        Report {
            schema: 1,
            version: env!("CARGO_PKG_VERSION"),
            started: timestamp(),
            dry_run,
            build: plan.build,
            level: plan.level,
            items: plan.selected().map(|i| i.id.clone()).collect(),
            results: Vec::new(),
            tally: Tally::default(),
        }
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).expect("report serialises")
    }

    pub fn summary(&self) -> String {
        let t = self.tally;
        let mut s = format!(
            "{} done, {} skipped, {} absent or already done, {} failed",
            t.done, t.skipped, t.absent, t.failed
        );
        if self.dry_run {
            s.push_str(" (dry run, nothing was changed)");
        }
        s
    }
}

fn serialize_outcome<S: serde::Serializer>(o: &Outcome, s: S) -> Result<S::Ok, S::Error> {
    use serde::ser::SerializeStruct;
    let (status, reason) = match o {
        Outcome::Done => ("done", None),
        Outcome::Skipped(r) => ("skipped", Some(r.as_str())),
        Outcome::Failed(r) => ("failed", Some(r.as_str())),
    };
    let mut st = s.serialize_struct("Outcome", 2)?;
    st.serialize_field("status", status)?;
    st.serialize_field("reason", &reason)?;
    st.end()
}

/// ISO-8601 UTC without pulling a date crate in. Civil-from-days after H. Hinnant.
pub fn timestamp() -> String {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let days = (secs / 86_400) as i64;
    let rem = secs % 86_400;
    let (h, m, s) = (rem / 3600, (rem % 3600) / 60, rem % 60);
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let mo = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if mo <= 2 { y + 1 } else { y };
    format!("{y:04}-{mo:02}-{d:02}T{h:02}:{m:02}:{s:02}Z")
}

pub fn file_stamp() -> String {
    timestamp().chars().filter(|c| c.is_ascii_digit()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timestamp_looks_like_iso_8601() {
        let t = timestamp();
        assert_eq!(t.len(), 20, "{t}");
        assert!(t.starts_with("20"), "{t}");
        assert!(t.ends_with('Z'));
    }
}
