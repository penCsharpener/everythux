//! Binds a parsed [`Expr`] to a [`SearchIndex`] and evaluates it chunk by chunk into bitmasks.
//!
//! Every node evaluates to a bitmask over a chunk of entries and only looks at *candidate* entries
//! (bits set in the incoming mask). `AND` threads the shrinking candidate set through its children,
//! cheapest and most selective first, so an expensive regex only runs on entries that survived the
//! cheap filters. Substring leaves switch to one bulk `memmem` scan over the whole name arena when
//! many candidates remain, which is several times faster than a call per name.

use everythux_core::ids::DirId;
use everythux_core::model::EntryKind;
use everythux_core::snapshot::{ByteArena, IndexSnapshot};
use memchr::memmem::Finder;
use rayon::prelude::*;
use regex::bytes::Regex;

use crate::engine::SearchIndex;
use crate::query::{Expr, Glob, Matcher, Scope, Term};

/// Entries per evaluation chunk (a multiple of 64). The cancellation flag is checked once per chunk.
pub(crate) const CHUNK_ENTRIES: usize = 1 << 15;
/// `u64` words per full chunk mask.
pub(crate) const CHUNK_WORDS: usize = CHUNK_ENTRIES / 64;

/// A plain bitset over dense indexes.
#[derive(Debug)]
pub(crate) struct Bits {
    words: Vec<u64>,
}

impl Bits {
    /// Evaluates `pred` for `0..len` in parallel.
    fn from_fn(len: usize, pred: impl Fn(usize) -> bool + Sync) -> Self {
        let mut words = vec![0u64; len.div_ceil(64)];
        words.par_chunks_mut(64).enumerate().for_each(|(c, chunk)| {
            for (w, word) in chunk.iter_mut().enumerate() {
                let base = (c * 64 + w) * 64;
                for b in 0..64.min(len.saturating_sub(base)) {
                    if pred(base + b) {
                        *word |= 1 << b;
                    }
                }
            }
        });
        Self { words }
    }

    #[inline]
    fn get(&self, i: usize) -> bool {
        self.words[i >> 6] >> (i & 63) & 1 != 0
    }

    fn count(&self) -> usize {
        self.words.iter().map(|w| w.count_ones() as usize).sum()
    }
}

