//! Mail through an SMTP relay the operator declared (ADR 0081): EHLO, an
//! optional STARTTLS or implicit TLS, `AUTH PLAIN` only over TLS, then one
//! message to every recipient the relay accepts.
//!
//! The connection goes through the deliverer's guard, so the egress policy,
//! the slots and the time bound apply as they do to a webhook. Nothing the
//! relay says is kept: a refusal is a status word. The message body is base64,
//! so no line of it can end the data early, and an address or a subject with a
//! line break is refused rather than written.

use std::io::{ErrorKind, Read, Write};
use std::net::TcpStream;
use std::sync::Arc;
use std::time::Instant;

use rustls::pki_types::ServerName;
use rustls::{ClientConfig, ClientConnection, RootCertStore, StreamOwned};

use super::accounts::SmtpAccount;
use super::deliver::{Deliverer, Delivery};

/// The longest line a relay may send, and the most lines of one reply.
const MAX_LINE: usize = 2048;
const MAX_REPLY_LINES: usize = 64;

enum Conn {
    Plain(TcpStream),
    Tls(Box<StreamOwned<ClientConnection, TcpStream>>),
}

impl Conn {
    fn tcp(&self) -> &TcpStream {
        match self {
            Self::Plain(s) => s,
            Self::Tls(s) => &s.sock,
        }
    }
}

impl Read for Conn {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        match self {
            Self::Plain(s) => s.read(buf),
            Self::Tls(s) => s.read(buf),
        }
    }
}

impl Write for Conn {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        match self {
            Self::Plain(s) => s.write(buf),
            Self::Tls(s) => s.write(buf),
        }
    }
    fn flush(&mut self) -> std::io::Result<()> {
        match self {
            Self::Plain(s) => s.flush(),
            Self::Tls(s) => s.flush(),
        }
    }
}

fn io_failure(e: &std::io::Error) -> Delivery {
    match e.kind() {
        ErrorKind::TimedOut | ErrorKind::WouldBlock => Delivery::failed(504, "timeout"),
        _ => Delivery::failed(502, "failed"),
    }
}

fn base64(bytes: &[u8], wrap: bool) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    // ALLOC: the encoded message, bounded by the body limit checked by the caller.
    let mut out = String::with_capacity(
        bytes
            .len()
            .saturating_mul(4)
            .saturating_div(3)
            .saturating_add(8),
    );
    let mut col = 0usize;
    for chunk in bytes.chunks(3) {
        let b = [
            chunk.first().copied().unwrap_or(0),
            chunk.get(1).copied().unwrap_or(0),
            chunk.get(2).copied().unwrap_or(0),
        ];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        let pick = |shift: u32| T[((n >> shift) & 63) as usize] as char;
        out.push(pick(18));
        out.push(pick(12));
        out.push(if chunk.len() > 1 { pick(6) } else { '=' });
        out.push(if chunk.len() > 2 { pick(0) } else { '=' });
        col = col.saturating_add(4);
        if wrap && col >= 76 {
            out.push_str("\r\n");
            col = 0;
        }
    }
    out
}

/// An address that can be written into a command and a header: no break,
/// space, angle bracket or control character, and one `@`.
fn address_ok(a: &str) -> bool {
    !a.is_empty()
        && a.len() <= 254
        && a.matches('@').count() == 1
        && !a.starts_with('@')
        && !a.ends_with('@')
        && !a
            .chars()
            .any(|c| c.is_control() || c.is_whitespace() || "<>,;\"".contains(c))
}

struct Session {
    conn: Conn,
    buf: Vec<u8>,
    deadline: Instant,
}

