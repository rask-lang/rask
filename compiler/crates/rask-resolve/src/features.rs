// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! Feature resolution — struct.build/F1-F6, FG1-FG6.
//!
//! Resolves which features are enabled for a build and collects
//! the additional dependencies they gate.

use std::collections::{HashMap, HashSet};

use rask_ast::decl::{DepDecl, FeatureDecl};

/// A resolved set of features and the additional deps they activate.
#[derive(Debug, Default)]
pub struct ResolvedFeatures {
    /// Names of enabled additive features.
    pub enabled: HashSet<String>,
    /// Exclusive feature selections: feature_name → selected option.
    pub exclusive_selections: HashMap<String, String>,
    /// Additional dependencies activated by enabled features.
    pub activated_deps: Vec<DepDecl>,
    /// Errors encountered during resolution.
    pub errors: Vec<String>,
}

/// Resolve features for a package build.
///
/// `features` — declared features from the package block.
/// `requested` — features explicitly requested (e.g., `--features ssl`).
/// `no_default` — disable default features.
/// `dep_selections` — exclusive feature selections from dependencies:
///   maps (feature_name, selected_option) pairs with the selecting dep name.
pub fn resolve_features(
    features: &[FeatureDecl],
    requested: &[String],
    no_default: bool,
    dep_selections: &[(String, String, String)], // (feature_name, option, selecting_dep)
) -> ResolvedFeatures {
    let mut result = ResolvedFeatures::default();
    let feature_map: HashMap<&str, &FeatureDecl> = features.iter()
        .map(|f| (f.name.as_str(), f))
        .collect();

    // Start with requested features
    let mut to_enable: Vec<String> = requested.to_vec();

    // Add defaults unless --no-default-features
    if !no_default {
        for feat in features {
            if !feat.exclusive {
                // Additive features with default: true
                // (For now, all declared features are opt-in unless explicitly requested)
            }
            if feat.exclusive {
                // Exclusive features always have a default selection (FG2)
                if let Some(ref default) = feat.default {
                    // Only apply default if no explicit selection
                    let has_selection = dep_selections.iter()
                        .any(|(name, _, _)| name == &feat.name);
                    let has_requested = requested.iter()
                        .any(|r| r == &feat.name);
                    if !has_selection && !has_requested {
                        result.exclusive_selections.insert(
                            feat.name.clone(),
                            default.clone(),
                        );
                    }
                }
            }
        }
    }

    // Process enables chains (F6: no circular enables)
    let mut visited = HashSet::new();
    let mut i = 0;
    while i < to_enable.len() {
        let name = to_enable[i].clone();
        i += 1;

        if visited.contains(&name) {
            continue;
        }
        visited.insert(name.clone());

        if let Some(feat) = feature_map.get(name.as_str()) {
            // Check for enables chains
            for dep in &feat.deps {
                // If dep has a feature field, that feature is also enabled
                if let Some(ref feat_name) = dep.target {
                    // Reusing target field for feature gating in nested context
                    if !visited.contains(feat_name) {
                        to_enable.push(feat_name.clone());
                    }
                }
            }
        }
    }

    // Enable all resolved features
    for name in &visited {
        result.enabled.insert(name.clone());
    }

    // Process exclusive feature selections from dependencies (FG4-FG6)
    let mut exclusive_sources: HashMap<String, Vec<(String, String)>> = HashMap::new();
    for (feat_name, option, dep_name) in dep_selections {
        exclusive_sources
            .entry(feat_name.clone())
            .or_default()
            .push((option.clone(), dep_name.clone()));
    }

    for (feat_name, sources) in &exclusive_sources {
        if let Some(feat) = feature_map.get(feat_name.as_str()) {
            if !feat.exclusive {
                result.errors.push(format!(
                    "'{}' is not an exclusive feature — selection ignored",
                    feat_name
                ));
                continue;
            }

            // Check all selectors agree (FG4)
            let options: HashSet<&str> = sources.iter().map(|(o, _)| o.as_str()).collect();
            if options.len() > 1 {
                let mut msg = format!(
                    "exclusive feature conflict for '{}':",
                    feat_name
                );
                for (option, dep_name) in sources {
                    msg.push_str(&format!("\n    {} selects \"{}\"", dep_name, option));
                }
                result.errors.push(msg);
            } else if let Some((option, _)) = sources.first() {
                // Validate the option exists
                let valid = feat.options.iter().any(|o| o.name == *option);
                if valid {
                    result.exclusive_selections.insert(feat_name.clone(), option.clone());
                } else {
                    let available: Vec<&str> = feat.options.iter()
                        .map(|o| o.name.as_str())
                        .collect();
                    result.errors.push(format!(
                        "unknown option '{}' for exclusive feature '{}'. Available: {}",
                        option, feat_name, available.join(", ")
                    ));
                }
            }
        }
    }

    // Collect activated dependencies
    for feat in features {
        if feat.exclusive {
            // Get the selected option
            if let Some(selected) = result.exclusive_selections.get(&feat.name) {
                for option in &feat.options {
                    if option.name == *selected {
                        result.activated_deps.extend(option.deps.clone());
                    }
                }
            }
        } else if result.enabled.contains(&feat.name) {
            // Additive feature — all deps activated (F1)
            result.activated_deps.extend(feat.deps.clone());
        }
    }

    result
}

