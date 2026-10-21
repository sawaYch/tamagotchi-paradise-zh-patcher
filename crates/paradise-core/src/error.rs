use std::fmt;

#[derive(Debug)]
pub enum Error {
    Io(std::io::Error),
    Format(String),
    Unsupported(String),
    Capacity(String),
    Verify(String),
    Mapping(String),
}

pub type Result<T> = std::result::Result<T, Error>;

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Io(e) => write!(f, "I/O error: {e}"),
            Error::Format(m) => write!(f, "unexpected data format: {m}"),
            Error::Unsupported(m) => write!(f, "unsupported ROM: {m}"),
            Error::Capacity(m) => write!(f, "not enough space: {m}"),
            Error::Verify(m) => write!(f, "verification failed: {m}"),
            Error::Mapping(m) => write!(f, "text mapping: {m}"),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Error::Io(e) => Some(e),
            _ => None,
        }
    }
}

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        Error::Io(e)
    }
}

pub(crate) fn fmt_err<T>(msg: impl Into<String>) -> Result<T> {
    Err(Error::Format(msg.into()))
}
