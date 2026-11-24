//! Which index template an index inherits from, and what it inherits.
//!
//! Ported from `serverless/shell/src/main/java/org/opensearch/serverless/template/TemplateResolver.java`.
//! Pure decisions over template documents already read: the caller hands
//! the set (from a `DocSetCache` snapshot) and gets the winning template's
//! settings and mappings, with its component templates applied under it.
//! Nothing here touches the object store.
//!
//! A template matches an index when any of its `index_patterns` does, where
//! `*` is the only wildcard and a pattern matches the whole name. The
//! highest `priority` wins and a tie goes to the smaller name. The winner's
//! `composed_of` components apply in order and the template on top:
//! settings merge object by object, mappings merge object by object with
//! `properties` merged field by field, and a value that is not an object on
//! both sides replaces the earlier one.

use std::fmt;

use oss_doc::mapping::json::{Json, parse};

/// Why a template could not be resolved.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TemplateError {
    /// A stored template is not a JSON object the parser accepts.
    Unreadable(String),
    /// The winning template names a component that is not stored.
    MissingComponent {
        /// The index template.
        template: String,
        /// The component it composes.
        component: String,
    },
    /// A matching template's priority is not a nonnegative long.
    Priority,
}

impl fmt::Display for TemplateError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unreadable(name) => write!(f, "stored template [{name}] could not be read"),
            Self::MissingComponent {
                template,
                component,
            } => write!(
                f,
                "index template [{template}] composes component template [{component}], \
                 which does not exist"
            ),
            Self::Priority => f.write_str("index template priority must be a nonnegative long"),
        }
    }
}

impl std::error::Error for TemplateError {}

/// What an index inherits from its template.
#[derive(Debug, Clone, PartialEq)]
pub struct Inherited {
    /// The merged settings, an object; empty when nothing matched.
    pub settings: Json,
    /// The merged mappings, an object; empty when nothing matched.
    pub mappings: Json,
    /// The winning template's name, `None` when none matched.
    pub from: Option<String>,
    /// The winning template's `data_stream` object, when it has one.
    pub data_stream: Option<Json>,
}

impl Inherited {
    /// Whether the winning template makes data streams.
    pub fn is_data_stream(&self) -> bool {
        self.data_stream.is_some()
    }

    /// The data stream's timestamp field: its `timestamp_field.name`, else
    /// `@timestamp`.
    pub fn timestamp_field(&self) -> String {
        let name = self
            .data_stream
            .as_ref()
            .and_then(|d| d.get("timestamp_field"))
            .and_then(|f| f.get("name"))
            .filter(|n| !matches!(n, Json::Null));
        // ALLOC: one name, once.
        name.map_or_else(|| "@timestamp".to_owned(), text_of)
    }
}

/// A template read, with the priority it carries.
struct Parsed {
    name: String,
    body: Json,
}

/// The best template so far for one index name.
struct Best {
    priority: i64,
    at: Option<usize>,
}

fn read(name: &str, source: &[u8]) -> Result<Json, TemplateError> {
    match parse(source) {
        Ok(json @ Json::Obj(_)) => Ok(json),
        // ALLOC: one name, once, on a refusal.
        _ => Err(TemplateError::Unreadable(name.to_owned())),
    }
}

/// The text Java's `String.valueOf` gives a value read from JSON: a string
/// as it is, a number as written, `true`, `false` and `null`; an array or
/// object as its JSON, where Java prints its map's own form.
fn text_of(value: &Json) -> String {
    match value {
        Json::Str(s) | Json::Num(s) => s.clone(),
        // ALLOC: one short word, once
        Json::Bool(true) => "true".to_owned(),
        Json::Bool(false) => "false".to_owned(),
        Json::Null => "null".to_owned(),
        other => other.to_json(),
    }
}

