//! Checking the signature of a JSON Web Token against the one configured
//! key ([ADR 0090](../../../docs/adr/0090-jwt-authentication-carries-an-identity-with-backend-roles.md)).
//!
//! The key decides the algorithm; the token's own `alg` is only compared
//! with it. The claims are judged by `oss_coordinator::security::claims`.

use std::sync::Arc;

use oss_coordinator::security::{Claims, MAX_TOKEN_BYTES, Rules, claims, header_alg};
use ring::{hmac, signature};

/// Seconds since the epoch.
pub type WallClock = Arc<dyn Fn() -> u64 + Send + Sync>;

/// Now, in seconds since the epoch; 0 for a clock before it.
pub fn wall_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// The key a token is checked with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Key {
    /// An HMAC secret.
    Secret(Vec<u8>),
    /// An RSA public key, as PKCS#1 DER.
    Rsa(Vec<u8>),
    /// A P-256 public point.
    P256(Vec<u8>),
    /// A P-384 public point.
    P384(Vec<u8>),
}

/// A configured JWT domain.
pub struct Jwt {
    key: Key,
    rules: Rules,
    now: WallClock,
}

// ARITH: bytes inside the matched ranges, so each subtraction is non-negative.
#[allow(clippy::arithmetic_side_effects)]
fn sextet(b: u8) -> Option<u8> {
    match b {
        b'A'..=b'Z' => Some(b - b'A'),
        b'a'..=b'z' => Some(b - b'a' + 26),
        b'0'..=b'9' => Some(b - b'0' + 52),
        b'+' | b'-' => Some(62),
        b'/' | b'_' => Some(63),
        _ => None,
    }
}

/// Decodes base64 in either alphabet, with or without padding.
// ARITH: at most 8 KiB of input; `acc` keeps 24 bits and `bits` stays under 14.
#[allow(clippy::arithmetic_side_effects)]
fn decode(text: &[u8]) -> Option<Vec<u8>> {
    let text: Vec<u8> = text
        .iter()
        .copied()
        .filter(|b| !matches!(b, b'=' | b'\n' | b'\r' | b' '))
        .collect();
    // ALLOC: the decoded token or key, at most MAX_TOKEN_BYTES.
    let mut out = Vec::with_capacity(text.len() / 4 * 3 + 3);
    let (mut acc, mut bits) = (0u32, 0u32);
    for &b in &text {
        acc = ((acc << 6) | u32::from(sextet(b)?)) & 0xff_ffff;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push(u8::try_from((acc >> bits) & 0xff).ok()?);
        }
    }
    Some(out)
}

/// One DER element: its tag, its content and what follows it.
fn der(bytes: &[u8]) -> Option<(u8, &[u8], &[u8])> {
    let (&tag, rest) = bytes.split_first()?;
    let (&first, rest) = rest.split_first()?;
    let (len, rest) = if first < 0x80 {
        (usize::from(first), rest)
    } else {
        let n = usize::from(first & 0x7f);
        if n == 0 || n > 4 || rest.len() < n {
            return None;
        }
        let len = rest[..n]
            .iter()
            .fold(0usize, |acc, b| (acc << 8) | usize::from(*b));
        (len, &rest[n..])
    };
    (rest.len() >= len).then(|| (tag, &rest[..len], &rest[len..]))
}

const RSA_OID: &[u8] = &[0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 0x01, 0x01, 0x01];
const EC_OID: &[u8] = &[0x2a, 0x86, 0x48, 0xce, 0x3d, 0x02, 0x01];
const P256_OID: &[u8] = &[0x2a, 0x86, 0x48, 0xce, 0x3d, 0x03, 0x01, 0x07];
const P384_OID: &[u8] = &[0x2b, 0x81, 0x04, 0x00, 0x22];

impl Key {
    /// An HMAC secret from its base64 text; at least 32 bytes.
    ///
    /// # Errors
    ///
    /// A message when it is not base64 or is too short.
    pub fn secret(text: &str) -> Result<Self, String> {
        let bytes = decode(text.trim().as_bytes())
            .ok_or_else(|| "OSS_SECURITY_JWT_KEY is not base64".to_owned())?;
        if bytes.len() < 32 {
            return Err("OSS_SECURITY_JWT_KEY is at least 32 bytes".to_owned());
        }
        Ok(Self::Secret(bytes))
    }

