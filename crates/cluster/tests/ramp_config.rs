//! docs/specs/reader-slow-start, Switch: the process's slow start setting and
//! the clock it ranks by. One test, so the process-wide setting is its own.

use oss_cluster::placement::{RampConfig, now_millis, ramp_config, set_ramp_config};

#[test]
fn the_process_setting_is_what_was_set_and_the_clock_is_the_wall_clock() {
    assert!(!ramp_config().is_on(), "off until set");
    assert_eq!(ramp_config(), RampConfig::OFF);
    let on = RampConfig {
        min_ms: 30_000,
        max_ms: 300_000,
    };
    set_ramp_config(on);
    assert_eq!(ramp_config(), on);
    assert!(ramp_config().is_on());
    // A ceiling of one millisecond is on; a ceiling of zero is not.
    set_ramp_config(RampConfig {
        min_ms: 0,
        max_ms: 1,
    });
    assert!(ramp_config().is_on());
    set_ramp_config(RampConfig::OFF);
    assert!(!ramp_config().is_on());

    // Epoch milliseconds: after 2023 and not a day past the machine's own clock.
    let now = now_millis();
    assert!(now > 1_700_000_000_000, "{now}");
    let again = now_millis();
    assert!(again >= now && again - now < 86_400_000, "{now} {again}");
}
