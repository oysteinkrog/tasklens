use crate::{Bounds, Half};
use std::{
    fmt::Debug,
    ops::{Add, Sub},
};

/// Maximum children per internal node (R-tree style branching factor).
const MAX_CHILDREN: usize = 12;

/// `Node::child` value of an entry that is a leaf.
const LEAF: u32 = u32::MAX;

/// A spatial tree optimized for finding maximum ordering among intersecting bounds.
///
/// This is an R-tree variant specifically designed for the use case of assigning
/// z-order to overlapping UI elements. Key optimizations:
/// - Tracks the leaf with global max ordering for O(1) fast-path queries
/// - Uses a branching factor of 12 for lower tree height
/// - Keeps each child's bounds and max ordering inline in its parent (tasklens-q8zcy), so a
///   search or an insert reads one node per level, not one node per child
/// - Aggressive pruning during search based on max_order metadata
#[derive(Debug)]
pub(crate) struct BoundsTree<U>
where
    U: Clone + Debug + Default + PartialEq,
{
    /// Internal nodes. Node 0 is the root once the tree holds a leaf.
    nodes: Vec<Node<U>>,
    /// The bounds and ordering of the leaf with the highest ordering (fast path).
    max_leaf: Option<(Bounds<U>, u32)>,
    /// Reusable stack for search operations: node index and that subtree's max ordering.
    search_stack: Vec<(u32, u32)>,
}

/// An internal node. Each entry is a child: its bounds, the max ordering in it, and either
/// [`LEAF`] or the index of the child node. The entries sit inline, so a search or a descent
/// reads one node instead of one node per child.
#[derive(Debug, Clone)]
struct Node<U>
where
    U: Clone + Debug + Default + PartialEq,
{
    bounds: [Bounds<U>; MAX_CHILDREN],
    max_order: [u32; MAX_CHILDREN],
    child: [u32; MAX_CHILDREN],
    len: u8,
}

impl<U> Node<U>
where
    U: Clone + Debug + Default + PartialEq,
{
    fn new() -> Self {
        Self {
            bounds: std::array::from_fn(|_| Bounds::default()),
            max_order: [0; MAX_CHILDREN],
            child: [LEAF; MAX_CHILDREN],
            len: 0,
        }
    }

    fn push(&mut self, bounds: Bounds<U>, max_order: u32, child: u32) {
        let i = self.len as usize;
        debug_assert!(i < MAX_CHILDREN);
        self.bounds[i] = bounds;
        self.max_order[i] = max_order;
        self.child[i] = child;
        self.len += 1;
    }
}

impl<U> BoundsTree<U>
where
    U: Clone
        + Debug
        + PartialEq
        + PartialOrd
        + Add<U, Output = U>
        + Sub<Output = U>
        + Half
        + Default,
{
    /// Clears all nodes from the tree.
    pub fn clear(&mut self) {
        self.nodes.clear();
        self.max_leaf = None;
        self.search_stack.clear();
    }

    /// Inserts bounds into the tree and returns its assigned ordering.
    ///
    /// The ordering is one greater than the maximum ordering of any
    /// existing bounds that intersect with the new bounds.
    pub fn insert(&mut self, new_bounds: Bounds<U>) -> u32 {
        let ordering = self.find_max_ordering(&new_bounds) + 1;
        self.insert_leaf(&new_bounds, ordering);
        match &self.max_leaf {
            Some((_, max)) if *max >= ordering => {}
            _ => self.max_leaf = Some((new_bounds, ordering)),
        }
        ordering
    }

    /// Finds the maximum ordering among all bounds that intersect with the query.
    fn find_max_ordering(&mut self, query: &Bounds<U>) -> u32 {
        if self.nodes.is_empty() {
            return 0;
        }
        // Fast path: the leaf with the highest ordering intersects.
        if let Some((bounds, max)) = &self.max_leaf
            && query.intersects(bounds)
        {
            return *max;
        }

        self.search_stack.clear();
        self.search_stack.push((0, u32::MAX));
        let mut max_found = 0u32;
        while let Some((idx, subtree_max)) = self.search_stack.pop() {
            if subtree_max <= max_found {
                continue;
            }
            let node = &self.nodes[idx as usize];
            for i in 0..node.len as usize {
                let order = node.max_order[i];
                if order <= max_found || !query.intersects(&node.bounds[i]) {
                    continue;
                }
                match node.child[i] {
                    LEAF => max_found = order,
                    child => self.search_stack.push((child, order)),
                }
            }
        }
        max_found
    }

    /// Inserts a leaf with the given bounds and ordering.
    fn insert_leaf(&mut self, bounds: &Bounds<U>, order: u32) {
        if self.nodes.is_empty() {
            let mut root = Node::new();
            root.push(bounds.clone(), order, LEAF);
            self.nodes.push(root);
            return;
        }
        let mut current = 0usize;
        loop {
            let node = &self.nodes[current];
            let len = node.len as usize;
            // The child whose union with the new bounds has the smallest half perimeter.
            let mut best = 0;
            let mut best_cost = bounds.union(&node.bounds[0]).half_perimeter();
            for i in 1..len {
                let cost = bounds.union(&node.bounds[i]).half_perimeter();
                if cost < best_cost {
                    best_cost = cost;
                    best = i;
                }
            }
            let child = node.child[best];
            if child == LEAF && len < MAX_CHILDREN {
                self.nodes[current].push(bounds.clone(), order, LEAF);
                return;
            }
            if child == LEAF {
                // The node is full: the best leaf and the new leaf move into a new node.
                let mut pair = Node::new();
                pair.push(node.bounds[best].clone(), node.max_order[best], LEAF);
                pair.push(bounds.clone(), order, LEAF);
                let pair_idx = self.nodes.len() as u32;
                self.nodes.push(pair);
                let node = &mut self.nodes[current];
                node.bounds[best] = node.bounds[best].union(bounds);
                node.max_order[best] = node.max_order[best].max(order);
                node.child[best] = pair_idx;
                return;
            }
            let node = &mut self.nodes[current];
            node.bounds[best] = node.bounds[best].union(bounds);
            node.max_order[best] = node.max_order[best].max(order);
            current = child as usize;
        }
    }
}

