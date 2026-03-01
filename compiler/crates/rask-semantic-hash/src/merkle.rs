// SPDX-License-Identifier: (MIT OR Apache-2.0)

//! Merkle tree for function dependency hashing (MK1-MK4).
//!
//! Each function gets a node whose hash incorporates:
//! - Its own semantic hash (from the hasher)
//! - The combined hashes of all functions it calls (transitive)
//!
//! MK3: Any change propagates upward — if a callee changes, all callers
//! get new hashes and their caches are invalidated.

use std::collections::{HashMap, HashSet};

use crate::SemanticHash;

/// A function node in the Merkle tree.
#[derive(Debug, Clone)]
pub struct FunctionNode {
    /// The function's own semantic hash (body only, no dependencies).
    pub self_hash: SemanticHash,
    /// Direct callees (function names).
    pub callees: Vec<String>,
    /// MK1: Combined hash including transitive dependencies.
    /// None until computed.
    pub combined_hash: Option<SemanticHash>,
}

/// The Merkle tree tracking function-level dependency hashes.
pub struct MerkleTree {
    nodes: HashMap<String, FunctionNode>,
}

const FNV_OFFSET: u64 = 0xcbf29ce484222325;
const FNV_PRIME: u64 = 0x100000001b3;

/// Combine two hashes deterministically.
fn combine_hashes(a: u64, b: u64) -> u64 {
    let mut h = a;
    for byte in b.to_le_bytes() {
        h ^= byte as u64;
        h = h.wrapping_mul(FNV_PRIME);
    }
    h
}

impl MerkleTree {
    pub fn new() -> Self {
        Self {
            nodes: HashMap::new(),
        }
    }

    /// Register a function with its semantic hash and direct callees.
    pub fn insert(&mut self, name: String, self_hash: SemanticHash, callees: Vec<String>) {
        self.nodes.insert(name, FunctionNode {
            self_hash,
            callees,
            combined_hash: None,
        });
    }

    /// MK2-MK4: Compute combined hashes for all functions.
    ///
    /// Handles cycles (mutual recursion) by using only self_hash for
    /// back-edge dependencies (nodes already on the recursion stack).
    pub fn compute_all(&mut self) {
        let names: Vec<String> = self.nodes.keys().cloned().collect();
        let mut computed: HashMap<String, u64> = HashMap::new();
        let mut stack: HashSet<String> = HashSet::new();

        for name in &names {
            if !computed.contains_key(name) {
                self.compute_recursive(name, &mut computed, &mut stack);
            }
        }

        // Write back combined hashes
        for (name, hash) in &computed {
            if let Some(node) = self.nodes.get_mut(name) {
                node.combined_hash = Some(SemanticHash(*hash));
            }
        }
    }

    fn compute_recursive(
        &self,
        name: &str,
        computed: &mut HashMap<String, u64>,
        stack: &mut HashSet<String>,
    ) -> u64 {
        if let Some(&h) = computed.get(name) {
            return h;
        }

        // Cycle detection: if on recursion stack, use self_hash only
        if stack.contains(name) {
            return self.nodes.get(name)
                .map_or(FNV_OFFSET, |n| n.self_hash.as_u64());
        }

        let node = match self.nodes.get(name) {
            Some(n) => n.clone(),
            None => {
                // External/unknown function — use a sentinel hash
                let sentinel = {
                    let mut h = FNV_OFFSET;
                    for b in name.as_bytes() {
                        h ^= *b as u64;
                        h = h.wrapping_mul(FNV_PRIME);
                    }
                    h
                };
                computed.insert(name.to_string(), sentinel);
                return sentinel;
            }
        };

        stack.insert(name.to_string());

        // Start with self_hash, then fold in callee hashes
        let mut combined = node.self_hash.as_u64();

        // MK3: Sort callees for deterministic ordering
        let mut sorted_callees = node.callees.clone();
        sorted_callees.sort();
        sorted_callees.dedup();

        for callee in &sorted_callees {
            let callee_hash = self.compute_recursive(callee, computed, stack);
            combined = combine_hashes(combined, callee_hash);
        }

        stack.remove(name);
        computed.insert(name.to_string(), combined);
        combined
    }

    /// Get the combined hash for a function (after compute_all).
    pub fn get(&self, name: &str) -> Option<SemanticHash> {
        self.nodes.get(name).and_then(|n| n.combined_hash)
    }

    /// Get the self-hash for a function.
    pub fn get_self_hash(&self, name: &str) -> Option<SemanticHash> {
        self.nodes.get(name).map(|n| n.self_hash)
    }

    /// Check whether a function's combined hash changed.
    pub fn changed(&self, name: &str, old_hash: SemanticHash) -> bool {
        self.get(name).map_or(true, |h| h != old_hash)
    }

