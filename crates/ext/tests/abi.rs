//! The ABI version rule, the status codes and the typed failures
//! (docs/specs/extension-abi/SPEC.md AC-1, AC-3, AC-4).

use oss_ext::{
    ABI_VERSION, CallError, FailureKind, MAX_DESCRIPTOR_BYTES, Status, VersionError, abi_accepted,
    pack_abi, status_of, unpack_abi,
};

// docs/specs/extension-abi AC-1
#[test]
fn a_module_is_accepted_for_the_same_major_and_a_minor_at_most_the_hosts() {
    assert_eq!(abi_accepted(ABI_VERSION), Ok(()));
    let (major, minor) = unpack_abi(ABI_VERSION);
    assert_eq!(pack_abi(major, minor), ABI_VERSION);
    // An older minor is accepted; a newer minor and another major are not.
    if minor > 0 {
        assert_eq!(abi_accepted(pack_abi(major, minor - 1)), Ok(()));
    }
    assert_eq!(
        abi_accepted(pack_abi(major, minor + 1)),
        Err(VersionError::NewerMinor {
            module: minor + 1,
            host: minor
        })
    );
    assert_eq!(
        abi_accepted(pack_abi(major + 1, 0)),
        Err(VersionError::OtherMajor {
            module: major + 1,
            host: major
        })
    );
    assert_eq!(
        abi_accepted(pack_abi(major.wrapping_sub(1), 0)),
        Err(VersionError::OtherMajor {
            module: major.wrapping_sub(1),
            host: major
        })
    );
}

// docs/specs/extension-abi AC-3
#[test]
fn zero_is_success_every_failure_is_negative_and_anything_else_is_malformed() {
    assert_eq!(status_of(0), Ok(Status::Ok));
    for (code, kind) in [
        (-1, Status::BufferTooSmall),
        (-2, Status::InvalidArgument),
        (-3, Status::Unsupported),
        (-4, Status::Limit),
        (-5, Status::Cancelled),
        (-6, Status::Module),
        (-7, Status::NotInitialized),
    ] {
        assert_eq!(status_of(code), Ok(kind), "{code}");
        assert_eq!(kind.code(), code);
    }
    for code in [1, 100, -8, -1000, i32::MIN, i32::MAX] {
        assert_eq!(status_of(code), Err(code), "{code} is not defined");
    }
}

// docs/specs/extension-abi AC-3
#[test]
fn a_failure_names_its_kind_and_the_http_status_it_answers() {
    // A buffer too small is final: it is the module's failure, not a retry.
    let e = CallError::from_status(Status::BufferTooSmall, &[]);
    assert_eq!(e.kind(), FailureKind::OutputTooLarge);
    assert_eq!(e.http_status(), 400);
    // Capacity is 429; a deterministic failure of one input is 400.
    assert_eq!(CallError::busy().http_status(), 429);
    assert_eq!(
        CallError::from_status(Status::Limit, &[]).http_status(),
        400
    );
    assert_eq!(
        CallError::from_status(Status::Module, b"boom").http_status(),
        400
    );
    assert_eq!(CallError::trap("out of fuel").http_status(), 400);
    // An undefined code is a malformed module, not an input failure.
    assert_eq!(CallError::malformed(-42).kind(), FailureKind::Malformed);
    assert_eq!(CallError::malformed(-42).http_status(), 502);
}

// docs/specs/extension-abi AC-3
#[test]
fn a_module_s_detail_is_bounded_and_cannot_forge_a_log_line() {
    let long = vec![b'x'; 10_000];
    let e = CallError::from_status(Status::Module, &long);
    assert!(e.detail().len() <= oss_ext::MAX_ERROR_DETAIL_BYTES);
    let e = CallError::from_status(Status::Module, b"line one\nline two\x00\x1b[31m");
    assert!(
        !e.detail().chars().any(char::is_control),
        "{:?}",
        e.detail()
    );
    let e = CallError::from_status(Status::Module, &[0xff, 0xfe, b'o', b'k']);
    assert!(e.detail().contains("ok"));
}

// docs/specs/extension-abi AC-4
#[test]
fn the_ceilings_are_the_ones_the_spec_names() {
    assert_eq!(MAX_DESCRIPTOR_BYTES, 16 * 1024);
    assert_eq!(oss_ext::MAX_CONFIG_BYTES, 64 * 1024);
    assert_eq!(oss_ext::MAX_MODULE_BYTES, 8 * 1024 * 1024);
}
