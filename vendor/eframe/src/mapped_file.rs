//! Read-only file mappings for large data that is read sparsely, such as
//! font collections: their pages come from the page cache on demand and are
//! shared with every other program using the same file, instead of being
//! copied whole into private memory. The unsafe call stays here, behind a
//! safe type.
#![allow(unsafe_code)]

/// A whole file mapped read-only.
pub struct MappedFile(memmap2::Mmap);

impl MappedFile {
    /// Maps `file` read-only.
    ///
    /// The bytes are only valid while nobody truncates the file in place, the
    /// same assumption FreeType and fontconfig make for installed fonts:
    /// package managers replace such files rather than rewrite them.
    pub fn map(file: &std::fs::File) -> std::io::Result<Self> {
        // SAFETY: a read-only, private mapping of a regular file; see above.
        unsafe { memmap2::Mmap::map(file) }.map(Self)
    }

    /// Keeps the mapping for the rest of the process, for data such as
    /// fonts that is loaded once and used until exit.
    pub fn leak(self) -> &'static [u8] {
        let mapping: &'static memmap2::Mmap = Box::leak(Box::new(self.0));
        mapping
    }
}

impl std::ops::Deref for MappedFile {
    type Target = [u8];

    fn deref(&self) -> &[u8] {
        &self.0
    }
}
