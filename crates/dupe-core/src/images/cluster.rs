//! Grouping images by perceptual distance.
//!
//! Comparing every image with every other is O(n²) — 20 000 photos is already
//! 200 million comparisons. A BK-tree exploits the fact that Hamming distance
//! obeys the triangle inequality: when a query is within `t` of a node, any
//! match must sit in the child edges `d - t ..= d + t`, so the rest of the
//! tree is skipped without being looked at.
//!
//! Matches are then fed to a union-find, which turns pairwise similarity into
//! clusters: if A matches B and B matches C, all three land in one group even
//! when A and C are further apart than the threshold. That is the behaviour
//! you want for a burst of shots or a chain of re-saves.

use std::collections::HashMap;

/// Metric tree over 64-bit hashes under Hamming distance.
///
/// Pruning weakens as the threshold grows (past roughly 16 of 64 bits it
/// degenerates towards a full scan), which is why the UI caps the slider well
/// below that.
#[derive(Default)]
pub struct BkTree {
    nodes: Vec<Node>,
}

struct Node {
    key: u64,
    /// Every item sharing this exact hash. Identical photos are common, and
    /// a zero-distance edge cannot be represented in the tree itself.
    items: Vec<usize>,
    children: HashMap<u32, usize>,
}

impl BkTree {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }

    pub fn insert(&mut self, key: u64, item: usize) {
        if self.nodes.is_empty() {
            self.nodes.push(Node {
                key,
                items: vec![item],
                children: HashMap::new(),
            });
            return;
        }

        let mut current = 0usize;
        loop {
            let distance = (self.nodes[current].key ^ key).count_ones();
            if distance == 0 {
                self.nodes[current].items.push(item);
                return;
            }

            match self.nodes[current].children.get(&distance) {
                Some(&next) => current = next,
                None => {
                    let new = self.nodes.len();
                    self.nodes.push(Node {
                        key,
                        items: vec![item],
                        children: HashMap::new(),
                    });
                    self.nodes[current].children.insert(distance, new);
                    return;
                }
            }
        }
    }

    /// Append every item within `max_distance` of `key` to `out`.
    pub fn find(&self, key: u64, max_distance: u32, out: &mut Vec<usize>) {
        if self.nodes.is_empty() {
            return;
        }

        let mut stack = vec![0usize];
        while let Some(index) = stack.pop() {
            let node = &self.nodes[index];
            let distance = (node.key ^ key).count_ones();

            if distance <= max_distance {
                out.extend_from_slice(&node.items);
            }

            let low = distance.saturating_sub(max_distance);
            let high = distance.saturating_add(max_distance);
            for (&edge, &child) in &node.children {
                if edge >= low && edge <= high {
                    stack.push(child);
                }
            }
        }
    }
}

/// Union-find with path halving and union by rank.
pub struct DisjointSet {
    parent: Vec<usize>,
    rank: Vec<u8>,
}

impl DisjointSet {
    pub fn new(len: usize) -> Self {
        Self {
            parent: (0..len).collect(),
            rank: vec![0; len],
        }
    }

    pub fn find(&mut self, mut x: usize) -> usize {
        while self.parent[x] != x {
            self.parent[x] = self.parent[self.parent[x]];
            x = self.parent[x];
        }
        x
    }

    pub fn union(&mut self, a: usize, b: usize) {
        let (mut a, mut b) = (self.find(a), self.find(b));
        if a == b {
            return;
        }
        if self.rank[a] < self.rank[b] {
            std::mem::swap(&mut a, &mut b);
        }
        self.parent[b] = a;
        if self.rank[a] == self.rank[b] {
            self.rank[a] += 1;
        }
    }

    /// All members of every set with more than one element, keyed by root.
    pub fn groups(&mut self) -> Vec<Vec<usize>> {
        let mut by_root: HashMap<usize, Vec<usize>> = HashMap::new();
        for i in 0..self.parent.len() {
            let root = self.find(i);
            by_root.entry(root).or_default().push(i);
        }

        let mut groups: Vec<Vec<usize>> = by_root
            .into_values()
            .filter(|members| members.len() > 1)
            .collect();
        for members in &mut groups {
            members.sort_unstable();
        }
        groups.sort_unstable();
        groups
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_everything_within_the_threshold() {
        let mut tree = BkTree::new();
        let base = 0b1010_1010_1010_1010u64;
        tree.insert(base, 0);
        tree.insert(base ^ 0b1, 1); // distance 1
        tree.insert(base ^ 0b111, 2); // distance 3
        tree.insert(!base, 3); // distance 64

        let mut out = Vec::new();
        tree.find(base, 3, &mut out);
        out.sort_unstable();
        assert_eq!(out, vec![0, 1, 2]);
    }

    #[test]
    fn identical_hashes_all_come_back() {
        let mut tree = BkTree::new();
        tree.insert(42, 0);
        tree.insert(42, 1);
        tree.insert(42, 2);

        let mut out = Vec::new();
        tree.find(42, 0, &mut out);
        out.sort_unstable();
        assert_eq!(out, vec![0, 1, 2]);
    }

    #[test]
    fn an_empty_tree_yields_nothing() {
        let tree = BkTree::new();
        let mut out = Vec::new();
        tree.find(1, 10, &mut out);
        assert!(out.is_empty());
    }

    #[test]
    fn similarity_is_transitive_across_a_chain() {
        // A~B and B~C, but A and C are too far apart to match directly.
        let mut set = DisjointSet::new(4);
        set.union(0, 1);
        set.union(1, 2);

        let groups = set.groups();
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0], vec![0, 1, 2]);
    }
}
