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
    wait_for_stop();
    let report = node.retire();
    println!(
        "opensearch-serverless node {} retired: {} shards slept, {} heads given back, {} busy, {} failed",
        config.node_id,
        report.hibernated,
        report.given_back,
        report.busy,
        report.failed.len()
    );
    for failed in &report.failed {
        eprintln!("opensearch-serverless node: not published: {failed}");
    }
    if report.failed.is_empty() {
        std::process::ExitCode::SUCCESS
    } else {
        std::process::ExitCode::FAILURE
    }
}

/// Blocks until the process is told to stop (`SIGTERM`, `SIGINT`); a killed
/// process leaves its lease to lapse in one lifetime.
#[cfg(all(unix, feature = "reader", feature = "writer"))]
fn wait_for_stop() {
    use signal_hook::consts::{SIGINT, SIGTERM};
    match signal_hook::iterator::Signals::new([SIGTERM, SIGINT]) {
        Ok(mut signals) => {
            let _ = signals.forever().next();
        }
        Err(why) => {
            eprintln!(
                "opensearch-serverless node: no signal handler ({why}); stop it by killing it"
            );
            loop {
                std::thread::park();
            }
        }
    }
}

/// Without signals to wait for, runs until the process is killed.
#[cfg(all(not(unix), feature = "reader", feature = "writer"))]
fn wait_for_stop() {
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
