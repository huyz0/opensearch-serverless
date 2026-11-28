//! The `rest` point's bytes: one request in, one response out
//! (docs/specs/plugin-system AC-5).
//!
//! Little-endian, length-prefixed, version byte first. Every count and length
//! is checked against a ceiling before anything is allocated, a decode refuses
//! trailing bytes, and the same bytes mean the same call on every tier.

const VERSION: u8 = 1;
/// The most query or path parameters one call carries.
pub const MAX_PAIRS: usize = 64;
/// The most bytes of one name or value.
pub const MAX_PART_BYTES: usize = 8 * 1024;

/// Why bytes were not a request or a response.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CodecError {
    /// The version byte is not 1.
    Version,
    /// The bytes end inside a field.
    Short,
    /// A count or a length is over its ceiling.
    Bound,
    /// A string is not UTF-8.
    Text,
    /// A method or status code is not one the codec knows.
    Value,
    /// Bytes remain after the last field.
    Trailing,
}

/// The HTTP methods a plugin route may declare.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Method {
    /// `GET`.
    Get,
    /// `POST`.
    Post,
    /// `PUT`.
    Put,
    /// `DELETE`.
    Delete,
    /// `HEAD`.
    Head,
}

impl Method {
    fn code(self) -> u8 {
        match self {
            Method::Get => 1,
            Method::Post => 2,
            Method::Put => 3,
            Method::Delete => 4,
            Method::Head => 5,
        }
    }

    fn from_code(code: u8) -> Option<Self> {
        Some(match code {
            1 => Method::Get,
            2 => Method::Post,
            3 => Method::Put,
            4 => Method::Delete,
            5 => Method::Head,
            _ => return None,
        })
    }

    /// The method named by `text` (upper case).
    pub fn parse(text: &str) -> Option<Self> {
        Some(match text {
            "GET" => Method::Get,
            "POST" => Method::Post,
            "PUT" => Method::Put,
            "DELETE" => Method::Delete,
            "HEAD" => Method::Head,
            _ => return None,
        })
    }
}

/// A name and a value.
pub type Pair = (String, String);

/// One request to a plugin route.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RestRequest {
    /// The request's method.
    pub method: Method,
    /// The tenant the call is for, empty when there is none.
    pub tenant: String,
    /// The route's `action` from the manifest.
    pub action: String,
    /// The path parameters the route's pattern named.
    pub path_params: Vec<Pair>,
    /// The query parameters.
    pub query: Vec<Pair>,
    /// The headers the host passes (never credentials).
    pub headers: Vec<Pair>,
    /// The body.
    pub body: Vec<u8>,
}

/// A plugin's answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RestResponse {
    /// An HTTP status, 100 to 599.
    pub status: u16,
    /// The response headers.
    pub headers: Vec<Pair>,
    /// The body.
    pub body: Vec<u8>,
}

struct Out(Vec<u8>);

impl Out {
    fn str(&mut self, s: &str) {
        self.0
            .extend_from_slice(&u32::try_from(s.len()).unwrap_or(u32::MAX).to_le_bytes());
        self.0.extend_from_slice(s.as_bytes());
    }

    fn pairs(&mut self, pairs: &[Pair]) {
        self.0
            .extend_from_slice(&u16::try_from(pairs.len()).unwrap_or(u16::MAX).to_le_bytes());
        for (a, b) in pairs {
            self.str(a);
            self.str(b);
        }
    }

    fn bytes(&mut self, b: &[u8]) {
        self.0
            .extend_from_slice(&u32::try_from(b.len()).unwrap_or(u32::MAX).to_le_bytes());
        self.0.extend_from_slice(b);
    }
}

struct In<'a>(&'a [u8]);

