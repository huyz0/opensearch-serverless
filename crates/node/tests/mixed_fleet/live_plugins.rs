//! docs/specs/plugin-proxy: the proxy against real stock plugins. The Java
//! node runs released plugin zips (3.9.0.0 builds, the lang-painless module
//! built from the shell's own tree) and a Rust node proxies the plugin's
//! endpoints to it.

use super::*;

/// Unpacks every `*.zip` in `zips` (job scheduler first, since ISM extends
/// it) into `plugins/<name>`, naming the node's own version in each
/// descriptor so a release built for a patch version loads.
fn install(zips: &Path, plugins: &Path, wanted: &[&str]) -> Vec<String> {
    let mut found: Vec<PathBuf> = std::fs::read_dir(zips)
        .expect("the zip directory")
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "zip"))
        .filter(|p| {
            let name = p
                .file_name()
                .expect("a name")
                .to_string_lossy()
                .into_owned();
            wanted.iter().any(|w| name.starts_with(w))
        })
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

/// One answer, whichever node gave it.
#[derive(Debug, PartialEq, Eq)]
struct Ans {
    status: u16,
    body: String,
}

impl Ans {
    fn text(&self) -> String {
        self.body.clone()
    }
}

/// A Java node running the named plugins, a counting relay in front of it,
/// and a Rust node that proxies to the relay.
struct Rig {
    cluster: Cluster,
    java: JavaNode,
    rust: usize,
    reached: std::sync::Arc<std::sync::atomic::AtomicUsize>,
}

impl Rig {
    /// `None` when the environment lacks a Java build or the zips. `wanted`
    /// are zip name prefixes, e.g. `opensearch-job-scheduler`.
    fn start(wanted: &[&str]) -> Option<Self> {
        let (Some(_), Some(_), Some(zips)) = (
            std::env::var_os("OSS_JAVA_CP"),
            std::env::var_os("OSS_JAVA_BIN"),
            std::env::var_os("OSS_PLUGIN_ZIPS"),
        ) else {
            eprintln!("skipped: OSS_JAVA_BIN, OSS_JAVA_CP and OSS_PLUGIN_ZIPS are not set");
            return None;
        };
        let mut cluster =
            Cluster::new(TTL_MS, 1, std::env::var_os("RUSTFS_BIN").is_some()).expect("the cluster");
        let home = temp("live-plugins");
        let names = install(Path::new(&zips), &home.join("plugins"), wanted);
        assert_eq!(names.len(), wanted.len(), "a zip per name: {names:?}");
        eprintln!("installed {names:?}");
        let java = JavaNode::start("java-plugins", &home, &cluster, false);
        let (relay, reached) = counting_relay(java.addr);
        cluster.set("OSS_PLUGIN_UPSTREAMS", &format!("http://{relay}"));
        let rust = cluster.add("rust-proxy");
        Some(Self {
            cluster,
            java,
            rust,
            reached,
        })
    }

    fn direct(&self, method: &str, path: &str, body: Option<&str>) -> Ans {
        let r = call(self.java.addr, method, path, body);
        Ans {
            status: r.status,
            body: r.text(),
        }
    }

    fn proxied(&self, method: &str, path: &str, body: Option<&str>) -> Ans {
        let r = self.cluster.call(self.rust, method, path, body);
        Ans {
            status: r.status,
            body: r.text(),
        }
    }

    /// Asks through the Rust node, retrying while the plugin answers 500
    /// "retry shortly" (a system index opening on first write).
    fn proxied_settled(&self, method: &str, path: &str, body: Option<&str>) -> Ans {
        let started = Instant::now();
        loop {
            let reply = self.proxied(method, path, body);
            if reply.status != 500 || started.elapsed() > Duration::from_secs(90) {
                return reply;
            }
            std::thread::sleep(Duration::from_secs(2));
        }
    }

    /// Asks both ways and requires the same status and body.
    fn same_both_ways(&self, method: &str, path: &str, body: Option<&str>) -> Ans {
        let direct = self.direct(method, path, body);
        let through = self.proxied(method, path, body);
        assert_eq!(
            (through.status, through.text()),
            (direct.status, direct.text()),
            "{method} {path}"
        );
        direct
    }

    fn requests_reached_java(&self) -> usize {
        self.reached.load(std::sync::atomic::Ordering::SeqCst)
    }

    fn finish(self) {
        self.java.kill();
    }
}

#[test]
#[ignore = "needs OSS_JAVA_BIN, OSS_JAVA_CP and OSS_PLUGIN_ZIPS (a directory of released plugin zips)"]
fn the_ism_endpoints_of_the_real_plugin_answer_through_a_rust_node() {
    let Some(rig) = Rig::start(&["opensearch-job-scheduler", "opensearch-index-management"]) else {
        return;
    };
    let direct = rig.direct("GET", "/_plugins/_ism/policies", None);
    assert_eq!(direct.status, 200, "{}", direct.text());
    assert!(
        !direct.text().contains("no handler found"),
        "{}",
        direct.text()
    );

    let policy = r#"{"policy":{"description":"proxy probe","default_state":"a","states":[{"name":"a","actions":[],"transitions":[]}]}}"#;
    let put = rig.proxied_settled("PUT", "/_plugins/_ism/policies/probe", Some(policy));
    assert!(
        matches!(put.status, 200 | 201),
        "{} {}",
        put.status,
        put.text()
    );
    assert!(put.text().contains("\"_id\":\"probe\""), "{}", put.text());
    let via_java = rig.direct("GET", "/_plugins/_ism/policies/probe", None);
    assert_eq!(via_java.status, 200, "{}", via_java.text());
    assert!(
        via_java.text().contains("proxy probe"),
        "{}",
        via_java.text()
    );
    rig.same_both_ways("GET", "/_plugins/_ism/policies/probe", None);
    let list = rig.proxied("GET", "/_plugins/_ism/policies", None);
    assert_eq!(list.status, 200, "{}", list.text());
    assert!(list.text().contains("probe"), "{}", list.text());
    let gone = rig.proxied("DELETE", "/_plugins/_ism/policies/probe", None);
    assert_eq!(gone.status, 200, "{}", gone.text());
    // A missing policy is a 404 on the shell as on a stock node, from either node.
    let missing = rig.same_both_ways("GET", "/_plugins/_ism/policies/probe", None);
    assert_eq!(missing.status, 404, "{}", missing.text());
    assert!(
        missing.text().contains("Policy not found"),
        "{}",
        missing.text()
    );
    assert!(rig.requests_reached_java() >= 4);
    rig.finish();
}
