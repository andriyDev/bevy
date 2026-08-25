//! In-memory [`Asset`] storage backend.
//!
//! See [`Dir`] for details.
//!
//! [`Asset`]: crate::Asset

use crate::{
    io::{
        AssetReader, AssetReaderError, AssetWriter, AssetWriterError, PathStream, Reader,
        ReaderNotSeekableError, SeekableReader,
    },
    normalize_path,
};
use alloc::{borrow::ToOwned, boxed::Box, format, string::String, sync::Arc, vec, vec::Vec};
use bevy_platform::{
    collections::{
        hash_map::{EntryRef, VacantEntryRef},
        HashMap,
    },
    hash::FixedHasher,
    sync::{PoisonError, RwLock},
};
use core::{pin::Pin, task::Poll};
use futures_io::{AsyncRead, AsyncWrite};
use slotmap::{new_key_type, SlotMap};
use std::{
    io::{Error, ErrorKind, SeekFrom},
    path::{Component, Path, PathBuf},
};

use super::AsyncSeek;

new_key_type! {
    /// The (stable) key of a directory in a [`VirtualFilesystem`].
    struct DirKey;
}
new_key_type! {
    /// The (stable) key of a value in a [`VirtualFilesystem`].
    struct ValueKey;
}

/// A virtual (aka in-memory) filesystem where the "files" store type `T`.
///
/// This allows finding elements with "path-like" syntax (e.g., `path/to/my/data.txt`). This
/// filesystem is a strict tree: there are no cycles and every node has a unique parent.
///
/// Paths are slash-delimited strings. Components in a path cannot be empty or `.`. Components may
/// be `..` if and only if A) the filesystem was created with `allow_above_root` set to true, and B)
/// all parents of a directory all the way to the root are also `..` (in other words, `../abc/..` is
/// invalid).
pub struct VirtualFilesystem<T> {
    /// The index of the root directory in [`Self::dirs`].
    root_key: DirKey,
    /// The directories of the virtual filesystem.
    dirs: SlotMap<DirKey, Dir>,
    /// The values stored inside the `dirs`.
    ///
    /// Only values stored in `dirs` are stored here, and each value may only be referenced by one
    /// `dirs`.
    values: SlotMap<ValueKey, T>,
}

/// A single directory in a [`VirtualFilesystem`].
///
/// This stores all children of a directory by their name.
struct Dir {
    children: HashMap<Box<str>, ChildEntry>,
    /// Allows using `..` to "escape" out of this directory.
    ///
    /// This is only supported at the top-level and only if this directory is on a `..` chain
    /// starting from the root.
    allow_above: bool,
}

/// An entry in a directory of a [`VirtualFilesystem`].
#[derive(Clone, Copy)]
enum ChildEntry {
    /// The entry is a directory with this key.
    Folder(DirKey),
    /// The entry is a value, with this key.
    Value(ValueKey),
}

/// The kind of entry in a [`VirtualFilesystem`].
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum EntryKind {
    /// The entry is a value.
    Value,
    /// The entry is a folder.
    Folder,
    /// The value is missing (but its parent is present).
    Missing,
}

impl<T> VirtualFilesystem<T> {
    /// Creates a new filesystem.
    ///
    /// `allow_above_root` determines whether paths going "outside" the root are allowed using `..`.
    pub fn new(allow_above_root: bool) -> Self {
        let mut dirs = SlotMap::with_key();
        let root_key = dirs.insert(Dir {
            children: Default::default(),
            allow_above: allow_above_root,
        });
        Self {
            root_key,
            dirs,
            values: SlotMap::with_key(),
        }
    }

    /// Normalizes a path by collapsing all occurrences of `.` and `..` segments where possible as
    /// per [RFC 1808](https://datatracker.ietf.org/doc/html/rfc1808).
    pub fn normalize_path(path: &str) -> String {
        let mut result_components = vec![];
        // Note: we split on both / and \\ because of Windows. Otherwise a path stored in an asset
        // from Windows could fail to load on Linux. So we do this regardless of platform.
        for component in path.split(&['/', '\\']) {
            if component == "." {
                // Skip
            } else if component == ".." {
                // Note: If the result_path ends in `..`, Path::file_name returns None, so we'll end up
                // preserving it.
                if let Some(&last_component) = result_components.last()
                    && last_component != ".."
                {
                    // This assert is just a sanity check - we already know the component is present.
                    assert!(result_components.pop().is_some());
                } else {
                    // Preserve ".." if insufficient matches (per RFC 1808).
                    result_components.push(component);
                }
            } else {
                result_components.push(component);
            }
        }

        result_components.join("/")
    }

