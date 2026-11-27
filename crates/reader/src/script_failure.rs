//! The failure a script's exception becomes: the engine reports it as one
//! `IllegalArgument` message the Java plugin decodes
//! (`lucene_engine::painless_score::SCRIPT_ERROR`); here it is a 400
//! `script_exception` caused by the exception the script threw.

use lucene_engine::painless_score::SCRIPT_ERROR;

use crate::failure::Failure;

/// The underscore-case type of a Java exception class.
fn kind_of(class: &str) -> &'static str {
    match class.rsplit('.').next().unwrap_or(class) {
        "IllegalArgumentException" => "illegal_argument_exception",
        "IllegalStateException" => "illegal_state_exception",
        "ArithmeticException" => "arithmetic_exception",
        "ClassCastException" => "class_cast_exception",
        "NullPointerException" => "null_pointer_exception",
        "NumberFormatException" => "number_format_exception",
        "ArrayIndexOutOfBoundsException" => "array_index_out_of_bounds_exception",
        "IndexOutOfBoundsException" => "index_out_of_bounds_exception",
        "UnsupportedOperationException" => "unsupported_operation_exception",
        _ => "exception",
    }
}

/// The failure a script error message stands for, when it is one.
pub(crate) fn of_message(message: &str) -> Option<Failure> {
    let rest = message.strip_prefix(SCRIPT_ERROR)?;
    // script id, leaf, doc, score, offset, class, then `1<message>` or `0`.
    let mut parts = rest.splitn(7, '\u{1}');
    let class = parts.nth(5)?;
    let detail = parts.next()?;
    let text = detail.strip_prefix('1').unwrap_or_default();
    let cause = Failure::plain(kind_of(class), text);
    // ALLOC: error path; one failure.
    let mut failure = Failure::opensearch("script_exception", "runtime error");
    failure.cause = Some(Box::new(cause));
    Some(failure)
}

#[cfg(test)]
mod tests {
    use super::*;

    // docs/specs/painless-script-score AC-4
    #[test]
    fn a_scripts_exception_is_a_script_exception_with_its_cause() {
        let m = format!(
            "{SCRIPT_ERROR}0\u{1}0\u{1}0\u{1}0\u{1}-1\u{1}java.lang.IllegalArgumentException\u{1}1No field found for [nope] in mapping"
        );
        let f = of_message(&m).expect("a script error");
        assert_eq!(f.kind, "script_exception");
        let cause = f.cause.expect("cause");
        assert_eq!(cause.kind, "illegal_argument_exception");
        assert_eq!(cause.reason, "No field found for [nope] in mapping");
        assert!(of_message("an engine failure").is_none());
    }
}
