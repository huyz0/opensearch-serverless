//! The accounts an operator declares for the channels that need a credential:
//! AWS keys for SNS and SES, a login for an SMTP relay (ADR 0081).
//!
//! A tenant's config names a role, a host or a port; it never carries a
//! secret. A send finds its credential here by what the config names, and a
//! config that names nothing declared here is refused at send, so a tenant
//! cannot use the node's credentials for a role or a relay the operator did
//! not declare. The file is read once at start; nothing in it is ever shown,
//! logged or put in an answer.

use std::fmt;
use std::io::Read;

use oss_blob::Credentials;
use serde_json::Value;

/// One set of AWS credentials, for the role (or none) a tenant's config names.
#[derive(Clone)]
pub struct AwsAccount {
    /// The `role_arn` a config must name to use these; none for a config that names none.
    pub role_arn: Option<String>,
    pub(super) credentials: Credentials,
    pub(super) session_token: Option<String>,
    /// A base URL used for the service instead of its public endpoint.
    pub(super) endpoint: Option<String>,
}

impl fmt::Debug for AwsAccount {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AwsAccount")
            .field("role_arn", &self.role_arn)
            .finish_non_exhaustive()
    }
}

/// An SMTP relay and its login.
#[derive(Clone)]
pub struct SmtpAccount {
    /// The relay's host, as an `smtp_account` config names it.
    pub host: String,
    /// The relay's port.
    pub port: u16,
    pub(super) login: Option<(String, String)>,
    /// Certificate authorities trusted for this relay besides the public roots.
    pub(super) extra_roots: Vec<rustls::pki_types::CertificateDer<'static>>,
}

impl fmt::Debug for SmtpAccount {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SmtpAccount")
            .field("host", &self.host)
            .field("port", &self.port)
            .finish_non_exhaustive()
    }
}

/// Every account the operator declared.
#[derive(Clone, Debug, Default)]
pub struct Accounts {
    aws: Vec<AwsAccount>,
    smtp: Vec<SmtpAccount>,
}

fn text(v: &Value, key: &str) -> Option<String> {
    v.get(key)
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
}

impl Accounts {
    /// No accounts: every send that needs one is refused.
    #[must_use]
    pub fn none() -> Self {
        Self::default()
    }

    /// The accounts of a JSON file's text:
    /// `{"aws": [{"role_arn"?, "access_key", "secret_key", "session_token"?, "endpoint"?}],
    ///   "smtp": [{"host", "port", "username"?, "password"?}]}`.
    ///
    /// # Errors
    ///
    /// A message naming what is wrong, never a secret.
    pub fn parse(text_of_file: &str) -> Result<Self, String> {
        let root: Value = serde_json::from_str(text_of_file)
            .map_err(|_| "the notification accounts file is not JSON".to_owned())?;
        let mut out = Self::default();
        for (i, a) in root
            .get("aws")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .enumerate()
        {
            let (Some(access), Some(secret)) = (text(a, "access_key"), text(a, "secret_key"))
            else {
                return Err(format!(
                    "aws account {i} needs an access_key and a secret_key"
                ));
            };
            let role_arn = text(a, "role_arn");
            if out.aws.iter().any(|x| x.role_arn == role_arn) {
                return Err(format!("aws account {i} repeats a role_arn"));
            }
            out.aws.push(AwsAccount {
                role_arn,
                credentials: Credentials::new(&access, &secret),
                session_token: text(a, "session_token"),
                endpoint: text(a, "endpoint"),
            });
        }
        for (i, s) in root
            .get("smtp")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .enumerate()
        {
            let port = s
                .get("port")
                .and_then(Value::as_u64)
                .and_then(|p| u16::try_from(p).ok())
                .filter(|p| *p > 0);
            let (Some(host), Some(port)) = (text(s, "host"), port) else {
                return Err(format!("smtp account {i} needs a host and a port"));
            };
            let login = text(s, "username").zip(text(s, "password"));
            let mut extra_roots = Vec::new();
            for pem in s.get("ca_pem").and_then(Value::as_str).into_iter().chain(
                s.get("ca_pem")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .filter_map(Value::as_str),
            ) {
                for der in rustls::pki_types::pem::PemObject::pem_slice_iter(pem.as_bytes()) {
                    extra_roots.push(
                        der.map_err(|_| format!("smtp account {i} has a ca_pem that is not PEM"))?,
                    );
                }
            }
            out.smtp.push(SmtpAccount {
                host,
                port,
                login,
                extra_roots,
            });
        }
        Ok(out)
    }

