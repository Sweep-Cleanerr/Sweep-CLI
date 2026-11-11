use std::io;
use std::path::PathBuf;

use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {
    #[error("{path}: {source}")]
    Io {
        path: PathBuf,

        #[source]
        source: io::Error,
    },

    #[error("i/o error: {0}")]
    PlainIo(#[from] io::Error),

    #[error("refusing to operate on protected path: {0}")]
    ProtectedPath(PathBuf),

    #[error("refusing to follow a link: {0}")]
    Link(PathBuf),

    #[error("invalid cleaner definition in {file}: {reason}")]
    Definition {
        file: String,
        reason: String,
    },

    #[error("cleaner `{cleaner}` has no action `{command}`")]
    UnknownAction {
        cleaner: String,
        command: String,
    },

    #[error("cleaner `{cleaner}` has no option `{option}`")]
    UnknownOption {
        cleaner: String,
        option: String,
    },

    #[error("unknown cleaner `{0}`")]
    UnknownCleaner(String),

    #[error("invalid glob `{pattern}`: {source}")]
    Glob {
        pattern: String,

        #[source]
        source: globset::Error,
    },

    #[error("invalid regex `{pattern}`: {source}")]
    Regex {
        pattern: String,

        #[source]
        source: regex::Error,
    },

    #[error("cannot parse {path}: {reason}")]
    Structured {
        path: PathBuf,
        reason: String,
    },

    #[error("configuration error: {0}")]
    Config(String),

    #[error("`{command}` failed with {status}: {stderr}")]
    ExternalCommand {
        command: String,
        status: String,
        stderr: String,
    },

    #[error("cannot run `{command}`: {reason}")]
    Spawn {
        command: String,
        reason: String,
    },

    #[error("operation aborted")]
    Aborted,

    #[error("database error at {path}: {source}")]
    Database { path: PathBuf, source: rusqlite::Error },

    #[error("{0}")]
    Message(String),
}

pub type Result<T> = std::result::Result<T, Error>;

impl Error {
    pub fn io(path: impl Into<PathBuf>, source: io::Error) -> Self {
        Self::Io {
            path: path.into(),
            source,
        }
    }

    pub fn definition(
        file: impl Into<String>,
        reason: impl Into<String>,
    ) -> Self {
        Self::Definition {
            file: file.into(),
            reason: reason.into(),
        }
    }

    pub fn msg(message: impl Into<String>) -> Self {
        Self::Message(message.into())
    }

    /// Errors that are normal while scanning (missing or locked files, a
    /// guard refusal) rather than bugs in Sweep.
    pub fn is_benign(&self) -> bool {
        match self {
            Error::Io { source, .. } | Error::PlainIo(source) => {
                matches!(
                    source.kind(),
                    io::ErrorKind::NotFound
                        | io::ErrorKind::PermissionDenied
                        | io::ErrorKind::AlreadyExists
                ) || crate::platform::is_locked(source)
            }
            Error::ProtectedPath(_) | Error::Link(_) | Error::Aborted => true,
            Error::Database { source, .. } => matches!(
                source,
                rusqlite::Error::SqliteFailure(_, _) | rusqlite::Error::QueryReturnedNoRows
            ),
            _ => false,
        }
    }
}

pub trait WithPath: Sized {
    fn with_path(self, pattern: impl Into<String>) -> Error;
}

impl WithPath for globset::Error {
    fn with_path(self, pattern: impl Into<String>) -> Error {
        Error::Glob {
            pattern: pattern.into(),
            source: self,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn messages_are_stable() {
        assert_eq!(
            Error::ProtectedPath(PathBuf::from("/etc")).to_string(),
            "refusing to operate on protected path: /etc"
        );

        assert_eq!(
            Error::definition("a.toml", "bad id").to_string(),
            "invalid cleaner definition in a.toml: bad id"
        );

        assert_eq!(
            Error::Aborted.to_string(),
            "operation aborted"
        );

        assert_eq!(Error::msg("boom").to_string(), "boom");
    }

    #[test]
    fn benign_classification() {
        let missing = Error::io(
            "/x",
            io::Error::from(io::ErrorKind::NotFound),
        );
        assert!(missing.is_benign());

        let denied = Error::io(
            "/x",
            io::Error::from(io::ErrorKind::PermissionDenied),
        );
        assert!(denied.is_benign());

        let other = Error::io(
            "/x",
            io::Error::from(io::ErrorKind::InvalidData),
        );
        assert!(!other.is_benign());

        assert!(Error::Aborted.is_benign());
        assert!(!Error::msg("nope").is_benign());
    }

    #[test]
    fn io_constructor_preserves_the_path_and_source_kind() {
        let error = Error::io(
            "/tmp/example",
            io::Error::from(io::ErrorKind::InvalidData),
        );

        match error {
            Error::Io { path, source } => {
                assert_eq!(path, PathBuf::from("/tmp/example"));
                assert_eq!(source.kind(), io::ErrorKind::InvalidData);
            }
            _ => panic!("Error::io must construct Error::Io"),
        }
    }
}