/// Query bound to one snapshot.
#[derive(Debug)]
pub(crate) enum Plan<'a> {
    All,
    Nothing,
    And(Vec<Plan<'a>>),
    Or(Vec<Plan<'a>>),
    Not(Box<Plan<'a>>),
    Leaf(Leaf<'a>),
}

#[derive(Debug)]
pub(crate) enum Leaf<'a> {
    NameSub {
        names: &'a ByteArena,
        finder: &'a Finder<'static>,
    },
    NamePrefix {
        names: &'a ByteArena,
        prefix: &'a [u8],
    },
    NameGlob {
        names: &'a ByteArena,
        glob: &'a Glob,
    },
    NameRegex {
        names: &'a ByteArena,
        re: &'a Regex,
    },
    /// Wildcard against the name or the full path.
    PathGlob {
        glob: &'a Glob,
        dirs: &'a ByteArena,
        names: &'a ByteArena,
    },
    /// Regex against the full path.
    PathRegex {
        re: &'a Regex,
        dirs: &'a ByteArena,
        names: &'a ByteArena,
    },
    /// The entry's folder is in the set.
    Dirs(Bits),
    Ext(Vec<bool>),
    Size {
        lo: u64,
        hi: u64,
    },
    Mtime {
        lo: i64,
        hi: i64,
    },
    Folder(bool),
}

impl<'a> Plan<'a> {
    /// Binds `expr` to `index`, resolving extension ids, folder sets and volume sets.
    pub(crate) fn bind(expr: &'a Expr, index: &'a SearchIndex) -> Self {
        Binder {
            index,
            snap: index.snapshot(),
        }
        .bind(expr)
    }

    fn and(children: Vec<Self>) -> Self {
        let mut flat = Vec::with_capacity(children.len());
        for c in children {
            match c {
                Self::All => {}
                Self::Nothing => return Self::Nothing,
                Self::And(inner) => flat.extend(inner),
                other => flat.push(other),
            }
        }
        flat.sort_by_cached_key(Self::cost);
        match flat.len() {
            0 => Self::All,
            1 => flat.remove(0),
            _ => Self::And(flat),
        }
    }

    fn or(children: Vec<Self>) -> Self {
        let mut flat = Vec::with_capacity(children.len());
        for c in children {
            match c {
                Self::Nothing => {}
                Self::All => return Self::All,
                Self::Or(inner) => flat.extend(inner),
                other => flat.push(other),
            }
        }
        flat.sort_by_cached_key(Self::cost);
        match flat.len() {
            0 => Self::Nothing,
            1 => flat.remove(0),
            _ => Self::Or(flat),
        }
    }

    fn not(child: Self) -> Self {
        match child {
            Self::All => Self::Nothing,
            Self::Nothing => Self::All,
            Self::Not(inner) => *inner,
            other => Self::Not(Box::new(other)),
        }
    }

    fn dirs(bits: Bits, ndirs: usize) -> Self {
        match bits.count() {
            0 => Self::Nothing,
            n if n == ndirs => Self::All,
            _ => Self::Leaf(Leaf::Dirs(bits)),
        }
    }

    /// Rough per-entry cost, used to order `AND`/`OR` children (cheap and selective first).
    fn cost(&self) -> u32 {
        match self {
            Self::All | Self::Nothing => 0,
            Self::Not(c) => c.cost() + 1,
            Self::And(c) | Self::Or(c) => c.iter().map(Self::cost).sum(),
            Self::Leaf(l) => match l {
                Leaf::Ext(_) | Leaf::Size { .. } | Leaf::Mtime { .. } | Leaf::Folder(_) => 10,
                Leaf::Dirs(_) => 12,
                Leaf::NamePrefix { .. } => 20,
                Leaf::NameSub { finder, .. } => 80 - finder.needle().len().min(50) as u32,
                Leaf::NameGlob { .. } => 120,
                Leaf::NameRegex { .. } => 200,
                Leaf::PathGlob { .. } | Leaf::PathRegex { .. } => 300,
            },
        }
    }
}

struct Binder<'a> {
    index: &'a SearchIndex,
    snap: &'a IndexSnapshot,
}

impl<'a> Binder<'a> {
    fn bind(&self, expr: &'a Expr) -> Plan<'a> {
        match expr {
            Expr::True => Plan::All,
            Expr::False => Plan::Nothing,
            Expr::And(c) => Plan::and(c.iter().map(|e| self.bind(e)).collect()),
            Expr::Or(c) => Plan::or(c.iter().map(|e| self.bind(e)).collect()),
            Expr::Not(c) => Plan::not(self.bind(c)),
            Expr::Term(t) => self.term(t),
            Expr::Ext(exts) => {
                let allowed: Vec<bool> = self
                    .snap
                    .extensions
                    .iter()
                    .map(|known| !known.is_empty() && exts.iter().any(|e| **e == **known))
                    .collect();
                if allowed.iter().any(|&a| a) {
                    Plan::Leaf(Leaf::Ext(allowed))
                } else {
                    Plan::Nothing
                }
            }
            Expr::Size { lo, hi } => {
                if (*lo, *hi) == (0, u64::MAX) {
                    Plan::All
                } else {
                    Plan::Leaf(Leaf::Size { lo: *lo, hi: *hi })
                }
            }
            Expr::Mtime { lo, hi } => {
                if (*lo, *hi) == (i64::MIN, i64::MAX) {
                    Plan::All
                } else {
                    Plan::Leaf(Leaf::Mtime { lo: *lo, hi: *hi })
                }
            }
            Expr::Folder(dir) => Plan::Leaf(Leaf::Folder(*dir)),
            Expr::Volume { needle, fold } => self.volume(needle, *fold),
        }
    }

    fn names(&self, fold: bool) -> &'a ByteArena {
        let e = &self.snap.entries;
        if fold { &e.names_fold } else { &e.names }
    }

    fn term(&self, t: &'a Term) -> Plan<'a> {
        let names = self.names(t.fold);
        match (t.scope, &t.matcher) {
            (Scope::Name, Matcher::Sub(finder)) if finder.needle().is_empty() => Plan::All,
            (Scope::Name, Matcher::Sub(finder)) => Plan::Leaf(Leaf::NameSub { names, finder }),
            (Scope::Name, Matcher::Glob(glob)) => Plan::Leaf(Leaf::NameGlob { names, glob }),
            (Scope::Name, Matcher::Regex(re)) => Plan::Leaf(Leaf::NameRegex {
                names: self.names(false),
                re,
            }),
            (Scope::Path, Matcher::Sub(finder)) => self.path_substring(finder, t.fold),
            (Scope::Path, Matcher::Glob(glob)) => Plan::Leaf(Leaf::PathGlob {
                glob,
                dirs: self.dir_paths(t.fold),
                names,
            }),
            (Scope::Path, Matcher::Regex(re)) => Plan::Leaf(Leaf::PathRegex {
                re,
                dirs: self.dir_paths(false),
                names: self.names(false),
            }),
        }
    }

    fn dir_paths(&self, fold: bool) -> &'a ByteArena {
        if fold {
            self.index.dir_fold()
        } else {
            &self.snap.dirs.paths
        }
    }

    /// Substring in `dir/name`. The only `/` between folder and name is the separator, so a needle
    /// without `/` lies entirely in the folder path or entirely in the name. A needle `a/b` either
    /// lies in the folder path, or `a` ends the folder path and `b` starts the name.
    fn path_substring(&self, finder: &'a Finder<'static>, fold: bool) -> Plan<'a> {
        let dirs = self.dir_paths(fold);
        let names = self.names(fold);
        let ndirs = self.snap.dirs.len();
        let needle = finder.needle();
        let in_dir = Plan::dirs(
            Bits::from_fn(ndirs, |d| finder.find(dirs.get(d)).is_some()),
            ndirs,
        );
        let Some(slash) = needle.iter().rposition(|&b| b == b'/') else {
            return Plan::or(vec![in_dir, Plan::Leaf(Leaf::NameSub { names, finder })]);
        };
        let (head, tail) = (&needle[..slash], &needle[slash + 1..]);
        let ends_dir = Plan::dirs(
            Bits::from_fn(ndirs, |d| {
                let path = dirs.get(d);
                path.strip_suffix(b"/").unwrap_or(path).ends_with(head)
            }),
            ndirs,
        );
        let name_part = if tail.is_empty() {
            Plan::All
        } else {
            Plan::Leaf(Leaf::NamePrefix { names, prefix: tail })
        };
        Plan::or(vec![in_dir, Plan::and(vec![ends_dir, name_part])])
    }

    fn volume(&self, needle: &[u8], fold: bool) -> Plan<'a> {
        let hit = |text: &str| {
            let text = if fold {
                everythux_core::text::fold(text.as_bytes())
            } else {
                text.to_owned()
            };
            Finder::new(needle).find(text.as_bytes()).is_some()
        };
        let ok: Vec<bool> = self
            .snap
            .volumes
            .iter()
            .map(|v| hit(&v.mount_point.to_string_lossy()) || v.label.as_deref().is_some_and(hit))
            .collect();
        if ok.iter().all(|&b| b) {
            return Plan::All;
        }
        let volume = &self.snap.dirs.volume;
        let ndirs = self.snap.dirs.len();
        Plan::dirs(
            Bits::from_fn(ndirs, |d| ok.get(volume[d].index()).copied().unwrap_or(false)),
            ndirs,
        )
    }
}

