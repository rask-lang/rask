// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! `rask claims`: what a reviewer reads about a package's claims
//! (type.claims, Tooling).
//!
//! - `rask claims <path>` lists every claim with its verdict.
//! - `--since <rev>` lists commits that changed a claim together with code
//!   that claim reaches: what it looks like when the grader is rewritten
//!   along with the work.
//! - `--mutate` changes the code claims reach in small ways and reruns
//!   them. A change no claim notices is code no claim constrains.

use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::process::{self, Command};

use colored::Colorize;
use rask_ast::token::TokenKind;

use super::claims::{self, SourceFn, Variant};
use crate::output;
use crate::Format;

pub fn cmd_claims(path: &str, format: Format) {
    let outcome = claims::check_package_claims(path, format, claims::ClaimsMode::Run);
    match outcome {
        claims::ClaimsOutcome::NoClaims if format == Format::Human => println!("{path}: no claims"),
        claims::ClaimsOutcome::Failed => process::exit(1),
        _ => {}
    }
}

// ─── --since ────────────────────────────────────────────────

fn git(dir: &Path, args: &[&str]) -> Result<String, String> {
    let out = Command::new("git")
        .args(args)
        .current_dir(dir)
        .output()
        .map_err(|e| format!("running git: {e}"))?;
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr).trim().to_string());
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// The file at `rev`, or empty when it didn't exist there.
fn file_at(repo: &Path, rev: &str, rel: &str) -> String {
    git(repo, &["show", &format!("{rev}:{rel}")]).unwrap_or_default()
}

/// Each function's text, by type and name.
fn fn_texts(src: &str) -> HashMap<(Option<String>, String), String> {
    claims::source_fns(Path::new(""), src)
        .into_iter()
        .map(|f| {
            let text = src.get(f.start..f.end).unwrap_or_default().to_string();
            ((f.owner, f.name), text)
        })
        .collect()
}

/// Each claim's text, by name.
fn claim_texts(src: &str) -> HashMap<String, String> {
    let tokens = rask_lexer::Lexer::new_with_file_id(src, 0).tokenize().tokens;
    rask_parser::Parser::new_with_file_id(tokens, 0, 0)
        .parse()
        .decls
        .iter()
        .filter_map(|d| match &d.kind {
            rask_ast::decl::DeclKind::Fn(f) if f.attrs.iter().any(|a| a.is("claim")) => {
                Some((f.name.clone(), src.get(f.span.start..f.span.end).unwrap_or_default().to_string()))
            }
            _ => None,
        })
        .collect()
}

/// Keys whose text differs between `old` and `new`, added and removed included.
fn changed<K: Clone + Eq + std::hash::Hash>(old: &HashMap<K, String>, new: &HashMap<K, String>) -> Vec<K> {
    let mut keys: Vec<K> = old.keys().chain(new.keys()).cloned().collect();
    keys.retain(|k| old.get(k) != new.get(k));
    let mut seen = std::collections::HashSet::new();
    keys.retain(|k| seen.insert(k.clone()));
    keys
}

