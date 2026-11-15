//! HTTP methods, as OpenSearch's `RestRequest.Method` names them.
//!
//! Ported from `server/src/main/java/org/opensearch/rest/RestRequest.java`
//! (the `Method` enum) at huyz0/OpenSearch@49f238ed. A set of methods is a bit
//! mask, so the `Allow` header and the "allowed" list in a 405 body need no
//! allocation; they list methods in the enum's order, where Java iterates a
//! `HashSet` in no defined order.

use std::fmt;

/// An HTTP method OpenSearch knows. Any other token is refused with 405.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Method {
    /// `GET`
    Get,
    /// `POST`
    Post,
    /// `PUT`
    Put,
    /// `DELETE`
    Delete,
    /// `OPTIONS`: answered by the router with the path's `Allow` set.
    Options,
    /// `HEAD`: routed like `GET` when no `HEAD` route exists; the body is
    /// not sent.
    Head,
    /// `PATCH`
    Patch,
    /// `TRACE`
    Trace,
    /// `CONNECT`
    Connect,
}

impl Method {
    /// Every method, in the order Java declares them.
    pub const ALL: [Method; 9] = [
        Method::Get,
        Method::Post,
        Method::Put,
        Method::Delete,
        Method::Options,
        Method::Head,
        Method::Patch,
        Method::Trace,
        Method::Connect,
    ];

    /// The method named by a request line token, or `None` for one OpenSearch
    /// does not know. Method tokens are case-sensitive.
    pub fn parse(token: &[u8]) -> Option<Method> {
        Method::ALL
            .into_iter()
            .find(|m| m.as_str().as_bytes() == token)
    }

    /// The method's name.
    pub const fn as_str(self) -> &'static str {
        match self {
            Method::Get => "GET",
            Method::Post => "POST",
            Method::Put => "PUT",
            Method::Delete => "DELETE",
            Method::Options => "OPTIONS",
            Method::Head => "HEAD",
            Method::Patch => "PATCH",
            Method::Trace => "TRACE",
            Method::Connect => "CONNECT",
        }
    }

    const fn bit(self) -> u16 {
        1 << (self as u16)
    }
}

impl fmt::Display for Method {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A set of methods.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MethodSet(u16);

impl MethodSet {
    /// The empty set.
    pub const EMPTY: MethodSet = MethodSet(0);

    /// Adds `m`.
    pub fn insert(&mut self, m: Method) {
        self.0 |= m.bit();
    }

    /// Whether `m` is in the set.
    pub fn contains(self, m: Method) -> bool {
        self.0 & m.bit() != 0
    }

    /// Whether the set is empty.
    pub fn is_empty(self) -> bool {
        self.0 == 0
    }

    /// The methods in the set, in declaration order.
    pub fn iter(self) -> impl Iterator<Item = Method> {
        Method::ALL.into_iter().filter(move |m| self.contains(*m))
    }

    /// Writes the set joined by `sep`: `GET,POST` for an `Allow` header.
    pub(crate) fn write_joined(self, out: &mut dyn fmt::Write, sep: &str) -> fmt::Result {
        for (i, m) in self.iter().enumerate() {
            if i > 0 {
                out.write_str(sep)?;
            }
            out.write_str(m.as_str())?;
        }
        Ok(())
    }
}

impl fmt::Display for MethodSet {
    /// Java's `Set#toString`: `[GET, POST]`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("[")?;
        self.write_joined(f, ", ")?;
        f.write_str("]")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_method_parses_from_its_name_and_others_do_not() {
        for m in Method::ALL {
            assert_eq!(Method::parse(m.as_str().as_bytes()), Some(m));
            assert_eq!(m.to_string(), m.as_str());
        }
        assert_eq!(Method::parse(b"get"), None);
        assert_eq!(Method::parse(b"PROPFIND"), None);
        assert_eq!(Method::parse(b""), None);
    }

    #[test]
    fn sets_list_in_declaration_order() {
        let mut s = MethodSet::EMPTY;
        assert!(s.is_empty());
        assert_eq!(s.to_string(), "[]");
        s.insert(Method::Put);
        s.insert(Method::Get);
        s.insert(Method::Connect);
        assert!(s.contains(Method::Get) && !s.contains(Method::Post));
        assert_eq!(s.to_string(), "[GET, PUT, CONNECT]");
        let mut header = String::new();
        s.write_joined(&mut header, ",").expect("string");
        assert_eq!(header, "GET,PUT,CONNECT");
    }
}
