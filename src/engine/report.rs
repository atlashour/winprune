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
    pub blocked: usize,
    pub deferred: usize,
    pub absent: usize,
    pub failed: usize,
}

impl Tally {
    pub fn count(&mut self, outcome: &Outcome) {
        match outcome {
            Outcome::Done => self.done += 1,
            Outcome::Skipped(_) => self.skipped += 1,
            Outcome::Blocked(_) => self.blocked += 1,
            Outcome::Deferred(_) => self.deferred += 1,
            Outcome::Failed(_) => self.failed += 1,
        }
    }
}

/// One line per selected item, so an item whose every op was absent still shows up.
#[derive(Debug, Clone, Serialize)]
pub struct ItemSummary {
    pub id: String,
    pub ops: usize,
    pub to_apply: usize,
    pub done: usize,
    pub skipped: usize,
    pub blocked: usize,
    pub deferred: usize,
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
    pub items: Vec<ItemSummary>,
    pub results: Vec<OpResult>,
    pub tally: Tally,
}

impl Report {
    pub fn new(plan: &Plan, dry_run: bool) -> Report {
        Report {
            schema: 3,
            version: env!("CARGO_PKG_VERSION"),
            started: timestamp(),
            dry_run,
            build: plan.build,
            level: plan.level,
            items: plan
                .selected()
                .map(|i| ItemSummary {
                    id: i.id.clone(),
                    ops: i.ops.len(),
                    to_apply: i.will_apply(),
                    done: 0,
                    skipped: 0,
                    blocked: 0,
                    deferred: 0,
                    failed: 0,
                })
                .collect(),
            results: Vec::new(),
            tally: Tally::default(),
        }
    }

    pub fn record(&mut self, result: OpResult) {
        if let Some(item) = self.items.iter_mut().find(|i| i.id == result.item) {
            match &result.outcome {
                Outcome::Done => item.done += 1,
                Outcome::Skipped(_) => item.skipped += 1,
                Outcome::Blocked(_) => item.blocked += 1,
                Outcome::Deferred(_) => item.deferred += 1,
                Outcome::Failed(_) => item.failed += 1,
            }
        }
        self.tally.count(&result.outcome);
        self.results.push(result);
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).expect("report serialises")
    }

    pub fn summary(&self) -> String {
        let t = self.tally;
        let mut s = format!(
            "{} done, {} skipped, {} blocked by Windows, {} absent or already done, {} failed",
            t.done, t.skipped, t.blocked, t.absent, t.failed
        );
        if t.deferred > 0 {
            s.push_str(&format!(", {} left for the next restart", t.deferred));
        }
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
        Outcome::Blocked(r) => ("blocked", Some(r.as_str())),
        Outcome::Deferred(r) => ("deferred", Some(r.as_str())),
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
    fn deferred_results_count_apart_from_failures() {
        let catalog = crate::catalog::Catalog::parse(
            "t",
            "[[item]]\nid = \"appx.demo\"\nname = \"Demo\"\ncategory = \"appx\"\nlevel = \"medium\"\nrisk = \"low\"\nsummary = \"x\"\n[[item.step]]\nkind = \"appx\"\npatterns = [\"*Demo*\"]\n",
        )
        .unwrap();
        let plan = crate::engine::build_plan(
            &catalog,
            &crate::engine::Selection::level(Level::Medium),
            22631,
            true,
            &crate::system::fake::Fake::default(),
        );
        let mut report = Report::new(&plan, false);
        report.record(OpResult {
            item: "appx.demo".into(),
            op: "delete C:\\x".into(),
            outcome: Outcome::Deferred("held by explorer.exe".into()),
        });
        assert_eq!(report.tally.deferred, 1);
        assert_eq!(report.tally.failed, 0);
        assert_eq!(report.items[0].deferred, 1);
        assert!(report.summary().contains("1 left for the next restart"));
        assert!(report.to_json().contains("\"status\": \"deferred\""));
    }

    #[test]
    fn timestamp_looks_like_iso_8601() {
        let t = timestamp();
        assert_eq!(t.len(), 20, "{t}");
        assert!(t.starts_with("20"), "{t}");
        assert!(t.ends_with('Z'));
    }
}
