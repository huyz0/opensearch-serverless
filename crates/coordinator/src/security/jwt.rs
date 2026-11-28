//! The rules of a JSON Web Token the signature does not decide: its header,
//! its claims, and who it vouches for (ADR 0090). Signatures are checked by
//! the node; this module sees only bytes that already verified, or that are
//! about to be.

use oss_doc::mapping::json::{Json, parse};

use super::Config;

/// The most bytes of a token read; a longer one is refused unread.
pub const MAX_TOKEN_BYTES: usize = 8192;
/// The most backend roles a token may carry.
pub const MAX_ROLES: usize = 64;
/// The longest backend role, in bytes.
pub const MAX_ROLE_BYTES: usize = 256;

/// What the node's settings say a token must be.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rules {
    /// The claim that names the user.
    pub subject_key: String,
    /// The claim that holds the backend roles, if roles are read.
    pub roles_key: Option<String>,
    /// The issuer a token must name, if one is required.
    pub issuer: Option<String>,
    /// The audience a token must name, if one is required.
    pub audience: Option<String>,
    /// Seconds a clock may differ by.
    pub skew: u64,
    /// The longest a token may be valid for, in seconds, if bounded.
    pub max_lifetime: Option<u64>,
}

impl Default for Rules {
    fn default() -> Self {
        Self {
            // ALLOC: built once at start.
            subject_key: "sub".to_owned(),
            roles_key: None,
            issuer: None,
            audience: None,
            skew: 30,
            max_lifetime: None,
        }
    }
}

/// Who a verified token vouches for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Claims {
    /// The user's name.
    pub subject: String,
    /// The backend roles the token carries.
    pub roles: Vec<String>,
}

/// The `alg` of a token header, or `None` when the header is not an object
/// with a string `alg`, or names a critical extension (`crit`) this node
/// does not understand.
pub fn header_alg(header: &[u8]) -> Option<String> {
    let Ok(Json::Obj(members)) = parse(header) else {
        return None;
    };
    if members.iter().any(|(k, _)| k == "crit") {
        return None;
    }
    members.into_iter().find_map(|(k, v)| match v {
        Json::Str(alg) if k == "alg" => Some(alg),
        _ => None,
    })
}

fn number(doc: &Json, key: &str) -> Option<Option<u64>> {
    match doc.get(key) {
        None => Some(None),
        Some(Json::Num(n)) => {
            let value: f64 = n.parse().ok()?;
            if value.is_finite() && value >= 0.0 {
                // The cast saturates; a claim past u64 is a far-future time.
                #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                Some(Some(value as u64))
            } else {
                None
            }
        }
        Some(_) => None,
    }
}

fn strings(value: &Json) -> Option<Vec<String>> {
    match value {
        // ALLOC: one request's claims, bounded by the 8 KiB token.
        Json::Str(s) => Some(vec![s.clone()]),
        Json::Arr(items) => items
            .iter()
            .map(|i| match i {
                Json::Str(s) => Some(s.clone()),
                _ => None,
            })
            // ALLOC: one request's claims, bounded by the 8 KiB token.
            .collect(),
        _ => None,
    }
}

/// The claims of a token payload at `now` (seconds since the epoch), or
/// `None` for any reason it is not acceptable: not an object, no `exp`, a
/// time outside its window, a lifetime over the bound, the wrong issuer or
/// audience, no usable subject, or roles of the wrong type or past the
/// bounds.
pub fn claims(payload: &[u8], rules: &Rules, now: u64) -> Option<Claims> {
    let doc = parse(payload).ok()?;
    if !matches!(doc, Json::Obj(_)) {
        return None;
    }
    let exp = number(&doc, "exp")??;
    if now >= exp.saturating_add(rules.skew) {
        return None;
    }
    if let Some(nbf) = number(&doc, "nbf")?
        && nbf > now.saturating_add(rules.skew)
    {
        return None;
    }
    if let Some(max) = rules.max_lifetime
        && exp > now.saturating_add(max).saturating_add(rules.skew)
    {
        return None;
    }
    if let Some(issuer) = &rules.issuer
        && !matches!(doc.get("iss"), Some(Json::Str(i)) if i == issuer)
    {
        return None;
    }
    if let Some(audience) = &rules.audience
        && !doc
            .get("aud")
            .and_then(strings)
            .is_some_and(|a| a.contains(audience))
    {
        return None;
    }
    let subject = match doc.get(&rules.subject_key) {
        Some(Json::Str(s)) if !s.is_empty() => s.clone(),
        _ => return None,
    };
    let roles = match &rules.roles_key {
        // ALLOC: one request's claims, bounded by the 8 KiB token.
        None => Vec::new(),
        Some(key) => match doc.get(key) {
            // ALLOC: one request's claims, bounded by the 8 KiB token.
            None => Vec::new(),
            Some(value) => strings(value)?,
        },
    };
    if roles.len() > MAX_ROLES || roles.iter().any(|r| r.len() > MAX_ROLE_BYTES) {
        return None;
    }
    Some(Claims { subject, roles })
}

/// Whether `claims` may act: its subject names neither a stored user nor
/// the static administrator.
pub fn admissible(config: &Config, claims: &Claims) -> bool {
    config.user(&claims.subject).is_none() && !config.is_admin(claims.subject.as_str())
}