    /// The accounts of the file `OSS_NOTIFICATIONS_ACCOUNTS` names; none when
    /// it is unset.
    ///
    /// # Errors
    ///
    /// The file cannot be read or is not a valid accounts file.
    pub fn from_env() -> Result<Self, String> {
        let Some(path) = std::env::var("OSS_NOTIFICATIONS_ACCOUNTS")
            .ok()
            .filter(|p| !p.is_empty())
        else {
            return Ok(Self::none());
        };
        let unreadable = |e: std::io::Error| {
            format!(
                "the notification accounts file cannot be read: {}",
                e.kind()
            )
        };
        let mut file = std::fs::File::open(&path).map_err(unreadable)?;
        // ALLOC: the file, at most 1 MiB; a larger one is not an accounts file.
        let mut bytes: Vec<u8> = Vec::new();
        let mut chunk = [0u8; 8192];
        loop {
            let n = file.read(&mut chunk).map_err(unreadable)?;
            if n == 0 {
                break;
            }
            bytes.extend_from_slice(chunk.get(..n).unwrap_or_default());
            if bytes.len() > (1 << 20) {
                return Err("the notification accounts file is too large".to_owned());
            }
        }
        let text = String::from_utf8(bytes)
            .map_err(|_| "the notification accounts file is not UTF-8".to_owned())?;
        Self::parse(&text)
    }

    /// The AWS account for a config that names `role_arn` (or none).
    pub(super) fn aws(&self, role_arn: Option<&str>) -> Option<&AwsAccount> {
        self.aws.iter().find(|a| a.role_arn.as_deref() == role_arn)
    }

    /// The SMTP account a config naming `host` and `port` may use.
    pub(super) fn smtp(&self, host: &str, port: u16) -> Option<&SmtpAccount> {
        self.smtp
            .iter()
            .find(|a| a.port == port && a.host.eq_ignore_ascii_case(host))
    }
}

#[cfg(test)]
mod tests {
    use super::Accounts;

    #[test]
    fn accounts_are_found_by_what_a_config_names_and_never_shown() {
        let a = Accounts::parse(
            r#"{"aws":[{"access_key":"AK","secret_key":"SECRETVALUE"},
                 {"role_arn":"arn:aws:iam::1:role/r","access_key":"AK2","secret_key":"S2"}],
                "smtp":[{"host":"Mail.Example.com","port":587,"username":"u","password":"PASSVALUE"}]}"#,
        )
        .expect("parses");
        assert!(a.aws(None).is_some());
        assert!(a.aws(Some("arn:aws:iam::1:role/r")).is_some());
        assert!(a.aws(Some("arn:aws:iam::1:role/other")).is_none());
        assert!(a.smtp("mail.example.com", 587).is_some());
        assert!(a.smtp("mail.example.com", 25).is_none());
        assert!(a.smtp("evil.example.com", 587).is_none());
        let shown = format!("{a:?}");
        assert!(
            !shown.contains("SECRETVALUE") && !shown.contains("PASSVALUE"),
            "{shown}"
        );
    }

    #[test]
    fn a_bad_file_is_refused_with_a_message_that_holds_no_secret() {
        for bad in [
            "not json",
            r#"{"aws":[{"access_key":"AK"}]}"#,
            r#"{"aws":[{"access_key":"A","secret_key":"S"},{"access_key":"B","secret_key":"T"}]}"#,
            r#"{"smtp":[{"host":"h"}]}"#,
            r#"{"smtp":[{"host":"h","port":70000}]}"#,
        ] {
            let e = Accounts::parse(bad).expect_err(bad);
            assert!(!e.contains("SECRET") && !e.contains("PASS"), "{e}");
        }
    }
}
