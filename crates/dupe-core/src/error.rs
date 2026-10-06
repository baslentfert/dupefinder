use std::path::PathBuf;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("io error at {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("io error: {0}")]
    PlainIo(#[from] std::io::Error),

    #[error("scan root is not a directory: {0}")]
    BadRoot(PathBuf),

    #[error("refusing to touch protected path: {0}")]
    Protected(PathBuf),

    #[error("cache error: {0}")]
    Cache(String),

    #[error("image decode failed for {path}: {reason}")]
    Decode { path: PathBuf, reason: String },

    #[error("mft enumeration failed for {volume}: {reason}")]
    Mft { volume: String, reason: String },

    #[error("operation cancelled")]
    Cancelled,

    #[error("{0}")]
    Other(String),
}

pub type Result<T> = std::result::Result<T, Error>;

pub(crate) trait IoContext<T> {
    fn at(self, path: impl Into<PathBuf>) -> Result<T>;
}

impl<T> IoContext<T> for std::result::Result<T, std::io::Error> {
    fn at(self, path: impl Into<PathBuf>) -> Result<T> {
        self.map_err(|source| Error::Io {
            path: path.into(),
            source,
        })
    }
}
