use std::fmt;
use std::path::PathBuf;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConflictedLink {
    pub path: PathBuf,
    pub owned_by: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Error {
    UnsupportedBottle {
        name: String,
    },
    ChecksumMismatch {
        expected: String,
        actual: String,
    },
    LinkConflict {
        conflicts: Vec<ConflictedLink>,
    },
    StoreCorruption {
        message: String,
    },
    NetworkFailure {
        message: String,
    },
    MissingFormula {
        name: String,
    },
    UnsupportedTap {
        name: String,
    },
    UnsupportedFormula {
        name: String,
        reason: String,
    },
    DependencyCycle {
        cycle: Vec<String>,
    },
    RequiredBy {
        name: String,
        dependents: Vec<String>,
    },
    NotInstalled {
        name: String,
    },
    FileError {
        message: String,
    },
    InvalidArgument {
        message: String,
    },
    ExecutionError {
        message: String,
    },
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::UnsupportedBottle { name } => {
                write!(f, "unsupported bottle for formula '{name}'")
            }
            Error::ChecksumMismatch { expected, actual } => {
                write!(f, "checksum mismatch (expected {expected}, got {actual})")
            }
            Error::LinkConflict { conflicts } => {
                if conflicts.len() == 1 {
                    let c = &conflicts[0];
                    write!(f, "link conflict at '{}'", c.path.display())?;
                    if let Some(ref owner) = c.owned_by {
                        write!(f, " (owned by {owner})")?;
                    }
                } else {
                    write!(f, "link conflicts:")?;
                    for c in conflicts {
                        write!(f, "\n  '{}'", c.path.display())?;
                        if let Some(ref owner) = c.owned_by {
                            write!(f, " (owned by {owner})")?;
                        }
                    }
                }
                Ok(())
            }
            Error::StoreCorruption { message } => write!(f, "store corruption: {message}"),
            Error::NetworkFailure { message } => write!(f, "network failure: {message}"),
            Error::MissingFormula { name } => write!(f, "missing formula '{name}'"),
            Error::UnsupportedTap { name } => {
                write!(
                    f,
                    "tap formula '{name}' is not supported (only homebrew/core)"
                )
            }
            Error::UnsupportedFormula { name, reason } => {
                write!(f, "formula '{name}' is not supported: {reason}")
            }
            Error::DependencyCycle { cycle } => {
                let rendered = cycle.join(" -> ");
                write!(f, "dependency cycle detected: {rendered}")
            }
            Error::RequiredBy { name, dependents } => write!(
                f,
                "cannot uninstall '{name}'; required by: {} (use --force to remove anyway)",
                dependents.join(", ")
            ),
            Error::NotInstalled { name } => write!(f, "formula '{name}' is not installed"),
            Error::FileError { message } => write!(f, "file error: {message}"),
            Error::InvalidArgument { message } => write!(f, "invalid argument: {message}"),
            Error::ExecutionError { message } => write!(f, "{message}"),
        }
    }
}

impl std::error::Error for Error {}

macro_rules! error_helpers {
    ($($fn_name:ident => $variant:ident),* $(,)?) => {
        impl Error {
            $(
                pub fn $fn_name<E: fmt::Display>(ctx: &str) -> impl FnOnce(E) -> Self + '_ {
                    move |err| Self::$variant { message: format!("{ctx}: {err}") }
                }
            )*
        }
    };
}

error_helpers! {
    store   => StoreCorruption,
    network => NetworkFailure,
    file    => FileError,
    exec    => ExecutionError,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unsupported_bottle_display_includes_name() {
        let err = Error::UnsupportedBottle {
            name: "libheif".to_string(),
        };

        assert!(err.to_string().contains("libheif"));
    }

    #[test]
    fn required_by_error_names_dependents_and_force_escape_hatch() {
        let err = Error::RequiredBy {
            name: "x264".to_string(),
            dependents: vec!["ffmpeg".to_string(), "vlc".to_string()],
        };

        assert_eq!(
            err.to_string(),
            "cannot uninstall 'x264'; required by: ffmpeg, vlc (use --force to remove anyway)"
        );
    }
}
