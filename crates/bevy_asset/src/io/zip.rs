use alloc::{
    boxed::Box,
    string::{String, ToString},
    sync::Arc,
    vec,
    vec::Vec,
};
use async_lock::{RwLock, RwLockReadGuard};
use bevy_platform::collections::HashMap;
use futures_io::AsyncRead;
use futures_util::stream::iter;
use self_cell::{self_cell, MutBorrow};
use thiserror::Error;

use core::task::Poll;
use std::{
    fs::File,
    io::{ErrorKind, Read},
    path::{Path, PathBuf},
};

use zip::{
    read::{ZipArchiveMetadata, ZipFile},
    result::ZipError,
    ZipArchive,
};

use crate::io::{
    get_meta_path, AssetReader, AssetReaderError, PathStream, Reader, ReaderNotSeekableError,
    SeekableReader,
};

pub struct ZipAssetReader {
    /// The file path where the archive resides.
    path: PathBuf,
    /// The metadata about the archive that has previously been read.
    ///
    /// This starts at [`None`]. When an archive is first read, we populate this metadata.
    /// Subsequent reads use that existing metadata.
    metadata: RwLock<Option<ZipAllMetadata>>,
}

impl ZipAssetReader {
    pub fn new(path: PathBuf) -> Self {
        Self {
            path,
            metadata: RwLock::new(None),
        }
    }

    /// Creates an archive from this reader.
    async fn create_archive(&self) -> Result<ZipArchive<File>, AssetReaderError> {
        let file = File::open(&self.path)?;
        if let Some(metadata) = self.metadata.read().await.as_ref() {
            let metadata = metadata.archive_metadata.clone();
            // Fast path - use the metadata to create the archive.
            // SAFETY: The metadata is present, so it must be up to date with the file.
            #[expect(
                unsafe_code,
                reason = "this unsafe lets us avoid reading the zip metadata for every individual asset read"
            )]
            return Ok(unsafe { ZipArchive::unsafe_new_with_metadata(file, metadata) });
        }

        // Slow path - read the metadata from the file, and try to update the metadata.
        let mut metadata = self.metadata.write().await;
        if let Some(metadata) = metadata.as_ref() {
            let metadata = metadata.archive_metadata.clone();
            // Someone raced us to updating the metadata. Just create the archive from it.
            // SAFETY: Another task updated the metadata to match the file, so it must be in sync.
            #[expect(
                unsafe_code,
                reason = "this unsafe lets us avoid reading the zip metadata for every individual asset read"
            )]
            return Ok(unsafe { ZipArchive::unsafe_new_with_metadata(file, metadata) });
        }

        let archive = ZipArchive::new(file).map_err(|err| {
            AssetReaderError::Io(Arc::new(std::io::Error::new(ErrorKind::InvalidData, err)))
        })?;
        *metadata = Some(ZipAllMetadata::new(&archive).map_err(|err| {
            AssetReaderError::Io(Arc::new(std::io::Error::new(ErrorKind::InvalidData, err)))
        })?);
        Ok(archive)
    }

    async fn get_metadata(
        &self,
    ) -> Result<RwLockReadGuard<'_, Option<ZipAllMetadata>>, AssetReaderError> {
        loop {
            let metadata = self.metadata.read().await;
            if metadata.is_some() {
                break Ok(metadata);
            } else {
                drop(metadata);
                self.create_archive().await?;
            }
        }
    }
}

impl AssetReader for ZipAssetReader {
    async fn read<'a>(&'a self, path: &'a Path) -> Result<impl Reader + 'a, AssetReaderError> {
        let archive = self.create_archive().await?;
        ZipReader::new(archive, path)
    }

    async fn read_meta<'a>(&'a self, path: &'a Path) -> Result<impl Reader + 'a, AssetReaderError> {
        let archive = self.create_archive().await?;
        ZipReader::new(archive, &get_meta_path(path))
    }

    async fn read_directory<'a>(
        &'a self,
        path: &'a Path,
    ) -> Result<Box<PathStream>, AssetReaderError> {
        let metadata = self.get_metadata().await?;
        let metadata = metadata.as_ref().unwrap();
        metadata.read_directory(path)
    }

    async fn is_directory<'a>(&'a self, path: &'a Path) -> Result<bool, AssetReaderError> {
        let metadata = self.get_metadata().await?;
        let metadata = metadata.as_ref().unwrap();
        metadata.is_directory(path)
    }
}

struct ZipAllMetadata {
    archive_metadata: Arc<ZipArchiveMetadata>,
    file_tree: Vec<FileNode>,
}

