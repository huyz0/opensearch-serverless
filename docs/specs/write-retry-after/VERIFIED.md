# Verified: a refused single-document write's Retry-After

- AC-1: `a_document_the_limiter_refuses_answers_429_with_a_retry_after` in crates/node/tests/server/doc_route.rs
- AC-2: `the_hint_a_refusal_would_give_is_asked_without_refusing` in crates/writer/src/admission/limiter.rs
