// SPDX-License-Identifier: (MIT OR Apache-2.0)

//! One closure body per set of closures that reach it.
//!
//! `ClosureTargets` answers "which closure bodies can this call reach", and it
//! answers per *function*: a capture slot's flow is keyed by the name of the
//! body that holds it. So when two places build the same closure over
//! different things, the body sees the union, and one member that hands back
//! somebody else's container makes the whole set unusable:
//!
//! ```text
//! keys.flat_map(|k| SHARED)    // hands back a const — must not be freed
//! v.flat_map(|x| { … pair })   // builds one per element — leaks if it isn't
//! ```
//!
//! Adding the second to a program made the first leak eight allocations, in a
//! function that hadn't changed (#1146).
//!
//! Two places build the same closure because inlining put them there: every
//! call to `flat_map` gets the same `Vec_flat_map$i64_i64__closure_0`, since
//! inlining copies the *create* site into each caller and leaves one body
//! behind it. `container_drop`'s environment glue already has to work around
//! this from the other side — "every site that builds this closure has to
//! agree about what its environment owns" — and agreeing is exactly what these
//! two don't do.
//!
//! So: a copy of the body per group of sites that see the same thing. Then
//! each body's target set has one member and the by-name machinery answers
//! precisely.
//!
//! Only when the sites actually disagree. Copying every multiply-built closure
//! would cost code size on the programs where one body was already the right
//! answer, which is most of them — the pass is here to recover precision, not
//! to duplicate on principle.
//!
//! Copying the body alone isn't enough: a copy has to name its own copies of
//! every closure *it* creates, transitively, or the same union reappears one
//! level down.

use std::collections::{BTreeMap, HashMap, HashSet};

use crate::{LocalId, MirFunction, MirStmtKind};

/// What a copy's name carries, so a reader can tell one from a mono instance.
const FOR: &str = "__for";

/// What a create site can see about the closures it captures. Two sites with
/// the same answer can share a body.
type Fingerprint = BTreeMap<u32, Option<Vec<String>>>;

/// Where a closure gets built.
struct Site {
    func: usize,
    block: usize,
    stmt: usize,
}

/// Give each group of disagreeing create sites its own copy of the body.
pub fn specialize_adapters(fns: &mut Vec<MirFunction>) {
    let targets = crate::closure_targets::ClosureTargets::build(fns);

    let mut sites: HashMap<String, Vec<(Site, Fingerprint)>> = HashMap::new();
    for (fi, func) in fns.iter().enumerate() {
        for (bi, block) in func.blocks.iter().enumerate() {
            for (si, stmt) in block.statements.iter().enumerate() {
                let MirStmtKind::ClosureCreate { func_name, captures, .. } = &stmt.kind else {
                    continue;
                };
                let print = captures
                    .iter()
                    .map(|c| (c.offset, sorted_targets(&targets, &func.name, c.local_id)))
                    .collect();
                sites
                    .entry(func_name.clone())
                    .or_default()
                    .push((Site { func: fi, block: bi, stmt: si }, print));
            }
        }
    }

    // Per body: the groups that disagree, and which one keeps the original
    // name. Keeping one saves a copy and keeps the common case — every site
    // agreeing — a no-op.
    let mut renames: Vec<(Site, String)> = Vec::new();
    let mut copies: Vec<(String, String)> = Vec::new();
    for (body, at) in sites {
        let distinct: HashSet<&Fingerprint> = at.iter().map(|(_, p)| p).collect();
        if distinct.len() < 2 {
            continue;
        }
        let mut group_of: HashMap<&Fingerprint, usize> = HashMap::new();
        for (site, print) in &at {
            let next = group_of.len();
            let group = *group_of.entry(print).or_insert(next);
            if group == 0 {
                continue;
            }
            let suffix = format!("{}{}", FOR, group);
            renames.push((Site::clone_of(site), format!("{}{}", body, suffix)));
            copies.push((body.clone(), suffix));
        }
    }
    if copies.is_empty() {
        return;
    }

    let mut made: HashSet<String> = HashSet::new();
    let mut out: Vec<MirFunction> = Vec::new();
    for (body, suffix) in &copies {
        copy_with_closures(fns, body, suffix, &mut out, &mut made);
    }
    for (site, name) in renames {
        if let MirStmtKind::ClosureCreate { func_name, .. } =
            &mut fns[site.func].blocks[site.block].statements[site.stmt].kind
        {
            *func_name = name;
        }
    }
    fns.extend(out);
}

impl Site {
    fn clone_of(other: &Site) -> Self {
        Self { func: other.func, block: other.block, stmt: other.stmt }
    }
}

fn sorted_targets(
    targets: &crate::closure_targets::ClosureTargets,
    func: &str,
    local: LocalId,
) -> Option<Vec<String>> {
    let mut names: Vec<String> = targets.known(func, local)?.iter().cloned().collect();
    names.sort();
    Some(names)
}

/// Copy `name` under `name + suffix`, and every closure body it creates under
/// the same suffix, recursively.
///
/// The recursion is what makes the copy independent. An adapter's closure holds
/// the next one in a capture slot, and those are keyed by the created body's
/// name — so a copy that kept pointing at the original's inner body would put
/// both back into one set, and nothing would have changed.
fn copy_with_closures(
    fns: &[MirFunction],
    name: &str,
    suffix: &str,
    out: &mut Vec<MirFunction>,
    made: &mut HashSet<String>,
) {
    let copied = format!("{}{}", name, suffix);
    if !made.insert(copied.clone()) {
        return;
    }
    let Some(original) = fns.iter().find(|f| f.name == name) else { return };
    let mut clone = original.clone();
    clone.name = copied;
    // An `extern "C"` export has one name by definition; a copy would be a
    // second symbol claiming it.
    clone.is_extern_c = false;

    let mut creates: Vec<String> = Vec::new();
    for block in clone.blocks.iter_mut() {
        for stmt in block.statements.iter_mut() {
            if let MirStmtKind::ClosureCreate { func_name, .. } = &mut stmt.kind {
                creates.push(func_name.clone());
                *func_name = format!("{}{}", func_name, suffix);
            }
        }
    }
    out.push(clone);
    for created in creates {
        copy_with_closures(fns, &created, suffix, out, made);
    }
}
