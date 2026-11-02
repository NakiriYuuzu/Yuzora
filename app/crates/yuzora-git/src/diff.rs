//! Text diff, computed here as IntelliJ does (B7), never by `git diff`:
//!
//! - lines are numbered once (equal text, equal number), common prefix and
//!   suffix are cut, then Myers' O(ND) algorithm in linear space (the
//!   "middle snake" split) finds the changes;
//! - past a budget of `max(20000 + 10·√(n+m), 20000)` edits Myers gives up
//!   and Patience (anchored on lines unique to both sides) takes over, with
//!   Myers again between anchors;
//! - inside a changed block, words are diffed the same way for highlights;
//! - whitespace can be ignored (trimmed at the ends, or all of it).

use std::collections::HashMap;
use std::ops::Range;

/// One changed block: `a` lines in the old text became `b` lines in the new.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Change {
    pub a: Range<usize>,
    pub b: Range<usize>,
}

/// How whitespace counts when lines are compared.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Whitespace {
    #[default]
    Exact,
    /// Leading and trailing whitespace ignored.
    Trim,
    /// All whitespace ignored.
    Ignore,
}

impl Whitespace {
    pub fn parse(s: &str) -> Self {
        match s {
            "trim" => Whitespace::Trim,
            "all" | "ignore" => Whitespace::Ignore,
            _ => Whitespace::Exact,
        }
    }

    fn key<'a>(self, line: &'a [u8]) -> std::borrow::Cow<'a, [u8]> {
        let body = strip_eol(line);
        match self {
            Whitespace::Exact => std::borrow::Cow::Borrowed(body),
            Whitespace::Trim => std::borrow::Cow::Borrowed(body.trim_ascii()),
            Whitespace::Ignore => std::borrow::Cow::Owned(body.iter().copied().filter(|b| !b.is_ascii_whitespace()).collect()),
        }
    }
}

pub fn strip_eol(line: &[u8]) -> &[u8] {
    let l = line.strip_suffix(b"\n").unwrap_or(line);
    l.strip_suffix(b"\r").unwrap_or(l)
}

/// Splits text into lines that keep their terminators (the last may have none).
pub fn lines(text: &[u8]) -> Vec<&[u8]> {
    let mut out = Vec::new();
    let mut start = 0;
    for (i, b) in text.iter().enumerate() {
        if *b == b'\n' {
            out.push(&text[start..=i]);
            start = i + 1;
        }
    }
    if start < text.len() {
        out.push(&text[start..]);
    }
    out
}

/// Line changes between two texts.
pub fn diff_lines<'a>(a: &[&'a [u8]], b: &[&'a [u8]], ws: Whitespace) -> Vec<Change> {
    let mut ids: HashMap<std::borrow::Cow<'a, [u8]>, u32> = HashMap::new();
    let mut number = |l: &'a [u8]| -> u32 {
        let n = ids.len() as u32;
        *ids.entry(ws.key(l)).or_insert(n)
    };
    let ai: Vec<u32> = a.iter().map(|l| number(l)).collect();
    let bi: Vec<u32> = b.iter().map(|l| number(l)).collect();
    diff_ids(&ai, &bi)
}

/// Changes between two sequences of numbers: Myers, else Patience.
pub fn diff_ids(a: &[u32], b: &[u32]) -> Vec<Change> {
    diff_ids_within(a, b, budget(a.len(), b.len()))
}

fn diff_ids_within(a: &[u32], b: &[u32], budget: usize) -> Vec<Change> {
    myers(a, b, budget).unwrap_or_else(|| patience(a, b))
}

/// IntelliJ's `MyersLCS` threshold on the number of edits.
pub fn budget(n: usize, m: usize) -> usize {
    (20_000 + 10 * ((n + m) as f64).sqrt() as usize).max(20_000)
}

