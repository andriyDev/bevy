#[cfg(not(target_os = "windows"))]
use async_io::Timer;
#[cfg(not(any(target_os = "macos", target_os = "ios", target_os = "windows")))]
use async_lock::Semaphore;
#[cfg(not(target_os = "windows"))]
use async_lock::SemaphoreGuard;
use futures_io::{AsyncRead, AsyncWrite};
use futures_lite::Stream;
#[cfg(not(target_os = "windows"))]
use futures_util::{future, pin_mut};

use crate::io::{
    get_meta_path, AssetReader, AssetReaderError, AssetWriter, AssetWriterError, AsyncSeek,
    PathStream, Reader, ReaderNotSeekableError, SeekableReader, Writer,
};

use alloc::{borrow::ToOwned, boxed::Box, vec::Vec};
#[cfg(not(target_os = "windows"))]
use core::time::Duration;
use core::{pin::Pin, task::Poll};
use std::{
    fs::{read_dir, File},
    io::{Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
};

use super::{FileAssetReader, FileAssetWriter};

// Set to OS default limit / 2
// macos & ios: 256
// linux & android: 1024
#[cfg(any(target_os = "macos", target_os = "ios"))]
static OPEN_FILE_LIMITER: Semaphore = Semaphore::new(128);
#[cfg(not(any(target_os = "macos", target_os = "ios", target_os = "windows")))]
static OPEN_FILE_LIMITER: Semaphore = Semaphore::new(512);

#[cfg(not(target_os = "windows"))]
async fn maybe_get_semaphore() -> Option<SemaphoreGuard<'static>> {
    let guard_future = OPEN_FILE_LIMITER.acquire();
    let timeout_future = Timer::after(Duration::from_millis(500));
    pin_mut!(guard_future);
    pin_mut!(timeout_future);

    match future::select(guard_future, timeout_future).await {
        future::Either::Left((guard, _)) => Some(guard),
        future::Either::Right((_, _)) => None,
    }
}

pub struct GuardedFile {
    file: File,
    #[cfg(not(target_os = "windows"))]
    _guard: Option<SemaphoreGuard<'static>>,
}

impl GuardedFile {
    pub async fn open(path: &Path) -> Result<Self, AssetReaderError> {
        #[cfg(not(target_os = "windows"))]
        let guard = maybe_get_semaphore().await;

        let file = match File::open(path) {
            Ok(file) => file,
            Err(err) => {
                return if err.kind() == std::io::ErrorKind::NotFound {
                    Err(AssetReaderError::NotFound(path.to_path_buf()))
                } else {
                    Err(err.into())
                };
            }
        };
        Ok(Self {
            file,
            #[cfg(not(target_os = "windows"))]
            _guard: guard,
        })
    }

    pub async fn create(path: &Path) -> Result<Self, AssetWriterError> {
        #[cfg(not(target_os = "windows"))]
        let guard = maybe_get_semaphore().await;

        let file = File::create(path)?;
        Ok(Self {
            file,
            #[cfg(not(target_os = "windows"))]
            _guard: guard,
        })
    }
}

impl AsyncRead for GuardedFile {
    fn poll_read(
        self: Pin<&mut Self>,
        _cx: &mut core::task::Context<'_>,
        buf: &mut [u8],
    ) -> Poll<Result<usize, std::io::Error>> {
        let read = self.get_mut().file.read(buf);
        Poll::Ready(read)
    }
}

impl AsyncSeek for GuardedFile {
    fn poll_seek(
        mut self: Pin<&mut Self>,
        _cx: &mut core::task::Context<'_>,
        pos: SeekFrom,
    ) -> Poll<std::io::Result<u64>> {
        Poll::Ready(self.file.seek(pos))
    }
}

impl Reader for GuardedFile {
    fn seekable(&mut self) -> Result<&mut dyn SeekableReader, ReaderNotSeekableError> {
        Ok(self)
    }
}

impl AsyncWrite for GuardedFile {
    fn poll_write(
        self: Pin<&mut Self>,
        _cx: &mut core::task::Context<'_>,
        buf: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        let wrote = self.get_mut().file.write(buf);
        Poll::Ready(wrote)
    }

    fn poll_flush(
        self: Pin<&mut Self>,
        _cx: &mut core::task::Context<'_>,
    ) -> Poll<std::io::Result<()>> {
        let flushed = self.get_mut().file.flush();
        Poll::Ready(flushed)
    }

    fn poll_close(
        self: Pin<&mut Self>,
        _cx: &mut core::task::Context<'_>,
    ) -> Poll<std::io::Result<()>> {
        Poll::Ready(Ok(()))
    }
}

struct DirReader(Vec<PathBuf>);

impl Stream for DirReader {
    type Item = PathBuf;

    fn poll_next(
        self: Pin<&mut Self>,
        _cx: &mut core::task::Context<'_>,
    ) -> Poll<Option<Self::Item>> {
        let this = self.get_mut();
        Poll::Ready(this.0.pop())
    }
}

impl AssetReader for FileAssetReader {
    async fn read<'a>(&'a self, path: &'a Path) -> Result<impl Reader + 'a, AssetReaderError> {
        let full_path = self.root_path.join(path);
        GuardedFile::open(&full_path).await
    }

