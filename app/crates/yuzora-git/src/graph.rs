//! The commit graph, as IntelliJ's `platform/vcs-log/graph` (B3):
//!
//! - **permanent graph**: rows are commits in date-topological order; a
//!   commit whose only parent is the next row stores no edge (a bit in
//!   `simple`), every other parent list lives in one `u32` array (CSR);
//!   a parent outside the loaded rows is [`NOT_LOADED`];
//! - **layout index** ([`Graph::build`], `GraphLayoutBuilder`): heads sorted
//!   with the important branches first (HEAD's, then main/master, local,
//!   remote, tags), each walked depth-first along its first unvisited
//!   parent; a walk that ends opens the next index. Lower indexes go left
//!   and colors follow the index, so they stay put;
//! - **print elements** ([`Graph::row`], `PrintElementGeneratorImpl`): made
//!   per row on demand and cached. Elements in a row are its node and the
//!   edges passing through it, ordered by IntelliJ's
//!   `GraphElementComparatorByLayoutIndex`. An edge 30 rows or longer only
//!   shows one row from each end, with an arrow, so long-lived branches
//!   don't widen the whole graph; that keeps every row local (no sweep).

use std::cmp::Ordering;
use std::collections::HashMap;

/// A parent that isn't in the loaded rows (yet).
pub const NOT_LOADED: u32 = u32::MAX;
/// IntelliJ's `LONG_EDGE_SIZE`.
pub const LONG_EDGE: u32 = 30;

/// One commit as read from git, before it has a row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RawCommit {
    pub hash: Vec<u8>,
    pub parents: Vec<Vec<u8>>,
    pub time: i64,
}

/// What a ref is, for sorting heads and drawing labels.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum RefKind {
    /// The branch HEAD is on (or HEAD itself, detached).
    Current = 0,
    /// main, master, trunk, develop.
    Main = 1,
    Local = 2,
    Remote = 3,
    Tag = 4,
}

#[derive(Default)]
pub struct Graph {
    /// Raw hash bytes per row, `hash_len` each.
    hashes: Vec<u8>,
    hash_len: usize,
    index: HashMap<Vec<u8>, u32>,
    pub times: Vec<i64>,
    simple: Vec<u64>,
    offsets: Vec<u32>,
    parents: Vec<u32>,
    child_offsets: Vec<u32>,
    children: Vec<u32>,
    pub layout: Vec<u32>,
    /// The commit HEAD points at.
    pub head_row: Option<u32>,
}

/// One row's drawing.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RowPrint {
    /// The node's column.
    pub node: u32,
    pub color: u8,
    /// How many columns the row uses.
    pub width: u32,
    pub segments: Vec<Segment>,
    pub merge: bool,
}

/// Half a row of one edge: from the row's center at `at` to its top or bottom
/// edge at `to` (a column, halfway between two rows' columns).
#[derive(Clone, Debug, PartialEq)]
pub struct Segment {
    pub at: f32,
    pub to: f32,
    pub up: bool,
    pub color: u8,
    /// The edge goes on, hidden: an arrow at this end.
    pub arrow: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum El {
    Node(u32),
    /// up row, down row (NOT_LOADED for an unloaded parent).
    Edge(u32, u32),
}

impl Graph {
    /// Builds the graph. `commits` must be children-before-parents (git's
    /// `--date-order`, or [`topo_sort`]); `refs` gives each head's kind.
    pub fn build(commits: &[RawCommit], refs: &HashMap<Vec<u8>, RefKind>, head: Option<&[u8]>) -> Graph {
        let n = commits.len();
        let hash_len = commits.first().map_or(20, |c| c.hash.len());
        let mut g = Graph { hash_len, ..Default::default() };
        g.hashes.reserve(n * hash_len);
        g.index.reserve(n);
        for (i, c) in commits.iter().enumerate() {
            g.hashes.extend_from_slice(&c.hash);
            g.index.insert(c.hash.clone(), i as u32);
        }
        g.times = commits.iter().map(|c| c.time).collect();
        g.simple = vec![0; n.div_ceil(64)];
        g.offsets.reserve(n + 1);
        g.offsets.push(0);
        let mut child_count = vec![0u32; n];
        for (i, c) in commits.iter().enumerate() {
            let rows: Vec<u32> = c.parents.iter().map(|p| g.index.get(p).copied().unwrap_or(NOT_LOADED)).collect();
            if rows.len() == 1 && rows[0] == i as u32 + 1 {
                g.simple[i / 64] |= 1 << (i % 64);
            } else {
                g.parents.extend_from_slice(&rows);
            }
            for &p in &rows {
                if p != NOT_LOADED {
                    child_count[p as usize] += 1;
                }
            }
            g.offsets.push(g.parents.len() as u32);
        }
        g.child_offsets = Vec::with_capacity(n + 1);
        g.child_offsets.push(0);
        let mut acc = 0;
        for c in &child_count {
            acc += c;
            g.child_offsets.push(acc);
        }
        g.children = vec![0; acc as usize];
        let mut fill = g.child_offsets.clone();
        for r in 0..n as u32 {
            let parents: Vec<u32> = g.parents_of(r).collect();
            for p in parents {
                if p != NOT_LOADED {
                    g.children[fill[p as usize] as usize] = r;
                    fill[p as usize] += 1;
                }
            }
        }
        g.head_row = head.and_then(|h| g.index.get(h).copied());
        g.layout = g.layout_index(refs);
        g
    }

