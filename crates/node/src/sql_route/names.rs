//! `SHOW TABLES` and `DESCRIBE TABLES` over a pattern: one capped listing, then `like`.

use oss_query::{Error, Mapping};
use oss_rest::{JSON, Request, Response, RestError};

use super::handle::{failure, send_error};
use super::{SqlFailure, SqlService};

/// The most names one listing answers; past it the statement is refused, never cut.
const MAX_NAMES: usize = 1_000;

/// The cluster name the answer's `TABLE_CAT` column carries.
const CLUSTER: &str = "serverless";

impl SqlService {
    /// The names `pattern` matches that the caller may read.
    fn matching(&self, req: &Request<'_>, pattern: &str) -> Result<Vec<String>, SqlFailure> {
        let Some(lister) = &self.lister else {
            return Err(SqlFailure::Body(
                400,
                Error::unsupported("this node cannot list index names")
                    .to_json()
                    .to_string()
                    .into_bytes(),
            ));
        };
        let names = lister.names_with_prefix(oss_query::prefix_of(pattern), MAX_NAMES)?;
        Ok(names
            .into_iter()
            .filter(|n| oss_query::like(pattern, n) && self.permit(req, &[n.as_str()]).is_ok())
            .collect())
    }

    /// `SHOW TABLES LIKE pattern`.
    pub(super) fn show_tables(
        &self,
        req: &Request<'_>,
        resp: &mut Response<'_>,
        pattern: &str,
    ) -> Result<(), RestError> {
        let names = match self.matching(req, pattern) {
            Ok(n) => n,
            Err(f) => return failure(resp, f),
        };
        match oss_query::show_tables(CLUSTER, &names, self.limits.size_limit) {
            Ok(text) => resp.send(200, JSON, text.as_bytes()),
            Err(e) => send_error(resp, &e),
        }
    }

    /// `DESCRIBE TABLES LIKE pattern [COLUMNS LIKE columns]` over a pattern with a wildcard.
    pub(super) fn describe_pattern(
        &self,
        req: &Request<'_>,
        resp: &mut Response<'_>,
        pattern: &str,
        columns: Option<&str>,
    ) -> Result<(), RestError> {
        let names = match self.matching(req, pattern) {
            Ok(n) => n,
            Err(f) => return failure(resp, f),
        };
        let mut held: Vec<(String, Mapping)> = Vec::new();
        for name in names {
            match self.mapping_of(&name) {
                Ok(m) => held.push((name, m)),
                Err(f) => return failure(resp, f),
            }
        }
        let tables: Vec<(&str, &Mapping)> = held.iter().map(|(n, m)| (n.as_str(), m)).collect();
        match oss_query::describe_many(CLUSTER, &tables, columns, self.limits.size_limit) {
            Ok(text) => resp.send(200, JSON, text.as_bytes()),
            Err(e) => send_error(resp, &e),
        }
    }
}
