use std::{fs, io::ErrorKind, path::PathBuf};

use thiserror::Error;

#[async_trait::async_trait]
pub trait FileRemote: Send + Sync {
    async fn list(&self, dir: &str) -> Result<Vec<DirEntry>, FileRemoteError>;
    async fn read(&self, path: &str) -> Result<Option<Vec<u8>>, FileRemoteError>;
    async fn write(&self, path: &str, data: &[u8]) -> Result<(), FileRemoteError>;
    async fn delete(&self, path: &str) -> Result<(), FileRemoteError>;
}

type BoxError = Box<dyn core::error::Error + Send + Sync>;

#[derive(Debug, Error)]
pub enum FileRemoteError {
    #[error("fatal error: {0}")]
    Fatal(BoxError),
    #[error("transient error: {0}")]
    Transient(BoxError),
}

impl From<std::io::Error> for FileRemoteError {
    fn from(e: std::io::Error) -> Self {
        Self::Transient(Box::new(e))
    }
}

pub struct StdFsRemote {
    root: PathBuf,
}

pub struct DirEntry {
    pub file_name: String,
    pub file_type: FileType,
}

pub enum FileType {
    File,
    Dir,
}

#[async_trait::async_trait]
impl FileRemote for StdFsRemote {
    async fn list(&self, dir: &str) -> Result<Vec<DirEntry>, FileRemoteError> {
        let mut res = vec![];
        for e in fs::read_dir(self.root.join(dir))? {
            let e = e?;
            let file_type = e.file_type()?;
            let file_type = if file_type.is_file() {
                FileType::File
            } else if file_type.is_dir() {
                FileType::Dir
            } else {
                // skip symlinks
                continue;
            };
            let Ok(file_name) = e.file_name().into_string() else {
                continue;
            };
            res.push(DirEntry {
                file_name,
                file_type,
            })
        }
        Ok(res)
    }

    async fn read(&self, path: &str) -> Result<Option<Vec<u8>>, FileRemoteError> {
        match fs::read(self.root.join(path)) {
            Ok(res) => Ok(Some(res)),
            Err(e) => {
                if e.kind() == ErrorKind::NotFound {
                    Ok(None)
                } else {
                    Err(e.into())
                }
            }
        }
    }

    async fn write(&self, path: &str, data: &[u8]) -> Result<(), FileRemoteError> {
        // TODO do we need to do any sort of fsync or writing to a tempfile and renaming first?
        let path = self.root.join(path);
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir)?;
        }
        fs::write(path, data)?;
        Ok(())
    }

    async fn delete(&self, path: &str) -> Result<(), FileRemoteError> {
        // do we allow deleting directories too?
        match fs::remove_file(self.root.join(path)) {
            Ok(_) => Ok(()),
            Err(e) => {
                if e.kind() == ErrorKind::NotFound {
                    Ok(())
                } else {
                    Err(e.into())
                }
            }
        }
    }
}
