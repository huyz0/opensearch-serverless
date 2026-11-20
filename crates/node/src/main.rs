//! Entry point. Kept to wiring only; logic lives in the library so it can be
//! tested.

#[cfg(all(feature = "reader", feature = "writer"))]
fn main() -> std::process::ExitCode {
    let config = match oss_node::boot::NodeConfig::from_lookup(|key| std::env::var(key).ok()) {
        Ok(config) => config,
        Err(why) => {
            eprintln!("opensearch-serverless node: {why}");
            return std::process::ExitCode::from(2);
        }
    };
    let node = match oss_node::boot::start(&config) {
        Ok(node) => node,
        Err(why) => {
            eprintln!("opensearch-serverless node: {why}");
            return std::process::ExitCode::FAILURE;
        }
    };
    println!(
        "opensearch-serverless node {} listening on {}",
        config.node_id,
        node.addr()
    );
    // Runs until the process is killed; the lease then lapses in one lifetime.
    loop {
        std::thread::park();
    }
}

#[cfg(not(all(feature = "reader", feature = "writer")))]
fn main() {
    let roles: Vec<&str> = oss_node::enabled_roles()
        .into_iter()
        .map(oss_node::Role::name)
        .collect();
    println!("opensearch-serverless node, roles: {}", roles.join(", "));
}