impl Session {
    fn arm(&self) -> Result<(), Delivery> {
        let left = self.deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return Err(Delivery::failed(504, "timeout"));
        }
        let tcp = self.conn.tcp();
        tcp.set_read_timeout(Some(left))
            .map_err(|e| io_failure(&e))?;
        tcp.set_write_timeout(Some(left))
            .map_err(|e| io_failure(&e))
    }

    fn line(&mut self) -> Result<String, Delivery> {
        loop {
            if let Some(end) = self.buf.iter().position(|b| *b == b'\n') {
                let raw: Vec<u8> = self.buf.drain(..=end).collect();
                return Ok(String::from_utf8_lossy(&raw).trim_end().to_owned());
            }
            if self.buf.len() > MAX_LINE {
                return Err(Delivery::failed(502, "failed"));
            }
            self.arm()?;
            let mut chunk = [0u8; 1024];
            let n = self.conn.read(&mut chunk).map_err(|e| io_failure(&e))?;
            if n == 0 {
                return Err(Delivery::failed(502, "failed"));
            }
            self.buf
                .extend_from_slice(chunk.get(..n).unwrap_or_default());
        }
    }

    /// The code of one reply, once its last line has arrived.
    fn reply(&mut self) -> Result<u16, Delivery> {
        for _ in 0..MAX_REPLY_LINES {
            let l = self.line()?;
            let code: u16 = l
                .get(..3)
                .and_then(|c| c.parse().ok())
                .ok_or_else(|| Delivery::failed(502, "failed"))?;
            if l.as_bytes().get(3) != Some(&b'-') {
                return Ok(code);
            }
        }
        // A reply that never ends is not one the dialog can go on from.
        Err(Delivery::failed(502, "failed"))
    }

    fn send(&mut self, text: &str) -> Result<(), Delivery> {
        self.arm()?;
        self.conn
            .write_all(text.as_bytes())
            .map_err(|e| io_failure(&e))?;
        self.conn.flush().map_err(|e| io_failure(&e))
    }

    /// Sends a command and answers its code.
    fn command(&mut self, text: &str) -> Result<u16, Delivery> {
        self.send(&format!("{text}\r\n"))?;
        self.reply()
    }

    fn expect(&mut self, text: &str, want: &[u16]) -> Result<(), Delivery> {
        let code = self.command(text)?;
        if want.contains(&code) {
            Ok(())
        } else {
            Err(Delivery::failed(502, "refused"))
        }
    }

    fn upgrade(&mut self, tls: &Arc<ClientConfig>, host: &str) -> Result<(), Delivery> {
        let name =
            ServerName::try_from(host.to_owned()).map_err(|_| Delivery::failed(400, "no url"))?;
        let conn = ClientConnection::new(Arc::clone(tls), name)
            .map_err(|_| Delivery::failed(502, "failed"))?;
        let spare = self.conn.tcp().try_clone().map_err(|e| io_failure(&e))?;
        let plain = std::mem::replace(&mut self.conn, Conn::Plain(spare));
        let Conn::Plain(tcp) = plain else {
            return Err(Delivery::failed(502, "failed"));
        };
        self.conn = Conn::Tls(Box::new(StreamOwned::new(conn, tcp)));
        Ok(())
    }
}

fn tls_config(account: &SmtpAccount) -> Result<Arc<ClientConfig>, Delivery> {
    let mut roots = RootCertStore::empty();
    roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    for der in &account.extra_roots {
        roots
            .add(der.clone())
            .map_err(|_| Delivery::failed(502, "failed"))?;
    }
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let config = ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .map_err(|_| Delivery::failed(502, "failed"))?
        .with_root_certificates(roots)
        .with_no_client_auth();
    Ok(Arc::new(config))
}

/// The message: headers an operator can read, and the text in base64.
fn message(from: &str, to: &[&str], subject: &str, text: &str) -> String {
    let subject: String = subject.chars().filter(|c| !c.is_control()).collect();
    let subject = if subject.is_ascii() {
        subject
    } else {
        format!("=?UTF-8?B?{}?=", base64(subject.as_bytes(), false))
    };
    // ALLOC: one message, bounded by the body limit.
    format!(
        "From: <{from}>\r\nTo: {}\r\nSubject: {subject}\r\nMIME-Version: 1.0\r\n\
         Content-Type: text/plain; charset=UTF-8\r\nContent-Transfer-Encoding: base64\r\n\r\n{}\r\n.\r\n",
        to.iter()
            .map(|a| format!("<{a}>"))
            .collect::<Vec<_>>()
            .join(", "),
        base64(text.as_bytes(), true),
    )
}

