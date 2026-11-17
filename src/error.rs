use std::fmt;
use std::time::Duration;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Op {
    Register,
    Renew,
    Lookup,
    Status,
}

impl Op {
    pub fn as_str(self) -> &'static str {
        match self {
            Op::Register => "register",
            Op::Renew => "renew",
            Op::Lookup => "lookup",
            Op::Status => "status",
        }
    }
}

impl fmt::Display for Op {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApiError {
    pub op: Op,
    pub status: u16,
    pub message: String,
}

impl fmt::Display for ApiError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.message.is_empty() {
            write!(f, "spectral {}: http {}", self.op, self.status)
        } else {
            write!(
                f,
                "spectral {}: http {}: {}",
                self.op, self.status, self.message
            )
        }
    }
}

impl std::error::Error for ApiError {}

#[derive(Debug)]
#[non_exhaustive]
pub enum Error {
    InvalidAddr(String),
    InvalidRegistration(ApiError),
    AlreadyRegistered(ApiError),
    ServiceNotFound(ApiError),
    LeaseExpired(Option<Box<Error>>),
    SidecarTimeout(ApiError),
    SidecarUnavailable {
        op: Op,
        addr: String,
        source: Box<dyn std::error::Error + Send + Sync>,
    },
    Api(ApiError),
    InvalidResponse {
        op: Op,
        reason: String,
    },
    TtlTooShort(Duration),
}

impl Error {
    pub fn api(&self) -> Option<&ApiError> {
        match self {
            Error::InvalidRegistration(api)
            | Error::AlreadyRegistered(api)
            | Error::ServiceNotFound(api)
            | Error::SidecarTimeout(api)
            | Error::Api(api) => Some(api),
            Error::LeaseExpired(Some(cause)) => cause.api(),
            _ => None,
        }
    }

    pub(crate) fn from_status(op: Op, status: u16, message: String) -> Error {
        let api = ApiError {
            op,
            status,
            message,
        };
        match (op, status) {
            (_, 504) => Error::SidecarTimeout(api),
            (Op::Register, 400) => Error::InvalidRegistration(api),
            (Op::Register, 409) => Error::AlreadyRegistered(api),
            (Op::Lookup, 404) => Error::ServiceNotFound(api),
            (Op::Renew, 404) => Error::LeaseExpired(Some(Box::new(Error::Api(api)))),
            _ => Error::Api(api),
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::InvalidAddr(reason) => write!(f, "spectral: {reason}"),
            Error::InvalidRegistration(api)
            | Error::AlreadyRegistered(api)
            | Error::ServiceNotFound(api)
            | Error::SidecarTimeout(api) => write!(f, "{api}"),
            Error::LeaseExpired(None) => f.write_str("spectral: lease expired"),
            Error::LeaseExpired(Some(cause)) => write!(f, "spectral: lease expired: {cause}"),
            Error::SidecarUnavailable { op, addr, source } => {
                write!(f, "spectral {op}: sidecar unavailable at {addr}: {source}")
            }
            Error::Api(api) => write!(f, "{api}"),
            Error::InvalidResponse { op, reason } => {
                write!(f, "spectral {op}: invalid response: {reason}")
            }
            Error::TtlTooShort(ttl) => {
                write!(f, "spectral keep_alive: lease ttl {ttl:?} is too short")
            }
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Error::LeaseExpired(Some(cause)) => Some(cause.as_ref()),
            Error::SidecarUnavailable { source, .. } => Some(source.as_ref()),
            _ => None,
        }
    }
}
