// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! Semantic versioning — struct.packages/VR1-VR3, D1.
//!
//! Parses version strings and constraint ranges. Supports
//! `^` (compatible), `~` (tilde), `=` (exact), and `>=` (minimum).

use std::fmt;

/// A parsed semantic version: MAJOR.MINOR.PATCH[-PRERELEASE].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Version {
    pub major: u32,
    pub minor: u32,
    pub patch: u32,
    /// Pre-release label (e.g., "beta.3"). Empty = stable release.
    pub pre: String,
}

impl Version {
    /// Parse a version string like "1.2.3" or "1.2.3-beta.1".
    pub fn parse(s: &str) -> Result<Self, String> {
        let s = s.trim().trim_matches('"');
        if s.is_empty() {
            return Err("empty version string".into());
        }

        let (version_part, pre) = if let Some(idx) = s.find('-') {
            (&s[..idx], s[idx + 1..].to_string())
        } else {
            (s, String::new())
        };

        let parts: Vec<&str> = version_part.split('.').collect();
        if parts.is_empty() || parts.len() > 3 {
            return Err(format!("invalid version: '{}' — expected MAJOR.MINOR.PATCH", s));
        }

        let major = parts[0].parse::<u32>()
            .map_err(|_| format!("invalid major version: '{}'", parts[0]))?;
        let minor = parts.get(1)
            .map(|p| p.parse::<u32>())
            .transpose()
            .map_err(|_| format!("invalid minor version: '{}'", parts.get(1).unwrap_or(&"")))?
            .unwrap_or(0);
        let patch = parts.get(2)
            .map(|p| p.parse::<u32>())
            .transpose()
            .map_err(|_| format!("invalid patch version: '{}'", parts.get(2).unwrap_or(&"")))?
            .unwrap_or(0);

        Ok(Version { major, minor, patch, pre })
    }

    /// True if this is a pre-release version (VR2).
    pub fn is_prerelease(&self) -> bool {
        !self.pre.is_empty()
    }

    /// True if this is a 0.x version (VR3: unstable).
    pub fn is_unstable(&self) -> bool {
        self.major == 0
    }
}

impl fmt::Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.pre.is_empty() {
            write!(f, "{}.{}.{}", self.major, self.minor, self.patch)
        } else {
            write!(f, "{}.{}.{}-{}", self.major, self.minor, self.patch, self.pre)
        }
    }
}

impl PartialOrd for Version {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Version {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.major.cmp(&other.major)
            .then(self.minor.cmp(&other.minor))
            .then(self.patch.cmp(&other.patch))
            .then_with(|| {
                // Pre-release sorts before release (VR2).
                // "1.0.0-beta" < "1.0.0"
                match (self.pre.is_empty(), other.pre.is_empty()) {
                    (true, true) => std::cmp::Ordering::Equal,
                    (true, false) => std::cmp::Ordering::Greater,
                    (false, true) => std::cmp::Ordering::Less,
                    (false, false) => self.pre.cmp(&other.pre),
                }
            })
    }
}

/// A version constraint parsed from a dependency declaration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Constraint {
    /// `^1.2.3` or bare `1.2.3` — compatible (>=1.2.3, <2.0.0).
    /// For 0.x: bare → tilde behavior (VR3).
    Compatible(Version),
    /// `~1.2.3` — tilde (>=1.2.3, <1.3.0).
    Tilde(Version),
    /// `=1.2.3` — exact match.
    Exact(Version),
    /// `>=1.2.3` — minimum version.
    Minimum(Version),
    /// `*` — any version.
    Any,
}

impl Constraint {
    /// Parse a constraint string from build.rk dep declarations.
    ///
    /// Supported formats:
    /// - `"^1.2.3"` or `"1.2.3"` → Compatible
    /// - `"~1.2.3"` → Tilde
    /// - `"=1.2.3"` → Exact
    /// - `">=1.2.3"` → Minimum
    /// - `"*"` → Any
    pub fn parse(s: &str) -> Result<Self, String> {
        let s = s.trim().trim_matches('"');
        if s.is_empty() || s == "*" {
            return Ok(Constraint::Any);
        }

        if let Some(rest) = s.strip_prefix(">=") {
            let v = Version::parse(rest)?;
            return Ok(Constraint::Minimum(v));
        }
        if let Some(rest) = s.strip_prefix('^') {
            let v = Version::parse(rest)?;
            return Ok(Constraint::Compatible(v));
        }
        if let Some(rest) = s.strip_prefix('~') {
            let v = Version::parse(rest)?;
            return Ok(Constraint::Tilde(v));
        }
        if let Some(rest) = s.strip_prefix('=') {
            let v = Version::parse(rest)?;
            return Ok(Constraint::Exact(v));
        }

        // Bare version: "1.2.3" → Compatible for >=1.0, Tilde for 0.x (VR3)
        let v = Version::parse(s)?;
        if v.is_unstable() {
            Ok(Constraint::Tilde(v))
        } else {
            Ok(Constraint::Compatible(v))
        }
    }

