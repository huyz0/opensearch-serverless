//! The decision's tests: actions, retries and transitions against a fake backend.

use super::*;
use backend::Unsupported;
use serde_json::json;

fn ev(record: &Value, policy: &Value, now: i64) -> Step {
    evaluate(record, policy, now, &Unsupported)
}

/// A backend that records the blocks it is asked for.
#[derive(Default)]
struct Fake {
    asked: std::sync::Mutex<Vec<(String, bool)>>,
    refuse: bool,
}

impl Backend for Fake {
    fn set_replicas(&self, index: &str, n: u32) -> Result<(), String> {
        if self.refuse {
            return Err("index [logs] is missing".to_owned());
        }
        self.asked
            .lock()
            .unwrap()
            .push((format!("{index}:{n}"), false));
        Ok(())
    }

    fn delete(&self, index: &str) -> Result<(), String> {
        if self.refuse {
            return Err("index [logs] is missing".to_owned());
        }
        self.asked.lock().unwrap().push((index.to_owned(), false));
        Ok(())
    }

    fn notify(&self, channel: &str, text: &str) -> Result<(), String> {
        if self.refuse {
            return Err("channel [c1] is disabled".to_owned());
        }
        self.asked
            .lock()
            .unwrap()
            .push((format!("notify {channel}: {text}"), false));
        Ok(())
    }

    fn set_alias(
        &self,
        index: &str,
        add: bool,
        alias: &str,
        write: Option<bool>,
    ) -> Result<(), String> {
        if self.refuse {
            return Err("index [logs] is missing".to_owned());
        }
        let verb = if add { "add" } else { "remove" };
        self.asked
            .lock()
            .unwrap()
            .push((format!("{verb} {alias} {index} {write:?}"), false));
        Ok(())
    }

    fn set_read_only(&self, index: &str, on: bool) -> Result<(), String> {
        if self.refuse {
            return Err("index [logs] is missing".to_owned());
        }
        self.asked.lock().unwrap().push((index.to_owned(), on));
        Ok(())
    }
}

fn acting(actions: Value) -> Value {
    json!({"policy_id":"p","default_state":"a","states":[
        {"name":"a","actions":actions,"transitions":[{"state_name":"b"}]},
        {"name":"b","actions":[],"transitions":[]}]})
}

fn started() -> Value {
    let mut r = record();
    r["state"] = json!({"name":"a","start_time":1000});
    r["info"] = json!({"message":"Successfully initialized policy: p"});
    r
}

#[test]
fn a_read_only_action_blocks_the_index_and_leaves_the_stock_shapes() {
    let p = acting(json!([{"read_only":{}}]));
    let fake = Fake::default();
    let s = evaluate(&started(), &p, 7000, &fake);
    assert_eq!(*fake.asked.lock().unwrap(), [("logs".to_owned(), true)]);
    assert_eq!(s.record["action"]["name"], "read_only");
    assert_eq!(s.record["action"]["index"], 0);
    assert_eq!(s.record["step"]["name"], "set_read_only");
    assert_eq!(s.record["step"]["step_status"], "completed");
    assert_eq!(
        s.record["info"]["message"],
        "Successfully set index to read-only [index=logs]"
    );
    assert_eq!(s.next_eval, Some(7000));
}

#[test]
fn actions_run_in_order_then_the_transition_is_evaluated() {
    let p = acting(json!([{"read_only":{}},{"read_write":{}}]));
    let fake = Fake::default();
    let one = evaluate(&started(), &p, 7000, &fake).record;
    let two = evaluate(&one, &p, 7001, &fake).record;
    assert_eq!(two["step"]["name"], "set_read_write");
    assert_eq!(
        *fake.asked.lock().unwrap(),
        [("logs".to_owned(), true), ("logs".to_owned(), false)]
    );
    let moved = evaluate(&two, &p, 7002, &fake).record;
    assert_eq!(moved["state"]["name"], "b");
    assert!(moved["action_index"].is_null());
    assert_eq!(fake.asked.lock().unwrap().len(), 2, "nothing ran twice");
}

#[test]
fn a_refused_block_fails_the_step_and_waits_for_a_retry() {
    let p = acting(json!([{"read_only":{}}]));
    let fake = Fake {
        refuse: true,
        ..Fake::default()
    };
    let s = evaluate(&started(), &p, 7000, &fake);
    assert_eq!(s.record["step"]["step_status"], "failed");
    assert_eq!(s.record["action"]["failed"], true);
    assert_eq!(s.record["retry_info"]["failed"], true);
    assert_eq!(s.record["info"]["message"], "index [logs] is missing");
    assert_eq!(s.next_eval, None);
    assert!(
        s.record["action_index"].is_null(),
        "the action is still the one to run"
    );
}