/// Myers in linear space, giving up (None) past `budget` edits.
pub fn myers(a: &[u32], b: &[u32], budget: usize) -> Option<Vec<Change>> {
    let max = (a.len() + b.len() + 1) / 2 + 1;
    let mut vf = V::new(max);
    let mut vb = V::new(max);
    let mut out = Vec::new();
    let mut work = Work { budget, edits: 0 };
    conquer(a, 0..a.len(), b, 0..b.len(), &mut vf, &mut vb, &mut out, &mut work)?;
    Some(merge(out))
}

struct Work {
    budget: usize,
    edits: usize,
}

/// A diagonal-indexed array (k from -max to max).
struct V {
    offset: isize,
    v: Vec<usize>,
}

impl V {
    fn new(max: usize) -> Self {
        Self { offset: max as isize, v: vec![0; 2 * max + 2] }
    }
}

impl std::ops::Index<isize> for V {
    type Output = usize;
    fn index(&self, k: isize) -> &usize {
        &self.v[(k + self.offset) as usize]
    }
}

impl std::ops::IndexMut<isize> for V {
    fn index_mut(&mut self, k: isize) -> &mut usize {
        &mut self.v[(k + self.offset) as usize]
    }
}

fn prefix(a: &[u32], b: &[u32]) -> usize {
    a.iter().zip(b).take_while(|(x, y)| x == y).count()
}

fn suffix(a: &[u32], b: &[u32]) -> usize {
    a.iter().rev().zip(b.iter().rev()).take_while(|(x, y)| x == y).count()
}

#[allow(clippy::too_many_arguments)]
fn conquer(a: &[u32], mut ar: Range<usize>, b: &[u32], mut br: Range<usize>, vf: &mut V, vb: &mut V, out: &mut Vec<Change>, work: &mut Work) -> Option<()> {
    let p = prefix(&a[ar.clone()], &b[br.clone()]);
    ar.start += p;
    br.start += p;
    let s = suffix(&a[ar.clone()], &b[br.clone()]);
    ar.end -= s;
    br.end -= s;
    if ar.is_empty() && br.is_empty() {
        return Some(());
    }
    if ar.is_empty() || br.is_empty() {
        work.edits += ar.len() + br.len();
        out.push(Change { a: ar, b: br });
        return (work.edits <= work.budget).then_some(());
    }
    let (x, y) = middle_snake(a, ar.clone(), b, br.clone(), vf, vb, work)?;
    conquer(a, ar.start..x, b, br.start..y, vf, vb, out, work)?;
    conquer(a, x..ar.end, b, y..br.end, vf, vb, out, work)
}

/// The start of a middle snake of an optimal path (Myers 1986, §4b).
fn middle_snake(a: &[u32], ar: Range<usize>, b: &[u32], br: Range<usize>, vf: &mut V, vb: &mut V, work: &mut Work) -> Option<(usize, usize)> {
    let n = ar.len();
    let m = br.len();
    let delta = n as isize - m as isize;
    let odd = delta & 1 == 1;
    vf[1] = 0;
    vb[1] = 0;
    let d_max = (n + m).div_ceil(2) as isize + 1;
    for d in 0..d_max {
        if work.edits + 2 * d as usize > work.budget {
            return None;
        }
        let mut k = -d;
        while k <= d {
            let mut x = if k == -d || (k != d && vf[k - 1] < vf[k + 1]) { vf[k + 1] } else { vf[k - 1] + 1 };
            let y = (x as isize - k) as usize;
            let (x0, y0) = (x, y);
            if x < n && y < m {
                x += prefix(&a[ar.start + x..ar.end], &b[br.start + y..br.end]);
            }
            vf[k] = x;
            if odd && (k - delta).abs() <= d - 1 && vf[k] + vb[-(k - delta)] >= n {
                return Some((x0 + ar.start, y0 + br.start));
            }
            k += 2;
        }
        let mut k = -d;
        while k <= d {
            let mut x = if k == -d || (k != d && vb[k - 1] < vb[k + 1]) { vb[k + 1] } else { vb[k - 1] + 1 };
            let mut y = (x as isize - k) as usize;
            if x < n && y < m {
                let s = suffix(&a[ar.start..ar.start + n - x], &b[br.start..br.start + m - y]);
                x += s;
                y += s;
            }
            vb[k] = x;
            if !odd && (k - delta).abs() <= d && vb[k] + vf[-(k - delta)] >= n {
                return Some((n - x + ar.start, m - y + br.start));
            }
            k += 2;
        }
    }
    // Unreachable for a correct d_max; treat as "too expensive".
    None
}

