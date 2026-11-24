# Verified: tenant publish stretch

- AC-1: `a_stretched_age_waits_that_many_times_longer_and_one_changes_nothing` in crates/writer/src/publish/lazy.rs
- AC-2: `an_enforced_tenant_at_level_one_publishes_four_times_less_often` in crates/node/tests/tenants.rs
- AC-3: `a_stretch_asked_by_index_name_lengthens_the_wait_of_that_index` in crates/node/tests/roles/writer_publish.rs
- AC-1 (operation bound, age zero): `a_stretch_does_not_lift_the_operation_bound_and_age_zero_is_never_stretched` in crates/writer/src/publish/lazy.rs