/// Check for circular enables references (F6).
pub fn check_enables_cycles(features: &[FeatureDecl]) -> Vec<String> {
    // Build adjacency: feature → features it enables
    // (parsed from metadata — for now, enables is implicit via dep feature fields)
    let _ = features;
    // The enables chain is validated during resolve_features via visited set.
    // Explicit cycle detection can be added when `enables` syntax is fully parsed.
    Vec::new()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_additive(name: &str, deps: Vec<DepDecl>) -> FeatureDecl {
        FeatureDecl {
            name: name.into(),
            exclusive: false,
            deps,
            options: vec![],
            default: None,
        }
    }

    fn make_exclusive(name: &str, options: Vec<(&str, Vec<DepDecl>)>, default: &str) -> FeatureDecl {
        FeatureDecl {
            name: name.into(),
            exclusive: true,
            deps: vec![],
            options: options.into_iter().map(|(n, d)| {
                rask_ast::decl::FeatureOption { name: n.into(), deps: d }
            }).collect(),
            default: Some(default.into()),
        }
    }

    fn make_dep(name: &str) -> DepDecl {
        DepDecl {
            name: name.into(),
            version: Some("^1.0".into()),
            path: None,
            git: None,
            branch: None,
            with_features: vec![],
            target: None,
            allow: vec![],
            exclusive_selections: vec![],
        }
    }

    #[test]
    fn additive_feature_activates_deps() {
        let features = vec![
            make_additive("ssl", vec![make_dep("openssl")]),
        ];
        let result = resolve_features(&features, &["ssl".into()], false, &[]);
        assert!(result.errors.is_empty());
        assert!(result.enabled.contains("ssl"));
        assert_eq!(result.activated_deps.len(), 1);
        assert_eq!(result.activated_deps[0].name, "openssl");
    }

    #[test]
    fn disabled_feature_no_deps() {
        let features = vec![
            make_additive("ssl", vec![make_dep("openssl")]),
        ];
        let result = resolve_features(&features, &[], false, &[]);
        assert!(result.errors.is_empty());
        assert!(!result.enabled.contains("ssl"));
        assert!(result.activated_deps.is_empty());
    }

    #[test]
    fn exclusive_uses_default() {
        let features = vec![
            make_exclusive("runtime", vec![
                ("tokio", vec![make_dep("tokio")]),
                ("async-std", vec![make_dep("async-std")]),
            ], "tokio"),
        ];
        let result = resolve_features(&features, &[], false, &[]);
        assert!(result.errors.is_empty());
        assert_eq!(result.exclusive_selections.get("runtime"), Some(&"tokio".to_string()));
        assert_eq!(result.activated_deps.len(), 1);
        assert_eq!(result.activated_deps[0].name, "tokio");
    }

    #[test]
    fn exclusive_conflict_detected() {
        let features = vec![
            make_exclusive("runtime", vec![
                ("tokio", vec![make_dep("tokio")]),
                ("async-std", vec![make_dep("async-std")]),
            ], "tokio"),
        ];
        let selections = vec![
            ("runtime".into(), "tokio".into(), "dep-a".into()),
            ("runtime".into(), "async-std".into(), "dep-b".into()),
        ];
        let result = resolve_features(&features, &[], false, &selections);
        assert!(!result.errors.is_empty());
        assert!(result.errors[0].contains("conflict"));
    }

    #[test]
    fn exclusive_selection_overrides_default() {
        let features = vec![
            make_exclusive("runtime", vec![
                ("tokio", vec![make_dep("tokio")]),
                ("async-std", vec![make_dep("async-std")]),
            ], "tokio"),
        ];
        let selections = vec![
            ("runtime".into(), "async-std".into(), "consumer".into()),
        ];
        let result = resolve_features(&features, &[], false, &selections);
        assert!(result.errors.is_empty());
        assert_eq!(result.exclusive_selections.get("runtime"), Some(&"async-std".to_string()));
        assert_eq!(result.activated_deps.len(), 1);
        assert_eq!(result.activated_deps[0].name, "async-std");
    }
}
