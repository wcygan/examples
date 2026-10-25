//! Two interchangeable B-trees behind every ordered index structure: std's
//! `BTreeMap`/`BTreeSet` and bbtree's `unsync::BBTreeMap`/`BBTreeSet`.
//!
//! The database picks one for all of its indexes. Each structure matches on
//! its variant once per call, so an index scan's inner loop runs on one tree.

use crate::{CustomerId, OrderId};
use bbtree::{
    unsync::{bbtree_set, BBTreeSet},
    InlineKey, InlineWidth, InlineWord, WithInlineKey,
};
use serde::{Deserialize, Serialize};
use std::{
    collections::{btree_set, BTreeSet},
    ops::RangeInclusive,
};

/// Which B-tree implementation holds the secondary indexes.
#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
pub enum Tree {
    /// `std::collections::{BTreeMap, BTreeSet}`.
    #[default]
    #[serde(rename = "std")]
    Std,
    /// `bbtree::unsync::{BBTreeMap, BBTreeSet}`, with inline keys.
    #[serde(rename = "bbtree")]
    BBTree,
}

/// Lets bbtree nodes keep a register-sized prefix of each ID beside the keys
/// and search those first, as they do for a plain `u64`.
impl InlineKey for OrderId {
    const WIDTH: InlineWidth = InlineWidth::Two;

    fn inline_key_at(&self, offset: usize) -> InlineWord {
        self.0.inline_key_at(offset)
    }
}

/// One entry of the customer/date index. Like an InnoDB secondary index
/// record, the key columns are followed by the primary key, so every entry is
/// unique and one sorted set holds the whole index. Field order is sort order,
/// which matches the big-endian bytes its inline keys come from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct CustomerDate {
    pub customer_id: CustomerId,
    pub placed_at: u64,
    pub order_id: OrderId,
}

impl InlineKey for CustomerDate {
    const WIDTH: InlineWidth = InlineWidth::Two;

    fn inline_key_at(&self, offset: usize) -> InlineWord {
        let mut bytes = [0; 24];
        bytes[..8].copy_from_slice(&self.customer_id.0.to_be_bytes());
        bytes[8..16].copy_from_slice(&self.placed_at.to_be_bytes());
        bytes[16..].copy_from_slice(&self.order_id.0.to_be_bytes());
        bytes.inline_key_at(offset)
    }
}

/// An iterator over one tree or the other.
pub(crate) enum TreeIter<S, B> {
    Std(S),
    BBTree(B),
}

impl<S: Iterator, B: Iterator<Item = S::Item>> Iterator for TreeIter<S, B> {
    type Item = S::Item;

    fn next(&mut self) -> Option<Self::Item> {
        match self {
            Self::Std(iter) => iter.next(),
            Self::BBTree(iter) => iter.next(),
        }
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        match self {
            Self::Std(iter) => iter.size_hint(),
            Self::BBTree(iter) => iter.size_hint(),
        }
    }

    // `for_each` and friends dispatch once, then run the inner tree's loop.
    fn fold<A, F: FnMut(A, Self::Item) -> A>(self, init: A, f: F) -> A {
        match self {
            Self::Std(iter) => iter.fold(init, f),
            Self::BBTree(iter) => iter.fold(init, f),
        }
    }
}

impl<S: DoubleEndedIterator, B: DoubleEndedIterator<Item = S::Item>> DoubleEndedIterator
    for TreeIter<S, B>
{
    fn next_back(&mut self) -> Option<Self::Item> {
        match self {
            Self::Std(iter) => iter.next_back(),
            Self::BBTree(iter) => iter.next_back(),
        }
    }

    fn rfold<A, F: FnMut(A, Self::Item) -> A>(self, init: A, f: F) -> A {
        match self {
            Self::Std(iter) => iter.rfold(init, f),
            Self::BBTree(iter) => iter.rfold(init, f),
        }
    }
}

