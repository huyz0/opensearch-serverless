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
    if let Err(why) = wait_for_stop() {
        eprintln!("opensearch-serverless node: {why}");
        return std::process::ExitCode::FAILURE;
    }
    let report = node.retire(config.retire_timeout_millis);
    println!(
        "opensearch-serverless node {} retired: {} shards slept, {} heads given back, {} busy, {} failed, {} unfinished",
        config.node_id,
        report.hibernated,
        report.given_back,
        report.busy,
        report.failed.len(),
        report.unfinished
    );
    for failed in &report.failed {
        eprintln!("opensearch-serverless node: not published: {failed}");
    }
    // 0: retired, or left to the lease (busy shards, a timeout of 0); 1: a
    // shard failed to publish or the deadline passed first.
    if report.failed.is_empty() && report.unfinished == 0 {
        std::process::ExitCode::SUCCESS
    } else {
        std::process::ExitCode::FAILURE
    }
}

/// Blocks until the process is told to stop (`SIGTERM`, `SIGINT`); a second
/// signal while the node retires ends the process at once with status 130,
/// and a killed process leaves its lease to lapse in one lifetime.
#[cfg(all(unix, feature = "reader", feature = "writer"))]
fn wait_for_stop() -> Result<(), String> {
    use signal_hook::consts::{SIGINT, SIGTERM};
    let mut signals = signal_hook::iterator::Signals::new([SIGTERM, SIGINT])
        .map_err(|e| format!("no signal handler: {e}"))?;
    let (first, wait) = std::sync::mpsc::sync_channel(1);
    std::thread::spawn(move || {
        let mut seen = 0usize;
        for _ in signals.forever() {
            seen = seen.saturating_add(1);
            if seen == 1 {
                let _ = first.try_send(());
            } else {
                std::process::exit(130);
            }
        }
    });
    wait.recv().map_err(|e| format!("signal thread ended: {e}"))
}

/// Without signals to wait for, runs until the process is killed.
#[cfg(all(not(unix), feature = "reader", feature = "writer"))]
fn wait_for_stop() -> Result<(), String> {
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
