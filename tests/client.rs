mod common;

use std::sync::Mutex;
use std::time::{Duration, UNIX_EPOCH};

use common::{FakeSidecar, Reply};
use serde_json::json;
use spectral_sdk::{
    ADDR_ENV, Client, Config, DEFAULT_ADDR, Error, Op, Provider, Registration, Status,
};

static ENV_LOCK: Mutex<()> = Mutex::new(());

fn registration() -> Registration {
    Registration {
        service: "payments".to_string(),
        instance_id: "payments-1".to_string(),
        endpoint: "http://100.64.1.3:9007".to_string(),
        ttl: Duration::from_secs(10),
    }
}

#[test]
fn addr_resolution() {
    let _guard = ENV_LOCK.lock().unwrap();
    let cases: [(&str, Option<&str>, Option<&str>, &str); 4] = [
        ("default", None, None, DEFAULT_ADDR),
        (
            "env",
            None,
            Some("http://10.0.0.5:8007"),
            "http://10.0.0.5:8007",
        ),
        (
            "explicit beats env",
            Some("http://127.0.0.1:8003"),
            Some("http://10.0.0.5:8007"),
            "http://127.0.0.1:8003",
        ),
        (
            "trailing slash",
            Some("http://127.0.0.1:8003/"),
            None,
            "http://127.0.0.1:8003",
        ),
    ];

    for (name, addr, env, want) in cases {
        unsafe {
            match env {
                Some(value) => std::env::set_var(ADDR_ENV, value),
                None => std::env::remove_var(ADDR_ENV),
            }
        }
        let client = Client::new(Config {
            addr: addr.map(str::to_string),
            timeout: None,
        })
        .unwrap();
        assert_eq!(client.addr(), want, "{name}");
    }
    unsafe { std::env::remove_var(ADDR_ENV) };
}

#[test]
fn rejects_bad_addr() {
    for addr in [
        "127.0.0.1:8000",
        "ftp://127.0.0.1:8000",
        "http://",
        "https://127.0.0.1:8000",
    ] {
        let result = Client::new(Config {
            addr: Some(addr.to_string()),
            timeout: None,
        });
        assert!(
            matches!(result, Err(Error::InvalidAddr(_))),
            "{addr} should be rejected"
        );
    }
}

#[test]
fn register_sends_registration_and_parses_lease() {
    let fake = FakeSidecar::start(|_| {
        Reply::Json(
            201,
            json!({"token": "abc123", "ttl": "10s", "expires_at": "2026-11-16T12:00:10.123456789-08:00"}),
        )
    });

    let lease = fake.client().register(&registration()).unwrap();

    let req = &fake.requests()[0];
    assert_eq!(
        (req.method.as_str(), req.path.as_str()),
        ("POST", "/register")
    );
    assert!(
        req.content_type
            .as_deref()
            .unwrap_or("")
            .starts_with("application/json")
    );
    assert_eq!(
        req.body,
        Some(json!({
            "service": "payments",
            "instance_id": "payments-1",
            "endpoint": "http://100.64.1.3:9007",
            "ttl": "10s",
        }))
    );
    assert_eq!(lease.token, "abc123");
    assert_eq!(lease.ttl, Duration::from_secs(10));
    assert_eq!(
        lease.expires_at.duration_since(UNIX_EPOCH).unwrap(),
        Duration::new(1_794_859_210, 123_456_789)
    );
}

#[test]
fn register_maps_errors() {
    for status in [400, 409, 504, 500] {
        let fake = FakeSidecar::start(move |_| Reply::Text(status, "nope\n"));
        let err = fake.client().register(&registration()).unwrap_err();

        let mapped = match status {
            400 => matches!(err, Error::InvalidRegistration(_)),
            409 => matches!(err, Error::AlreadyRegistered(_)),
            504 => matches!(err, Error::SidecarTimeout(_)),
            _ => matches!(err, Error::Api(_)),
        };
        assert!(mapped, "http {status} mapped to {err:?}");
        let api = err.api().expect("api error");
        assert_eq!(
            (api.op, api.status, api.message.as_str()),
            (Op::Register, status, "nope")
        );
    }
}

#[test]
fn renew_sends_token() {
    let fake = FakeSidecar::start(|_| Reply::Empty(202));
    fake.client().renew("abc123").unwrap();

    let req = &fake.requests()[0];
    assert_eq!((req.method.as_str(), req.path.as_str()), ("POST", "/renew"));
    assert_eq!(req.body, Some(json!({"token": "abc123"})));
}

#[test]
fn renew_unknown_token_is_lease_expired() {
    let fake = FakeSidecar::start(|_| Reply::Text(404, "unknown lease"));
    let err = fake.client().renew("gone").unwrap_err();

    assert!(matches!(err, Error::LeaseExpired(Some(_))), "{err:?}");
    assert_eq!(err.api().map(|api| api.status), Some(404));
}

#[test]
fn lookup_queries_by_service() {
    let fake = FakeSidecar::start(|_| {
        Reply::Json(
            200,
            json!({"service": "model-server/llama 70b", "instance_id": "ms-0412", "endpoint": "http://100.64.1.6:9002"}),
        )
    });

    let provider = fake.client().lookup("model-server/llama 70b").unwrap();

    let req = &fake.requests()[0];
    assert_eq!((req.method.as_str(), req.path.as_str()), ("GET", "/lookup"));
    assert_eq!(
        req.query.get("service").map(String::as_str),
        Some("model-server/llama 70b")
    );
    assert_eq!(
        provider,
        Provider {
            service: "model-server/llama 70b".to_string(),
            instance_id: "ms-0412".to_string(),
            endpoint: "http://100.64.1.6:9002".to_string(),
        }
    );
}

#[test]
fn lookup_not_found() {
    let fake = FakeSidecar::start(|_| Reply::Text(404, "no providers"));
    let err = fake.client().lookup("inventory").unwrap_err();
    assert!(matches!(err, Error::ServiceNotFound(_)), "{err:?}");
}

#[test]
fn status_parses_response() {
    let fake = FakeSidecar::start(|_| Reply::Json(200, json!({"ready": true, "state": "active"})));
    let status = fake.client().status().unwrap();

    assert_eq!(fake.requests()[0].path, "/status");
    assert_eq!(
        status,
        Status {
            ready: true,
            state: "active".to_string()
        }
    );
}

#[test]
fn status_rejects_malformed_response() {
    let fake = FakeSidecar::start(|_| Reply::Json(200, json!({"state": "active"})));
    let err = fake.client().status().unwrap_err();
    assert!(
        matches!(err, Error::InvalidResponse { op: Op::Status, .. }),
        "{err:?}"
    );
}

#[test]
fn keeps_base_path_prefix() {
    let fake = FakeSidecar::start(|_| Reply::Json(200, json!({"ready": true, "state": "active"})));
    let client = Client::new(Config {
        addr: Some(format!("{}/sidecar/", fake.url)),
        timeout: None,
    })
    .unwrap();

    client.status().unwrap();
    assert_eq!(fake.requests()[0].path, "/sidecar/status");
}

#[test]
fn unreachable_sidecar() {
    let url = {
        let fake = FakeSidecar::start(|_| Reply::Empty(200));
        fake.url.clone()
    };

    let client = Client::new(Config {
        addr: Some(url),
        timeout: Some(Duration::from_secs(2)),
    })
    .unwrap();
    let err = client.status().unwrap_err();
    assert!(
        matches!(err, Error::SidecarUnavailable { op: Op::Status, .. }),
        "{err:?}"
    );
}
