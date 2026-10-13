//! Newtype identifiers. They index into the arrays of [`crate::snapshot::IndexSnapshot`].

use std::fmt;

macro_rules! id_type {
    ($(#[$meta:meta])* $name:ident) => {
        $(#[$meta])*
        #[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
        pub struct $name(pub u32);

        impl $name {
            /// Sentinel meaning "no id" (used e.g. for the parent of a volume root).
            pub const NONE: Self = Self(u32::MAX);

            /// The id as an array index.
            #[inline]
            pub const fn index(self) -> usize {
                self.0 as usize
            }

            /// Whether this is the [`Self::NONE`] sentinel.
            #[inline]
            pub const fn is_none(self) -> bool {
                self.0 == u32::MAX
            }
        }

        impl fmt::Debug for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                if self.is_none() {
                    write!(f, concat!(stringify!($name), "(NONE)"))
                } else {
                    write!(f, concat!(stringify!($name), "({})"), self.0)
                }
            }
        }
    };
}

id_type!(
    /// Index of an entry (file, folder, symlink, …) in [`crate::snapshot::EntryTable`].
    EntryId
);
id_type!(
    /// Index of a folder in [`crate::snapshot::DirTable`].
    DirId
);
id_type!(
    /// Index of a volume in [`crate::snapshot::IndexSnapshot::volumes`].
    VolumeId
);
