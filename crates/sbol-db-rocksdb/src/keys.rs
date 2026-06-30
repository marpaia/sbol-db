//! Index-key layout for the permuted triple indexes.
//!
//! A triple is four term ids — graph (G), subject (S), predicate (P), object
//! (O) — and each index stores them concatenated in a fixed slot order so that
//! a triple pattern with bound leading positions becomes a single prefix range
//! scan. Default-graph triples (no G) live in the three `d*` indexes; named-graph
//! triples live in the six `g*`/`*g` indexes. The key *is* the triple, so a
//! re-insert writes the same key and set semantics hold for free.

use crate::codec::{TermId, ID_LEN};

/// A triple position; the unit an index orders its key by.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Slot {
    S,
    P,
    O,
    G,
}

/// One permuted index: its column family and the slot order of its key.
#[derive(Clone, Copy)]
pub struct Index {
    pub cf: &'static str,
    pub slots: &'static [Slot],
}

use Slot::{G, O, P, S};

// Default-graph triple indexes (key = 3 ids).
pub const DSPO: Index = Index {
    cf: "dspo",
    slots: &[S, P, O],
};
pub const DPOS: Index = Index {
    cf: "dpos",
    slots: &[P, O, S],
};
pub const DOSP: Index = Index {
    cf: "dosp",
    slots: &[O, S, P],
};

// Named-graph triple indexes (key = 4 ids).
pub const SPOG: Index = Index {
    cf: "spog",
    slots: &[S, P, O, G],
};
pub const POSG: Index = Index {
    cf: "posg",
    slots: &[P, O, S, G],
};
pub const OSPG: Index = Index {
    cf: "ospg",
    slots: &[O, S, P, G],
};
pub const GSPO: Index = Index {
    cf: "gspo",
    slots: &[G, S, P, O],
};
pub const GPOS: Index = Index {
    cf: "gpos",
    slots: &[G, P, O, S],
};
pub const GOSP: Index = Index {
    cf: "gosp",
    slots: &[G, O, S, P],
};

pub const DEFAULT_INDEXES: [Index; 3] = [DSPO, DPOS, DOSP];
pub const NAMED_INDEXES: [Index; 6] = [SPOG, POSG, OSPG, GSPO, GPOS, GOSP];

/// A triple's four ids; `g` is `None` for the default graph.
#[derive(Clone, Copy)]
pub struct Quad {
    pub g: Option<TermId>,
    pub s: TermId,
    pub p: TermId,
    pub o: TermId,
}

impl Quad {
    fn slot(&self, slot: Slot) -> TermId {
        match slot {
            Slot::S => self.s,
            Slot::P => self.p,
            Slot::O => self.o,
            Slot::G => self.g.expect("named-graph index built without a graph id"),
        }
    }

    /// Build this triple's key for `index`.
    pub fn key(&self, index: Index) -> Vec<u8> {
        let mut out = Vec::with_capacity(index.slots.len() * ID_LEN);
        for &slot in index.slots {
            out.extend_from_slice(&self.slot(slot));
        }
        out
    }
}

/// Read the `n`-th 16-byte id out of an index key.
pub fn id_at(key: &[u8], n: usize) -> TermId {
    let mut id = [0u8; ID_LEN];
    id.copy_from_slice(&key[n * ID_LEN..(n + 1) * ID_LEN]);
    id
}

/// Decode an index key back into a [`Quad`] using the index's slot order.
pub fn decode_key(index: Index, key: &[u8]) -> Quad {
    let mut g = None;
    let mut s = [0u8; ID_LEN];
    let mut p = [0u8; ID_LEN];
    let mut o = [0u8; ID_LEN];
    for (n, &slot) in index.slots.iter().enumerate() {
        let id = id_at(key, n);
        match slot {
            Slot::S => s = id,
            Slot::P => p = id,
            Slot::O => o = id,
            Slot::G => g = Some(id),
        }
    }
    Quad { g, s, p, o }
}

/// Concatenate ids into a scan prefix.
pub fn prefix(ids: &[TermId]) -> Vec<u8> {
    let mut out = Vec::with_capacity(ids.len() * ID_LEN);
    for id in ids {
        out.extend_from_slice(id);
    }
    out
}
