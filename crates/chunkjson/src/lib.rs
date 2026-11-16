//! A sans-IO streaming JSON tokenizer.
//!
//! Feed byte slices to a [`Tokenizer`] and it returns one [`Event`] per call.
//! It never reads or blocks, so the same code runs in a blocking loop, a tokio
//! task, or over a pooled buffer. A slice may end anywhere, even inside a token
//! or a UTF-8 character.
//!
//! - **One document streams.** A string value arrives as fragments, so a huge
//!   value is never held whole; only numbers are buffered, up to a limit.
//! - **Framing in the same scan.** One document, whitespace-separated
//!   documents, or strict NDJSON; each document's end carries its byte range,
//!   so its raw text can be copied without re-encoding.
//! - **Bounded memory.** Everything the tokenizer holds is allocated once, at
//!   construction, and sized by [`Config`].
//!
//! ```
//! use chunkjson::{Config, Event, Tokenizer};
//!
//! let mut tokenizer = Tokenizer::new(Config::default());
//! let mut input: &[u8] = br#"{"title": "a long \"page\""}"#;
//! let mut text = String::new();
//! loop {
//!     let (event, used) = tokenizer.next(input, true).unwrap();
//!     if let Event::String(fragment) = event {
//!         text.push_str(fragment.fragment);
//!     }
//!     if event == Event::Eof {
//!         break;
//!     }
//!     input = &input[used..];
//! }
//! assert_eq!(text, "a long \"page\"");
//! ```

mod config;
mod error;
mod escape;
mod event;
mod feed;
mod number;
mod read;
mod resolve;
mod string;
mod tokenizer;
mod writer;

pub use config::{Config, Framing};
pub use error::{Error, ErrorKind};
pub use event::{Event, Text};
pub use feed::Sink;
pub use read::{Buffer, JsonReader, ReadError};
pub use tokenizer::Tokenizer;
pub use writer::{IoOutput, Misuse, Output, WriteError, Writer};