/// Position of a chunk within the snapshot.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Chunk {
    pub start: usize,
    pub len: usize,
}

impl Chunk {
    fn words(self) -> usize {
        self.len.div_ceil(64)
    }

    fn range(self) -> std::ops::Range<usize> {
        self.start..self.start + self.len
    }

    /// Mask with the first `len` bits set.
    pub(crate) fn full_mask(self, sc: &mut Scratch) -> Vec<u64> {
        let mut m = sc.take(self.words());
        m.fill(u64::MAX);
        if !self.len.is_multiple_of(64) {
            let last = m.len() - 1;
            m[last] = (1u64 << (self.len % 64)) - 1;
        }
        m
    }
}

/// Reusable per-thread buffers so evaluating a chunk allocates (almost) nothing.
#[derive(Debug, Default)]
pub(crate) struct Scratch {
    pool: Vec<Vec<u64>>,
    path: Vec<u8>,
}

impl Scratch {
    /// A zeroed mask of `words` words.
    fn take(&mut self, words: usize) -> Vec<u64> {
        let mut v = self.pool.pop().unwrap_or_default();
        v.clear();
        v.resize(words, 0);
        v
    }

    fn copy_of(&mut self, src: &[u64]) -> Vec<u64> {
        let mut v = self.pool.pop().unwrap_or_default();
        v.clear();
        v.extend_from_slice(src);
        v
    }