impl<'a> In<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8], CodecError> {
        if self.0.len() < n {
            return Err(CodecError::Short);
        }
        let (head, rest) = self.0.split_at(n);
        self.0 = rest;
        Ok(head)
    }

    fn u8(&mut self) -> Result<u8, CodecError> {
        Ok(self.take(1)?.first().copied().unwrap_or(0))
    }

    fn u16(&mut self) -> Result<u16, CodecError> {
        let b = self.take(2)?;
        Ok(u16::from_le_bytes([
            b.first().copied().unwrap_or(0),
            b.get(1).copied().unwrap_or(0),
        ]))
    }

    fn len(&mut self) -> Result<usize, CodecError> {
        let b = self.take(4)?;
        let mut a = [0_u8; 4];
        a.copy_from_slice(b);
        Ok(u32::from_le_bytes(a) as usize)
    }

    fn str(&mut self) -> Result<String, CodecError> {
        let n = self.len()?;
        if n > MAX_PART_BYTES {
            return Err(CodecError::Bound);
        }
        let b = self.take(n)?;
        // ALLOC: at most MAX_PART_BYTES, checked above.
        std::str::from_utf8(b)
            .map(str::to_owned)
            .map_err(|_| CodecError::Text)
    }

    fn pairs(&mut self) -> Result<Vec<Pair>, CodecError> {
        let n = usize::from(self.u16()?);
        if n > MAX_PAIRS {
            return Err(CodecError::Bound);
        }
        // ALLOC: at most MAX_PAIRS entries, checked above.
        let mut out = Vec::with_capacity(n);
        for _ in 0..n {
            let a = self.str()?;
            let b = self.str()?;
            out.push((a, b));
        }
        Ok(out)
    }

    /// A body no longer than `max`.
    fn body(&mut self, max: usize) -> Result<Vec<u8>, CodecError> {
        let n = self.len()?;
        if n > max {
            return Err(CodecError::Bound);
        }
        // ALLOC: at most `max`, the point's ceiling, checked above.
        Ok(self.take(n)?.to_vec())
    }

    fn done(&self) -> Result<(), CodecError> {
        if self.0.is_empty() {
            Ok(())
        } else {
            Err(CodecError::Trailing)
        }
    }
}

impl RestRequest {
    /// The request's bytes.
    pub fn encode(&self) -> Vec<u8> {
        // ALLOC: one call's bytes, bounded by the point's ceiling on the host.
        let mut o = Out(vec![VERSION, self.method.code()]);
        o.str(&self.tenant);
        o.str(&self.action);
        o.pairs(&self.path_params);
        o.pairs(&self.query);
        o.pairs(&self.headers);
        o.bytes(&self.body);
        o.0
    }

    /// A request from bytes, its body at most `max_body`.
    pub fn decode(bytes: &[u8], max_body: usize) -> Result<Self, CodecError> {
        let mut i = In(bytes);
        if i.u8()? != VERSION {
            return Err(CodecError::Version);
        }
        let method = Method::from_code(i.u8()?).ok_or(CodecError::Value)?;
        let req = Self {
            method,
            tenant: i.str()?,
            action: i.str()?,
            path_params: i.pairs()?,
            query: i.pairs()?,
            headers: i.pairs()?,
            body: i.body(max_body)?,
        };
        i.done()?;
        Ok(req)
    }
}

impl RestResponse {
    /// The response's bytes.
    pub fn encode(&self) -> Vec<u8> {
        // ALLOC: one call's bytes, bounded by the point's ceiling on the host.
        let mut o = Out(vec![VERSION]);
        o.0.extend_from_slice(&self.status.to_le_bytes());
        o.pairs(&self.headers);
        o.bytes(&self.body);
        o.0
    }

    /// A response from bytes, its body at most `max_body`.
    pub fn decode(bytes: &[u8], max_body: usize) -> Result<Self, CodecError> {
        let mut i = In(bytes);
        if i.u8()? != VERSION {
            return Err(CodecError::Version);
        }
        let status = i.u16()?;
        if !(100..=599).contains(&status) {
            return Err(CodecError::Value);
        }
        let resp = Self {
            status,
            headers: i.pairs()?,
            body: i.body(max_body)?,
        };
        i.done()?;
        Ok(resp)
    }
}
