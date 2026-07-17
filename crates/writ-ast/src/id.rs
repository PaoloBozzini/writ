//! Stable node identifiers.
//!
//! `NodeId` is the key type for the checker passes' side tables (see the "Pass
//! architecture and annotations" section of the spec). Each analysis pass keeps
//! its results in its own `Map<NodeId, _>` rather than mutating the AST, so the
//! AST stays the immutable shared contract and `writ-ast` stays free of any
//! checker dependency.
//!
//! An id is threaded onto AST nodes when the first side-table consumer lands;
//! this type reserves the key so that change touches only node construction, not
//! the wider contract.

/// A stable identifier for an AST node, unique within a parsed module and usable
/// as a side-table key.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct NodeId(pub u32);

impl NodeId {
    /// The raw index.
    #[must_use]
    pub fn index(self) -> u32 {
        self.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_distinct_keys() {
        use std::collections::HashMap;
        let mut table = HashMap::new();
        table.insert(NodeId(0), "a");
        table.insert(NodeId(1), "b");
        assert_eq!(table.get(&NodeId(0)), Some(&"a"));
        assert_eq!(table.get(&NodeId(1)), Some(&"b"));
        assert_ne!(NodeId(0), NodeId(1));
    }
}