pub fn cmd_since(path: &str, rev: &str) {
    let Some((root, reached)) = claims::reach(path) else { process::exit(1) };
    let repo = match git(&root, &["rev-parse", "--show-toplevel"]) {
        Ok(r) => PathBuf::from(r.trim()),
        Err(e) => {
            eprintln!("{}: `--since` reads the history: {e}", output::error_label());
            process::exit(1);
        }
    };
    let rel = |p: &Path| -> Option<String> {
        let p = p.canonicalize().ok()?;
        Some(p.strip_prefix(&repo).ok()?.to_string_lossy().into_owned())
    };
    let claim_files: BTreeSet<String> = reached.iter().filter_map(|c| rel(&c.file)).collect();
    let package_files: BTreeSet<String> = rask_resolve::package::package_source_files(&root)
        .iter()
        .filter_map(|p| rel(p))
        .collect();
    let commits = match git(&repo, &["rev-list", "--reverse", &format!("{rev}..HEAD")]) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("{}: {e}", output::error_label());
            process::exit(1);
        }
    };

    let mut found = 0;
    for commit in commits.lines() {
        let files = git(&repo, &["diff-tree", "--no-commit-id", "--name-only", "-r", "--root", commit]).unwrap_or_default();
        let files: Vec<&str> = files.lines().collect();
        let parent = format!("{commit}^");
        let mut claims_changed: BTreeSet<(String, String)> = BTreeSet::new();
        let mut fns_changed: BTreeSet<(String, Option<String>, String)> = BTreeSet::new();
        for f in &files {
            if claim_files.contains(*f) {
                let (old, new) = (file_at(&repo, &parent, f), file_at(&repo, commit, f));
                for name in changed(&claim_texts(&old), &claim_texts(&new)) {
                    claims_changed.insert((f.to_string(), name));
                }
            }
            if package_files.contains(*f) {
                let (old, new) = (file_at(&repo, &parent, f), file_at(&repo, commit, f));
                for (owner, name) in changed(&fn_texts(&old), &fn_texts(&new)) {
                    fns_changed.insert((f.to_string(), owner, name));
                }
            }
        }
        let mut lines = Vec::new();
        for c in &reached {
            let Some(file) = rel(&c.file) else { continue };
            if !claims_changed.contains(&(file, c.declared.clone())) {
                continue;
            }
            let hits: Vec<String> = c
                .fns
                .iter()
                .filter(|sf| rel(&sf.file).is_some_and(|f| fns_changed.contains(&(f, sf.owner.clone(), sf.name.clone()))))
                .map(SourceFn::label)
                .collect();
            if !hits.is_empty() {
                lines.push(format!("claim `{}` and {}", c.name, hits.iter().map(|h| format!("`{h}`")).collect::<Vec<_>>().join(", ")));
            }
        }
        if lines.is_empty() {
            continue;
        }
        found += 1;
        let subject = git(&repo, &["log", "-1", "--format=%h %s", commit]).unwrap_or_default();
        println!("{}", subject.trim().yellow());
        for l in lines {
            println!("  changed {l}, which it reaches");
        }
    }
    if found == 0 {
        println!("No commit since {rev} changed a claim together with code it reaches.");
    } else {
        process::exit(1);
    }
}

// ─── --mutate ───────────────────────────────────────────────

/// One small change to a function a claim reaches.
struct Mutant {
    func: SourceFn,
    start: usize,
    end: usize,
    from: String,
    to: String,
    line: usize,
}

/// The changes `--mutate` tries inside a function: flip a comparison, swap
/// `+` and `-`, `&&` and `||`, negate a `bool` literal, bump an integer.
fn mutants_in(func: &SourceFn, src: &str) -> Vec<Mutant> {
    let tokens = rask_lexer::Lexer::new_with_file_id(src, 0).tokenize().tokens;
    let mut out = Vec::new();
    let mut prev: Option<&TokenKind> = None;
    for t in &tokens {
        if t.span.start < func.start || t.span.end > func.end {
            prev = Some(&t.kind);
            continue;
        }
        let value_before = matches!(
            prev,
            Some(TokenKind::Ident(_) | TokenKind::Int(..) | TokenKind::RParen | TokenKind::RBracket)
        );
        let to: Option<String> = match &t.kind {
            TokenKind::Lt => Some("<=".into()),
            TokenKind::LtEq => Some("<".into()),
            TokenKind::Gt => Some(">=".into()),
            TokenKind::GtEq => Some(">".into()),
            TokenKind::EqEq => Some("!=".into()),
            TokenKind::BangEq => Some("==".into()),
            TokenKind::Plus => Some("-".into()),
            TokenKind::Minus if value_before => Some("+".into()),
            TokenKind::AmpAmp => Some("||".into()),
            TokenKind::PipePipe => Some("&&".into()),
            TokenKind::Bool(b) => Some((!b).to_string()),
            TokenKind::Int(n, None) => Some((n + 1).to_string()),
            _ => None,
        };
        if let Some(to) = to {
            let from = src.get(t.span.start..t.span.end).unwrap_or_default().to_string();
            let line = src[..t.span.start].matches('\n').count() + 1;
            out.push(Mutant { func: func.clone(), start: t.span.start, end: t.span.end, from, to, line });
        }
        prev = Some(&t.kind);
    }
    out
}