/// An array's items, `null` as none, and anything else as an array of one.
fn list(value: Option<&Json>) -> Vec<&Json> {
    match value {
        // ALLOC: empty, nothing held
        None | Some(Json::Null) => Vec::new(),
        // ALLOC: one reference an item, at most the array's length
        Some(Json::Arr(items)) => items.iter().collect(),
        // ALLOC: one item.
        Some(one) => vec![one],
    }
}

/// Whether `pattern` matches all of `name`, `*` standing for any run of
/// characters and every other character for itself.
fn glob(pattern: &str, name: &str) -> bool {
    // ALLOC: the two names as characters, bounded by an index name and a pattern.
    let (p, n): (Vec<char>, Vec<char>) = (pattern.chars().collect(), name.chars().collect());
    let (mut pi, mut ni) = (0usize, 0usize);
    let mut star: Option<(usize, usize)> = None;
    while ni < n.len() {
        match p.get(pi) {
            Some('*') => {
                star = Some((pi, ni));
                pi = pi.saturating_add(1);
            }
            Some(c) if Some(c) == n.get(ni) => {
                pi = pi.saturating_add(1);
                ni = ni.saturating_add(1);
            }
            _ => match star {
                Some((sp, sn)) => {
                    pi = sp.saturating_add(1);
                    ni = sn.saturating_add(1);
                    star = Some((sp, sn.saturating_add(1)));
                }
                None => return false,
            },
        }
    }
    p.get(pi..)
        .is_some_and(|rest| rest.iter().all(|c| *c == '*'))
}

fn matches(template: &Json, index: &str) -> bool {
    list(template.get("index_patterns"))
        .into_iter()
        .any(|pattern| glob(&text_of(pattern), index))
}

/// The template's priority as core's `Numbers.toLong(text, true)` reads it:
/// an integer, or a decimal truncated toward zero; absent or `null` is 0.
fn priority(template: &Json) -> Result<i64, TemplateError> {
    let value = match template.get("priority") {
        None | Some(Json::Null) => return Ok(0),
        Some(v) => text_of(v),
    };
    let long = value
        .parse::<i64>()
        .ok()
        .or_else(|| truncated(&value))
        .ok_or(TemplateError::Priority)?;
    if long < 0 {
        return Err(TemplateError::Priority);
    }
    Ok(long)
}

/// A decimal's integer part (`BigDecimal.toBigInteger`) when it fits an
/// `i64` and is not below zero: optional sign, digits with an optional fraction, optional exponent.
fn truncated(text: &str) -> Option<i64> {
    let (negative, rest) = match text.as_bytes().first()? {
        b'-' => (true, text.get(1..)?),
        b'+' => (false, text.get(1..)?),
        _ => (false, text),
    };
    let (mantissa, exponent) = match rest.find(['e', 'E']) {
        Some(at) => (
            rest.get(..at)?,
            rest.get(at.saturating_add(1)..)?.parse::<i64>().ok()?,
        ),
        None => (rest, 0),
    };
    let (whole, fraction) = mantissa.split_once('.').unwrap_or((mantissa, ""));
    if (whole.is_empty() && fraction.is_empty())
        || !whole
            .bytes()
            .chain(fraction.bytes())
            .all(|b| b.is_ascii_digit())
    {
        return None;
    }
    // The digits and where the point sits among them after the exponent.
    // ALLOC: the digits of one priority.
    let digits: Vec<u8> = whole.bytes().chain(fraction.bytes()).collect();
    let point = i64::try_from(whole.len()).ok()?.checked_add(exponent)?;
    let kept = usize::try_from(point.clamp(0, i64::try_from(digits.len()).ok()?)).ok()?;
    let mut value: i128 = 0;
    for digit in digits.get(..kept)? {
        value = value
            .checked_mul(10)?
            .checked_add(i128::from(digit.wrapping_sub(b'0')))?;
    }
    // Zeros the exponent shifts in past the digits given: any nonzero value
    // overflows long before 40 of them, and zero stays zero.
    let zeros = point.checked_sub(i64::try_from(digits.len()).ok()?)?;
    for _ in 0..zeros.clamp(0, 40) {
        value = value.checked_mul(10)?;
    }
    // A negative integer part is a refused priority whatever its size.
    if negative && value != 0 {
        return None;
    }
    i64::try_from(value).ok()
}