    /// Returns a mask to the pool.
    pub(crate) fn give(&mut self, v: Vec<u64>) {
        self.pool.push(v);
    }
}

fn popcount(mask: &[u64]) -> usize {
    mask.iter().map(|w| w.count_ones() as usize).sum()
}

fn is_zero(mask: &[u64]) -> bool {
    mask.iter().all(|&w| w == 0)
}

/// Whether enough candidates remain that one bulk scan beats per-entry calls.
fn dense(cand: &[u64], len: usize) -> bool {
    popcount(cand) * 6 >= len
}

/// Writes `out[word] = bits of cand whose entry passes pred`.
fn filter_bits(cand: &[u64], out: &mut [u64], mut pred: impl FnMut(usize) -> bool) {
    for (w, (&c, o)) in cand.iter().zip(out.iter_mut()).enumerate() {
        let mut bits = c;
        let mut res = 0u64;
        while bits != 0 {
            let b = bits.trailing_zeros() as usize;
            if pred(w * 64 + b) {
                res |= 1 << b;
            }
            bits &= bits - 1;
        }
        *o = res;
    }
}

/// Sets the bit of every entry in `ch` whose name contains the needle, with one `memmem` scan over
/// the arena's bytes. A hit that straddles two names is discarded.
fn bulk_substring(names: &ByteArena, finder: &Finder<'_>, ch: Chunk, out: &mut [u64]) {
    let offsets = &names.offsets()[ch.start..=ch.start + ch.len];
    let base = offsets[0] as usize;
    let hay = &names.bytes()[base..offsets[ch.len] as usize];
    let needle_len = finder.needle().len();
    let mut from = 0usize;
    let mut entry = 0usize;
    while let Some(hit) = finder.find(&hay[from..]) {
        let pos = from + hit;
        while offsets[entry + 1] as usize - base <= pos {
            entry += 1;
        }
        let end = offsets[entry + 1] as usize - base;
        if pos + needle_len <= end {
            out[entry >> 6] |= 1 << (entry & 63);
        }
        // Later starts inside this entry end even further right, so none can fit either.
        from = end;
    }
}

fn and_assign(out: &mut [u64], other: &[u64]) {
    for (o, &c) in out.iter_mut().zip(other) {
        *o &= c;
    }
}

/// `dir path + '/' + name` into `buf` (no doubled slash for a root folder).
fn full_path(buf: &mut Vec<u8>, dirs: &ByteArena, names: &ByteArena, dir: DirId, entry: usize) {
    buf.clear();
    let d = dirs.get(dir.index());
    buf.extend_from_slice(d);
    if !d.ends_with(b"/") {
        buf.push(b'/');
    }
    buf.extend_from_slice(names.get(entry));
}

/// A path wildcard matches the whole path or any suffix that starts after a `/`, so
/// `path:src/*.rs` finds `/home/me/src/lib.rs` without a leading `*/`.
fn glob_matches_path(glob: &Glob, path: &[u8]) -> bool {
    glob.is_match(path) || memchr::memchr_iter(b'/', path).any(|slash| glob.is_match(&path[slash + 1..]))
}