pub fn cmd_mutate(path: &str) {
    let Some((root, reached)) = claims::reach(path) else { process::exit(1) };
    if reached.is_empty() {
        println!("{path}: no claims");
        return;
    }
    // Every claim has to hold before a change can show it up.
    let Some(base) = claims::verdicts(path, Variant { edit: None, quiet: true }) else {
        eprintln!("{}: the claims don't compile; `rask check --claims` says why", output::error_label());
        process::exit(1);
    };
    if let Some((name, _)) = base.iter().find(|(_, held)| !held) {
        eprintln!("{}: claim `{name}` is false; fix it before measuring what the claims constrain", output::error_label());
        process::exit(1);
    }

    let mut funcs: Vec<SourceFn> = reached.iter().flat_map(|c| c.fns.iter().cloned()).collect();
    funcs.sort();
    funcs.dedup();
    let mut texts: HashMap<PathBuf, String> = HashMap::new();
    let mut mutants = Vec::new();
    for f in &funcs {
        let src = texts.entry(f.file.clone()).or_insert_with(|| std::fs::read_to_string(&f.file).unwrap_or_default());
        mutants.extend(mutants_in(f, src));
    }
    println!("{} {} change{} to {} function{} the claims reach",
        "Mutating".green().bold(),
        mutants.len(),
        if mutants.len() == 1 { "" } else { "s" },
        funcs.len(),
        if funcs.len() == 1 { "" } else { "s" });

    // For each claim, the changes it reached that none of its checks noticed.
    let mut survived: HashMap<String, Vec<String>> = HashMap::new();
    let mut tried: HashMap<String, usize> = HashMap::new();
    let mut uncompiled = 0;
    for m in &mutants {
        let src = &texts[&m.func.file];
        let mut edited = src.clone();
        edited.replace_range(m.start..m.end, &m.to);
        let Some(after) = claims::verdicts(path, Variant { edit: Some((&m.func.file, &edited)), quiet: true }) else {
            uncompiled += 1;
            continue;
        };
        let at = format!(
            "{}:{} in `{}`: `{}` → `{}`",
            m.func.file.strip_prefix(&root).unwrap_or(&m.func.file).display(),
            m.line,
            m.func.label(),
            m.from,
            m.to
        );
        for c in reached.iter().filter(|c| c.fns.contains(&m.func)) {
            *tried.entry(c.name.clone()).or_default() += 1;
            if after.iter().any(|(n, held)| *n == c.name && *held) {
                survived.entry(c.name.clone()).or_default().push(at.clone());
            }
        }
    }

    let mut weak = 0;
    for c in &reached {
        let n = tried.get(&c.name).copied().unwrap_or(0);
        let s = survived.get(&c.name).map_or(&[][..], |v| &v[..]);
        if n == 0 {
            println!("  {:<24} reaches nothing that could be changed", c.name);
        } else if s.is_empty() {
            println!("  {:<24} noticed all {n} changes", c.name);
        } else {
            weak += 1;
            println!("  {:<24} held after {} of {n} changes:", c.name, s.len());
            for at in s {
                println!("      {at}");
            }
        }
    }
    if uncompiled > 0 {
        println!("  ({uncompiled} change{} didn't compile and weren't counted)", if uncompiled == 1 { "" } else { "s" });
    }
    if weak > 0 {
        process::exit(1);
    }
}