impl<U> Default for BoundsTree<U>
where
    U: Clone + Debug + Default + PartialEq,
{
    fn default() -> Self {
        BoundsTree {
            nodes: Vec::new(),
            max_leaf: None,
            search_stack: Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Bounds, Point, Size};
    use rand::{Rng, SeedableRng};

    #[test]
    fn test_insert() {
        let mut tree = BoundsTree::<f32>::default();
        let bounds1 = Bounds {
            origin: Point { x: 0.0, y: 0.0 },
            size: Size {
                width: 10.0,
                height: 10.0,
            },
        };
        let bounds2 = Bounds {
            origin: Point { x: 5.0, y: 5.0 },
            size: Size {
                width: 10.0,
                height: 10.0,
            },
        };
        let bounds3 = Bounds {
            origin: Point { x: 10.0, y: 10.0 },
            size: Size {
                width: 10.0,
                height: 10.0,
            },
        };

        // Insert the bounds into the tree and verify the order is correct
        assert_eq!(tree.insert(bounds1), 1);
        assert_eq!(tree.insert(bounds2), 2);
        assert_eq!(tree.insert(bounds3), 3);

        // Insert non-overlapping bounds and verify they can reuse orders
        let bounds4 = Bounds {
            origin: Point { x: 20.0, y: 20.0 },
            size: Size {
                width: 10.0,
                height: 10.0,
            },
        };
        let bounds5 = Bounds {
            origin: Point { x: 40.0, y: 40.0 },
            size: Size {
                width: 10.0,
                height: 10.0,
            },
        };
        let bounds6 = Bounds {
            origin: Point { x: 25.0, y: 25.0 },
            size: Size {
                width: 10.0,
                height: 10.0,
            },
        };
        assert_eq!(tree.insert(bounds4), 1); // bounds4 does not overlap with bounds1, bounds2, or bounds3
        assert_eq!(tree.insert(bounds5), 1); // bounds5 does not overlap with any other bounds
        assert_eq!(tree.insert(bounds6), 2); // bounds6 overlaps with bounds4, so it should have a different order
    }

    #[test]
    fn test_random_iterations() {
        let max_bounds = 100;
        for seed in 1..=1000 {
            // let seed = 44;
            let mut tree = BoundsTree::default();
            let mut rng = rand::rngs::StdRng::seed_from_u64(seed as u64);
            let mut expected_quads: Vec<(Bounds<f32>, u32)> = Vec::new();

            // Insert a random number of random AABBs into the tree.
            let num_bounds = rng.random_range(1..=max_bounds);
            for _ in 0..num_bounds {
                let min_x: f32 = rng.random_range(-100.0..100.0);
                let min_y: f32 = rng.random_range(-100.0..100.0);
                let width: f32 = rng.random_range(0.0..50.0);
                let height: f32 = rng.random_range(0.0..50.0);
                let bounds = Bounds {
                    origin: Point { x: min_x, y: min_y },
                    size: Size { width, height },
                };

                let expected_ordering = expected_quads
                    .iter()
                    .filter_map(|quad| quad.0.intersects(&bounds).then_some(quad.1))
                    .max()
                    .unwrap_or(0)
                    + 1;
                expected_quads.push((bounds, expected_ordering));

                // Insert the AABB into the tree and collect intersections.
                let actual_ordering = tree.insert(bounds);
                assert_eq!(actual_ordering, expected_ordering);
            }
        }
    }

    /// A table frame paints the window, then each row's fill, cells and text in turn, and the
    /// tree is cleared and reused every frame. Every ordering must match a brute-force scan.
    #[test]
    fn test_table_frames_match_brute_force() {
        let rect = |x: f32, y: f32, width: f32, height: f32| Bounds {
            origin: Point { x, y },
            size: Size { width, height },
        };
        let mut frame = vec![rect(0., 0., 1920., 1200.)];
        for i in 0..60 {
            frame.push(rect(i as f32 * 30., 0., 28., 30.));
        }
        for row in 0..40 {
            let y = 90. + row as f32 * 26.;
            frame.push(rect(0., y, 1600., 26.));
            for col in 0..10 {
                let x = col as f32 * 160.;
                frame.push(rect(x, y, 160., 26.));
                frame.push(rect(x + 8., y + 4., 140., 18.));
            }
        }
        for card in 0..6 {
            let (x, y) = (1600. + (card % 2) as f32 * 160., 300. + (card / 2) as f32 * 120.);
            frame.push(rect(x, y, 150., 110.));
            for line in 0..8 {
                frame.push(rect(x + 4., y + 4. + line as f32 * 12., 140., 12.));
            }
        }
        let mut tree = BoundsTree::<f32>::default();
        for scroll in 0..3 {
            tree.clear();
            let mut expected: Vec<(Bounds<f32>, u32)> = Vec::new();
            for bounds in &frame {
                let mut bounds = *bounds;
                bounds.origin.y += scroll as f32 * 7.;
                let ordering = expected
                    .iter()
                    .filter_map(|(other, order)| other.intersects(&bounds).then_some(*order))
                    .max()
                    .unwrap_or(0)
                    + 1;
                expected.push((bounds, ordering));
                assert_eq!(tree.insert(bounds), ordering);
            }
        }
    }
}