    pub fn len(&self) -> usize {
        self.times.len()
    }

    pub fn is_empty(&self) -> bool {
        self.times.is_empty()
    }

    pub fn hash(&self, row: u32) -> &[u8] {
        let s = row as usize * self.hash_len;
        &self.hashes[s..s + self.hash_len]
    }

    pub fn row_of(&self, hash: &[u8]) -> Option<u32> {
        self.index.get(hash).copied()
    }

    /// Every row's hash, in order (for joining a refresh onto this graph).
    pub fn hashes(&self) -> impl Iterator<Item = &[u8]> {
        self.hashes.chunks(self.hash_len.max(1))
    }

    fn is_simple(&self, r: u32) -> bool {
        self.simple[r as usize / 64] >> (r % 64) & 1 == 1
    }

    pub fn parents_of(&self, r: u32) -> impl Iterator<Item = u32> + '_ {
        let simple = self.is_simple(r).then_some(r + 1);
        let (s, e) = if simple.is_some() { (0, 0) } else { (self.offsets[r as usize] as usize, self.offsets[r as usize + 1] as usize) };
        simple.into_iter().chain(self.parents[s..e].iter().copied())
    }

    pub fn parent_count(&self, r: u32) -> usize {
        if self.is_simple(r) { 1 } else { (self.offsets[r as usize + 1] - self.offsets[r as usize]) as usize }
    }

    pub fn children_of(&self, r: u32) -> &[u32] {
        &self.children[self.child_offsets[r as usize] as usize..self.child_offsets[r as usize + 1] as usize]
    }

    /// IntelliJ's `GraphLayoutBuilder`.
    fn layout_index(&self, refs: &HashMap<Vec<u8>, RefKind>) -> Vec<u32> {
        let n = self.len();
        let mut heads: Vec<u32> = (0..n as u32).filter(|&r| self.children_of(r).is_empty()).collect();
        // Rows pointed to by refs are heads too (a branch whose tip has children).
        for (h, _) in refs.iter() {
            if let Some(&r) = self.index.get(h)
                && !self.children_of(r).is_empty()
            {
                heads.push(r);
            }
        }
        let kind = |r: u32| -> (u8, u32) {
            let k = if Some(r) == self.head_row { RefKind::Current } else { refs.get(self.hash(r)).copied().unwrap_or(RefKind::Tag) };
            let has_ref = Some(r) == self.head_row || refs.contains_key(self.hash(r));
            ((k as u8) + if has_ref { 0 } else { 10 }, r)
        };
        heads.sort_by_key(|&r| kind(r));
        heads.dedup();
        let mut layout = vec![0u32; n];
        let mut current = 1u32;
        let mut stack: Vec<u32> = Vec::new();
        for head in heads {
            if layout[head as usize] != 0 {
                continue;
            }
            stack.push(head);
            while let Some(&node) = stack.last() {
                let first = layout[node as usize] == 0;
                if first {
                    layout[node as usize] = current;
                }
                match self.parents_of(node).find(|&p| p != NOT_LOADED && layout[p as usize] == 0) {
                    Some(p) => stack.push(p),
                    None => {
                        if first {
                            current += 1;
                        }
                        stack.pop();
                    }
                }
            }
        }
        layout
    }

    fn li(&self, r: u32) -> i64 {
        if r == NOT_LOADED { i64::MAX / 4 } else { self.layout[r as usize] as i64 }
    }

