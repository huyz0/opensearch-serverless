//! What each tenant's extension module calls cost, as the
//! `GET /_serverless/usage` body reports it: calls, fuel, and the bytes in
//! and out. Written only once a module has been called, so a node that runs
//! none answers as before.

use std::io::Write;

use oss_ext_wasm::Usage;

use oss_rest::write_json_str;

/// Writes `,"extensions":{"tenants":{...}}` for `tenants`, the `top` that
/// ran the most fuel when `top` is given, and nothing for none.
pub(crate) fn write(
    out: &mut Vec<u8>,
    mut tenants: Vec<(String, Usage)>,
    top: Option<usize>,
) -> std::io::Result<()> {
    if tenants.is_empty() {
        return Ok(());
    }
    if let Some(n) = top {
        tenants.sort_by(|a, b| b.1.fuel.cmp(&a.1.fuel).then_with(|| a.0.cmp(&b.0)));
        tenants.truncate(n);
    }
    out.write_all(b",\"extensions\":{\"tenants\":{")?;
    for (i, (name, u)) in tenants.iter().enumerate() {
        if i > 0 {
            out.write_all(b",")?;
        }
        write_json_str(&mut *out, name)?;
        write!(
            out,
            ":{{\"calls\":{},\"fuel\":{},\"input_bytes\":{},\"output_bytes\":{}}}",
            u.calls, u.fuel, u.input_bytes, u.output_bytes
        )?;
    }
    out.write_all(b"}}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn usage(calls: u64, fuel: u64) -> Usage {
        Usage {
            calls,
            fuel,
            input_bytes: 5,
            output_bytes: 7,
        }
    }

    fn text(tenants: Vec<(String, Usage)>, top: Option<usize>) -> String {
        let mut out = Vec::new();
        write(&mut out, tenants, top).expect("writes");
        String::from_utf8(out).expect("utf8")
    }

    #[test]
    fn a_node_that_ran_no_module_adds_nothing() {
        assert_eq!(text(Vec::new(), None), "");
    }

    #[test]
    fn each_tenant_reports_its_four_counters() {
        let t = vec![("a".to_owned(), usage(2, 100))];
        assert_eq!(
            text(t, None),
            r#","extensions":{"tenants":{"a":{"calls":2,"fuel":100,"input_bytes":5,"output_bytes":7}}}"#
        );
    }

    #[test]
    fn top_keeps_the_tenants_that_ran_the_most_fuel() {
        let t = vec![
            ("a".to_owned(), usage(1, 10)),
            ("b".to_owned(), usage(1, 30)),
            ("c".to_owned(), usage(1, 20)),
        ];
        let body = text(t, Some(2));
        assert!(body.contains("\"b\"") && body.contains("\"c\""), "{body}");
        assert!(!body.contains("\"a\""), "{body}");
    }
}