#[test]
fn a_failing_action_is_retried_with_its_backoff_then_waits_for_a_manual_retry() {
    let retry = json!({"count":2,"backoff":"exponential","delay":"1m"});
    let p = acting(json!([{"read_only":{},"retry":retry}]));
    let fake = Fake {
        refuse: true,
        ..Fake::default()
    };
    let one = evaluate(&started(), &p, 7000, &fake);
    assert_eq!(one.next_eval, Some(67_000));
    assert_eq!(one.record["step"]["step_status"], "failed");
    assert_eq!(one.record["action"]["failed"], false);
    assert_eq!(one.record["action"]["consumed_retries"], 1);
    assert_eq!(one.record["action"]["last_retry_time"], 7000);
    let two = evaluate(&one.record, &p, 67_000, &fake);
    assert_eq!(two.next_eval, Some(187_000), "the delay doubles");
    assert_eq!(two.record["action"]["consumed_retries"], 2);
    let last = evaluate(&two.record, &p, 187_000, &fake);
    assert_eq!(last.next_eval, None);
    assert_eq!(last.record["action"]["failed"], true);
    assert_eq!(last.record["retry_info"]["failed"], true);
    assert_eq!(last.record["action"]["consumed_retries"], 2);
    // A retry that works moves on.
    let ok = evaluate(&two.record, &p, 187_000, &Fake::default());
    assert_eq!(ok.record["step"]["step_status"], "completed");
    assert_eq!(ok.record["action_index"], 1);
}

#[test]
fn linear_and_constant_backoffs_wait_as_named() {
    let at = |backoff: &str, consumed| {
        wait(
            &json!({"retry":{"backoff":backoff,"delay":"10s"}}),
            consumed,
        )
    };
    assert_eq!(
        (at("constant", 3), at("linear", 2), at("exponential", 3)),
        (10_000, 30_000, 80_000)
    );
}

#[test]
fn a_delete_action_deletes_the_index_and_marks_the_record_for_removal() {
    let p = acting(json!([{"delete":{}}]));
    let fake = Fake::default();
    let s = evaluate(&started(), &p, 7000, &fake);
    assert_eq!(*fake.asked.lock().unwrap(), [("logs".to_owned(), false)]);
    assert_eq!(s.record["step"]["name"], "attempt_delete");
    assert_eq!(s.record["step"]["step_status"], "completed");
    assert_eq!(s.record["deleted"], true);
    let refused = Fake {
        refuse: true,
        ..Fake::default()
    };
    let s = evaluate(&started(), &p, 7000, &refused);
    assert_eq!(s.record["step"]["step_status"], "failed");
    assert!(s.record["deleted"].is_null());
}

#[test]
fn a_replica_count_action_sets_the_number_and_a_missing_number_fails() {
    let p = acting(json!([{"replica_count":{"number_of_replicas":0}}]));
    let fake = Fake::default();
    let s = evaluate(&started(), &p, 7000, &fake);
    assert_eq!(*fake.asked.lock().unwrap(), [("logs:0".to_owned(), false)]);
    assert_eq!(s.record["step"]["name"], "attempt_set_replica_count");
    assert_eq!(
        s.record["info"]["message"],
        "Successfully updated number_of_replicas to 0 [index=logs]"
    );
    let p = acting(json!([{"replica_count":{}}]));
    let s = evaluate(&started(), &p, 7000, &fake);
    assert_eq!(s.record["step"]["step_status"], "failed");
    assert_eq!(fake.asked.lock().unwrap().len(), 1, "nothing was asked");
}

#[test]
fn an_alias_action_adds_and_removes_each_alias_in_order() {
    let p = acting(json!([{"alias":{"actions":[
        {"add":{"aliases":["a","b"],"is_write_index":true}},
        {"remove":{"alias":"old"}}]}}]));
    let fake = Fake::default();
    let s = evaluate(&started(), &p, 7000, &fake);
    assert_eq!(
        *fake.asked.lock().unwrap(),
        [
            ("add a logs Some(true)".to_owned(), false),
            ("add b logs Some(true)".to_owned(), false),
            ("remove old logs None".to_owned(), false),
        ]
    );
    assert_eq!(s.record["step"]["name"], "attempt_alias");
    assert_eq!(s.record["step"]["step_status"], "completed");
    assert_eq!(
        s.record["info"]["message"],
        "Successfully updated aliases [index=logs]"
    );
    let none = acting(json!([{"alias":{"actions":[]}}]));
    let s = evaluate(&started(), &none, 7000, &fake);
    assert_eq!(s.record["step"]["step_status"], "failed");
    let refused = Fake {
        refuse: true,
        ..Fake::default()
    };
    let s = evaluate(&started(), &p, 7000, &refused);
    assert_eq!(s.record["step"]["step_status"], "failed");
}

