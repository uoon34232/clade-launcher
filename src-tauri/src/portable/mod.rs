//! Portable-folder architecture: path handling, atomic IO, locking, verified installs.

pub mod fsutil;
pub mod relpath;
pub mod root;

pub use relpath::RelPath;
pub use root::{PortableRoot, RootLock};
