// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! Advisory database for dependency auditing — struct.build/AU1-AU5.
//!
//! Checks locked dependency versions against known vulnerabilities.
//! Supports both online fetch from the advisory server and offline
//! mode with a local JSON database file.

use serde::Deserialize;

/// Default advisory database URL (AU1).
pub const DEFAULT_ADVISORY_URL: &str = "https://advisories.rk-lang.org";

/// A single security advisory.
#[derive(Debug, Clone, Deserialize)]
pub struct Advisory {
    /// Advisory identifier (e.g., "CVE-2026-1234" or "RKSA-001").
    pub id: String,
    /// Affected package name.
    pub package: String,
    /// Affected version range (e.g., "<2.1.0", ">=1.0,<1.5.3").
    pub affected: String,
    /// Short description of the vulnerability.
    pub title: String,
    /// Severity level: "low", "medium", "high", "critical".
    #[serde(default)]
    pub severity: String,
    /// URL with more details.
    #[serde(default)]
    pub url: Option<String>,
}

/// The full advisory database.
#[derive(Debug, Clone, Deserialize)]
pub struct AdvisoryDb {
    /// All known advisories.
    pub advisories: Vec<Advisory>,
    /// ISO 8601 timestamp of last database update.
    #[serde(default)]
    pub updated: String,
}

impl AdvisoryDb {
    /// Fetch the advisory database from a remote URL (AU1).
    pub fn fetch(url: &str) -> Result<Self, String> {
        let response = reqwest::blocking::get(url)
            .map_err(|e| format!("failed to fetch advisory database: {}", e))?;

        if !response.status().is_success() {
            return Err(format!(
                "advisory database returned HTTP {}", response.status()
            ));
        }

        let body = response.text()
            .map_err(|e| format!("failed to read advisory response: {}", e))?;

        serde_json::from_str(&body)
            .map_err(|e| format!("failed to parse advisory database: {}", e))
    }

    /// Load from a local JSON file (AU5: offline mode).
    pub fn load_file(path: &str) -> Result<Self, String> {
        let content = std::fs::read_to_string(path)
            .map_err(|e| format!("failed to read {}: {}", path, e))?;

        serde_json::from_str(&content)
            .map_err(|e| format!("failed to parse {}: {}", path, e))
    }

    /// Find all advisories affecting a specific package version (AU2).
    pub fn lookup(&self, name: &str, version: &str) -> Vec<&Advisory> {
        let parsed_version = match crate::semver::Version::parse(version) {
            Ok(v) => v,
            Err(_) => return Vec::new(),
        };

        self.advisories.iter()
            .filter(|a| a.package == name && version_matches_affected(&parsed_version, &a.affected))
            .collect()
    }
}

/// Check if a version matches an affected range string.
///
/// Supports formats:
///   - "<2.1.0" — all versions below 2.1.0
///   - ">=1.0,<1.5.3" — versions in range [1.0, 1.5.3)
///   - "=1.2.3" — exact version match
fn version_matches_affected(version: &crate::semver::Version, affected: &str) -> bool {
    // Split on comma for range expressions
    let parts: Vec<&str> = affected.split(',').map(|s| s.trim()).collect();

    for part in &parts {
        if !single_constraint_matches(version, part) {
            return false;
        }
    }

    true
}

fn single_constraint_matches(version: &crate::semver::Version, constraint: &str) -> bool {
    let constraint = constraint.trim();

    if constraint.starts_with(">=") {
        match crate::semver::Version::parse(&constraint[2..]) {
            Ok(bound) => version >= &bound,
            Err(_) => false,
        }
    } else if constraint.starts_with('>') {
        match crate::semver::Version::parse(&constraint[1..]) {
            Ok(bound) => version > &bound,
            Err(_) => false,
        }
    } else if constraint.starts_with("<=") {
        match crate::semver::Version::parse(&constraint[2..]) {
            Ok(bound) => version <= &bound,
            Err(_) => false,
        }
    } else if constraint.starts_with('<') {
        match crate::semver::Version::parse(&constraint[1..]) {
            Ok(bound) => version < &bound,
            Err(_) => false,
        }
    } else if constraint.starts_with('=') {
        match crate::semver::Version::parse(&constraint[1..]) {
            Ok(bound) => version == &bound,
            Err(_) => false,
        }
    } else {
        // Bare version treated as exact match
        match crate::semver::Version::parse(constraint) {
            Ok(bound) => version == &bound,
            Err(_) => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_db(advisories: Vec<Advisory>) -> AdvisoryDb {
        AdvisoryDb {
            advisories,
            updated: "2026-01-01T00:00:00Z".to_string(),
        }
    }

    fn advisory(id: &str, package: &str, affected: &str) -> Advisory {
        Advisory {
            id: id.to_string(),
            package: package.to_string(),
            affected: affected.to_string(),
            title: format!("Test advisory {}", id),
            severity: "high".to_string(),
            url: None,
        }
    }

    #[test]
    fn test_lookup_exact_match() {
        let db = make_db(vec![
            advisory("CVE-001", "http", "=1.0.0"),
        ]);
        assert_eq!(db.lookup("http", "1.0.0").len(), 1);
        assert_eq!(db.lookup("http", "1.0.1").len(), 0);
    }

    #[test]
    fn test_lookup_less_than() {
        let db = make_db(vec![
            advisory("CVE-002", "json", "<2.0.0"),
        ]);
        assert_eq!(db.lookup("json", "1.5.0").len(), 1);
        assert_eq!(db.lookup("json", "2.0.0").len(), 0);
        assert_eq!(db.lookup("json", "2.1.0").len(), 0);
    }

    #[test]
    fn test_lookup_range() {
        let db = make_db(vec![
            advisory("CVE-003", "net", ">=1.0.0,<1.5.3"),
        ]);
        assert_eq!(db.lookup("net", "1.0.0").len(), 1);
        assert_eq!(db.lookup("net", "1.5.2").len(), 1);
        assert_eq!(db.lookup("net", "1.5.3").len(), 0);
        assert_eq!(db.lookup("net", "0.9.0").len(), 0);
    }

    #[test]
    fn test_lookup_wrong_package() {
        let db = make_db(vec![
            advisory("CVE-004", "http", "<5.0.0"),
        ]);
        assert_eq!(db.lookup("json", "1.0.0").len(), 0);
    }

    #[test]
    fn test_parse_advisory_db_json() {
        let json = r#"{
            "advisories": [
                {
                    "id": "RKSA-001",
                    "package": "crypto",
                    "affected": "<1.2.0",
                    "title": "Weak random number generation",
                    "severity": "critical",
                    "url": "https://example.com/RKSA-001"
                }
            ],
            "updated": "2026-02-01T00:00:00Z"
        }"#;

        let db: AdvisoryDb = serde_json::from_str(json).unwrap();
        assert_eq!(db.advisories.len(), 1);
        assert_eq!(db.advisories[0].id, "RKSA-001");
        assert_eq!(db.advisories[0].severity, "critical");
    }
}
