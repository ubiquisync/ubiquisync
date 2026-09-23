use std::{fs, path::PathBuf};

#[async_trait::async_trait]
pub trait FileRemote {
    async fn list(&self, dir: &str) -> Result<Vec<DirEntry>, FileRemoteError>;
    async fn read(&self, path: &str) -> Result<Option<Vec<u8>>, FileRemoteError>;
    async fn write(&self, path: &str, data: &[u8]) -> Result<(), FileRemoteError>;
    async fn delete(&self, path: &str) -> Result<(), FileRemoteError>;
}

pub struct FileRemoteError;

impl From<std::io::Error> for FileRemoteError {
    fn from(_: std::io::Error) -> Self {
        Self
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
        let res = fs::read(path)?;
        Ok(Some(res))
    }

    async fn write(&self, path: &str, data: &[u8]) -> Result<(), FileRemoteError> {
        // TODO do we need to do any sort of fsync or writing to a tempfile and renaming first?
        fs::write(path, data)?;
        Ok(())
    }

    async fn delete(&self, path: &str) -> Result<(), FileRemoteError> {
        fs::remove_file(path)?;
        Ok(())
    }
}
