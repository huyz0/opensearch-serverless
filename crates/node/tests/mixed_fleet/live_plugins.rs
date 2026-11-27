//! docs/specs/plugin-proxy: the proxy against real stock plugins. The Java
//! node runs Job Scheduler and Index Management 3.9.0.0 from their released zips,
//! and a Rust node proxies the ISM endpoints to it.

use super::*;

/// Unpacks every `*.zip` in `zips` (job scheduler first, since ISM extends
/// it) into `plugins/<name>`, naming the node's own version in each
/// descriptor so a release built for a patch version loads.
fn install(zips: &Path, plugins: &Path) -> Vec<String> {
    let mut found: Vec<PathBuf> = std::fs::read_dir(zips)
        .expect("the zip directory")
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "zip"))
        .collect();
    found.sort();
    let mut names = Vec::new();
    for zip in found {
        let name = zip
            .file_stem()
            .expect("a stem")
            .to_string_lossy()
            .into_owned();
        let to = plugins.join(&name);
        std::fs::create_dir_all(&to).expect("a plugin directory");
        let status = Command::new("unzip")
            .arg("-q")
            .arg("-o")
            .arg(&zip)
            .arg("-d")
            .arg(&to)
            .status()
            .expect("unzip runs");
        assert!(status.success(), "unzip {}", zip.display());
        // A plugin jar the node's own classpath already carries is jar hell
        // here (a classic node keeps those jars in module class loaders); the
        // plugin then runs on the node's copy.
        for drop in std::env::var("OSS_PLUGIN_DROP")
            .unwrap_or_default()
            .split(',')
            .filter(|d| !d.is_empty())
        {
            for entry in std::fs::read_dir(&to)
                .expect("the plugin directory")
                .flatten()
            {
                if entry.file_name().to_string_lossy().starts_with(drop) {
                    std::fs::remove_file(entry.path()).expect("dropped");
                }
            }
        }
        let descriptor = to.join("plugin-descriptor.properties");
        let text = std::fs::read_to_string(&descriptor).expect("a descriptor");
        let patched: String = text
            .lines()
            .map(|l| {
                if l.starts_with("opensearch.version=") {
                    "opensearch.version=3.10.0".to_owned()
                } else {
                    l.to_owned()
                }
            })
            .collect::<Vec<_>>()
            .join("\n");
        std::fs::write(&descriptor, patched).expect("patched");
        names.push(name);
    }
    names
}

#[test]
#[ignore = "needs OSS_JAVA_BIN, OSS_JAVA_CP and OSS_PLUGIN_ZIPS (a directory of released plugin zips)"]
fn the_ism_endpoints_of_the_real_plugin_answer_through_a_rust_node() {
    let (Some(_), Some(_), Some(zips)) = (
        std::env::var_os("OSS_JAVA_CP"),
        std::env::var_os("OSS_JAVA_BIN"),
        std::env::var_os("OSS_PLUGIN_ZIPS"),
    ) else {
        eprintln!("skipped: OSS_JAVA_BIN, OSS_JAVA_CP and OSS_PLUGIN_ZIPS are not set");
        return;
    };
    let mut cluster =
        Cluster::new(TTL_MS, 1, std::env::var_os("RUSTFS_BIN").is_some()).expect("the cluster");
    let home = temp("live-plugins");
    let names = install(Path::new(&zips), &home.join("plugins"));
    eprintln!("installed {names:?}");
    let java = JavaNode::start("java-ism", &home, &cluster, false);
    let (relay, reached) = counting_relay(java.addr);

    // The real plugin answers on the Java node itself.
    let direct = call(java.addr, "GET", "/_plugins/_ism/policies", None);
    assert_eq!(direct.status, 200, "{}", direct.text());
    assert!(
        !direct.text().contains("no handler found"),
        "{}",
        direct.text()
    );

    cluster.set("OSS_PLUGIN_UPSTREAMS", &format!("http://{relay}"));
    let rust = cluster.add("rust-proxy");
    let policy = r#"{"policy":{"description":"proxy probe","default_state":"a","states":[{"name":"a","actions":[],"transitions":[]}]}}"#;
    // The plugin's system index opens on first write; the Java node answers
    // 500 "retry shortly" until its lease and shard heads settle.
    let started = Instant::now();
    let put = loop {
        let put = cluster.call(rust, "PUT", "/_plugins/_ism/policies/probe", Some(policy));
        if put.status != 500 || started.elapsed() > Duration::from_secs(90) {
            break put;
        }
        std::thread::sleep(Duration::from_secs(2));
    };
    assert!(
        matches!(put.status, 200 | 201),
        "{} {}",
        put.status,
        put.text()
    );
    assert!(put.text().contains("\"_id\":\"probe\""), "{}", put.text());

    // What Rust wrote through the proxy, the Java node reads, and the other way round.
    let via_java = call(java.addr, "GET", "/_plugins/_ism/policies/probe", None);
    assert_eq!(via_java.status, 200, "{}", via_java.text());
    assert!(
        via_java.text().contains("proxy probe"),
        "{}",
        via_java.text()
    );
    let via_rust = cluster.call(rust, "GET", "/_plugins/_ism/policies/probe", None);
    assert_eq!(via_rust.status, 200, "{}", via_rust.text());
    assert_eq!(
        via_java.text().contains("proxy probe"),
        via_rust.text().contains("proxy probe")
    );
    let list = cluster.call(rust, "GET", "/_plugins/_ism/policies", None);
    assert_eq!(list.status, 200, "{}", list.text());
    assert!(list.text().contains("probe"), "{}", list.text());
    let gone = cluster.call(rust, "DELETE", "/_plugins/_ism/policies/probe", None);
    assert_eq!(gone.status, 200, "{}", gone.text());
    // A missing policy is the plugin's own answer, whichever node is asked.
    let missing = call(java.addr, "GET", "/_plugins/_ism/policies/probe", None);
    let missing_via = cluster.call(rust, "GET", "/_plugins/_ism/policies/probe", None);
    assert!(
        missing.text().contains("Policy not found"),
        "{}",
        missing.text()
    );
    assert_eq!(
        (missing_via.status, missing_via.text()),
        (missing.status, missing.text())
    );
    assert!(reached.load(std::sync::atomic::Ordering::SeqCst) >= 4);
    java.kill();
}