    /// All function names in the tree.
    pub fn functions(&self) -> Vec<&str> {
        self.nodes.keys().map(|s| s.as_str()).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hash(v: u64) -> SemanticHash {
        SemanticHash(v)
    }

    #[test]
    fn single_function_combined_equals_self() {
        let mut tree = MerkleTree::new();
        tree.insert("main".into(), hash(100), vec![]);
        tree.compute_all();
        assert_eq!(tree.get("main"), Some(hash(100)));
    }

    #[test]
    fn callee_change_propagates() {
        // main → helper
        let mut tree1 = MerkleTree::new();
        tree1.insert("main".into(), hash(100), vec!["helper".into()]);
        tree1.insert("helper".into(), hash(200), vec![]);
        tree1.compute_all();

        // Same main, different helper body
        let mut tree2 = MerkleTree::new();
        tree2.insert("main".into(), hash(100), vec!["helper".into()]);
        tree2.insert("helper".into(), hash(300), vec![]);
        tree2.compute_all();

        // MK3: main's combined hash changes because helper changed
        assert_ne!(tree1.get("main"), tree2.get("main"));
        // helper's own hash is different
        assert_ne!(tree1.get("helper"), tree2.get("helper"));
    }

    #[test]
    fn unchanged_callee_same_hash() {
        let mut tree1 = MerkleTree::new();
        tree1.insert("main".into(), hash(100), vec!["helper".into()]);
        tree1.insert("helper".into(), hash(200), vec![]);
        tree1.compute_all();

        let mut tree2 = MerkleTree::new();
        tree2.insert("main".into(), hash(100), vec!["helper".into()]);
        tree2.insert("helper".into(), hash(200), vec![]);
        tree2.compute_all();

        assert_eq!(tree1.get("main"), tree2.get("main"));
    }

    #[test]
    fn transitive_propagation() {
        // a → b → c
        let mut tree1 = MerkleTree::new();
        tree1.insert("a".into(), hash(1), vec!["b".into()]);
        tree1.insert("b".into(), hash(2), vec!["c".into()]);
        tree1.insert("c".into(), hash(3), vec![]);
        tree1.compute_all();

        // Change c
        let mut tree2 = MerkleTree::new();
        tree2.insert("a".into(), hash(1), vec!["b".into()]);
        tree2.insert("b".into(), hash(2), vec!["c".into()]);
        tree2.insert("c".into(), hash(999), vec![]);
        tree2.compute_all();

        // Both a and b should have different combined hashes
        assert_ne!(tree1.get("a"), tree2.get("a"));
        assert_ne!(tree1.get("b"), tree2.get("b"));
        assert_ne!(tree1.get("c"), tree2.get("c"));
    }

    #[test]
    fn mutual_recursion_terminates() {
        // a → b → a (cycle)
        let mut tree = MerkleTree::new();
        tree.insert("a".into(), hash(1), vec!["b".into()]);
        tree.insert("b".into(), hash(2), vec!["a".into()]);
        tree.compute_all();

        // Should terminate and produce valid hashes
        assert!(tree.get("a").is_some());
        assert!(tree.get("b").is_some());
    }

    #[test]
    fn diamond_dependency() {
        // a → b, a → c, b → d, c → d
        let mut tree1 = MerkleTree::new();
        tree1.insert("a".into(), hash(1), vec!["b".into(), "c".into()]);
        tree1.insert("b".into(), hash(2), vec!["d".into()]);
        tree1.insert("c".into(), hash(3), vec!["d".into()]);
        tree1.insert("d".into(), hash(4), vec![]);
        tree1.compute_all();

        // Change d
        let mut tree2 = MerkleTree::new();
        tree2.insert("a".into(), hash(1), vec!["b".into(), "c".into()]);
        tree2.insert("b".into(), hash(2), vec!["d".into()]);
        tree2.insert("c".into(), hash(3), vec!["d".into()]);
        tree2.insert("d".into(), hash(999), vec![]);
        tree2.compute_all();

        // All should be affected
        assert_ne!(tree1.get("a"), tree2.get("a"));
        assert_ne!(tree1.get("b"), tree2.get("b"));
        assert_ne!(tree1.get("c"), tree2.get("c"));
    }

    #[test]
    fn unknown_callee_gets_sentinel() {
        // main calls "external_fn" which isn't in the tree
        let mut tree = MerkleTree::new();
        tree.insert("main".into(), hash(100), vec!["external_fn".into()]);
        tree.compute_all();

        // Should still produce a hash (using sentinel for unknown callee)
        assert!(tree.get("main").is_some());
    }

    #[test]
    fn changed_detects_difference() {
        let mut tree = MerkleTree::new();
        tree.insert("f".into(), hash(100), vec![]);
        tree.compute_all();

        assert!(!tree.changed("f", hash(100)));
        assert!(tree.changed("f", hash(999)));
        assert!(tree.changed("nonexistent", hash(100)));
    }

    #[test]
    fn deterministic_ordering() {
        // Insert callees in different orders, should produce same hash
        let mut tree1 = MerkleTree::new();
        tree1.insert("main".into(), hash(1), vec!["b".into(), "a".into()]);
        tree1.insert("a".into(), hash(10), vec![]);
        tree1.insert("b".into(), hash(20), vec![]);
        tree1.compute_all();

        let mut tree2 = MerkleTree::new();
        tree2.insert("main".into(), hash(1), vec!["a".into(), "b".into()]);
        tree2.insert("a".into(), hash(10), vec![]);
        tree2.insert("b".into(), hash(20), vec![]);
        tree2.compute_all();

        assert_eq!(tree1.get("main"), tree2.get("main"));
    }
}
