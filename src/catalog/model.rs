use serde::{Deserialize, Serialize};
use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Level {
    Medium,
    High,
    Max,
}

impl Level {
    pub const ALL: [Level; 3] = [Level::Medium, Level::High, Level::Max];

    pub fn describe(self) -> &'static str {
        match self {
            Level::Medium => "any machine: bloatware, telemetry, privacy policies",
            Level::High => {
                "workstations without cloud ties: adds OneDrive, Xbox, Copilot, identity reset"
            }
            Level::Max => {
                "kiosks and signage: adds Windows Update, Search, Widgets, OneDrive folder"
            }
        }
    }
}

impl fmt::Display for Level {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Level::Medium => "medium",
            Level::High => "high",
            Level::Max => "max",
        })
    }
}

impl std::str::FromStr for Level {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_ascii_lowercase().as_str() {
            "medium" => Ok(Level::Medium),
            "high" => Ok(Level::High),
            "max" => Ok(Level::Max),
            other => Err(format!(
                "unknown level '{other}', expected medium, high or max"
            )),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Risk {
    Low,
    Medium,
    High,
}

impl fmt::Display for Risk {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Risk::Low => "low",
            Risk::Medium => "medium",
            Risk::High => "high",
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Category {
    Appx,
    Services,
    Telemetry,
    Privacy,
    OneDrive,
    Xbox,
    Identity,
    Update,
    Shell,
}

impl Category {
    pub const ALL: [Category; 9] = [
        Category::Appx,
        Category::Services,
        Category::Telemetry,
        Category::Privacy,
        Category::OneDrive,
        Category::Xbox,
        Category::Identity,
        Category::Update,
        Category::Shell,
    ];

    pub fn title(self) -> &'static str {
        match self {
            Category::Appx => "Preinstalled apps",
            Category::Services => "Services",
            Category::Telemetry => "Telemetry",
            Category::Privacy => "Privacy",
            Category::OneDrive => "OneDrive",
            Category::Xbox => "Xbox and gaming",
            Category::Identity => "Identifiers",
            Category::Update => "Windows Update",
            Category::Shell => "Shell",
        }
    }
}

impl fmt::Display for Category {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.title())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Startup {
    Disabled,
    Manual,
    Automatic,
}

impl fmt::Display for Startup {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Startup::Disabled => "disabled",
            Startup::Manual => "manual",
            Startup::Automatic => "automatic",
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Hive {
    Hklm,
    Hkcu,
    Hkcr,
}

impl fmt::Display for Hive {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Hive::Hklm => "HKLM",
            Hive::Hkcu => "HKCU",
            Hive::Hkcr => "HKCR",
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RegType {
    Dword,
    String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TaskAction {
    Disable,
    Delete,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase", deny_unknown_fields)]
pub enum Step {
    Appx {
        patterns: Vec<String>,
    },
    Service {
        names: Vec<String>,
        startup: Startup,
    },
    Registry {
        hive: Hive,
        path: String,
        name: String,
        #[serde(rename = "type", default)]
        kind: Option<RegType>,
        #[serde(default)]
        value: Option<toml::Value>,
        #[serde(default)]
        delete: bool,
    },
    Task {
        patterns: Vec<String>,
        action: TaskAction,
    },
    Kill {
        processes: Vec<String>,
    },
    Run {
        candidates: Vec<String>,
        #[serde(default)]
        args: Vec<String>,
    },
    Delete {
        paths: Vec<String>,
    },
}

impl Step {
    pub fn kind_name(&self) -> &'static str {
        match self {
            Step::Appx { .. } => "appx",
            Step::Service { .. } => "service",
            Step::Registry { .. } => "registry",
            Step::Task { .. } => "task",
            Step::Kill { .. } => "kill",
            Step::Run { .. } => "run",
            Step::Delete { .. } => "delete",
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Applicability {
    #[serde(default)]
    pub min_build: Option<u32>,
    #[serde(default)]
    pub max_build: Option<u32>,
}

impl Applicability {
    pub fn matches(&self, build: u32) -> bool {
        self.min_build.is_none_or(|min| build >= min)
            && self.max_build.is_none_or(|max| build <= max)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Item {
    pub id: String,
    pub name: String,
    pub category: Category,
    pub level: Level,
    pub risk: Risk,
    pub summary: String,
    #[serde(default)]
    pub warning: Option<String>,
    #[serde(default)]
    pub requires: Vec<String>,
    #[serde(default)]
    pub windows: Applicability,
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default)]
    pub step: Vec<Step>,
}

fn default_true() -> bool {
    true
}