    /// Gets the kind of entry stored at `path`.
    ///
    /// Returns [`EntryKind::Missing`] if the parent is present, but the child is missing. If the
    /// parent is also missing, returns an error.
    pub fn get_entry_kind(&self, path: &str) -> Result<EntryKind, ()> {
        let (parent_key, child_name) = self.get_parent_and_child_name(path)?;

        let parent_dir = &self.dirs[parent_key];
        Ok(match parent_dir.children.get(child_name) {
            Some(ChildEntry::Folder(_)) => EntryKind::Folder,
            Some(ChildEntry::Value(_)) => EntryKind::Value,
            None => EntryKind::Missing,
        })
    }

    /// Iterates all children of the directory at `path`.
    pub fn iter_directory(&self, path: &str) -> Result<impl ExactSizeIterator<Item = &str>, ()> {
        match self.find_entry(path)? {
            ChildEntry::Value(_) => return Err(()),
            ChildEntry::Folder(dir_key) => Ok(self.dirs[dir_key].children.keys().map(|b| &**b)),
        }
    }

    /// Creates a new, empty directory at `path`.
    pub fn create_directory(&mut self, path: &str) -> Result<(), ()> {
        let (parent_key, child_name) = self.get_parent_and_child_name(path)?;
        let parent_dir = &self.dirs[parent_key];
        Self::check_child_name(child_name, parent_dir.allow_above)?;

        if parent_dir.children.contains_key(child_name) {
            return Err(());
        }

        let dir_key = self.dirs.insert(Dir {
            children: Default::default(),
            // This condition also implies that the parent is `allow_above`, since otherwise
            // check_child_name would have filtered it out.
            allow_above: child_name == "..",
        });

        self.dirs[parent_key]
            .children
            .insert(child_name.into(), ChildEntry::Folder(dir_key));

        Ok(())
    }

    /// Gets the value at `path` if present.
    pub fn get_value(&self, path: &str) -> Result<&T, ()> {
        match self.find_entry(path)? {
            ChildEntry::Folder(_) => Err(()),
            ChildEntry::Value(value_key) => Ok(&self.values[value_key]),
        }
    }

    /// Gets the value mutably at `path`, or a [`MissingValueMut`] if not present.
    pub fn get_value_mut<'f, 'p>(&'f mut self, path: &'p str) -> Result<ValueMut<'f, 'p, T>, ()> {
        let (parent_key, child_name) = self.get_parent_and_child_name(path)?;
        Self::check_child_name(child_name, /*allow_above=*/ false)?;

        let parent_dir = &mut self.dirs[parent_key];
        Ok(match parent_dir.children.entry_ref(child_name) {
            EntryRef::Occupied(entry) => {
                let value_key = match *entry.get() {
                    ChildEntry::Folder(_) => return Err(()),
                    ChildEntry::Value(value_key) => value_key,
                };
                ValueMut::Present(&mut self.values[value_key])
            }
            EntryRef::Vacant(entry) => ValueMut::Missing(MissingValueMut {
                entry,
                values: &mut self.values,
            }),
        })
    }

    /// Deletes the entry at `path`, returning the value if `path` corresponds to a value.
    pub fn delete_entry(&mut self, path: &str) -> Result<Option<T>, ()> {
        let (parent_key, child_name) = self.get_parent_and_child_name(path)?;
        let parent_dir = &mut self.dirs[parent_key];

        let entry = match parent_dir.children.entry_ref(child_name) {
            EntryRef::Vacant(_) => return Err(()),
            EntryRef::Occupied(entry) => entry,
        };

        match *entry.get() {
            ChildEntry::Value(value_key) => {
                entry.remove();
                Ok(self.values.remove(value_key))
            }
            ChildEntry::Folder(child_dir) => {
                if !self.dirs[child_dir].children.is_empty() {
                    return Err(());
                }

                self.dirs.remove(child_dir);
                self.dirs[parent_key].children.remove(child_name);
                Ok(None)
            }
        }
    }