impl Plan<'_> {
    /// Evaluates the plan for the entries of `ch` that are set in `cand`; the result is a subset of
    /// `cand`. The returned mask belongs to the caller (hand it back with [`Scratch::give`]).
    pub(crate) fn eval(&self, snap: &IndexSnapshot, ch: Chunk, cand: &[u64], sc: &mut Scratch) -> Vec<u64> {
        match self {
            Self::All => sc.copy_of(cand),
            Self::Nothing => sc.take(cand.len()),
            Self::And(children) => {
                let mut cur = sc.copy_of(cand);
                for child in children {
                    let next = child.eval(snap, ch, &cur, sc);
                    sc.give(cur);
                    cur = next;
                    if is_zero(&cur) {
                        break;
                    }
                }
                cur
            }
            Self::Or(children) => {
                let mut rest = sc.copy_of(cand);
                let mut acc = sc.take(cand.len());
                for child in children {
                    let hit = child.eval(snap, ch, &rest, sc);
                    for ((a, r), &h) in acc.iter_mut().zip(rest.iter_mut()).zip(&hit) {
                        *a |= h;
                        *r &= !h;
                    }
                    sc.give(hit);
                    if is_zero(&rest) {
                        break;
                    }
                }
                sc.give(rest);
                acc
            }
            Self::Not(child) => {
                let hit = child.eval(snap, ch, cand, sc);
                let mut out = sc.copy_of(cand);
                for (o, &h) in out.iter_mut().zip(&hit) {
                    *o &= !h;
                }
                sc.give(hit);
                out
            }
            Self::Leaf(leaf) => {
                let mut out = sc.take(cand.len());
                leaf.eval(snap, ch, cand, &mut out, sc);
                out
            }
        }
    }
}

impl Leaf<'_> {
    fn eval(&self, snap: &IndexSnapshot, ch: Chunk, cand: &[u64], out: &mut [u64], sc: &mut Scratch) {
        let e = &snap.entries;
        let start = ch.start;
        match self {
            Self::NameSub { names, finder } => {
                if dense(cand, ch.len) {
                    bulk_substring(names, finder, ch, out);
                    and_assign(out, cand);
                } else {
                    filter_bits(cand, out, |k| finder.find(names.get(start + k)).is_some());
                }
            }
            Self::NamePrefix { names, prefix } => {
                filter_bits(cand, out, |k| names.get(start + k).starts_with(prefix));
            }
            Self::NameGlob { names, glob } => {
                if let Some(pre) = glob.prefilter().filter(|_| dense(cand, ch.len)) {
                    let mut pre_hits = sc.take(cand.len());
                    bulk_substring(names, pre, ch, &mut pre_hits);
                    and_assign(&mut pre_hits, cand);
                    filter_bits(&pre_hits, out, |k| glob.is_match(names.get(start + k)));
                    sc.give(pre_hits);
                } else {
                    filter_bits(cand, out, |k| glob.is_match(names.get(start + k)));
                }
            }
            Self::NameRegex { names, re } => filter_bits(cand, out, |k| re.is_match(names.get(start + k))),
            Self::PathGlob { glob, dirs, names } => {
                let mut buf = std::mem::take(&mut sc.path);
                filter_bits(cand, out, |k| {
                    glob.is_match(names.get(start + k)) || {
                        full_path(&mut buf, dirs, names, e.dir[start + k], start + k);
                        glob_matches_path(glob, &buf)
                    }
                });
                sc.path = buf;
            }
            Self::PathRegex { re, dirs, names } => {
                let mut buf = std::mem::take(&mut sc.path);
                filter_bits(cand, out, |k| {
                    full_path(&mut buf, dirs, names, e.dir[start + k], start + k);
                    re.is_match(&buf)
                });
                sc.path = buf;
            }
            Self::Dirs(bits) => {
                let dirs = &e.dir[ch.range()];
                filter_bits(cand, out, |k| bits.get(dirs[k].index()));
            }
            Self::Ext(allowed) => {
                let ext = &e.ext[ch.range()];
                filter_bits(cand, out, |k| {
                    allowed.get(usize::from(ext[k])).copied().unwrap_or(false)
                });
            }
            Self::Size { lo, hi } => {
                let size = &e.size[ch.range()];
                filter_bits(cand, out, |k| (*lo..=*hi).contains(&size[k]));
            }
            Self::Mtime { lo, hi } => {
                let mtime = &e.mtime_ns[ch.range()];
                filter_bits(cand, out, |k| mtime[k] >= *lo && mtime[k] < *hi);
            }
            Self::Folder(want_dir) => {
                let kind = &e.kind[ch.range()];
                filter_bits(cand, out, |k| (kind[k] == EntryKind::Dir) == *want_dir);
            }
        }
    }
}
