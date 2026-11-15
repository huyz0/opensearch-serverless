# SigV4 test vectors

Test data only: AWS's published Signature Version 4 test suite, as kept in
botocore (`tests/unit/auth/aws4_testsuite`, Apache-2.0), copied unchanged on
2026-11-15. Each directory holds a request (`.req`) and the canonical request
(`.creq`), string to sign (`.sts`) and `Authorization` header (`.authz`) AWS
computed for it, with the suite's example credentials (`AKIDEXAMPLE`), region
`us-east-1` and service `service`. Nothing here is sent anywhere:
`tests/s3/sigv4.rs` signs each request offline and compares the three files
byte for byte.

Left out: the suite's path-normalisation cases (`get-slash`, `get-relative`,
...), because S3 signs the path as sent and never normalises it, the
multi-line header case, and the session-token and pre-signed cases, which
this client does not use.
