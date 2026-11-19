# Verified: one signed registry holds every action a node serves

- AC-1: `an_unsigned_request_never_reaches_a_handler` in crates/node/tests/handlers.rs
- AC-2: `a_signed_tail_push_reaches_the_held_writes` in crates/node/tests/handlers.rs
- AC-3: `a_second_handler_for_an_action_is_refused` in crates/node/tests/handlers.rs
- AC-4: `the_registry_lists_what_it_holds` in crates/node/tests/handlers.rs