    /// Check whether a candidate version satisfies this constraint.
    pub fn matches(&self, candidate: &Version) -> bool {
        match self {
            Constraint::Any => true,
            Constraint::Exact(v) => candidate == v,
            Constraint::Minimum(v) => candidate >= v,
            Constraint::Compatible(v) => {
                if candidate < v {
                    return false;
                }
                if v.major == 0 {
                    // 0.x: compatible means same minor (VR3)
                    candidate.major == 0 && candidate.minor == v.minor
                } else {
                    candidate.major == v.major
                }
            }
            Constraint::Tilde(v) => {
                if candidate < v {
                    return false;
                }
                candidate.major == v.major && candidate.minor == v.minor
            }
        }
    }
}

impl fmt::Display for Constraint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Constraint::Any => write!(f, "*"),
            Constraint::Compatible(v) => write!(f, "^{}", v),
            Constraint::Tilde(v) => write!(f, "~{}", v),
            Constraint::Exact(v) => write!(f, "={}", v),
            Constraint::Minimum(v) => write!(f, ">={}", v),
        }
    }
}

/// Select the maximum compatible version from a list of available versions
/// that satisfies all constraints (MV1).
pub fn resolve_version(
    constraints: &[Constraint],
    available: &[Version],
) -> Result<Version, String> {
    let mut candidates: Vec<&Version> = available.iter()
        .filter(|v| constraints.iter().all(|c| c.matches(v)))
        .collect();

    // MV1: select newest (maximum compatible)
    candidates.sort();
    candidates.last()
        .cloned()
        .cloned()
        .ok_or_else(|| {
            let cs: Vec<String> = constraints.iter().map(|c| c.to_string()).collect();
            format!("no version satisfies constraints: {}", cs.join(", "))
        })
}

