use std::time::{Duration, SystemTime};

use serde::{Deserialize, Serialize};
use ureq::http::{Response, Uri};
use ureq::{Agent, Body};

use crate::error::{Error, Op};
use crate::keepalive::KeepAlive;
use crate::wire::{format_duration, parse_duration, parse_rfc3339};

pub const VERSION: &str = env!("CARGO_PKG_VERSION");
pub const DEFAULT_ADDR: &str = "http://127.0.0.1:8000";
pub const ADDR_ENV: &str = "SPECTRAL_ADDR";

const DEFAULT_TIMEOUT: Duration = Duration::from_secs(10);
const MAX_BODY: u64 = 1 << 20;
const MAX_ERROR_BODY: usize = 4096;

#[derive(Debug, Clone, Default)]
pub struct Config {
    pub addr: Option<String>,
    pub timeout: Option<Duration>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Registration {
    pub service: String,
    pub instance_id: String,
    pub endpoint: String,
    pub ttl: Duration,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Lease {
    pub token: String,
    pub ttl: Duration,
    pub expires_at: SystemTime,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Provider {
    pub service: String,
    pub instance_id: String,
    pub endpoint: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Status {
    pub ready: bool,
    pub state: String,
}

#[derive(Serialize)]
struct RegisterRequest<'a> {
    service: &'a str,
    instance_id: &'a str,
    endpoint: &'a str,
    ttl: String,
}

#[derive(Deserialize)]
struct LeaseResponse {
    token: String,
    ttl: String,
    expires_at: String,
}

#[derive(Serialize)]
struct RenewRequest<'a> {
    token: &'a str,
}

#[derive(Deserialize)]
struct ProviderResponse {
    service: String,
    #[serde(default)]
    instance_id: String,
    endpoint: String,
}

#[derive(Deserialize)]
struct StatusResponse {
    ready: bool,
    state: String,
}

#[derive(Debug, Clone)]
pub struct Client {
    addr: String,
    agent: Agent,
}

impl Client {
    pub fn new(config: Config) -> Result<Client, Error> {
        let addr = match config.addr.filter(|a| !a.is_empty()) {
            Some(addr) => addr,
            None => std::env::var(ADDR_ENV)
                .ok()
                .filter(|a| !a.is_empty())
                .unwrap_or_else(|| DEFAULT_ADDR.to_string()),
        };
        let addr = addr.trim_end_matches('/').to_string();
        validate_addr(&addr)?;

        let agent: Agent = Agent::config_builder()
            .http_status_as_error(false)
            .proxy(None)
            .timeout_global(Some(config.timeout.unwrap_or(DEFAULT_TIMEOUT)))
            .user_agent(format!("spectral-rs/{VERSION}"))
            .build()
            .into();

        Ok(Client { addr, agent })
    }

    pub fn addr(&self) -> &str {
        &self.addr
    }

    pub fn register(&self, reg: &Registration) -> Result<Lease, Error> {
        let body = RegisterRequest {
            service: &reg.service,
            instance_id: &reg.instance_id,
            endpoint: &reg.endpoint,
            ttl: format_duration(reg.ttl),
        };
        let result = self.agent.post(self.url("/register")).send_json(&body);
        let resp: LeaseResponse = self.decode(Op::Register, result)?;

        let ttl = parse_duration(&resp.ttl).ok_or_else(|| Error::InvalidResponse {
            op: Op::Register,
            reason: format!("invalid ttl {:?}", resp.ttl),
        })?;
        let expires_at = parse_rfc3339(&resp.expires_at).ok_or_else(|| Error::InvalidResponse {
            op: Op::Register,
            reason: format!("invalid expires_at {:?}", resp.expires_at),
        })?;

        Ok(Lease {
            token: resp.token,
            ttl,
            expires_at,
        })
    }

    pub fn renew(&self, token: &str) -> Result<(), Error> {
        let result = self
            .agent
            .post(self.url("/renew"))
            .send_json(RenewRequest { token });
        self.finish(Op::Renew, result).map(|_| ())
    }

    pub fn lookup(&self, service: &str) -> Result<Provider, Error> {
        let result = self
            .agent
            .get(self.url("/lookup"))
            .query("service", service)
            .call();
        let resp: ProviderResponse = self.decode(Op::Lookup, result)?;

        Ok(Provider {
            service: resp.service,
            instance_id: resp.instance_id,
            endpoint: resp.endpoint,
        })
    }

    pub fn status(&self) -> Result<Status, Error> {
        let result = self.agent.get(self.url("/status")).call();
        let resp: StatusResponse = self.decode(Op::Status, result)?;

        Ok(Status {
            ready: resp.ready,
            state: resp.state,
        })
    }

    pub fn keep_alive(&self, lease: &Lease) -> Result<KeepAlive, Error> {
        KeepAlive::start(self.clone(), lease.clone())
    }

    fn url(&self, path: &str) -> String {
        format!("{}{path}", self.addr)
    }

    fn decode<T: for<'de> Deserialize<'de>>(
        &self,
        op: Op,
        result: Result<Response<Body>, ureq::Error>,
    ) -> Result<T, Error> {
        let text = self.finish(op, result)?;
        serde_json::from_str(&text).map_err(|err| Error::InvalidResponse {
            op,
            reason: err.to_string(),
        })
    }

    fn finish(&self, op: Op, result: Result<Response<Body>, ureq::Error>) -> Result<String, Error> {
        let mut resp = result.map_err(|source| Error::SidecarUnavailable {
            op,
            addr: self.addr.clone(),
            source: Box::new(source),
        })?;

        let status = resp.status().as_u16();
        let text = resp
            .body_mut()
            .with_config()
            .limit(MAX_BODY)
            .read_to_string()
            .map_err(|err| Error::InvalidResponse {
                op,
                reason: err.to_string(),
            })?;

        if !(200..300).contains(&status) {
            let message: String = text.trim().chars().take(MAX_ERROR_BODY).collect();
            return Err(Error::from_status(op, status, message));
        }

        Ok(text)
    }
}

fn validate_addr(addr: &str) -> Result<(), Error> {
    let invalid = || Error::InvalidAddr(format!("addr {addr:?} must be an http URL with a host"));

    let uri: Uri = addr.parse().map_err(|_| invalid())?;
    let host = uri.host().unwrap_or("");
    match uri.scheme_str() {
        Some("http") if !host.is_empty() => Ok(()),
        Some("https") => Err(Error::InvalidAddr(format!(
            "addr {addr:?} uses https; the sidecar API is plain http on the local machine"
        ))),
        _ => Err(invalid()),
    }
}
