use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use crate::client::{Client, Lease};
use crate::error::Error;

#[must_use = "dropping the KeepAlive stops renewing the lease"]
#[derive(Debug)]
pub struct KeepAlive {
    stop: Option<Sender<()>>,
    thread: Option<JoinHandle<Result<(), Error>>>,
}

impl KeepAlive {
    pub(crate) fn start(client: Client, lease: Lease) -> Result<KeepAlive, Error> {
        let interval = lease.ttl / 3;
        if interval.is_zero() {
            return Err(Error::TtlTooShort(lease.ttl));
        }

        let (stop, stopped) = mpsc::channel();
        let thread = thread::Builder::new()
            .name("spectral-keepalive".to_string())
            .spawn(move || run(&client, &lease, interval, &stopped))
            .expect("spawn spectral-keepalive thread");

        Ok(KeepAlive {
            stop: Some(stop),
            thread: Some(thread),
        })
    }

    pub fn is_finished(&self) -> bool {
        self.thread.as_ref().is_none_or(JoinHandle::is_finished)
    }

    pub fn stop(mut self) -> Result<(), Error> {
        self.stop.take();
        match self.thread.take() {
            Some(thread) => match thread.join() {
                Ok(result) => result,
                Err(panic) => std::panic::resume_unwind(panic),
            },
            None => Ok(()),
        }
    }
}

impl Drop for KeepAlive {
    fn drop(&mut self) {
        self.stop.take();
    }
}

fn run(
    client: &Client,
    lease: &Lease,
    interval: Duration,
    stopped: &Receiver<()>,
) -> Result<(), Error> {
    let mut deadline = Instant::now() + lease.ttl;
    loop {
        match stopped.recv_timeout(interval) {
            Err(RecvTimeoutError::Timeout) => {}
            Ok(()) | Err(RecvTimeoutError::Disconnected) => return Ok(()),
        }

        let now = Instant::now();
        match client.renew(&lease.token) {
            Ok(()) => deadline = now + lease.ttl,
            Err(err @ Error::LeaseExpired(_)) => return Err(err),
            Err(err) if now >= deadline => return Err(Error::LeaseExpired(Some(Box::new(err)))),
            Err(_) => {}
        }
    }
}