    /// Splits the path into its parent directory (which is then looked up), and the name of the
    /// child.
    fn get_parent_and_child_name<'a>(&self, path: &'a str) -> Result<(DirKey, &'a str), ()> {
        match path.rsplit_once('/') {
            None => Ok((self.root_key, path)),
            Some((parent_path, basename)) => {
                let parent_key = match self.find_entry(parent_path)? {
                    ChildEntry::Value(_) => return Err(()),
                    ChildEntry::Folder(parent_key) => parent_key,
                };
                Ok((parent_key, basename))
            }
        }
    }

    /// Finds the entry key at the given `path`.
    fn find_entry(&self, path: &str) -> Result<ChildEntry, ()> {
        let mut last_entry = ChildEntry::Folder(self.root_key);
        for component in path.split('/') {
            let last_dir = match last_entry {
                ChildEntry::Folder(last_dir) => last_dir,
                ChildEntry::Value(_) => return Err(()),
            };

            let dir = &self.dirs[last_dir];
            match dir.children.get(component) {
                None => return Err(()),
                Some(&entry) => {
                    last_entry = entry;
                }
            }
        }

        Ok(last_entry)
    }

    /// Checks if the `child_name` is a valid child.
    ///
    /// `allow_above` controls whether the child is allowed to be an "above" relative name (`..`).
    fn check_child_name(child_name: &str, allow_above: bool) -> Result<(), ()> {
        if child_name == "" || child_name == "." {
            return Err(());
        }
        if child_name.contains('/') {
            return Err(());
        }

        if !allow_above && child_name == ".." {
            return Err(());
        }

        Ok(())
    }
}

/// Mutable access to a value in a [`VirtualFilesystem`].
pub enum ValueMut<'f, 'p, T> {
    /// The value is present, and the mutable borrow was returned.
    Present(&'f mut T),
    /// The value is missing, and can be inserted.
    Missing(MissingValueMut<'f, 'p, T>),
}

impl<'f, T> ValueMut<'f, '_, T> {
    /// Inserts the value, either replacing the existing value, or inserting this new value into the
    /// filesystem.
    ///
    /// Returns a reference to the value.
    pub fn insert(self, value: T) -> &'f mut T {
        match self {
            Self::Present(stored_value) => {
                *stored_value = value;
                stored_value
            }
            Self::Missing(entry) => entry.insert(value),
        }
    }

    /// Ensures a value exists, by inserting the default if the value is missing.
    pub fn or_insert(self, default: T) -> &'f mut T {
        match self {
            Self::Present(value) => value,
            Self::Missing(entry) => entry.insert(default),
        }
    }
}

impl<'f, T: Default> ValueMut<'f, '_, T> {
    /// Ensures a value exists, by inserting the default if the value is missing.
    pub fn or_default(self) -> &'f mut T {
        match self {
            Self::Present(value) => value,
            Self::Missing(entry) => entry.insert(T::default()),
        }
    }
}

/// A struct to insert a value that was missing.
pub struct MissingValueMut<'f, 'p, T> {
    /// The entry into the parent directory to insert the new value key.
    entry: VacantEntryRef<'f, 'p, Box<str>, str, ChildEntry, FixedHasher>,
    /// The values map where the value will be inserted.
    values: &'f mut SlotMap<ValueKey, T>,
}

impl<'f, T> MissingValueMut<'f, '_, T> {
    /// Inserts a value into the parent directory's slot, and returns a mutable reference to the
    /// value.
    pub fn insert(self, value: T) -> &'f mut T {
        let value_key = self.values.insert(value);
        self.entry.insert(ChildEntry::Value(value_key));

        &mut self.values[value_key]
    }
}

#[derive(Default)]
struct MemoryAsset {
    asset_bytes: Option<Value>,
    meta_bytes: Option<Value>,
}

impl MemoryAsset {
    fn is_empty(&self) -> bool {
        self.asset_bytes.is_none() && self.meta_bytes.is_none()
    }
}

#[derive(Clone)]
pub struct MemoryAssetFilesystem(Arc<RwLock<VirtualFilesystem<MemoryAsset>>>);