    async fn read_meta<'a>(&'a self, path: &'a Path) -> Result<impl Reader + 'a, AssetReaderError> {
        let meta_path = get_meta_path(path);
        let full_path = self.root_path.join(meta_path);
        GuardedFile::open(&full_path).await
    }

    async fn read_directory<'a>(
        &'a self,
        path: &'a Path,
    ) -> Result<Box<PathStream>, AssetReaderError> {
        let full_path = self.root_path.join(path);
        match read_dir(&full_path) {
            Ok(read_dir) => {
                let root_path = self.root_path.clone();
                let mapped_stream = read_dir.filter_map(move |f| {
                    f.ok().and_then(|dir_entry| {
                        let path = dir_entry.path();
                        // filter out meta files as they are not considered assets
                        if let Some(ext) = path.extension().and_then(|e| e.to_str())
                            && ext.eq_ignore_ascii_case("meta")
                        {
                            return None;
                        }
                        // filter out hidden files. they are not listed by default but are directly targetable
                        if path
                            .file_name()
                            .and_then(|file_name| file_name.to_str())
                            .map(|file_name| file_name.starts_with('.'))
                            .unwrap_or_default()
                        {
                            return None;
                        }

                        let relative_path = path.strip_prefix(&root_path).unwrap();
                        Some(relative_path.to_owned())
                    })
                });
                let read_dir: Box<PathStream> = Box::new(DirReader(mapped_stream.collect()));
                Ok(read_dir)
            }
            Err(e) => {
                if e.kind() == std::io::ErrorKind::NotFound {
                    Err(AssetReaderError::NotFound(full_path))
                } else {
                    Err(e.into())
                }
            }
        }
    }

    async fn is_directory<'a>(&'a self, path: &'a Path) -> Result<bool, AssetReaderError> {
        let full_path = self.root_path.join(path);
        let metadata = full_path
            .metadata()
            .map_err(|_e| AssetReaderError::NotFound(path.to_owned()))?;
        Ok(metadata.file_type().is_dir())
    }
}

impl AssetWriter for FileAssetWriter {
    async fn write<'a>(&'a self, path: &'a Path) -> Result<Box<Writer>, AssetWriterError> {
        let full_path = self.root_path.join(path);
        if let Some(parent) = full_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let file = GuardedFile::create(&full_path).await?;
        Ok(Box::new(file))
    }

    async fn write_meta<'a>(&'a self, path: &'a Path) -> Result<Box<Writer>, AssetWriterError> {
        let meta_path = get_meta_path(path);
        let full_path = self.root_path.join(meta_path);
        if let Some(parent) = full_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let file = GuardedFile::create(&full_path).await?;
        Ok(Box::new(file))
    }

    async fn remove<'a>(&'a self, path: &'a Path) -> Result<(), AssetWriterError> {
        let full_path = self.root_path.join(path);
        std::fs::remove_file(full_path)?;
        Ok(())
    }

    async fn remove_meta<'a>(&'a self, path: &'a Path) -> Result<(), AssetWriterError> {
        let meta_path = get_meta_path(path);
        let full_path = self.root_path.join(meta_path);
        std::fs::remove_file(full_path)?;
        Ok(())
    }

    async fn create_directory<'a>(&'a self, path: &'a Path) -> Result<(), AssetWriterError> {
        let full_path = self.root_path.join(path);
        std::fs::create_dir_all(full_path)?;
        Ok(())
    }

    async fn remove_directory<'a>(&'a self, path: &'a Path) -> Result<(), AssetWriterError> {
        let full_path = self.root_path.join(path);
        std::fs::remove_dir_all(full_path)?;
        Ok(())
    }

    async fn remove_empty_directory<'a>(&'a self, path: &'a Path) -> Result<(), AssetWriterError> {
        let full_path = self.root_path.join(path);
        std::fs::remove_dir(full_path)?;
        Ok(())
    }

    async fn remove_assets_in_directory<'a>(
        &'a self,
        path: &'a Path,
    ) -> Result<(), AssetWriterError> {
        let full_path = self.root_path.join(path);
        std::fs::remove_dir_all(&full_path)?;
        std::fs::create_dir_all(&full_path)?;
        Ok(())
    }

    async fn rename<'a>(
        &'a self,
        old_path: &'a Path,
        new_path: &'a Path,
    ) -> Result<(), AssetWriterError> {
        let full_old_path = self.root_path.join(old_path);
        let full_new_path = self.root_path.join(new_path);
        if let Some(parent) = full_new_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::rename(full_old_path, full_new_path)?;
        Ok(())
    }

    async fn rename_meta<'a>(
        &'a self,
        old_path: &'a Path,
        new_path: &'a Path,
    ) -> Result<(), AssetWriterError> {
        let old_meta_path = get_meta_path(old_path);
        let new_meta_path = get_meta_path(new_path);
        let full_old_path = self.root_path.join(old_meta_path);
        let full_new_path = self.root_path.join(new_meta_path);
        if let Some(parent) = full_new_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::rename(full_old_path, full_new_path)?;
        Ok(())
    }
}