impl ZipAllMetadata {
    fn new(archive: &ZipArchive<File>) -> Result<Self, ZipIndexingError> {
        let mut nodes = vec![FileNode::Directory(HashMap::default())];

        for mut path in archive.file_names() {
            let mut is_dir = false;
            if path.ends_with('/') {
                path = &path[..(path.len() - 1)];
                is_dir = true;
            }

            let (basename, parent_entry) = if let Some((parent, basename)) = path.rsplit_once('/') {
                let parent_path = parent.split("/");
                let mut current_entry = 0usize;
                for component in parent_path {
                    match &nodes[current_entry] {
                        FileNode::File => {
                            return Err(ZipIndexingError::ParentIsAFile(path.to_string()));
                        }
                        FileNode::Directory(children) => {
                            let child = *children.get(component).ok_or_else(|| {
                                ZipIndexingError::ParentIsMissing(path.to_string())
                            })?;
                            current_entry = child;
                        }
                    }
                }
                (basename, current_entry)
            } else {
                (path, 0)
            };

            let new_index = nodes.len();
            match &mut nodes[parent_entry] {
                FileNode::File => {
                    return Err(ZipIndexingError::ParentIsAFile(path.to_string()));
                }
                FileNode::Directory(children) => {
                    if children.insert(basename.to_string(), new_index).is_some() {
                        return Err(ZipIndexingError::DuplicateFile(path.to_string()));
                    }
                    nodes.push(if is_dir {
                        FileNode::Directory(Default::default())
                    } else {
                        FileNode::File
                    })
                }
            }
        }

        Ok(ZipAllMetadata {
            archive_metadata: archive.metadata(),
            file_tree: nodes,
        })
    }

    fn find_entry(&self, path: &Path) -> Result<usize, AssetReaderError> {
        let mut current_entry = 0usize;
        for component in path.components() {
            let Some(component) = component.as_os_str().to_str() else {
                return Err(AssetReaderError::NotFound(path.to_path_buf()));
            };
            match &self.file_tree[current_entry] {
                FileNode::File => return Err(AssetReaderError::NotFound(path.to_path_buf())),
                FileNode::Directory(children) => {
                    let Some(&child) = children.get(component) else {
                        return Err(AssetReaderError::NotFound(path.to_path_buf()));
                    };

                    current_entry = child;
                }
            }
        }
        Ok(current_entry)
    }

    fn read_directory<'a>(&'a self, path: &'a Path) -> Result<Box<PathStream>, AssetReaderError> {
        let entry_index = self.find_entry(path)?;
        match &self.file_tree[entry_index] {
            FileNode::File => Err(AssetReaderError::NotFound(path.to_path_buf())),
            FileNode::Directory(children) => Ok(Box::new(iter(
                children
                    .keys()
                    .map(|basename| path.join(basename))
                    .collect::<Vec<_>>()
                    .into_iter(),
            ))),
        }
    }

    fn is_directory(&self, path: &Path) -> Result<bool, AssetReaderError> {
        let entry_index = self.find_entry(path)?;
        match &self.file_tree[entry_index] {
            FileNode::File => Ok(false),
            FileNode::Directory(_) => Ok(true),
        }
    }
}

enum FileNode {
    Directory(HashMap<String, usize>),
    File,
}

type FileZipFile<'a> = ZipFile<'a, File>;

self_cell!(
    struct ZipReaderInner {
        owner: MutBorrow<ZipArchive<File>>,

        #[covariant]
        dependent: FileZipFile,
    }
);

struct ZipReader {
    inner: ZipReaderInner,
}

impl ZipReader {
    fn new(archive: ZipArchive<File>, path: &Path) -> Result<Self, AssetReaderError> {
        Ok(Self {
            inner: ZipReaderInner::try_new(MutBorrow::new(archive), |archive| {
                archive.borrow_mut().by_path(path).map_err(|err| match err {
                    ZipError::FileNotFound => AssetReaderError::NotFound(path.to_path_buf()),
                    ZipError::Io(io) => AssetReaderError::Io(io.into()),
                    err => AssetReaderError::Io(Arc::new(std::io::Error::new(
                        ErrorKind::InvalidData,
                        err,
                    ))),
                })
            })?,
        })
    }
}

impl AsyncRead for ZipReader {
    fn poll_read(
        mut self: core::pin::Pin<&mut Self>,
        _cx: &mut core::task::Context<'_>,
        buf: &mut [u8],
    ) -> Poll<std::io::Result<usize>> {
        Poll::Ready(self.inner.with_dependent_mut(|_, file| file.read(buf)))
    }
}

impl Reader for ZipReader {
    fn seekable(&mut self) -> Result<&mut dyn SeekableReader, ReaderNotSeekableError> {
        Err(ReaderNotSeekableError)
    }
}

#[derive(Error, Debug)]
enum ZipIndexingError {
    #[error("a parent of the path \"{0}\" is actually a file")]
    ParentIsAFile(String),
    #[error("a parent of the path \"{0}\" is misssing")]
    ParentIsMissing(String),
    #[error("a duplicate file path \"{0}\" was present")]
    DuplicateFile(String),
}
