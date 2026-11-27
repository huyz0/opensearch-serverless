//! `function_score`'s `random_score`: `RandomScoreFunctionBuilder.fromXContent`
//! and `doToFunction` at OpenSearch 3.10.0, over the engine's random score.

use lucene_engine::function_score::{Function as Engine, Numeric, RandomField, mix};

use crate::dsl::error::DslError;
use crate::dsl::queries::function_score::Function;
use crate::dsl::tokens::{Kind, Parser};
use crate::failure::Failure;
use crate::lucene_query::{QueryContext, unsupported};
use crate::mapping::FieldType;

/// `fromXContent`: the parser is on the object's start.
pub fn parse(p: &mut Parser<'_>) -> Result<Function, DslError> {
    let mut seed = None;
    let mut field = None;
    let mut current = "";
    loop {
        match p.next_token() {
            Some(Kind::EndObject) | None => break,
            Some(Kind::FieldName) => current = p.text(),
            Some(k) if k.is_value() => match current {
                "seed" => seed = Some(seed_value(p, k)?),
                // ALLOC: one name a random score.
                "field" => field = Some(p.text().to_owned()),
                "_name" => return Err(DslError::unsupported("[function_score] [_name]")),
                _ => return Err(does_not_support(p, current)),
            },
            Some(_) => return Err(does_not_support(p, current)),
        }
    }
    Ok(Function::Random { seed, field })
}

fn does_not_support(p: &Parser<'_>, name: &str) -> DslError {
    DslError::parsing(
        p.location(),
        // ALLOC: error path; one message.
        format!("random_score query does not support [{name}]"),
    )
}

/// The seed: an int as it is, a long through `Long.hashCode`, a string
/// through `String.hashCode`.
fn seed_value(p: &Parser<'_>, kind: Kind) -> Result<i32, DslError> {
    let text = p.text();
    match kind {
        Kind::String => Ok(string_hash(text)),
        Kind::Number => match (text.parse::<i32>(), text.parse::<i64>()) {
            (Ok(int), _) => Ok(int),
            (_, Ok(long)) => Ok(long_hash(long)),
            _ => Err(seed_error(p, "an int, long or string")),
        },
        _ => Err(seed_error(p, "an int/long or string")),
    }
}

fn seed_error(p: &Parser<'_>, wanted: &str) -> DslError {
    let token = p.current_token().map_or("", Kind::name);
    DslError::parsing(
        p.location(),
        // ALLOC: error path; one message.
        format!("random_score seed must be {wanted}, not '{token}'"),
    )
}

/// `Long.hashCode`.
fn long_hash(v: i64) -> i32 {
    #[allow(clippy::cast_possible_truncation)]
    {
        (v ^ ((v as u64) >> 32) as i64) as i32
    }
}

/// `String.hashCode`: over UTF-16 units.
pub fn string_hash(s: &str) -> i32 {
    s.encode_utf16()
        .fold(0i32, |h, u| h.wrapping_mul(31).wrapping_add(i32::from(u)))
}

/// `RandomScoreFunctionBuilder.doToFunction`.
pub fn build(
    cx: &QueryContext<'_>,
    seed: Option<i32>,
    field: Option<&str>,
) -> Result<Engine, Failure> {
    let salt = cx.shard.map_or(0, |s| s.salt());
    let now = match cx.now_millis {
        Some(now) => now,
        None => std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(0)),
    };
    let by_doc = || Engine::Random {
        salted_seed: mix(long_hash(now), salt),
        field: RandomField::DocId,
    };
    let Some(seed) = seed else {
        return Ok(by_doc());
    };
    let name = field.unwrap_or("_id");
    let hashed = |field: RandomField| Engine::Random {
        salted_seed: mix(seed, salt),
        field,
    };
    if name == "_id" {
        return Ok(hashed(RandomField::Id));
    }
    let Some(mapped) = cx.mapping.get(name) else {
        // No mapping at all is an empty index: scored by document.
        if cx.mapping.is_empty() {
            return Ok(by_doc());
        }
        let index = cx
            .shard
            // ALLOC: error path; one message.
            .map_or(String::new(), |s| format!("{}/{}", s.index, s.uuid));
        return Err(Failure::plain(
            "illegal_argument_exception",
            // ALLOC: error path; one message.
            format!(
                "Field [{name}] is not mapped on [[{index}]] and cannot be used as a source of random numbers."
            ),
        ));
    };
    match &mapped.ty {
        FieldType::Text => Err(super::function_score_build::text_fielddata(name)),
        // ALLOC: error path; one message.
        _ if !mapped.doc_values => Err(unsupported(&format!(
            // ALLOC: error path; one message.
            "random_score on the field [{name}], which has no doc values,"
        ))),
        // ALLOC: one name a random score.
        FieldType::Keyword => Ok(hashed(RandomField::Keyword(name.to_owned()))),
        FieldType::Int { .. } | FieldType::Long | FieldType::Date => {
            // ALLOC: one name a random score.
            Ok(hashed(RandomField::Numeric(name.to_owned(), Numeric::Long)))
        }
        FieldType::Float | FieldType::Double | FieldType::HalfFloat | FieldType::Unsupported(_) => {
            // ALLOC: error path; one message.
            Err(unsupported(&format!(
                "random_score on the field [{name}], which is not a keyword, number or date,"
            )))
        }
    }
}