    fn long(u: u32, d: u32) -> bool {
        d == NOT_LOADED || d - u >= LONG_EDGE
    }

    /// Whether edge (u, d) takes a column in row r (u < r < d).
    fn edge_in_row(u: u32, d: u32, r: u32) -> bool {
        u < r && (d == NOT_LOADED || r < d) && (!Self::long(u, d) || r == u + 1 || (d != NOT_LOADED && r + 1 == d))
    }

    /// The elements of row r, in column order.
    fn elements(&self, r: u32) -> Vec<El> {
        let mut els = vec![El::Node(r)];
        // Short edges start at most LONG_EDGE rows above; long ones show only
        // one row from either end, so they start at r-1 or end at r+1.
        let from = r.saturating_sub(LONG_EDGE);
        for u in from..r {
            for d in self.parents_of(u) {
                if Self::edge_in_row(u, d, r) {
                    els.push(El::Edge(u, d));
                }
            }
        }
        if (r as usize) + 1 < self.len() {
            for &u in self.children_of(r + 1) {
                if u + 1 < r && u < from && Self::edge_in_row(u, r + 1, r) {
                    els.push(El::Edge(u, r + 1));
                }
            }
        }
        // Insertion sort: rows hold a handful of elements, and IntelliJ's
        // comparator isn't guaranteed total (slice::sort may panic on that).
        for i in 1..els.len() {
            let mut j = i;
            while j > 0 && self.compare(els[j - 1], els[j]) == Ordering::Greater {
                els.swap(j - 1, j);
                j -= 1;
            }
        }
        els
    }

    /// IntelliJ's `GraphElementComparatorByLayoutIndex`.
    fn compare(&self, a: El, b: El) -> Ordering {
        match (a, b) {
            (El::Node(x), El::Node(y)) => x.cmp(&y),
            (El::Edge(..), El::Node(n)) => self.compare_edge_node(a, n),
            (El::Node(n), El::Edge(..)) => self.compare_edge_node(b, n).reverse(),
            (El::Edge(u1, d1), El::Edge(u2, d2)) => {
                if d1 == NOT_LOADED {
                    return self.compare_edge_node(b, u1).reverse();
                }
                if d2 == NOT_LOADED {
                    return self.compare_edge_node(a, u2);
                }
                if u1 == u2 {
                    if d1 < d2 { self.compare_edge_node(b, d1).reverse() } else { self.compare_edge_node(a, d2) }
                } else if u1 < u2 {
                    self.compare_edge_node(a, u2)
                } else {
                    self.compare_edge_node(b, u1).reverse()
                }
            }
        }
    }

    fn compare_edge_node(&self, e: El, node: u32) -> Ordering {
        let El::Edge(u, d) = e else { return Ordering::Equal };
        if d == NOT_LOADED {
            return self.li(u).cmp(&self.li(node)).then(Ordering::Less);
        }
        let m = self.li(u).max(self.li(d));
        let n = self.li(node);
        if m != n { m.cmp(&n) } else { (u as i64).cmp(&(node as i64)).then(Ordering::Less) }
    }

    /// The color of a layout index.
    pub fn color_of(&self, li: i64) -> u8 {
        if li >= i64::MAX / 4 {
            return 0;
        }
        // Index 1 (HEAD's branch) gets the first color; the rest spread out.
        (((li.max(1) - 1) as u64).wrapping_mul(7) % PALETTE_SIZE as u64) as u8
    }

    fn edge_color(&self, u: u32, d: u32) -> u8 {
        self.color_of(self.li(u).max(self.li(d)))
    }

    /// Column of edge (u, d) in row r, which must hold it (or be u or d).
    fn column(els: &[El], r: u32, u: u32, d: u32) -> Option<usize> {
        if r == u || r == d {
            return els.iter().position(|e| *e == El::Node(r));
        }
        els.iter().position(|e| *e == El::Edge(u, d))
    }