impl MemoryAssetFilesystem {
    pub fn new() -> Self {
        Self(Arc::new(RwLock::new(VirtualFilesystem::new(
            /*allow_above_root=*/ true,
        ))))
    }

    pub fn get_asset(&self, path: &Path) -> Option<Value> {
        let path = path.to_str().unwrap();

        let vfs = self.0.read().unwrap_or_else(PoisonError::into_inner);
        let value = vfs.get_value(path).ok()?;
        value.asset_bytes.clone()
    }

    pub fn get_metadata(&self, path: &Path) -> Option<Value> {
        let path = path.to_str().unwrap();

        let vfs = self.0.read().unwrap_or_else(PoisonError::into_inner);
        let value = vfs.get_value(path).ok()?;
        value.meta_bytes.clone()
    }

    pub fn get_children(&self, path: &Path) -> Result<Vec<String>, ()> {
        let path = path.to_str().unwrap();

        let vfs = self.0.read().unwrap_or_else(PoisonError::into_inner);
        let children = vfs.iter_directory(path)?;
        Ok(children.map(ToOwned::to_owned).collect())
    }

    pub fn is_directory(&self, path: &Path) -> Result<bool, ()> {
        let path = path.to_str().unwrap();

        let vfs = self.0.read().unwrap_or_else(PoisonError::into_inner);
        match vfs.get_entry_kind(path)? {
            EntryKind::Value => Ok(false),
            EntryKind::Folder => Ok(true),
            EntryKind::Missing => return Err(()),
        }
    }

    pub fn insert_asset_text(&self, path: &Path, asset: &str) {
        self.insert_asset(path, asset.as_bytes().to_vec());
    }

    pub fn insert_meta_text(&self, path: &Path, value: &str) {
        self.insert_meta(path, value.as_bytes().to_vec());
    }

    pub fn insert_asset(&self, path: &Path, value: impl Into<Value>) {
        let path = path.to_str().unwrap();

        let mut vfs = self.0.write().unwrap_or_else(PoisonError::into_inner);
        let memory_asset = match vfs.get_value_mut(path) {
            Ok(ValueMut::Present(memory_asset)) => memory_asset,
            Ok(ValueMut::Missing(missing)) => missing.insert(MemoryAsset::default()),
            Err(err) => panic!("Failed to find (or create) value entry: {err:?}"),
        };

        memory_asset.asset_bytes = Some(value.into());
    }

    pub fn remove_asset(&self, path: &Path) -> Option<Value> {
        let path = path.to_str().unwrap();

        let mut vfs = self.0.write().unwrap_or_else(PoisonError::into_inner);
        let memory_asset = match vfs.get_value_mut(path) {
            Ok(ValueMut::Present(memory_asset)) => memory_asset,
            Ok(ValueMut::Missing(_)) => return None,
            Err(err) => panic!("Failed to find value entry: {err:?}"),
        };

        memory_asset.asset_bytes.take()
    }

    pub fn insert_meta(&self, path: &Path, value: impl Into<Value>) {
        let path = path.to_str().unwrap();

        let mut vfs = self.0.write().unwrap_or_else(PoisonError::into_inner);
        let memory_asset = match vfs.get_value_mut(path) {
            Ok(ValueMut::Present(memory_asset)) => memory_asset,
            Ok(ValueMut::Missing(missing)) => missing.insert(MemoryAsset::default()),
            Err(err) => panic!("Failed to find value (or create) value entry: {err:?}"),
        };

        memory_asset.meta_bytes = Some(value.into());
    }

    pub fn remove_metadata(&self, path: &Path) -> Option<Value> {
        let path = path.to_str().unwrap();

        let mut vfs = self.0.write().unwrap_or_else(PoisonError::into_inner);
        let memory_asset = match vfs.get_value_mut(path) {
            Ok(ValueMut::Present(memory_asset)) => memory_asset,
            Ok(ValueMut::Missing(_)) => return None,
            Err(err) => panic!("Failed to find value entry: {err:?}"),
        };

        memory_asset.meta_bytes.take()
    }

    pub fn create_dir(&self, path: &Path) {
        let path = path.to_str().unwrap();

        let mut vfs = self.0.write().unwrap_or_else(PoisonError::into_inner);
        vfs.create_directory(path).unwrap();
    }

