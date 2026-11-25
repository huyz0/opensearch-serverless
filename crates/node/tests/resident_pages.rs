//! docs/specs/resident-pages: the bound on the pages a reader keeps
//! resident. Its own test binary: the pager is process-wide, so its budget
//! and counters belong to this one test.

#![cfg(all(feature = "reader", feature = "writer", feature = "coordinator"))]

#[path = "support/mod.rs"]
mod support;

// ARITH: test arithmetic on counts; an overflow panics the test.
#[allow(clippy::arithmetic_side_effects)]
#[cfg(target_os = "linux")]
mod run {
    use super::support;

    use lucene_store::paged_input::Pager;
    use oss_node::boot::{Node, NodeConfig, start};
    use oss_testkit::TempDir;

    const DOCS: usize = 20_000;
    const BOUND: u64 = 1 << 20;

    fn boot(root: &str, id: &str, roles: &str, bound: Option<&str>) -> Node {
        let lookup = |key: &str| match key {
            "OSS_NODE_ID" => Some(id.to_owned()),
            "OSS_ROLES" => Some(roles.to_owned()),
            "OSS_LEASE_TTL_MS" => Some("30000".to_owned()),
            "OSS_STORE" => Some("fs".to_owned()),
            "OSS_FS_ROOT" => Some(root.to_owned()),
            "OSS_HTTP_ADDR" | "OSS_TRANSPORT_ADDR" => Some("127.0.0.1:0".to_owned()),
            "OSS_STORE_USAGE" => Some("true".to_owned()),
            "OSS_RESIDENT_PAGE_BYTES" => bound.map(str::to_owned),
            _ => None,
        };
        start(&NodeConfig::from_lookup(lookup).expect("valid")).expect("starts")
    }

    fn fill(node: &Node) {
        let addr = node.addr();
        assert_eq!(
            support::request(
                addr,
                "PUT",
                "/big",
                Some(
                    br#"{"mappings":{"properties":{"body":{"type":"text"},"n":{"type":"long"}}}}"#
                )
            )
            .status,
            200
        );
        let mut body = String::new();
        let mut x: u64 = 0x9E37_79B9_7F4A_7C15;
        let mut next = || {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            x
        };
        for i in 0..DOCS {
            body.push_str(&format!("{{\"index\":{{\"_id\":\"{i}\"}}}}\n"));
            let words: Vec<String> = (0..30).map(|_| format!("w{}", next() % 50_000)).collect();
            let noise: String = (0..12).map(|_| format!("{:016x}", next())).collect();
            body.push_str(&format!(
                "{{\"body\":\"{} {noise}\",\"n\":{i}}}\n",
                words.join(" ")
            ));
        }
        let r = support::request(
            addr,
            "POST",
            "/big/_bulk?refresh=true",
            Some(body.as_bytes()),
        );
        assert_eq!(r.status, 200, "{}", r.text());
    }

    /// Searches until the index answers: a new boot serves it once its
    /// shard has caught up with the published segments.
    fn await_ready(node: &Node) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
        loop {
            let r = support::request(node.addr(), "GET", "/big/_count", None);
            if r.status == 200 && r.text().contains(&format!("\"count\":{DOCS}")) {
                return;
            }
            assert!(std::time::Instant::now() < deadline, "{}", r.text());
            std::thread::sleep(std::time::Duration::from_millis(250));
        }
    }

    fn searches(node: &Node, n: usize) -> Vec<Option<String>> {
        let mut all = Vec::new();
        for i in 0..n {
            let q = format!(
                r#"{{"size":50,"query":{{"match":{{"body":"w{}"}}}}}}"#,
                i * 37 % 50_000
            );
            let r = support::request(node.addr(), "POST", "/big/_search", Some(q.as_bytes()));
            assert_eq!(r.status, 200, "{}", r.text());
            all.push(r.text().split_once("\"hits\"").map(|(_, h)| h.to_owned()));
        }
        all
    }

    fn usage(node: &Node) -> String {
        support::request(node.addr(), "GET", "/_serverless/usage", None).text()
    }

    // AC-1 (pool refusal), AC-2 to AC-5
    #[test]
    fn a_bound_holds_resident_pages_and_forces_refaults_and_answers_are_unchanged() {
        let pager = Pager::global().expect("userfaultfd in this environment");
        let dir = TempDir::new("resident-pages");
        let root = dir.path().display().to_string();

        // Write the index and wait for it to be published, so the next
        // boots read segments from the store rather than the writer's memory.
        let writer = boot(&root, "w1", "ingest", None);
        fill(&writer);

        // Unbounded (setting 0): the working set stays resident.
        let open = boot(&root, "s1", "search", None);
        await_ready(&open);
        let before = searches(&open, 300);
        assert!(
            !usage(&open).contains("resident_pages"),
            "no report when the setting is 0"
        );
        assert_eq!(pager.budget(), lucene_store::paged_input::DEFAULT_BUDGET);
        let evicted = pager.stats().evictions;
        assert_eq!(evicted, 0, "nothing past the default budget");
        assert!(
            pager.resident_bytes() > 2 * BOUND,
            "the index is bigger than the bound"
        );
        drop(open);

        // Bounded: same index, far below its pages.
        let bound = boot(&root, "s2", "search", Some(&BOUND.to_string()));
        await_ready(&bound);
        assert_eq!(pager.budget(), BOUND, "AC-2");
        let after = searches(&bound, 300);
        let report = usage(&bound);
        assert!(report.contains("\"resident_pages\""), "AC-5: {report}");
        let stats = pager.stats();
        assert!(stats.evictions > 0 && stats.refaults > 0, "AC-3: {stats:?}");
        assert!(
            pager.resident_bytes() <= BOUND + (BOUND / 4),
            "AC-3: resident {} over bound {BOUND}",
            pager.resident_bytes()
        );
        assert_eq!(before, after, "AC-4: every answer");
    }
}