    /// One row's print elements.
    pub fn row(&self, r: u32) -> RowPrint {
        if r as usize >= self.len() {
            return RowPrint::default();
        }
        let here = self.elements(r);
        let above = (r > 0).then(|| self.elements(r - 1));
        let below = ((r as usize) + 1 < self.len()).then(|| self.elements(r + 1));
        let node = here.iter().position(|e| *e == El::Node(r)).unwrap_or(0) as u32;
        let mut segments = Vec::new();
        // Edges reaching this row from above: those passing through, and
        // those ending at this node.
        let mut ups: Vec<(u32, u32)> = here.iter().filter_map(|e| if let El::Edge(u, d) = *e { Some((u, d)) } else { None }).collect();
        for &c in self.children_of(r) {
            ups.push((c, r));
        }
        for (u, d) in ups {
            let at = Self::column(&here, r, u, d).unwrap_or(node as usize) as f32;
            let prev_row = r.saturating_sub(1);
            let shown_above = u == prev_row || Self::edge_in_row(u, d, prev_row);
            if let Some(above) = &above
                && shown_above
                && let Some(p) = Self::column(above, prev_row, u, d)
            {
                segments.push(Segment { at, to: (at + p as f32) / 2.0, up: true, color: self.edge_color(u, d), arrow: false });
            } else {
                // Hidden above (a long edge): an arrow pointing up.
                segments.push(Segment { at, to: at, up: true, color: self.edge_color(u, d), arrow: true });
            }
        }
        // Edges leaving downward: passing through, and this node's parents.
        let mut downs: Vec<(u32, u32)> = here.iter().filter_map(|e| if let El::Edge(u, d) = *e { Some((u, d)) } else { None }).collect();
        for p in self.parents_of(r) {
            downs.push((r, p));
        }
        for (u, d) in downs {
            let at = Self::column(&here, r, u, d).unwrap_or(node as usize) as f32;
            let next_row = r + 1;
            let shown_below = d == next_row || (d != NOT_LOADED && d > next_row && Self::edge_in_row(u, d, next_row)) || (d == NOT_LOADED && Self::edge_in_row(u, d, next_row));
            match &below {
                Some(below) if shown_below => {
                    let col = if d == next_row { below.iter().position(|e| *e == El::Node(next_row)) } else { below.iter().position(|e| *e == El::Edge(u, d)) };
                    match col {
                        Some(p) => segments.push(Segment { at, to: (at + p as f32) / 2.0, up: false, color: self.edge_color(u, d), arrow: false }),
                        None => segments.push(Segment { at, to: at, up: false, color: self.edge_color(u, d), arrow: true }),
                    }
                }
                _ => segments.push(Segment { at, to: at, up: false, color: self.edge_color(u, d), arrow: true }),
            }
        }
        RowPrint { node, color: self.color_of(self.li(r)), width: here.len() as u32, segments, merge: self.parent_count(r) > 1 }
    }

    /// Widest row among the first `sample` (IntelliJ samples for the width).
    pub fn width(&self, sample: usize) -> u32 {
        (0..self.len().min(sample) as u32).map(|r| self.elements(r).len() as u32).max().unwrap_or(1)
    }
}

/// How many colors [`Graph::color_of`] cycles through.
pub const PALETTE_SIZE: usize = 8;