    /// A public key from a PEM (`BEGIN PUBLIC KEY`, RSA or P-256 or P-384).
    ///
    /// # Errors
    ///
    /// A message when it is not such a PEM.
    pub fn public(pem: &str) -> Result<Self, String> {
        let bad = || {
            "OSS_SECURITY_JWT_PUBLIC_KEY is not a PEM public key (RSA, P-256 or P-384)".to_owned()
        };
        let body: String = pem
            .lines()
            .filter(|l| !l.starts_with("-----"))
            .collect::<Vec<_>>()
            .concat();
        if !pem.contains("BEGIN PUBLIC KEY") {
            return Err(bad());
        }
        let spki = decode(body.as_bytes()).ok_or_else(bad)?;
        let (_, outer, _) = der(&spki).ok_or_else(bad)?;
        let (_, algorithm, rest) = der(outer).ok_or_else(bad)?;
        let (_, key, _) = der(rest).ok_or_else(bad)?;
        let key = key
            .split_first()
            .filter(|(unused, _)| **unused == 0)
            .ok_or_else(bad)?
            .1;
        let (_, oid, params) = der(algorithm).ok_or_else(bad)?;
        // ALLOC: one key, once, at start.
        let key = key.to_vec();
        if oid == RSA_OID {
            return Ok(Self::Rsa(key));
        }
        if oid == EC_OID {
            let (_, curve, _) = der(params).ok_or_else(bad)?;
            return match curve {
                c if c == P256_OID => Ok(Self::P256(key)),
                c if c == P384_OID => Ok(Self::P384(key)),
                _ => Err(bad()),
            };
        }
        Err(bad())
    }
}

enum Check {
    Hmac(hmac::Algorithm),
    Rsa(&'static signature::RsaParameters),
    Ec(&'static signature::EcdsaVerificationAlgorithm),
}

impl Jwt {
    /// A domain with `key` and `rules`, reading the time from `now`.
    pub fn new(key: Key, rules: Rules, now: WallClock) -> Self {
        Self { key, rules, now }
    }

    fn check_for(&self, alg: &str) -> Option<Check> {
        match (&self.key, alg) {
            (Key::Secret(k), "HS256") if k.len() >= 32 => Some(Check::Hmac(hmac::HMAC_SHA256)),
            (Key::Secret(k), "HS384") if k.len() >= 48 => Some(Check::Hmac(hmac::HMAC_SHA384)),
            (Key::Secret(k), "HS512") if k.len() >= 64 => Some(Check::Hmac(hmac::HMAC_SHA512)),
            (Key::Rsa(_), "RS256") => Some(Check::Rsa(&signature::RSA_PKCS1_2048_8192_SHA256)),
            (Key::Rsa(_), "RS384") => Some(Check::Rsa(&signature::RSA_PKCS1_2048_8192_SHA384)),
            (Key::Rsa(_), "RS512") => Some(Check::Rsa(&signature::RSA_PKCS1_2048_8192_SHA512)),
            (Key::P256(_), "ES256") => Some(Check::Ec(&signature::ECDSA_P256_SHA256_FIXED)),
            (Key::P384(_), "ES384") => Some(Check::Ec(&signature::ECDSA_P384_SHA384_FIXED)),
            _ => None,
        }
    }

    fn signed(&self, check: &Check, message: &[u8], sig: &[u8]) -> bool {
        match (check, &self.key) {
            (Check::Hmac(alg), Key::Secret(k)) => {
                hmac::verify(&hmac::Key::new(*alg, k), message, sig).is_ok()
            }
            (Check::Rsa(p), Key::Rsa(k)) => signature::UnparsedPublicKey::new(*p, k)
                .verify(message, sig)
                .is_ok(),
            (Check::Ec(a), Key::P256(k) | Key::P384(k)) => signature::UnparsedPublicKey::new(*a, k)
                .verify(message, sig)
                .is_ok(),
            _ => false,
        }
    }

    /// The claims of `token` when it is signed by the key and acceptable at
    /// the current time; `None` for any reason at all.
    // ARITH: the three parts are slices of `token`, so their lengths sum to at most its length.
    #[allow(clippy::arithmetic_side_effects)]
    pub fn verify(&self, token: &str) -> Option<Claims> {
        if token.len() > MAX_TOKEN_BYTES {
            return None;
        }
        let mut parts = token.split('.');
        let (header, payload, sig) = (parts.next()?, parts.next()?, parts.next()?);
        if parts.next().is_some() {
            return None;
        }
        let alg = header_alg(&decode(header.as_bytes())?)?;
        let check = self.check_for(&alg)?;
        let sig = decode(sig.as_bytes())?;
        let message = &token.as_bytes()[..header.len() + 1 + payload.len()];
        if !self.signed(&check, message, &sig) {
            return None;
        }
        claims(&decode(payload.as_bytes())?, &self.rules, (self.now)())
    }
}
