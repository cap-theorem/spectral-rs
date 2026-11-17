use std::time::Duration;

use spectral_sdk::{Client, Config, Error, Registration};

fn main() -> Result<(), Error> {
    let client = Client::new(Config::default())?;

    let lease = client.register(&Registration {
        service: "payments".to_string(),
        instance_id: "payments-1".to_string(),
        endpoint: "http://127.0.0.1:9000".to_string(),
        ttl: Duration::from_secs(10),
    })?;
    println!(
        "registered with {}, lease ttl {:?}",
        client.addr(),
        lease.ttl
    );

    let keeper = client.keep_alive(&lease)?;

    match client.lookup("inventory") {
        Ok(inventory) => println!("inventory is at {}", inventory.endpoint),
        Err(Error::ServiceNotFound(_)) => println!("no inventory instance registered yet"),
        Err(err) => return Err(err),
    }

    while !keeper.is_finished() {
        std::thread::sleep(Duration::from_secs(1));
    }
    keeper.stop()
}