/// Validate a version constraint string without resolving it.
/// Returns a human-readable description or error.
pub fn validate_constraint(s: &str) -> Result<String, String> {
    let c = Constraint::parse(s)?;
    match c {
        Constraint::Any => Ok("any version".into()),
        Constraint::Compatible(v) => {
            if v.major == 0 {
                Ok(format!(">={}, <0.{}.0", v, v.minor + 1))
            } else {
                Ok(format!(">={}, <{}.0.0", v, v.major + 1))
            }
        }
        Constraint::Tilde(v) => {
            Ok(format!(">={}, <{}.{}.0", v, v.major, v.minor + 1))
        }
        Constraint::Exact(v) => Ok(format!("exactly {}", v)),
        Constraint::Minimum(v) => Ok(format!(">={}", v)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_version() {
        let v = Version::parse("1.2.3").unwrap();
        assert_eq!(v.major, 1);
        assert_eq!(v.minor, 2);
        assert_eq!(v.patch, 3);
        assert!(v.pre.is_empty());
    }

    #[test]
    fn parse_version_prerelease() {
        let v = Version::parse("1.0.0-beta.3").unwrap();
        assert_eq!(v.major, 1);
        assert_eq!(v.pre, "beta.3");
        assert!(v.is_prerelease());
    }

    #[test]
    fn parse_version_short() {
        let v = Version::parse("2.1").unwrap();
        assert_eq!(v.major, 2);
        assert_eq!(v.minor, 1);
        assert_eq!(v.patch, 0);
    }

    #[test]
    fn version_ordering() {
        let v1 = Version::parse("1.0.0").unwrap();
        let v2 = Version::parse("1.0.1").unwrap();
        let v3 = Version::parse("1.1.0").unwrap();
        let v4 = Version::parse("2.0.0").unwrap();
        assert!(v1 < v2);
        assert!(v2 < v3);
        assert!(v3 < v4);
    }

    #[test]
    fn prerelease_sorts_before_release() {
        let pre = Version::parse("1.0.0-beta.1").unwrap();
        let rel = Version::parse("1.0.0").unwrap();
        assert!(pre < rel);
    }

    #[test]
    fn compatible_constraint() {
        let c = Constraint::parse("^1.2.3").unwrap();
        assert!(c.matches(&Version::parse("1.2.3").unwrap()));
        assert!(c.matches(&Version::parse("1.9.0").unwrap()));
        assert!(!c.matches(&Version::parse("2.0.0").unwrap()));
        assert!(!c.matches(&Version::parse("1.2.2").unwrap()));
    }

    #[test]
    fn compatible_0x_is_conservative() {
        // VR3: 0.x bare version → tilde-like behavior
        let c = Constraint::parse("0.5.0").unwrap();
        assert!(c.matches(&Version::parse("0.5.0").unwrap()));
        assert!(c.matches(&Version::parse("0.5.9").unwrap()));
        assert!(!c.matches(&Version::parse("0.6.0").unwrap()));
    }

    #[test]
    fn tilde_constraint() {
        let c = Constraint::parse("~1.2.3").unwrap();
        assert!(c.matches(&Version::parse("1.2.3").unwrap()));
        assert!(c.matches(&Version::parse("1.2.9").unwrap()));
        assert!(!c.matches(&Version::parse("1.3.0").unwrap()));
    }

    #[test]
    fn exact_constraint() {
        let c = Constraint::parse("=1.2.3").unwrap();
        assert!(c.matches(&Version::parse("1.2.3").unwrap()));
        assert!(!c.matches(&Version::parse("1.2.4").unwrap()));
    }

    #[test]
    fn minimum_constraint() {
        let c = Constraint::parse(">=1.5").unwrap();
        assert!(c.matches(&Version::parse("1.5.0").unwrap()));
        assert!(c.matches(&Version::parse("2.0.0").unwrap()));
        assert!(!c.matches(&Version::parse("1.4.9").unwrap()));
    }

    #[test]
    fn any_constraint() {
        let c = Constraint::parse("*").unwrap();
        assert!(c.matches(&Version::parse("0.0.1").unwrap()));
        assert!(c.matches(&Version::parse("99.0.0").unwrap()));
    }

    #[test]
    fn resolve_picks_newest() {
        let constraints = vec![Constraint::parse("^1.0").unwrap()];
        let available = vec![
            Version::parse("1.0.0").unwrap(),
            Version::parse("1.2.0").unwrap(),
            Version::parse("1.5.0").unwrap(),
            Version::parse("2.0.0").unwrap(),
        ];
        let resolved = resolve_version(&constraints, &available).unwrap();
        assert_eq!(resolved, Version::parse("1.5.0").unwrap());
    }

    #[test]
    fn resolve_multiple_constraints() {
        let constraints = vec![
            Constraint::parse("^1.0").unwrap(),
            Constraint::parse(">=1.3").unwrap(),
        ];
        let available = vec![
            Version::parse("1.0.0").unwrap(),
            Version::parse("1.2.0").unwrap(),
            Version::parse("1.5.0").unwrap(),
            Version::parse("2.0.0").unwrap(),
        ];
        let resolved = resolve_version(&constraints, &available).unwrap();
        assert_eq!(resolved, Version::parse("1.5.0").unwrap());
    }

    #[test]
    fn resolve_fails_on_conflict() {
        let constraints = vec![
            Constraint::parse("^1.0").unwrap(),
            Constraint::parse("^2.0").unwrap(),
        ];
        let available = vec![
            Version::parse("1.5.0").unwrap(),
            Version::parse("2.1.0").unwrap(),
        ];
        assert!(resolve_version(&constraints, &available).is_err());
    }

    #[test]
    fn validate_constraint_readable() {
        assert_eq!(validate_constraint("^2.0").unwrap(), ">=2.0.0, <3.0.0");
        assert_eq!(validate_constraint("~1.2.3").unwrap(), ">=1.2.3, <1.3.0");
        assert_eq!(validate_constraint("=1.0.0").unwrap(), "exactly 1.0.0");
    }

    #[test]
    fn quoted_version() {
        let v = Version::parse("\"1.2.3\"").unwrap();
        assert_eq!(v.major, 1);
        let c = Constraint::parse("\"^2.0\"").unwrap();
        assert!(c.matches(&Version::parse("2.5.0").unwrap()));
    }
}