    pub fn remove_dir(&self, path: &Path) {
        let path = path.to_str().unwrap();
        let mut vfs = self.0.write().unwrap_or_else(PoisonError::into_inner);
        if vfs.get_value(path).is_ok() {
            return;
        }
        vfs.delete_entry(path).unwrap();
    }
}

/// In-memory [`AssetReader`] implementation.
///
/// This is primarily used by unit tests and the [`embedded`](super::embedded) backend.
#[derive(Clone)]
pub struct MemoryAssetReader {
    /// The root of the in-memory filesystem backing this asset reader.
    pub root: MemoryAssetFilesystem,
}

/// In-memory [`AssetWriter`] implementation.
///
/// This is primarily used by unit tests and the [`embedded`](super::embedded) backend.
#[derive(Clone)]
pub struct MemoryAssetWriter {
    /// The root of the in-memory filesystem backing this asset writer.
    pub root: MemoryAssetFilesystem,
}

/// Stores either an allocated vec of bytes or a static array of bytes.
#[derive(Clone, Debug)]
pub enum Value {
    Vec(Arc<Vec<u8>>),
    Static(&'static [u8]),
}

impl From<Vec<u8>> for Value {
    fn from(value: Vec<u8>) -> Self {
        Self::Vec(Arc::new(value))
    }
}

impl From<&'static [u8]> for Value {
    fn from(value: &'static [u8]) -> Self {
        Self::Static(value)
    }
}

impl<const N: usize> From<&'static [u8; N]> for Value {
    fn from(value: &'static [u8; N]) -> Self {
        Self::Static(value)
    }
}

impl Value {
    pub fn bytes(&self) -> &[u8] {
        match self {
            Self::Static(bytes) => bytes,
            Self::Vec(bytes) => bytes,
        }
    }
}

struct ValueReader {
    value: Value,
    bytes_read: usize,
}

impl AsyncRead for ValueReader {
    fn poll_read(
        self: Pin<&mut Self>,
        _cx: &mut core::task::Context<'_>,
        buf: &mut [u8],
    ) -> Poll<futures_io::Result<usize>> {
        // Get the mut borrow to avoid trying to borrow the pin itself multiple times.
        let this = self.get_mut();
        Poll::Ready(Ok(crate::io::slice_read(
            this.value.bytes(),
            &mut this.bytes_read,
            buf,
        )))
    }
}

impl AsyncSeek for ValueReader {
    fn poll_seek(
        self: Pin<&mut Self>,
        _cx: &mut core::task::Context<'_>,
        pos: SeekFrom,
    ) -> Poll<std::io::Result<u64>> {
        // Get the mut borrow to avoid trying to borrow the pin itself multiple times.
        let this = self.get_mut();
        Poll::Ready(crate::io::slice_seek(
            this.value.bytes(),
            &mut this.bytes_read,
            pos,
        ))
    }
}

impl Reader for ValueReader {
    fn read_to_end<'a>(
        &'a mut self,
        buf: &'a mut Vec<u8>,
    ) -> stackfuture::StackFuture<'a, std::io::Result<usize>, { super::STACK_FUTURE_SIZE }> {
        crate::io::read_to_end(self.value.bytes(), &mut self.bytes_read, buf)
    }

    fn seekable(&mut self) -> Result<&mut dyn SeekableReader, ReaderNotSeekableError> {
        Ok(self)
    }
}

fn normalize_virtual_path_with_io_error(path: &Path) -> Result<PathBuf, Error> {
    match VirtualFilesystem::<()>::path_to_virtual_path(path) {
        Ok(path) => Ok(PathBuf::from(path)),
        Err(err) => Err(Error::new(ErrorKind::InvalidFilename, format!("{err:?}"))),
    }
}

impl AssetReader for MemoryAssetReader {
    async fn read<'a>(&'a self, path: &'a Path) -> Result<impl Reader + 'a, AssetReaderError> {
        let virtual_path = normalize_virtual_path_with_io_error(path)?;
        match self.root.get_asset(&virtual_path) {
            Some(value) => Ok(ValueReader {
                value,
                bytes_read: 0,
            }),
            None => Err(AssetReaderError::NotFound(path.to_path_buf())),
        }
    }