/// Java's `String.compareTo`: UTF-16 code units in order.
fn java_order(a: &str, b: &str) -> std::cmp::Ordering {
    a.encode_utf16().cmp(b.encode_utf16())
}

/// Whether a candidate beats the best so far.
fn better(priority: i64, name: &str, best: &Best, current: Option<&str>) -> bool {
    priority > best.priority
        || (priority == best.priority && current.is_some_and(|c| java_order(name, c).is_lt()))
}

/// The template an index inherits from, and what it inherits.
///
/// `templates` is every index template as name and source; `component`
/// answers a component template's source by name. Every template is read, as
/// a stored one that cannot be read is an error whether or not it matches.
///
/// # Errors
///
/// [`TemplateError`] for a template or component that cannot be read, a
/// component the winner names that is not stored, and a matching template
/// with a bad priority.
pub fn resolve<'a, I, F>(
    templates: I,
    component: F,
    index: &str,
) -> Result<Inherited, TemplateError>
where
    I: IntoIterator<Item = (&'a str, &'a [u8])>,
    F: Fn(&str) -> Option<&'a [u8]>,
{
    // ALLOC: one parsed template each, at most the set's cap.
    let mut parsed: Vec<Parsed> = Vec::new();
    let mut best = Best {
        priority: i64::MIN,
        at: None,
    };
    for (name, source) in templates {
        let body = read(name, source)?;
        let candidate = Parsed {
            // ALLOC: one name, once a template
            name: name.to_owned(),
            body,
        };
        if matches(&candidate.body, index) {
            let rank = priority(&candidate.body)?;
            let current = best
                .at
                .and_then(|at| parsed.get(at))
                .map(|p| p.name.as_str());
            if better(rank, name, &best, current) {
                best = Best {
                    priority: rank,
                    at: Some(parsed.len()),
                };
            }
        }
        parsed.push(candidate);
    }
    let Some(winner) = best.at.and_then(|at| parsed.get(at)) else {
        return Ok(Inherited {
            // ALLOC: empty, nothing held
            settings: Json::Obj(Vec::new()),
            mappings: Json::Obj(Vec::new()),
            from: None,
            data_stream: None,
        });
    };
    // ALLOC: empty until a member is merged
    let mut settings = Vec::new();
    let mut mappings = Vec::new();
    for named in list(winner.body.get("composed_of")) {
        let name = text_of(named);
        let Some(source) = component(&name) else {
            return Err(TemplateError::MissingComponent {
                template: winner.name.clone(),
                component: name,
            });
        };
        merge(&mut settings, &mut mappings, &read(&name, source)?);
    }
    merge(&mut settings, &mut mappings, &winner.body);
    Ok(Inherited {
        settings: Json::Obj(settings),
        mappings: Json::Obj(mappings),
        from: Some(winner.name.clone()),
        data_stream: match winner.body.get("data_stream") {
            Some(block @ Json::Obj(_)) => Some(block.clone()),
            _ => None,
        },
    })
}

/// For each of `indices`, the name of the template that wins it, in the
/// order the indices first appear, leaving out those no template matches.
///
/// # Errors
///
/// As [`resolve`], for the templates.
pub fn matching_template_names<'a, I>(
    templates: I,
    indices: &[&str],
) -> Result<Vec<(String, String)>, TemplateError>
where
    I: IntoIterator<Item = (&'a str, &'a [u8])>,
{
    // ALLOC: one slot a distinct index.
    let mut slots: Vec<(&str, Best, Option<String>)> = Vec::new();
    for index in indices {
        if !slots.iter().any(|(i, ..)| i == index) {
            slots.push((
                index,
                Best {
                    priority: i64::MIN,
                    at: None,
                },
                None,
            ));
        }
    }
    if slots.is_empty() {
        // ALLOC: empty, nothing held
        return Ok(Vec::new());
    }
    for (name, source) in templates {
        let template = read(name, source)?;
        let mut rank = None;
        for (index, best, current) in &mut slots {
            if matches(&template, index) {
                let rank = match rank {
                    Some(rank) => rank,
                    None => *rank.insert(priority(&template)?),
                };
                if better(rank, name, best, current.as_deref()) {
                    best.priority = rank;
                    // ALLOC: one name.
                    *current = Some(name.to_owned());
                }
            }
        }
    }
    Ok(slots
        .into_iter()
        // ALLOC: one pair a matched index, at most the indices given
        .filter_map(|(index, _, name)| name.map(|n| (index.to_owned(), n)))
        .collect())
}

