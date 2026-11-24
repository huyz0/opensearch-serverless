//! A pinned analyzer name in a mapping (docs/specs/extension-analyzer AC-4):
//! admitted only to a caller that says the index pins it.

use oss_doc::mapping::{Mapping, ParseOptions};

const M: &str = r#"{"properties":{"a":{"type":"text","analyzer":"ext_stem"},
  "b":{"type":"text","search_analyzer":"ext_stem","analyzer":"ext_stem",
       "fields":{"c":{"type":"text","analyzer":"standard"}}},
  "d":{"type":"text","analyzer":"ext_other","search_quote_analyzer":"ext_other"}}}"#;

#[test]
fn a_pinned_name_is_refused_unless_the_caller_pins() {
    let e = Mapping::parse(M.as_bytes()).expect_err("refused");
    assert!(format!("{e:?}").contains("ext_stem"), "{e:?}");
    let e = Mapping::parse_with(M.as_bytes(), ParseOptions::default()).expect_err("refused");
    assert!(format!("{e:?}").contains("analyzer"), "{e:?}");
    assert!(Mapping::parse_pinned(M.as_bytes(), ParseOptions::default()).is_ok());
}

#[test]
fn a_name_that_is_not_a_pin_name_is_refused_even_to_a_caller_that_pins() {
    for bad in ["stem", "ext_", "ext_Up", "whitespace"] {
        let m = format!(r#"{{"properties":{{"a":{{"type":"text","analyzer":"{bad}"}}}}}}"#);
        assert!(
            Mapping::parse_pinned(m.as_bytes(), ParseOptions::default()).is_err(),
            "{bad}"
        );
    }
}

#[test]
fn the_names_a_mapping_uses_are_listed_once_each_in_order() {
    let m = Mapping::parse_pinned(M.as_bytes(), ParseOptions::default()).expect("parses");
    assert_eq!(m.ext_analyzer_names(), vec!["ext_other", "ext_stem"]);
    let plain = Mapping::parse(br#"{"properties":{"a":{"type":"text"}}}"#).expect("parses");
    assert!(plain.ext_analyzer_names().is_empty());
}
