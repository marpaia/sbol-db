use serde::{Deserialize, Serialize};

use crate::iri::IriString;

/// Validation severity recorded in `validation_findings.severity`.
///
/// The Postgres CHECK constraint allows info/warning/error/fatal, but the
/// upstream `sbol` crate currently emits only `Warning` and `Error`. We
/// preserve the full enum so the storage layer doesn't have to widen later.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    Info,
    Warning,
    Error,
    Fatal,
}

impl Severity {
    pub fn as_db_str(self) -> &'static str {
        match self {
            Self::Info => "info",
            Self::Warning => "warning",
            Self::Error => "error",
            Self::Fatal => "fatal",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ValidationStatus {
    Passed,
    Warning,
    Failed,
    Error,
}

impl ValidationStatus {
    pub fn as_db_str(self) -> &'static str {
        match self {
            Self::Passed => "passed",
            Self::Warning => "warning",
            Self::Failed => "failed",
            Self::Error => "error",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ValidationFinding {
    pub severity: Severity,
    pub rule_id: Option<String>,
    pub message: String,
    pub subject_iri: Option<IriString>,
    pub property: Option<String>,
}