/// Joins touching changes into one block.
fn merge(mut cs: Vec<Change>) -> Vec<Change> {
    cs.sort_by_key(|c| (c.a.start, c.b.start));
    let mut out: Vec<Change> = Vec::with_capacity(cs.len());
    for c in cs {
        if let Some(last) = out.last_mut()
            && last.a.end == c.a.start
            && last.b.end == c.b.start
        {
            last.a.end = c.a.end;
            last.b.end = c.b.end;
            continue;
        }
        out.push(c);
    }
    out
}

/// Patience diff: anchors on lines unique to both sides (in order), Myers in
/// between; a gap with no anchors that Myers can't afford is one change.
pub fn patience(a: &[u32], b: &[u32]) -> Vec<Change> {
    let mut out = Vec::new();
    patience_into(a, 0..a.len(), b, 0..b.len(), &mut out);
    merge(out)
}

fn patience_into(a: &[u32], mut ar: Range<usize>, b: &[u32], mut br: Range<usize>, out: &mut Vec<Change>) {
    let p = prefix(&a[ar.clone()], &b[br.clone()]);
    ar.start += p;
    br.start += p;
    let s = suffix(&a[ar.clone()], &b[br.clone()]);
    ar.end -= s;
    br.end -= s;
    if ar.is_empty() && br.is_empty() {
        return;
    }
    if ar.is_empty() || br.is_empty() {
        out.push(Change { a: ar, b: br });
        return;
    }
    let anchors = unique_lcs(&a[ar.clone()], &b[br.clone()]);
    if anchors.is_empty() {
        let (sa, sb) = (&a[ar.clone()], &b[br.clone()]);
        match myers(sa, sb, budget(sa.len(), sb.len()) / 4) {
            Some(cs) => out.extend(cs.into_iter().map(|c| Change { a: c.a.start + ar.start..c.a.end + ar.start, b: c.b.start + br.start..c.b.end + br.start })),
            None => out.push(Change { a: ar, b: br }),
        }
        return;
    }
    let (mut pa, mut pb) = (ar.start, br.start);
    for (ia, ib) in anchors {
        patience_into(a, pa..ar.start + ia, b, pb..br.start + ib, out);
        pa = ar.start + ia + 1;
        pb = br.start + ib + 1;
    }
    patience_into(a, pa..ar.end, b, pb..br.end, out);
}

/// Lines appearing exactly once on each side, longest increasing run.
fn unique_lcs(a: &[u32], b: &[u32]) -> Vec<(usize, usize)> {
    let mut count: HashMap<u32, (u32, u32, usize, usize)> = HashMap::new();
    for (i, x) in a.iter().enumerate() {
        let e = count.entry(*x).or_insert((0, 0, i, 0));
        e.0 += 1;
    }
    for (j, y) in b.iter().enumerate() {
        if let Some(e) = count.get_mut(y) {
            e.1 += 1;
            e.3 = j;
        }
    }
    let mut pairs: Vec<(usize, usize)> = count.values().filter(|e| e.0 == 1 && e.1 == 1).map(|e| (e.2, e.3)).collect();
    pairs.sort_unstable();
    // Longest increasing subsequence on b (patience sorting).
    let mut tails: Vec<usize> = Vec::new();
    let mut prev: Vec<Option<usize>> = vec![None; pairs.len()];
    let mut tail_idx: Vec<usize> = Vec::new();
    for (i, &(_, bj)) in pairs.iter().enumerate() {
        let pos = tails.partition_point(|&t| t < bj);
        if pos > 0 {
            prev[i] = Some(tail_idx[pos - 1]);
        }
        if pos == tails.len() {
            tails.push(bj);
            tail_idx.push(i);
        } else {
            tails[pos] = bj;
            tail_idx[pos] = i;
        }
    }
    let mut out = Vec::new();
    let mut cur = tail_idx.last().copied();
    while let Some(i) = cur {
        out.push(pairs[i]);
        cur = prev[i];
    }
    out.reverse();
    out
}