    async fn read_meta<'a>(&'a self, path: &'a Path) -> Result<impl Reader + 'a, AssetReaderError> {
        let virtual_path = normalize_virtual_path_with_io_error(path)?;
        match self.root.get_metadata(&virtual_path) {
            Some(value) => Ok(ValueReader {
                value,
                bytes_read: 0,
            }),
            None => Err(AssetReaderError::NotFound(path.to_path_buf())),
        }
    }

    async fn read_directory<'a>(
        &'a self,
        path: &'a Path,
    ) -> Result<Box<PathStream>, AssetReaderError> {
        let virtual_path = normalize_virtual_path_with_io_error(path)?;
        match self.root.get_children(&virtual_path) {
            Ok(children) => Ok(Box::new(futures_util::stream::iter(
                children
                    .into_iter()
                    .map(|child_name| path.join(child_name))
                    .collect::<Vec<_>>(),
            ))),
            Err(()) => Err(AssetReaderError::Io(todo!())),
        }
    }

    async fn is_directory<'a>(&'a self, path: &'a Path) -> Result<bool, AssetReaderError> {
        let virtual_path = normalize_virtual_path_with_io_error(path)?;
        match self.root.is_directory(&virtual_path) {
            Ok(is_directory) => Ok(is_directory),
            Err(err) => Err(AssetReaderError::Io(todo!())),
        }
    }
}

/// A writer that writes into [`Dir`], buffering internally until flushed/closed.
struct DataWriter {
    /// The filesystem to write to.
    fs: MemoryAssetFilesystem,
    /// The path to write to.
    path: PathBuf,
    /// The current buffer of data.
    ///
    /// This will include data that has been flushed already.
    current_data: Vec<u8>,
    /// Whether to write to the data or to the meta.
    is_meta_writer: bool,
}

impl AsyncWrite for DataWriter {
    fn poll_write(
        self: Pin<&mut Self>,
        _: &mut core::task::Context<'_>,
        buf: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        self.get_mut().current_data.extend_from_slice(buf);
        Poll::Ready(Ok(buf.len()))
    }

    fn poll_flush(
        self: Pin<&mut Self>,
        _: &mut core::task::Context<'_>,
    ) -> Poll<std::io::Result<()>> {
        // Write the data to our fake disk. This means we will repeatedly reinsert the asset.
        if self.is_meta_writer {
            self.fs.insert_meta(&self.path, self.current_data.clone());
        } else {
            self.fs.insert_asset(&self.path, self.current_data.clone());
        }
        Poll::Ready(Ok(()))
    }

    fn poll_close(
        self: Pin<&mut Self>,
        cx: &mut core::task::Context<'_>,
    ) -> Poll<std::io::Result<()>> {
        // A flush will just write the data to Dir, which is all we need to do for close.
        self.poll_flush(cx)
    }
}

