//! A remote model's predict call, as ml-commons makes it: the connector's `PREDICT` action
//! rendered into an HTTP request for the caller's parameters, and the answer read back
//! into the `inference_results` body. The caller sends the request.

use lucene_ml::MlError;
use lucene_ml::remote::connector::{Call, Connector, Input, Parsed, Prepared};
use serde_json::{Map, Value};

use crate::credentials;

/// Why a predict call could not be made, with the status it answers with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Failure {
    /// The HTTP status to answer with.
    pub status: u16,
    /// Stock's text for it.
    pub reason: String,
}

fn failure(status: u16, reason: impl Into<String>) -> Failure {
    Failure {
        status,
        reason: reason.into(),
    }
}

impl From<MlError> for Failure {
    fn from(e: MlError) -> Self {
        match e {
            MlError::InvalidArgument(m) => failure(400, m),
            MlError::Model(m) => failure(400, m),
            other => failure(500, other.to_string()),
        }
    }
}

/// The scripts a connector may name run in Painless, which this node does not run yet.
fn scripts_not_served() -> Failure {
    failure(
        501,
        "connectors with a pre or post process script are not served by this node yet",
    )
}

/// A connector with its credentials opened, ready to make requests.
#[derive(Debug, Clone)]
pub struct Remote {
    connector: Connector,
}

/// One request, and what reading its response needs.
#[derive(Debug, Clone)]
pub struct Prepared1 {
    /// The method.
    pub method: String,
    /// The URL.
    pub url: String,
    /// The headers, in order.
    pub headers: Vec<(String, String)>,
    /// The body.
    pub body: Vec<u8>,
    call: Call,
}

impl Remote {
    /// Opens a stored connector document: every credential value is decrypted under
    /// `master_key`.
    ///
    /// # Errors
    /// The document is not a connector, or a credential does not open.
    pub fn open(stored: &Map<String, Value>, master_key: &str) -> Result<Self, Failure> {
        let mut doc = stored.clone();
        if let Some(Value::Object(creds)) = doc.get_mut("credential") {
            for value in creds.values_mut() {
                let sealed = value
                    .as_str()
                    .ok_or_else(|| failure(500, "a stored credential is not text"))?;
                *value = Value::String(credentials::open(master_key, sealed).map_err(|_| {
                    failure(
                        500,
                        "a stored credential does not open under the master key",
                    )
                })?);
            }
        }
        let connector = Connector::from_json(&Value::Object(doc).to_string())?;
        Ok(Self { connector })
    }

    /// The request for `parameters`, signed at `amz_date` (`yyyyMMddTHHmmssZ`) when the
    /// connector is AWS's.
    ///
    /// # Errors
    /// A placeholder is not filled (400, stock's text) or the connector runs a script (501).
    pub fn prepare(
        &self,
        parameters: &[(String, Option<String>)],
        amz_date: &str,
    ) -> Result<Prepared1, Failure> {
        let input = Input::Remote(parameters.to_vec());
        match self
            .connector
            .prepare("PREDICT", "REMOTE", &input, &[], None, amz_date)?
        {
            Prepared::Call(call) => Ok(Prepared1 {
                method: call.request.method.clone(),
                url: call.request.url.clone(),
                headers: call.request.headers.clone(),
                body: call.request.body.clone(),
                call: *call,
            }),
            Prepared::Script(_) => Err(scripts_not_served()),
        }
    }

    /// The `inference_results` body for a successful response `body` of `prepared`.
    ///
    /// # Errors
    /// The response cannot be read the way the connector says (400), or needs a script (501).
    pub fn read(&self, prepared: &Prepared1, body: &str) -> Result<String, Failure> {
        match self
            .connector
            .parse("PREDICT", &prepared.call, body, None)?
        {
            Parsed::Tensors(tensors) => {
                let output: Vec<String> =
                    tensors.iter().map(|t| t.to_json().to_compact()).collect();
                Ok(format!(
                    "{{\"inference_results\":[{{\"output\":[{}],\"status_code\":200}}]}}",
                    output.join(",")
                ))
            }
            Parsed::Script(_) => Err(scripts_not_served()),
        }
    }
}

/// `yyyyMMddTHHmmssZ` for `unix_secs`, the time format SigV4 signs with.
#[must_use]
// ARITH: `unix_secs / 86_400` is below 2^48, so `era` is below 2^31 and every product here
// (at most `5 * 146_097` or `era * 400`) is far inside an `i64`.
#[allow(clippy::arithmetic_side_effects)]
pub fn amz_date(unix_secs: u64) -> String {
    let (days, rem) = (unix_secs / 86_400, unix_secs % 86_400);
    // Civil date from days since 1970-01-01 (Howard Hinnant's algorithm).
    let z = i64::try_from(days).unwrap_or(0).saturating_add(719_468);
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}{month:02}{day:02}T{:02}{:02}{:02}Z",
        rem / 3_600,
        rem % 3_600 / 60,
        rem % 60
    )
}

#[cfg(test)]
mod tests;
