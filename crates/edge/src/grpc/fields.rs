//! Reading a decoded request message field by field, refusing what is not read.
//!
//! The mapping to a REST request names each field it covers by taking it. `finish` then fails
//! with the path of any field the client set that was not taken, so an unsupported field is
//! refused and never ignored (docs/specs/grpc-front/SPEC.md).

use prost_reflect::{DynamicMessage, ReflectMessage, Value};

use super::status::{INVALID_ARGUMENT, Status, UNIMPLEMENTED};

/// A message being read, and the names taken from it so far.
pub(crate) struct Fields<'a> {
    msg: &'a DynamicMessage,
    path: String,
    taken: Vec<String>,
}

impl<'a> Fields<'a> {
    pub(crate) fn new(msg: &'a DynamicMessage, path: &str) -> Self {
        // ALLOC: a field path, a few bytes per nesting level.
        Self {
            msg,
            path: path.to_owned(),
            // ALLOC: a field path or name, a few bytes per nesting level.
            taken: Vec::new(),
        }
    }

    /// The path of `name` inside this message, for errors.
    pub(crate) fn at(&self, name: &str) -> String {
        // ALLOC: a field path, a few bytes per nesting level.
        format!("{}.{name}", self.path)
    }

    fn get(&mut self, name: &str) -> Option<&'a Value> {
        let found = self
            .msg
            .fields()
            .find(|(f, _)| f.name() == name)
            .map(|(_, v)| v);
        if found.is_some() {
            // ALLOC: one name per field the client set, bounded by the schema.
            self.taken.push(name.to_owned());
        }
        found
    }

    pub(crate) fn string(&mut self, name: &str) -> Option<&'a str> {
        match self.get(name) {
            Some(Value::String(s)) => Some(s.as_str()),
            _ => None,
        }
    }

    pub(crate) fn boolean(&mut self, name: &str) -> Option<bool> {
        match self.get(name) {
            Some(Value::Bool(b)) => Some(*b),
            _ => None,
        }
    }

    pub(crate) fn bytes(&mut self, name: &str) -> Option<&'a [u8]> {
        match self.get(name) {
            Some(Value::Bytes(b)) => Some(b.as_ref()),
            _ => None,
        }
    }

    /// An enum value as REST spells it: the type's prefix removed and the rest lower case, so
    /// `REFRESH_WAIT_FOR` is `wait_for`. The zero value, `*_UNSPECIFIED`, reads as not set.
    pub(crate) fn enum_word(&mut self, name: &str) -> Option<String> {
        let Some(Value::EnumNumber(n)) = self.get(name) else {
            return None;
        };
        let field = self.msg.descriptor().get_field_by_name(name)?;
        let kind = field.kind();
        let desc = kind.as_enum()?;
        let value = desc.get_value(*n)?;
        // ALLOC: a field path or name, a few bytes per nesting level.
        let mut prefix = String::new();
        for (i, c) in desc.name().char_indices() {
            if i > 0 && c.is_ascii_uppercase() {
                prefix.push('_');
            }
            prefix.push(c.to_ascii_uppercase());
        }
        prefix.push('_');
        let word = value.name().strip_prefix(&prefix)?.to_ascii_lowercase();
        (word != "unspecified").then_some(word)
    }

    pub(crate) fn message(&mut self, name: &str) -> Option<&'a DynamicMessage> {
        match self.get(name) {
            Some(Value::Message(m)) => Some(m),
            _ => None,
        }
    }

    pub(crate) fn repeated(&mut self, name: &str) -> &'a [Value] {
        match self.get(name) {
            Some(Value::List(items)) => items.as_slice(),
            _ => &[],
        }
    }

    /// Fails with `UNIMPLEMENTED` naming the first field the client set that was not taken.
    pub(crate) fn finish(self) -> Result<(), Status> {
        if self.msg.unknown_fields().next().is_some() {
            return Err(Status::new(
                INVALID_ARGUMENT,
                // ALLOC: a field path or name, a few bytes per nesting level.
                format!("{} has a field the schema does not define", self.path),
            ));
        }
        for (field, _) in self.msg.fields() {
            if !self.taken.iter().any(|t| t == field.name()) {
                return Err(Status::new(
                    UNIMPLEMENTED,
                    // ALLOC: a field path or name, a few bytes per nesting level.
                    format!("{}.{} is not supported", self.path, field.name()),
                ));
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::grpc::schema;

    fn delete_op(json: &str) -> DynamicMessage {
        DynamicMessage::deserialize(
            schema::message("DeleteOperation"),
            &mut serde_json::Deserializer::from_str(json),
        )
        .expect("protojson")
    }

    #[test]
    fn enum_words_drop_the_type_prefix_and_keep_underscores() {
        let m = delete_op(r#"{"versionType":"VERSION_TYPE_EXTERNAL_GTE"}"#);
        let mut f = Fields::new(&m, "D");
        assert_eq!(f.enum_word("version_type").as_deref(), Some("external_gte"));
        let m = delete_op(r#"{"versionType":"VERSION_TYPE_UNSPECIFIED"}"#);
        assert_eq!(Fields::new(&m, "D").enum_word("version_type"), None);
    }
}