impl AssetWriter for MemoryAssetWriter {
    async fn write<'a>(&'a self, path: &'a Path) -> Result<Box<super::Writer>, AssetWriterError> {
        let virtual_path = normalize_virtual_path_with_io_error(path)?;
        Ok(Box::new(DataWriter {
            fs: self.root.clone(),
            path: virtual_path,
            current_data: vec![],
            is_meta_writer: false,
        }))
    }

    async fn write_meta<'a>(
        &'a self,
        path: &'a Path,
    ) -> Result<Box<super::Writer>, AssetWriterError> {
        let virtual_path = normalize_virtual_path_with_io_error(path)?;
        Ok(Box::new(DataWriter {
            fs: self.root.clone(),
            path: virtual_path,
            current_data: vec![],
            is_meta_writer: true,
        }))
    }

    async fn remove<'a>(&'a self, path: &'a Path) -> Result<(), AssetWriterError> {
        let virtual_path = normalize_virtual_path_with_io_error(path)?;
        match self.root.remove_asset(&virtual_path) {
            Some(_) => Ok(()),
            None => todo!(),
        }
    }

    async fn remove_meta<'a>(&'a self, path: &'a Path) -> Result<(), AssetWriterError> {
        let virtual_path = normalize_virtual_path_with_io_error(path)?;
        self.root.remove_metadata(&virtual_path);
        Ok(())
    }

    async fn rename<'a>(
        &'a self,
        old_path: &'a Path,
        new_path: &'a Path,
    ) -> Result<(), AssetWriterError> {
        let Some(old_asset) = self.root.get_asset(old_path) else {
            return Err(AssetWriterError::Io(Error::new(
                ErrorKind::NotFound,
                "no such file",
            )));
        };
        self.root.insert_asset(new_path, old_asset);
        // Remove the asset after instead of before since otherwise there'd be a moment where the
        // Dir is unlocked and missing both the old and new paths. This just prevents race
        // conditions.
        self.root.remove_asset(old_path);
        Ok(())
    }

    async fn rename_meta<'a>(
        &'a self,
        old_path: &'a Path,
        new_path: &'a Path,
    ) -> Result<(), AssetWriterError> {
        let Some(old_meta) = self.root.get_metadata(old_path) else {
            return Err(AssetWriterError::Io(Error::new(
                ErrorKind::NotFound,
                "no such file",
            )));
        };
        self.root.insert_meta(new_path, old_meta);
        // Remove the meta after instead of before since otherwise there'd be a moment where the
        // Dir is unlocked and missing both the old and new paths. This just prevents race
        // conditions.
        self.root.remove_metadata(old_path);
        Ok(())
    }

    async fn create_directory<'a>(&'a self, path: &'a Path) -> Result<(), AssetWriterError> {
        // Just pretend we're on a file system that doesn't consider directory re-creation a
        // failure.
        self.root.get_or_insert_dir(path);
        Ok(())
    }

    async fn remove_directory<'a>(&'a self, path: &'a Path) -> Result<(), AssetWriterError> {
        if self.root.remove_dir(path).is_none() {
            return Err(AssetWriterError::Io(Error::new(
                ErrorKind::NotFound,
                "no such dir",
            )));
        }
        Ok(())
    }

    async fn remove_empty_directory<'a>(&'a self, path: &'a Path) -> Result<(), AssetWriterError> {
        let Some(dir) = self.root.get_dir(path) else {
            return Err(AssetWriterError::Io(Error::new(
                ErrorKind::NotFound,
                "no such dir",
            )));
        };

        let dir = dir.0.read().unwrap();
        if !dir.assets.is_empty() || !dir.metadata.is_empty() || !dir.dirs.is_empty() {
            return Err(AssetWriterError::Io(Error::new(
                ErrorKind::DirectoryNotEmpty,
                "not empty",
            )));
        }

        self.root.remove_dir(path);
        Ok(())
    }

    async fn remove_assets_in_directory<'a>(
        &'a self,
        path: &'a Path,
    ) -> Result<(), AssetWriterError> {
        let Some(dir) = self.root.get_dir(path) else {
            return Err(AssetWriterError::Io(Error::new(
                ErrorKind::NotFound,
                "no such dir",
            )));
        };

        let mut dir = dir.0.write().unwrap();
        dir.assets.clear();
        dir.dirs.clear();
        dir.metadata.clear();
        Ok(())
    }
}

#[cfg(test)]
mod test {
    use crate::io::memory::MemoryAssetFilesystem;

    use std::path::Path;

    #[test]
    fn memory_dir() {
        let dir = MemoryAssetFilesystem::default();
        let a_path = Path::new("a.txt");
        let a_data = "a".as_bytes().to_vec();
        let a_meta = "ameta".as_bytes().to_vec();

        dir.insert_asset(a_path, a_data.clone());
        let asset = dir.get_asset(a_path).unwrap();
        assert_eq!(asset.path(), a_path);
        assert_eq!(asset.value(), a_data);

        dir.insert_meta(a_path, a_meta.clone());
        let meta = dir.get_metadata(a_path).unwrap();
        assert_eq!(meta.path(), a_path);
        assert_eq!(meta.value(), a_meta);

        let b_path = Path::new("x/y/b.txt");
        let b_data = "b".as_bytes().to_vec();
        let b_meta = "meta".as_bytes().to_vec();
        dir.insert_asset(b_path, b_data.clone());
        dir.insert_meta(b_path, b_meta.clone());

        let asset = dir.get_asset(b_path).unwrap();
        assert_eq!(asset.path(), b_path);
        assert_eq!(asset.value(), b_data);

        let meta = dir.get_metadata(b_path).unwrap();
        assert_eq!(meta.path(), b_path);
        assert_eq!(meta.value(), b_meta);
    }
}
