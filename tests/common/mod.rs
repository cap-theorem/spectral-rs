#![allow(dead_code)]

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

use spectral_sdk::{Client, Config};
use tiny_http::{Header, Response, Server};

#[derive(Debug, Clone)]
pub struct FakeRequest {
    pub method: String,
    pub path: String,
    pub query: HashMap<String, String>,
    pub content_type: Option<String>,
    pub body: Option<serde_json::Value>,
}

pub enum Reply {
    Json(u16, serde_json::Value),
    Text(u16, &'static str),
    Empty(u16),
}

type Responder = dyn Fn(&FakeRequest) -> Reply + Send + Sync;

pub struct FakeSidecar {
    pub url: String,
    pub requests: Arc<Mutex<Vec<FakeRequest>>>,
    server: Arc<Server>,
    thread: Option<JoinHandle<()>>,
}

impl FakeSidecar {
    pub fn start(responder: impl Fn(&FakeRequest) -> Reply + Send + Sync + 'static) -> FakeSidecar {
        let server = Arc::new(Server::http("127.0.0.1:0").expect("start fake sidecar"));
        let port = server.server_addr().to_ip().expect("ip listener").port();
        let requests = Arc::new(Mutex::new(Vec::new()));
        let responder: Arc<Responder> = Arc::new(responder);

        let thread = {
            let server = Arc::clone(&server);
            let requests = Arc::clone(&requests);
            std::thread::spawn(move || {
                for mut request in server.incoming_requests() {
                    let mut raw = String::new();
                    request
                        .as_reader()
                        .read_to_string(&mut raw)
                        .expect("read body");

                    let (path, query) = match request.url().split_once('?') {
                        Some((path, query)) => (path.to_string(), parse_query(query)),
                        None => (request.url().to_string(), HashMap::new()),
                    };
                    let content_type = request
                        .headers()
                        .iter()
                        .find(|h| h.field.equiv("Content-Type"))
                        .map(|h| h.value.as_str().to_string());

                    let fake = FakeRequest {
                        method: request.method().as_str().to_string(),
                        path,
                        query,
                        content_type,
                        body: if raw.is_empty() {
                            None
                        } else {
                            Some(serde_json::from_str(&raw).expect("json body"))
                        },
                    };
                    requests.lock().unwrap().push(fake.clone());

                    let result = match responder(&fake) {
                        Reply::Json(status, value) => request.respond(
                            Response::from_string(value.to_string())
                                .with_status_code(status)
                                .with_header(header("Content-Type", "application/json")),
                        ),
                        Reply::Text(status, text) => request.respond(
                            Response::from_string(text)
                                .with_status_code(status)
                                .with_header(header("Content-Type", "text/plain; charset=utf-8")),
                        ),
                        Reply::Empty(status) => request.respond(Response::empty(status)),
                    };
                    let _ = result;
                }
            })
        };

        FakeSidecar {
            url: format!("http://127.0.0.1:{port}"),
            requests,
            server,
            thread: Some(thread),
        }
    }

    pub fn client(&self) -> Client {
        Client::new(Config {
            addr: Some(self.url.clone()),
            timeout: Some(std::time::Duration::from_secs(2)),
        })
        .expect("client")
    }

    pub fn requests(&self) -> Vec<FakeRequest> {
        self.requests.lock().unwrap().clone()
    }
}

impl Drop for FakeSidecar {
    fn drop(&mut self) {
        self.server.unblock();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn header(name: &str, value: &str) -> Header {
    Header::from_bytes(name.as_bytes(), value.as_bytes()).expect("header")
}

fn parse_query(query: &str) -> HashMap<String, String> {
    query
        .split('&')
        .filter_map(|pair| pair.split_once('='))
        .map(|(k, v)| (decode(k), decode(v)))
        .collect()
}

fn decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'+' => out.push(b' '),
            b'%' if i + 2 < bytes.len() => {
                let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).expect("utf8");
                out.push(u8::from_str_radix(hex, 16).expect("hex"));
                i += 2;
            }
            b => out.push(b),
        }
        i += 1;
    }
    String::from_utf8(out).expect("utf8")
}
