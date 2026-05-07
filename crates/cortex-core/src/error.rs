use std::path::PathBuf;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("io error at {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("json decode at {path}: {source}")]
    Json {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },

    #[error("malformed entry: {0}")]
    Malformed(String),

    #[error("hash mismatch: stored={stored}, computed={computed}")]
    HashMismatch { stored: String, computed: String },

    #[error("signature verification failed: {0}")]
    SignatureFailed(String),

    #[error("crypto: {0}")]
    Crypto(String),

    #[error("missing keypair at {0}")]
    MissingKeypair(PathBuf),

    #[error("keypair already exists at {0}")]
    KeypairExists(PathBuf),

    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

pub type Result<T, E = Error> = std::result::Result<T, E>;

impl Error {
    pub(crate) fn io(path: impl Into<PathBuf>, source: std::io::Error) -> Self {
        Self::Io {
            path: path.into(),
            source,
        }
    }

    pub(crate) fn json(path: impl Into<PathBuf>, source: serde_json::Error) -> Self {
        Self::Json {
            path: path.into(),
            source,
        }
    }
}