/// Order IDs sharing one secondary key, in primary-key order.
#[derive(Clone)]
pub(crate) enum IdSet {
    Std(BTreeSet<OrderId>),
    BBTree(BBTreeSet<OrderId, WithInlineKey>),
}

impl IdSet {
    pub fn new(tree: Tree) -> Self {
        match tree {
            Tree::Std => Self::Std(BTreeSet::new()),
            Tree::BBTree => Self::BBTree(BBTreeSet::with_strategy(WithInlineKey)),
        }
    }

    pub fn tree(&self) -> Tree {
        match self {
            Self::Std(_) => Tree::Std,
            Self::BBTree(_) => Tree::BBTree,
        }
    }

    pub fn insert(&mut self, id: OrderId) -> bool {
        match self {
            Self::Std(ids) => ids.insert(id),
            Self::BBTree(ids) => ids.insert(id),
        }
    }

    pub fn remove(&mut self, id: OrderId) -> bool {
        match self {
            Self::Std(ids) => ids.remove(&id),
            Self::BBTree(ids) => ids.remove(&id),
        }
    }

    pub fn is_empty(&self) -> bool {
        match self {
            Self::Std(ids) => ids.is_empty(),
            Self::BBTree(ids) => ids.is_empty(),
        }
    }

    pub fn iter(
        &self,
    ) -> TreeIter<btree_set::Iter<'_, OrderId>, bbtree_set::Iter<'_, OrderId, WithInlineKey>> {
        match self {
            Self::Std(ids) => TreeIter::Std(ids.iter()),
            Self::BBTree(ids) => TreeIter::BBTree(ids.iter()),
        }
    }
}

/// Same IDs in the same order, whichever trees hold them.
impl PartialEq for IdSet {
    fn eq(&self, other: &Self) -> bool {
        self.iter().eq(other.iter())
    }
}

/// The composite customer/date index: one ordered set of entries.
///
/// Not a map of ID sets: most `(customer, placed_at)` keys hold one order, and a
/// million one-element sets cost a node each (1 KiB apiece in bbtree).
#[derive(Clone, PartialEq)]
pub(crate) enum CustomerDateIndex {
    Std(BTreeSet<CustomerDate>),
    BBTree(BBTreeSet<CustomerDate, WithInlineKey>),
}

impl Default for CustomerDateIndex {
    fn default() -> Self {
        Self::new(Tree::default())
    }
}

impl CustomerDateIndex {
    pub fn new(tree: Tree) -> Self {
        match tree {
            Tree::Std => Self::Std(BTreeSet::new()),
            Tree::BBTree => Self::BBTree(BBTreeSet::with_strategy(WithInlineKey)),
        }
    }

    pub fn tree(&self) -> Tree {
        match self {
            Self::Std(_) => Tree::Std,
            Self::BBTree(_) => Tree::BBTree,
        }
    }

    pub fn insert(&mut self, entry: CustomerDate) -> bool {
        match self {
            Self::Std(entries) => entries.insert(entry),
            Self::BBTree(entries) => entries.insert(entry),
        }
    }

    pub fn remove(&mut self, entry: CustomerDate) -> bool {
        match self {
            Self::Std(entries) => entries.remove(&entry),
            Self::BBTree(entries) => entries.remove(&entry),
        }
    }

    pub fn range(
        &self,
        entries: RangeInclusive<CustomerDate>,
    ) -> TreeIter<
        btree_set::Range<'_, CustomerDate>,
        bbtree_set::Range<'_, CustomerDate, WithInlineKey>,
    > {
        match self {
            Self::Std(set) => TreeIter::Std(set.range(entries)),
            Self::BBTree(set) => TreeIter::BBTree(set.range(entries)),
        }
    }

    pub fn iter(
        &self,
    ) -> TreeIter<
        btree_set::Iter<'_, CustomerDate>,
        bbtree_set::Iter<'_, CustomerDate, WithInlineKey>,
    > {
        match self {
            Self::Std(set) => TreeIter::Std(set.iter()),
            Self::BBTree(set) => TreeIter::BBTree(set.iter()),
        }
    }
}
