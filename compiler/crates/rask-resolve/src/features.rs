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
            if !feat.exclusive && feat.on_by_default && !to_enable.contains(&feat.name) {
                to_enable.push(feat.name.clone());
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

        // `enables` follows chains (F6). It used to read each dep's `target`
        // as a feature name, so a dep filtered to "linux" turned on a feature
        // called "linux" and `enables` itself did nothing.
        if let Some(feat) = feature_map.get(name.as_str()) {
            for next in &feat.enables {
                if !visited.contains(next) {
                    to_enable.push(next.clone());
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
            // Additive feature — its deps are activated (F1), except one also
            // gated on a feature that isn't on (F5)
            result.activated_deps.extend(
                feat.deps
                    .iter()
                    .filter(|d| d.feature.as_ref().map_or(true, |f| result.enabled.contains(f)))
                    .cloned(),
            );
        }
    }

    result
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
            on_by_default: false,
            enables: vec![],
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
            on_by_default: false,
            enables: vec![],
        }
    }

    fn make_dep(name: &str) -> DepDecl {
        DepDecl {
            name: name.into(),
            version: Some("^1.0".into()),
            path: None,
            git: None,
            branch: None,
            rev: None,
            with_features: vec![],
            target: None,
            allow: vec![],
            exclusive_selections: vec![],
            scope: None,
            feature: None,
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

    /// F3: `default: true` is on unless the build turns defaults off.
    #[test]
    fn default_feature_is_on_unless_turned_off() {
        let mut flag = make_additive("logging", vec![]);
        flag.on_by_default = true;
        let features = vec![flag];
        assert!(resolve_features(&features, &[], false, &[]).enabled.contains("logging"));
        assert!(!resolve_features(&features, &[], true, &[]).enabled.contains("logging"));
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

    /// F6: `enables` turns the named features on, through chains.
    #[test]
    fn enables_follows_chains() {
        let mut full = make_additive("full", vec![]);
        full.enables = vec!["ssl".into()];
        let mut ssl = make_additive("ssl", vec![make_dep("openssl")]);
        ssl.enables = vec!["crypto".into()];
        let crypto = make_additive("crypto", vec![make_dep("ring")]);
        let result = resolve_features(&[full, ssl, crypto], &["full".into()], false, &[]);
        assert!(result.enabled.contains("ssl") && result.enabled.contains("crypto"));
        assert_eq!(result.activated_deps.len(), 2);
    }

    /// A dep's platform filter is not a feature name.
    #[test]
    fn target_is_not_read_as_a_feature() {
        let mut dep = make_dep("epoll");
        dep.target = Some("linux".into());
        let net = make_additive("net", vec![dep]);
        let linux = make_additive("linux", vec![make_dep("other")]);
        let result = resolve_features(&[net, linux], &["net".into()], false, &[]);
        assert!(!result.enabled.contains("linux"));
    }

    /// F5: a dep gated on a second feature waits for that one too.
    #[test]
    fn feature_gate_on_a_dep() {
        let mut ring = make_dep("ring");
        ring.feature = Some("crypto".into());
        let ssl = make_additive("ssl", vec![make_dep("openssl"), ring]);
        let crypto = make_additive("crypto", vec![]);
        let only_ssl = resolve_features(&[ssl.clone(), crypto.clone()], &["ssl".into()], false, &[]);
        assert_eq!(only_ssl.activated_deps.len(), 1);
        let both = resolve_features(&[ssl, crypto], &["ssl".into(), "crypto".into()], false, &[]);
        assert_eq!(both.activated_deps.len(), 2);
    }
}
