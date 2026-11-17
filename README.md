# spectral-rs

Rust client for the Spectral sidecar.

Every service instance runs next to a Spectral sidecar. Your service uses this client to talk to its own sidecar over HTTP: register itself, keep its lease alive, and look up other services. The sidecar handles everything else (gossip, failure detection, keeping the directory in sync across machines).

## Install

```toml
[dependencies]
spectral-sdk = { git = "https://github.com/cap-theorem/spectral-rs" }
```

Rust 1.88+. The client is blocking and built on [`ureq`](https://crates.io/crates/ureq); from async code, call it inside `spawn_blocking`. The crate is named `spectral-sdk` because `spectral` on crates.io is an unrelated testing library.

## Quickstart

```rust
use std::time::Duration;
use spectral_sdk::{Client, Config, Error, Registration};

let client = Client::new(Config::default())?;

let lease = client.register(&Registration {
    service: "payments".to_string(),
    instance_id: "payments-1".to_string(),
    endpoint: "http://100.64.1.3:9007".to_string(),
    ttl: Duration::from_secs(10),
})?;

let keeper = client.keep_alive(&lease)?;

match client.lookup("inventory") {
    Ok(inventory) => call(&inventory.endpoint),
    Err(Error::ServiceNotFound(_)) => {}
    Err(err) => return Err(err),
}
```

A runnable version is in [`examples/quickstart.rs`](examples/quickstart.rs) (`cargo run --example quickstart`).

## Connecting to the sidecar

`Client::new(Config { addr, timeout })` picks the sidecar address in this order:

1. `addr`, if set
2. the `SPECTRAL_ADDR` environment variable
3. `http://127.0.0.1:8000`, the sidecar's default API port

`timeout` (default 10s) applies to each request. The address must be plain `http`: the sidecar API only listens on the local machine. Proxy environment variables are ignored. `Client` is cheap to clone and safe to share across threads.

## API

| Method | What it does |
|---|---|
| `register(&Registration) -> Result<Lease>` | Announce this instance. The sidecar spreads it to the whole network. |
| `renew(token) -> Result<()>` | Extend the lease once. |
| `keep_alive(&Lease) -> Result<KeepAlive>` | Renew every TTL/3 on a background thread. Retries failed renewals; gives up with `Error::LeaseExpired` if none succeed for a full TTL, or right away if the sidecar no longer knows the lease. |
| `lookup(service) -> Result<Provider>` | Get one healthy instance of a service, answered from the sidecar's local copy of the directory. |
| `status() -> Result<Status>` | Whether the sidecar is ready, and its state. |

`KeepAlive` is a handle to the renewal thread. `stop()` ends it and returns how it finished; `is_finished()` tells you whether it already gave up. **Dropping the handle stops renewing**, so keep it alive for as long as the instance should stay registered.

If your instance stops renewing (crash, hang, handle dropped), the lease expires and the instance disappears from every sidecar's directory. There is no explicit deregister: stop renewing, or shut the sidecar down.

## Errors

`spectral_sdk::Error`:

| Variant | Meaning |
|---|---|
| `InvalidRegistration` | The sidecar rejected the registration (missing fields, bad endpoint URL, non-positive TTL). |
| `AlreadyRegistered` | This registration already exists. |
| `ServiceNotFound` | No healthy instance of that service is known. |
| `LeaseExpired` | The lease is gone; register again. Carries the error that caused it, if any. |
| `SidecarTimeout` | The sidecar accepted the request but did not answer in time. |
| `SidecarUnavailable` | The sidecar could not be reached at all. |
| `Api` | Any other non-2xx response. |
| `InvalidAddr`, `InvalidResponse`, `TtlTooShort` | Bad configuration, a malformed response, or a lease too short to renew. |

`Error::api()` returns the `ApiError` (`op`, `status`, `message`) behind any HTTP error.

## Development

```sh
cargo test
cargo clippy --all-targets -- -D warnings
```

## HTTP contract

This is the sidecar API the client speaks. Every Spectral SDK uses the same contract.

| Call | Request | Success | Errors |
|---|---|---|---|
| `POST /register` | `{"service", "instance_id", "endpoint", "ttl": "10s"}` | `201 {"token", "ttl", "expires_at"}` | `400` invalid, `409` exists, `504` timeout |
| `POST /renew` | `{"token"}` | `202` | `404` unknown or expired lease, `504` timeout |
| `GET /lookup?service=<name>` | | `200 {"service", "instance_id", "endpoint"}` | `404` no provider, `504` timeout |
| `GET /status` | | `200 {"ready", "state"}` | `504` timeout |

Durations are Go duration strings (`"10s"`, `"1m30s"`). Timestamps are RFC 3339. Error bodies are plain text.
