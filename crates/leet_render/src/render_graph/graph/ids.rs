//! Typed ids used by render graph storage.
//!
//! Node, dependency, implementation, and group ids are separate Rust newtypes so
//! APIs cannot accidentally accept a dependency id where a node id is required.

macro_rules! define_graph_id {
    ($name:ident, $kind:literal) => {
        #[doc = concat!("Typed id for a render graph ", $kind, ".")]
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
        #[repr(transparent)]
        pub struct $name(u32);

        impl $name {
            /// Raw value reserved for invalid/null ids.
            pub const INVALID_RAW: u32 = u32::MAX;
            /// Invalid/null id sentinel.
            pub const INVALID: Self = Self(Self::INVALID_RAW);

            /// Creates an id from a storage slot index.
            pub const fn from_index(index: u32) -> Self {
                Self(index)
            }

            /// Creates an id from a raw value.
            ///
            /// This is intended for tests, diagnostics, cache serialization, and
            /// storage internals. Normal graph code should prefer typed values
            /// produced by graph storage.
            pub const fn from_raw(raw: u32) -> Self {
                Self(raw)
            }

            /// Returns the raw id value.
            pub const fn raw(self) -> u32 {
                self.0
            }

            /// Returns the storage slot index, or `None` for the invalid id.
            pub const fn index(self) -> Option<u32> {
                if self.is_valid() {
                    Some(self.0)
                } else {
                    None
                }
            }

            /// Returns whether this id is not the invalid/null sentinel.
            pub const fn is_valid(self) -> bool {
                self.0 != Self::INVALID_RAW
            }
        }

        impl Default for $name {
            fn default() -> Self {
                Self::INVALID
            }
        }

        impl GraphStorageId for $name {
            fn from_index(index: u32) -> Self {
                Self::from_index(index)
            }

            fn index(self) -> Option<u32> {
                self.index()
            }

            fn raw(self) -> u32 {
                self.raw()
            }
        }
    };
}

/// Id behavior required by graph storage.
pub(crate) trait GraphStorageId: Copy + Eq {
    fn from_index(index: u32) -> Self;
    fn index(self) -> Option<u32>;
    fn raw(self) -> u32;
}

define_graph_id!(RenderNodeId, "node");
define_graph_id!(RenderDependencyId, "dependency");
define_graph_id!(RenderNodeImplId, "node implementation");
define_graph_id!(NodeGroupId, "node group");