#[test]
fn a_notification_action_sends_its_text_with_the_index_filled_in() {
    let p = acting(json!([{"notification":{"channel":{"id":"c1"},
        "message_template":{"source":"{{ctx.index}} moved"}}}]));
    let fake = Fake::default();
    let s = evaluate(&started(), &p, 7000, &fake);
    assert_eq!(
        *fake.asked.lock().unwrap(),
        [("notify c1: logs moved".to_owned(), false)]
    );
    assert_eq!(s.record["step"]["name"], "attempt_notification");
    assert_eq!(s.record["step"]["step_status"], "completed");
    assert_eq!(
        s.record["info"]["message"],
        "Successfully sent notification [index=logs]"
    );
    let refused = Fake {
        refuse: true,
        ..Fake::default()
    };
    let s = evaluate(&started(), &p, 7000, &refused);
    assert_eq!(s.record["step"]["step_status"], "failed");
    assert_eq!(s.record["info"]["message"], "channel [c1] is disabled");
    let bare = acting(json!([{"notification":{"message_template":{"source":"x"}}}]));
    let s = evaluate(&started(), &bare, 7000, &fake);
    assert_eq!(s.record["step"]["step_status"], "failed");
}

#[test]
fn an_action_the_node_cannot_run_yet_fails_naming_itself() {
    let p = acting(json!([{"snapshot":{}}]));
    let s = ev(&started(), &p, 7000);
    assert_eq!(s.record["step"]["step_status"], "failed");
    assert_eq!(
        s.record["info"]["message"],
        "action [snapshot] cannot run on this node yet"
    );
}

const DAY: i64 = 86_400_000;

fn policy(condition: Value) -> Value {
    json!({"policy_id":"p","default_state":"hot","states":[
        {"name":"hot","actions":[],"transitions":[{"state_name":"ro","conditions":condition}]},
        {"name":"ro","actions":[{"read_only":{}}],"transitions":[]}]})
}

fn record() -> Value {
    json!({"policy_id":"p","index":"logs","index_creation_date":1000,
           "state":{"name":"hot","start_time":1000},"enabled":true})
}

#[test]
fn the_first_run_initialises_and_asks_to_run_again_now() {
    let s = ev(&record(), &policy(json!({"min_index_age":"1d"})), 5000);
    assert_eq!(
        s.record["info"]["message"],
        "Successfully initialized policy: p"
    );
    assert_eq!(
        s.record["retry_info"],
        json!({"failed":false,"consumed_retries":0})
    );
    assert_eq!(s.next_eval, Some(5000));
}

#[test]
fn an_unmet_age_waits_until_the_index_is_old_enough() {
    let first = ev(&record(), &policy(json!({"min_index_age":"1d"})), 5000).record;
    let s = ev(&first, &policy(json!({"min_index_age":"1d"})), 6000);
    assert_eq!(s.record["step"]["step_status"], "condition_not_met");
    assert_eq!(s.record["step"]["name"], "attempt_transition_step");
    assert_eq!(s.record["action"]["name"], "transition");
    assert_eq!(
        s.record["info"]["message"],
        "Evaluating transition conditions [index=logs]"
    );
    assert_eq!(s.next_eval, Some(1000 + DAY));
}

#[test]
fn a_met_age_moves_to_the_target_state() {
    let p = policy(json!({"min_index_age":"1d"}));
    let first = ev(&record(), &p, 5000).record;
    let s = ev(&first, &p, 1000 + DAY);
    assert_eq!(
        s.record["state"],
        json!({"name":"ro","start_time":1000 + DAY})
    );
    assert!(s.record["action"].is_null() && s.record["step"].is_null());
    assert_eq!(s.next_eval, Some(1000 + DAY));
}

#[test]
fn a_transition_without_conditions_is_always_met() {
    let p = policy(Value::Null);
    let first = ev(&record(), &p, 5000).record;
    assert_eq!(ev(&first, &p, 6000).record["state"]["name"], "ro");
}

#[test]
fn a_rollover_age_without_a_rollover_waits_for_an_outside_change() {
    let p = policy(json!({"min_rollover_age":"1h"}));
    let first = ev(&record(), &p, 5000).record;
    assert_eq!(ev(&first, &p, 9000).next_eval, None);
    let mut rolled = first;
    rolled["rolled_over_at"] = json!(7000);
    assert_eq!(ev(&rolled, &p, 9000).next_eval, Some(7000 + 3_600_000));
}