/// A token of a line for word diff: a word, a run of spaces, or one other char.
pub fn tokens(text: &str) -> Vec<Range<usize>> {
    let mut out = Vec::new();
    let mut it = text.char_indices().peekable();
    while let Some((i, c)) = it.next() {
        let class = |c: char| if c.is_alphanumeric() || c == '_' { 1 } else if c.is_whitespace() { 2 } else { 3 };
        let k = class(c);
        let mut end = i + c.len_utf8();
        if k != 3 {
            while let Some(&(j, d)) = it.peek() {
                if class(d) != k {
                    break;
                }
                end = j + d.len_utf8();
                it.next();
            }
        }
        out.push(i..end);
    }
    out
}

/// Word-level changes between two texts: the byte ranges that differ on each side.
pub fn diff_words(a: &str, b: &str, ws: Whitespace) -> (Vec<Range<usize>>, Vec<Range<usize>>) {
    let ta = tokens(a);
    let tb = tokens(b);
    let mut ids: HashMap<&str, u32> = HashMap::new();
    let mut number = |t| -> u32 {
        let key = if ws != Whitespace::Exact && str::chars(t).all(char::is_whitespace) { " " } else { t };
        let n = ids.len() as u32;
        *ids.entry(key).or_insert(n)
    };
    let ia: Vec<u32> = ta.iter().map(|r| number(&a[r.clone()])).collect();
    let ib: Vec<u32> = tb.iter().map(|r| number(&b[r.clone()])).collect();
    let cs = myers(&ia, &ib, 2_000).unwrap_or_else(|| vec![Change { a: 0..ia.len(), b: 0..ib.len() }]);
    let span = |toks: &[Range<usize>], r: &Range<usize>| -> Option<Range<usize>> {
        if r.is_empty() { None } else { Some(toks[r.start].start..toks[r.end - 1].end) }
    };
    let skip_ws = |s: &str, r: &Range<usize>| ws != Whitespace::Exact && s[r.clone()].chars().all(char::is_whitespace);
    let left = cs.iter().filter_map(|c| span(&ta, &c.a)).filter(|r| !skip_ws(a, r)).collect();
    let right = cs.iter().filter_map(|c| span(&tb, &c.b)).filter(|r| !skip_ws(b, r)).collect();
    (left, right)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lcs(a: &[u32], b: &[u32]) -> usize {
        let mut dp = vec![vec![0usize; b.len() + 1]; a.len() + 1];
        for i in (0..a.len()).rev() {
            for j in (0..b.len()).rev() {
                dp[i][j] = if a[i] == b[j] { dp[i + 1][j + 1] + 1 } else { dp[i + 1][j].max(dp[i][j + 1]) };
            }
        }
        dp[0][0]
    }

    /// Applying the changes to `a` gives `b`, and unchanged lines match.
    fn check(a: &[u32], b: &[u32], cs: &[Change]) -> usize {
        let mut out = Vec::new();
        let mut i = 0;
        let mut j = 0;
        let mut edits = 0;
        for c in cs {
            assert!(c.a.start >= i && c.b.start >= j, "ordered: {cs:?}");
            assert_eq!(c.a.start - i, c.b.start - j, "equal runs line up: {cs:?}");
            for (x, y) in a[i..c.a.start].iter().zip(&b[j..c.b.start]) {
                assert_eq!(x, y, "unchanged lines are equal");
            }
            out.extend_from_slice(&a[i..c.a.start]);
            out.extend_from_slice(&b[c.b.clone()]);
            edits += c.a.len() + c.b.len();
            i = c.a.end;
            j = c.b.end;
        }
        out.extend_from_slice(&a[i..]);
        assert_eq!(out, b);
        edits
    }

    /// A small deterministic generator (no rand crate).
    struct Rng(u64);
    impl Rng {
        fn next(&mut self) -> u64 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            self.0
        }
        fn seq(&mut self, len: usize, alphabet: u64) -> Vec<u32> {
            (0..len).map(|_| (self.next() % alphabet) as u32).collect()
        }
    }

    #[test]
    fn myers_is_minimal_on_random_inputs() {
        let mut r = Rng(0x9e3779b97f4a7c15);
        for round in 0..400 {
            let n = (r.next() % 40) as usize;
            let m = (r.next() % 40) as usize;
            let alpha = 2 + r.next() % 6;
            let a = r.seq(n, alpha);
            let b = if round % 3 == 0 {
                // A mutation of a, the usual case.
                let mut b = a.clone();
                for _ in 0..(r.next() % 5) {
                    if !b.is_empty() {
                        let i = (r.next() as usize) % b.len();
                        b[i] = (r.next() % alpha) as u32;
                    }
                }
                b
            } else {
                r.seq(m, alpha)
            };
            let cs = myers(&a, &b, usize::MAX).unwrap();
            let edits = check(&a, &b, &cs);
            assert_eq!(edits, a.len() + b.len() - 2 * lcs(&a, &b), "minimal for {a:?} → {b:?}: {cs:?}");
            let p = patience(&a, &b);
            check(&a, &b, &p);
        }
    }

    #[test]
    fn a_huge_difference_falls_back_to_patience() {
        // Past the budget (20 000 edits and up for real files; 100 here, so
        // the test stays quick) Myers gives up and Patience anchors instead.
        assert!(budget(10, 10) >= 20_000);
        let a: Vec<u32> = (0..300).collect();
        let mut b: Vec<u32> = (1_000..1_300).collect();
        b[150] = 150;
        assert!(myers(&a, &b, 100).is_none());
        let cs = diff_ids_within(&a, &b, 100);
        check(&a, &b, &cs);
        assert_eq!(cs.len(), 2, "anchored on the one shared unique line: {:?}", cs.iter().map(|c| (c.a.clone(), c.b.clone())).collect::<Vec<_>>());
    }

    #[test]
    fn lines_keep_terminators_and_whitespace_can_be_ignored() {
        let a = b"one\r\ntwo\n  three\nfour";
        let l = lines(a);
        assert_eq!(l, [&b"one\r\n"[..], b"two\n", b"  three\n", b"four"]);
        let b = b"one\ntwo\nthree  \nfour\n";
        let lb = lines(b);
        assert_eq!(diff_lines(&l, &lb, Whitespace::Exact).len(), 1);
        // CR and missing final newline are line endings, not content.
        assert_eq!(diff_lines(&l, &lb, Whitespace::Exact)[0], Change { a: 2..3, b: 2..3 });
        assert!(diff_lines(&l, &lb, Whitespace::Trim).is_empty());
        let c = lines(b"one\nt w o\nthree\nfour\n");
        assert!(diff_lines(&lb, &c, Whitespace::Ignore).is_empty());
    }

    #[test]
    fn words_highlight_only_what_changed() {
        let (l, r) = diff_words("let offset = 0;", "let offset = remote.stat(&path)?.size;", Whitespace::Exact);
        assert_eq!(l.iter().map(|x| &"let offset = 0;"[x.clone()]).collect::<Vec<_>>(), ["0"]);
        let rt = "let offset = remote.stat(&path)?.size;";
        assert_eq!(r.iter().map(|x| &rt[x.clone()]).collect::<String>(), "remote.stat(&path)?.size");
        let (l, r) = diff_words("a  b", "a b", Whitespace::Trim);
        assert!(l.is_empty() && r.is_empty());
        assert_eq!(tokens("fn 名前(x)").len(), 6);
    }
}
