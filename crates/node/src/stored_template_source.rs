//! Where a search that names a stored template finds it: the stored-script
//! document set (`_scripts/{id}`), read through a snapshot the node keeps
//! while the set has not moved. A request that names no id never reaches
//! it; one that does costs a marker GET on a warm node.

use std::sync::Arc;

use oss_blob::BlobStore;
use oss_doc::mapping::json::{Json, parse as parse_json};
use oss_format::{DocKind, MAX_DOC_BYTES};
use oss_meta::{DocSet, DocSetCache, MetaError};

use crate::search::{TemplateFault, TemplateSource};

/// The stored-script set and what the node keeps of it.
pub struct StoredTemplates {
    docs: DocSet,
    cache: DocSetCache,
    buffer_bytes: usize,
}

impl StoredTemplates {
    /// The set under `base` in `store`; a read holds two buffers of
    /// `MAX_DOC_BYTES`, which the request reserves first.
    ///
    /// # Errors
    ///
    /// [`MetaError`] for a base that is not empty and does not end in `/`.
    pub fn open(store: &Arc<dyn BlobStore>, base: &str) -> Result<Arc<Self>, MetaError> {
        // ALLOC: one holder, once, at boot.
        Ok(Arc::new(Self {
            docs: DocSet::new(Arc::clone(store), base, DocKind::StoredScript)?,
            cache: DocSetCache::new(),
            buffer_bytes: MAX_DOC_BYTES,
        }))
    }
}

/// The language and source of a stored `{"script":{"lang","source"}}`.
fn script(body: &[u8]) -> Option<(String, String)> {
    let Json::Obj(top) = parse_json(body).ok()? else {
        return None;
    };
    let [(_, Json::Obj(members))] = top.as_slice() else {
        return None;
    };
    let (mut lang, mut source) = (None, None);
    for (name, value) in members {
        match (name.as_str(), value) {
            ("lang", Json::Str(s)) => lang = Some(s.clone()),
            ("source", Json::Str(s)) => source = Some(s.clone()),
            _ => {}
        }
    }
    Some((lang?, source?))
}

impl TemplateSource for StoredTemplates {
    fn reserve_bytes(&self) -> usize {
        self.buffer_bytes.saturating_mul(2)
    }

    fn resolve(&self, id: &str) -> Result<String, TemplateFault> {
        // ALLOC: two scratch buffers a request that names a template, reserved by the caller.
        let mut marker = vec![0u8; self.buffer_bytes];
        // ALLOC: as above.
        let mut doc = vec![0u8; self.buffer_bytes];
        let snapshot = self
            .docs
            .snapshot(&self.cache, &mut marker, &mut doc)
            // ALLOC: error path; one message.
            .map_err(|e| TemplateFault::Unreadable(e.to_string()))?;
        let body = snapshot.get(id).ok_or(TemplateFault::Missing)?;
        let (lang, source) = script(body).ok_or_else(|| {
            // ALLOC: error path; one message.
            TemplateFault::Unreadable(format!("the script [{id}] is not a stored script"))
        })?;
        if lang == "mustache" {
            Ok(source)
        } else {
            Err(TemplateFault::WrongLang(lang))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use oss_blob::Emulator;

    #[test]
    fn a_stored_script_gives_its_language_and_source() {
        let got = script(br#"{"script":{"lang":"mustache","source":"{\"q\":\"{{x}}\"}"}}"#);
        assert_eq!(
            got,
            Some(("mustache".to_owned(), r#"{"q":"{{x}}"}"#.to_owned()))
        );
        assert!(script(br#"{"script":{"lang":"mustache"}}"#).is_none());
        assert!(script(b"[]").is_none());
        assert!(script(br#"{"a":1,"b":2}"#).is_none());
    }

    fn put(store: &Arc<dyn BlobStore>, name: &str, body: &str) {
        DocSet::new(Arc::clone(store), "", DocKind::StoredScript)
            .unwrap()
            .put(name, body.as_bytes(), false, &mut vec![0u8; 1 << 16])
            .unwrap();
    }

    #[test]
    fn an_id_resolves_to_mustache_source_and_nothing_else() {
        let store: Arc<dyn BlobStore> = Arc::new(Emulator::new(1 << 24, 1));
        let templates = StoredTemplates::open(&store, "").unwrap();
        assert!(matches!(
            templates.resolve("none"),
            Err(TemplateFault::Missing)
        ));
        put(
            &store,
            "m",
            r#"{"script":{"lang":"mustache","source":"{}"}}"#,
        );
        put(
            &store,
            "p",
            r#"{"script":{"lang":"painless","source":"1"}}"#,
        );
        put(&store, "junk", r#"{"x":1,"y":2}"#);
        assert_eq!(templates.resolve("m").unwrap(), "{}");
        assert!(matches!(
            templates.resolve("p"),
            Err(TemplateFault::WrongLang(l)) if l == "painless"
        ));
        assert!(matches!(
            templates.resolve("junk"),
            Err(TemplateFault::Unreadable(_))
        ));
        assert_eq!(templates.reserve_bytes(), 2 * MAX_DOC_BYTES);
    }
}