/// Applies a template's `template.settings` and `template.mappings` over
/// what has been merged so far.
fn merge(settings: &mut Vec<(String, Json)>, mappings: &mut Vec<(String, Json)>, template: &Json) {
    let Some(block) = template.get("template") else {
        return;
    };
    if let Some(Json::Obj(given)) = block.get("settings") {
        deep_merge(settings, given);
    }
    if let Some(Json::Obj(given)) = block.get("mappings") {
        merge_mappings(mappings, given);
    }
}

/// Whether a field definition is an object or nested field: of that type, or
/// untyped with `properties`.
fn is_object_field(definition: &[(String, Json)]) -> bool {
    match definition.iter().find(|(k, _)| k == "type") {
        None => definition.iter().any(|(k, _)| k == "properties"),
        Some((_, Json::Str(t))) => t == "object" || t == "nested",
        Some(_) => false,
    }
}

/// Puts `value` under `key`, in the member's place when it exists and at the
/// end when it does not, as a `LinkedHashMap` does.
fn put(into: &mut Vec<(String, Json)>, key: &str, value: Json) {
    match into.iter_mut().find(|(k, _)| k == key) {
        Some((_, existing)) => *existing = value,
        // ALLOC: one member, bounded by the template's own size.
        None => into.push((key.to_owned(), value)),
    }
}

fn existing<'a>(into: &'a [(String, Json)], key: &str) -> Option<&'a Json> {
    into.iter().find(|(k, _)| k == key).map(|(_, v)| v)
}

/// Merges objects member by member; any other pair is replaced.
fn deep_merge(into: &mut Vec<(String, Json)>, from: &[(String, Json)]) {
    for (key, value) in from {
        match (existing(into, key), value) {
            (Some(Json::Obj(left)), Json::Obj(right)) => {
                // ALLOC: a copy of one object, as the Java's.
                let mut merged = left.clone();
                deep_merge(&mut merged, right);
                put(into, key, Json::Obj(merged));
            }
            _ => put(into, key, value.clone()),
        }
    }
}

/// As [`deep_merge`], except that the members of `properties` merge field
/// by field.
fn merge_mappings(into: &mut Vec<(String, Json)>, from: &[(String, Json)]) {
    for (key, value) in from {
        match (existing(into, key), value) {
            (Some(Json::Obj(left)), Json::Obj(right)) => {
                // ALLOC: a copy of one object.
                let mut merged = left.clone();
                if key == "properties" {
                    merge_properties(&mut merged, right);
                } else {
                    merge_mappings(&mut merged, right);
                }
                put(into, key, Json::Obj(merged));
            }
            _ => put(into, key, value.clone()),
        }
    }
}

/// A field present on both sides merges only when both are object or nested
/// fields; otherwise the later definition replaces the earlier.
fn merge_properties(into: &mut Vec<(String, Json)>, from: &[(String, Json)]) {
    for (field, value) in from {
        match (existing(into, field), value) {
            (Some(Json::Obj(left)), Json::Obj(right))
                if is_object_field(left) && is_object_field(right) =>
            {
                // ALLOC: a copy of one field.
                let mut merged = left.clone();
                merge_mappings(&mut merged, right);
                put(into, field, Json::Obj(merged));
            }
            _ => put(into, field, value.clone()),
        }
    }
}
