//! The JSON Web Token domain's settings: the variables that configure it.

use super::admin_pair;

/// The domain a JSON Web Token is checked by: its key and its rules.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JwtSettings {
    /// The signing key.
    pub key: crate::security_jwt::Key,
    /// What a token must say.
    pub rules: oss_coordinator::security::Rules,
}

/// The static administrator and the JSON Web Token domain.
pub type Settings = (Option<(String, String)>, Option<JwtSettings>);

/// The administrator and the JWT domain, from the variables `get` names.
///
/// # Errors
///
/// A message naming the variable that is unusable.
pub fn read(get: &impl Fn(&str) -> Option<String>) -> Result<Settings, String> {
    let set = |key: &str| get(key).filter(|v| !v.is_empty());
    let admin = admin_pair(
        set("OSS_SECURITY_ADMIN_USER"),
        set("OSS_SECURITY_ADMIN_HASH"),
    )?;
    let key = match (
        set("OSS_SECURITY_JWT_KEY"),
        set("OSS_SECURITY_JWT_PUBLIC_KEY"),
    ) {
        (None, None) => return Ok((admin, None)),
        (Some(_), Some(_)) => {
            return Err(
                "OSS_SECURITY_JWT_KEY and OSS_SECURITY_JWT_PUBLIC_KEY are exclusive".to_owned(),
            );
        }
        (Some(secret), None) => crate::security_jwt::Key::secret(&secret)?,
        (None, Some(pem)) => crate::security_jwt::Key::public(&pem)?,
    };
    let number = |key: &str, default: u64| match set(key) {
        None => Ok(default),
        Some(v) => v
            .parse::<u64>()
            .map_err(|_| format!("{key} is a whole number of seconds")),
    };
    let rules = oss_coordinator::security::Rules {
        subject_key: set("OSS_SECURITY_JWT_SUBJECT_KEY").unwrap_or_else(|| "sub".to_owned()),
        roles_key: set("OSS_SECURITY_JWT_ROLES_KEY"),
        issuer: set("OSS_SECURITY_JWT_ISSUER"),
        audience: set("OSS_SECURITY_JWT_AUDIENCE"),
        skew: number("OSS_SECURITY_JWT_SKEW_SECONDS", 30)?.min(3600),
        max_lifetime: set("OSS_SECURITY_JWT_MAX_LIFETIME_SECONDS")
            .map(|_| number("OSS_SECURITY_JWT_MAX_LIFETIME_SECONDS", 0))
            .transpose()?,
    };
    Ok((admin, Some(JwtSettings { key, rules })))
}