/// Sorts commits children-before-parents, newest first among the ready
/// ones (IntelliJ's `VcsLogSorter.sortByDateTopoOrder`).
pub fn topo_sort(mut commits: Vec<RawCommit>) -> Vec<RawCommit> {
    let index: HashMap<Vec<u8>, usize> = commits.iter().enumerate().map(|(i, c)| (c.hash.clone(), i)).collect();
    let mut pending_children = vec![0u32; commits.len()];
    for c in &commits {
        for p in &c.parents {
            if let Some(&i) = index.get(p) {
                pending_children[i] += 1;
            }
        }
    }
    let mut ready: std::collections::BinaryHeap<(i64, std::cmp::Reverse<usize>)> = commits.iter().enumerate().filter(|(i, _)| pending_children[*i] == 0).map(|(i, c)| (c.time, std::cmp::Reverse(i))).collect();
    let mut order = Vec::with_capacity(commits.len());
    while let Some((_, std::cmp::Reverse(i))) = ready.pop() {
        order.push(i);
        for p in &commits[i].parents {
            if let Some(&j) = index.get(p) {
                pending_children[j] -= 1;
                if pending_children[j] == 0 {
                    ready.push((commits[j].time, std::cmp::Reverse(j)));
                }
            }
        }
    }
    // A cycle can't happen in git; keep anything left anyway.
    if order.len() < commits.len() {
        let seen: std::collections::HashSet<usize> = order.iter().copied().collect();
        order.extend((0..commits.len()).filter(|i| !seen.contains(i)));
    }
    let mut slots: Vec<Option<RawCommit>> = commits.drain(..).map(Some).collect();
    order.into_iter().map(|i| slots[i].take().expect("each row once")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn c(h: &str, parents: &[&str], time: i64) -> RawCommit {
        RawCommit { hash: h.as_bytes().to_vec(), parents: parents.iter().map(|p| p.as_bytes().to_vec()).collect(), time }
    }

    #[test]
    fn simple_parents_take_no_edge_storage() {
        let g = Graph::build(&[c("c", &["b"], 3), c("b", &["a"], 2), c("a", &[], 1)], &HashMap::new(), Some(b"c"));
        assert!(g.parents.is_empty(), "a straight line stores no edges");
        assert_eq!(g.parents_of(0).collect::<Vec<_>>(), [1]);
        assert_eq!(g.children_of(2), [1]);
        assert_eq!(g.layout, [1, 1, 1]);
        let r = g.row(1);
        assert_eq!((r.node, r.width), (0, 1));
        assert_eq!(r.segments.len(), 2);
        assert!(r.segments.iter().all(|s| s.at == 0.0 && s.to == 0.0 && !s.arrow));
    }

    /// main: m3 ← m2 ← m1; feature f2 ← f1 ← m1; merge M joins them.
    fn branchy() -> Graph {
        let commits = vec![
            c("M", &["m3", "f2"], 9),
            c("f2", &["f1"], 8),
            c("m3", &["m2"], 7),
            c("f1", &["m1"], 6),
            c("m2", &["m1"], 5),
            c("m1", &[], 1),
        ];
        let refs = HashMap::from([(b"M".to_vec(), RefKind::Main)]);
        Graph::build(&commits, &refs, Some(b"M"))
    }

    #[test]
    fn the_first_parent_line_stays_left() {
        let g = branchy();
        // M, m3, m2, m1 are layout 1 (first-parent walk); f2, f1 open layout 2.
        let li: Vec<u32> = (0..6).map(|r| g.layout[r]).collect();
        assert_eq!(li, [1, 2, 1, 2, 1, 1]);
        let m3 = g.row(2);
        assert_eq!(m3.node, 0, "main stays in column 0");
        assert_eq!(m3.width, 2, "the feature edge passes to its right");
        let f2 = g.row(1);
        assert_eq!(f2.node, 1);
        assert!(g.row(0).merge);
        // The merge's second parent edge bends from column 0 to column 1.
        let row0 = g.row(0);
        let down: Vec<&Segment> = row0.segments.iter().filter(|s| !s.up).collect();
        assert_eq!(down.len(), 2);
        assert!(down.iter().any(|s| s.to == 0.5));
        assert_ne!(g.color_of(1), g.color_of(2));
    }

    #[test]
    fn long_edges_show_only_near_their_ends() {
        let mut commits = vec![c("top", &["n0", "base"], 1000)];
        for i in 0..40 {
            commits.push(c(&format!("n{i}"), &[&format!("n{}", i + 1)], 999 - i));
        }
        commits.push(c("n40", &["base"], 10));
        commits.push(c("base", &[], 1));
        let g = Graph::build(&commits, &HashMap::new(), Some(b"top"));
        assert_eq!(g.row(1).width, 2, "next to its start the long edge has a column");
        assert_eq!(g.row(20).width, 1, "in the middle it doesn't");
        assert!(g.row(1).segments.iter().any(|s| s.arrow && !s.up));
        let last = g.len() as u32 - 2;
        assert_eq!(g.row(last).width, 2);
        assert!(g.row(last).segments.iter().any(|s| s.arrow && s.up));
    }

    #[test]
    fn unloaded_parents_end_in_an_arrow() {
        let g = Graph::build(&[c("b", &["a"], 2), c("x", &["gone"], 1)], &HashMap::new(), None);
        assert_eq!(g.parents_of(1).collect::<Vec<_>>(), [NOT_LOADED]);
        assert!(g.row(1).segments.iter().any(|s| s.arrow && !s.up));
    }

    #[test]
    fn topo_sort_puts_children_first_by_date() {
        // Clock skew: the parent claims to be newer than its child.
        let sorted = topo_sort(vec![c("p", &[], 50), c("child", &["p"], 10), c("other", &[], 30)]);
        let order: Vec<&[u8]> = sorted.iter().map(|c| c.hash.as_slice()).collect();
        assert_eq!(order, [&b"other"[..], b"child", b"p"]);
    }
}