fn run(
    d: &Deliverer,
    account: &SmtpAccount,
    method: &str,
    from: &str,
    to: &[&str],
    msg: &str,
) -> Result<Vec<Delivery>, Delivery> {
    let deadline = Instant::now()
        .checked_add(d.total())
        .unwrap_or_else(Instant::now);
    let tcp = d.connect(&account.host, account.port, deadline)?;
    let tls = if method == "none" {
        None
    } else {
        Some(tls_config(account)?)
    };
    let mut s = Session {
        conn: Conn::Plain(tcp),
        buf: Vec::new(),
        deadline,
    };
    if method == "ssl"
        && let Some(t) = &tls
    {
        s.upgrade(t, &account.host)?;
    }
    if s.reply()? != 220 {
        return Err(Delivery::failed(502, "refused"));
    }
    s.expect("EHLO oss-node", &[250])?;
    if method == "start_tls"
        && let Some(t) = &tls
    {
        s.expect("STARTTLS", &[220])?;
        if !s.buf.is_empty() {
            return Err(Delivery::failed(502, "failed"));
        }
        s.upgrade(t, &account.host)?;
        s.expect("EHLO oss-node", &[250])?;
    }
    if let Some((user, pass)) = &account.login {
        let token = base64(format!("\0{user}\0{pass}").as_bytes(), false);
        if s.command(&format!("AUTH PLAIN {token}"))? != 235 {
            return Err(Delivery::failed(502, "auth"));
        }
    }
    s.expect(&format!("MAIL FROM:<{from}>"), &[250])?;
    let mut results: Vec<Delivery> = Vec::new();
    for a in to {
        let code = s.command(&format!("RCPT TO:<{a}>"))?;
        results.push(if matches!(code, 250 | 251) {
            Delivery {
                status_code: 200,
                status_text: "Success".to_owned(),
            }
        } else {
            Delivery::failed(502, "refused")
        });
    }
    if results.iter().all(|r| r.status_code != 200) {
        let _ = s.command("QUIT");
        return Ok(results);
    }
    s.expect("DATA", &[354])?;
    s.send(msg)?;
    if s.reply()? != 250 {
        return Ok(to
            .iter()
            .map(|_| Delivery::failed(502, "refused"))
            .collect());
    }
    let _ = s.command("QUIT");
    Ok(results)
}

/// Sends `text` from `from` to each of `to` through `account`; one status for
/// each recipient, in order.
pub(super) fn send_mail(
    d: &Deliverer,
    account: &SmtpAccount,
    method: &str,
    from: &str,
    to: &[String],
    subject: &str,
    text: &str,
) -> Vec<Delivery> {
    let all = |x: Delivery| to.iter().map(|_| x.clone()).collect::<Vec<_>>();
    if !address_ok(from) || !matches!(method, "ssl" | "start_tls" | "none") {
        return all(Delivery::failed(400, "bad address"));
    }
    if account.login.is_some() && method == "none" {
        // A password is never sent in the clear.
        return all(Delivery::failed(403, "no account"));
    }
    let good: Vec<&str> = to
        .iter()
        .map(String::as_str)
        .filter(|a| address_ok(a))
        .collect();
    let msg = message(from, &good, subject, text);
    if msg.len() > d.max_body() {
        return all(Delivery::failed(413, "too large"));
    }
    let ran = d.slotted(|| run(d, account, method, from, &good, &msg));
    let sent = match ran {
        None => return all(Delivery::failed(429, "rejected")),
        Some(Err(e)) => return all(e),
        Some(Ok(v)) => v,
    };
    let mut sent = sent.into_iter();
    to.iter()
        .map(|a| {
            if address_ok(a) {
                sent.next()
                    .unwrap_or_else(|| Delivery::failed(502, "failed"))
            } else {
                Delivery::failed(400, "bad address")
            }
        })
        .collect()
}
