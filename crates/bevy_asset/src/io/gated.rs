use crate::io::{AssetReader, AssetReaderError, Reader};
use alloc::{boxed::Box, sync::Arc};
use async_channel::{Receiver, Sender};
use bevy_platform::{collections::HashMap, sync::RwLock};
use std::{path::Path, sync::PoisonError};

/// A "gated" reader that will prevent asset reads from returning until
/// a given path has been "opened" using [`GateOpener`].
///
/// This is built primarily for unit tests.
pub struct GatedReader<R: AssetReader> {
    reader: R,
    gates: Arc<RwLock<HashMap<Box<Path>, (Sender<()>, Receiver<()>)>>>,
}

impl<R: AssetReader + Clone> Clone for GatedReader<R> {
    fn clone(&self) -> Self {
        Self {
            reader: self.reader.clone(),
            gates: self.gates.clone(),
        }
    }
}

/// Opens path "gates" for a [`GatedReader`].
pub struct GateOpener {
    gates: Arc<RwLock<HashMap<Box<Path>, (Sender<()>, Receiver<()>)>>>,
}

impl GateOpener {
    /// Opens the `path` "gate", allowing a _single_ [`AssetReader`] operation to return for that path.
    /// If multiple operations are expected, call `open` the expected number of calls.
    pub fn open<P: AsRef<Path>>(&self, path: P) {
        let mut gates = self.gates.write().unwrap_or_else(PoisonError::into_inner);
        let gates = gates
            .entry_ref(path.as_ref())
            .or_insert_with(async_channel::unbounded);
        gates.0.send_blocking(()).unwrap();
    }
}

impl<R: AssetReader> GatedReader<R> {
    /// Creates a new [`GatedReader`], which wraps the given `reader`. Also returns a [`GateOpener`] which
    /// can be used to open "path gates" for this [`GatedReader`].
    pub fn new(reader: R) -> (Self, GateOpener) {
        let gates = Arc::new(RwLock::new(HashMap::default()));
        (
            Self {
                reader,
                gates: gates.clone(),
            },
            GateOpener { gates },
        )
    }
}

impl<R: AssetReader> AssetReader for GatedReader<R> {
    async fn read_meta_info(&self) -> Result<super::SourceMetaInfo, AssetReaderError> {
        self.reader.read_meta_info().await
    }

    async fn read<'a>(&'a self, path: &'a Path) -> Result<impl Reader + 'a, AssetReaderError> {
        let receiver = {
            let mut gates = self.gates.write().unwrap_or_else(PoisonError::into_inner);
            let gates = gates
                .entry_ref(path.as_ref())
                .or_insert_with(async_channel::unbounded);
            gates.1.clone()
        };
        receiver.recv().await.unwrap();
        let result = self.reader.read(path).await?;
        Ok(result)
    }
}
