//! The first backing index of a data stream, created as `PUT /{index}`
//! creates an index but layering the template that wins the stream's name.

use oss_coordinator::indices::create::parse;
use oss_rest::{Request, RestError};

use super::{IndexService, is_bucket_key, refusal, refuse};

impl IndexService {
    /// Creates the first backing index of the data stream `stream`
    /// (`.ds-<stream>-000001`), layering the template that wins the
    /// stream's name, and mapping `timestamp` as a date unless the template
    /// maps it. Answers the index's uuid.
    pub(crate) fn create_backing(
        &self,
        req: &mut Request<'_>,
        backing: &str,
        stream: &str,
    ) -> Result<(String, String), RestError> {
        let Some(templates) = &self.templates else {
            return Err(no_stream_template(stream));
        };
        let (mut marker, mut doc) = self.scratch(req)?;
        let inherited = templates
            .inherited(stream, &mut marker, &mut doc)
            .map_err(refusal)?;
        let Some(inherited) = inherited.filter(|i| i.is_data_stream()) else {
            return Err(no_stream_template(stream));
        };
        let timestamp = inherited.timestamp_field();
        let uuid = self.stream_index(req, backing, Some(&inherited), &timestamp)?;
        Ok((uuid, timestamp))
    }

    /// Creates the backing index `backing` of an existing data stream, as
    /// [`IndexService::create_backing`] does, under the template that wins
    /// `stream` if there still is one; the timestamp is the stream's own.
    pub(crate) fn create_next_backing(
        &self,
        req: &mut Request<'_>,
        backing: &str,
        stream: &str,
        timestamp: &str,
    ) -> Result<String, RestError> {
        let inherited = match &self.templates {
            Some(templates) => {
                let (mut marker, mut doc) = self.scratch(req)?;
                templates
                    .inherited(stream, &mut marker, &mut doc)
                    .map_err(refusal)?
            }
            None => None,
        };
        self.stream_index(req, backing, inherited.as_ref(), timestamp)
    }

    /// Creates the index `name` a rollover of an alias moves to, as
    /// `PUT /{index}` with no body creates it.
    pub(crate) fn create_rolled(
        &self,
        req: &mut Request<'_>,
        name: &str,
    ) -> Result<String, RestError> {
        let mut asked = parse(1, None).map_err(refusal)?;
        if let Some(templates) = &self.templates {
            let (mut marker, mut doc) = self.scratch(req)?;
            asked = templates
                .apply(name, asked, &mut marker, &mut doc)
                .map_err(refusal)?;
        }
        crate::create_settings::refuse_others(&asked.settings, is_bucket_key)?;
        self.build(req, name, asked, &[])
    }

    /// The two scratch buffers a template read uses, reserved from the
    /// request-body pool first.
    fn scratch(&self, req: &mut Request<'_>) -> Result<(Vec<u8>, Vec<u8>), RestError> {
        req.body().reserve(self.buffer_bytes.saturating_mul(2))?;
        // ALLOC: two scratch buffers a create, buffer_bytes each, reserved above.
        Ok((vec![0u8; self.buffer_bytes], vec![0u8; self.buffer_bytes]))
    }

    fn stream_index(
        &self,
        req: &mut Request<'_>,
        backing: &str,
        inherited: Option<&oss_coordinator::templates::Inherited>,
        timestamp: &str,
    ) -> Result<String, RestError> {
        let mut asked = parse(1, None).map_err(refusal)?;
        if let Some(inherited) = inherited {
            asked = oss_coordinator::indices::create_inherit::inherit(asked, inherited)
                .map_err(refusal)?;
        }
        asked.mapping = Some(with_timestamp(asked.mapping.take(), timestamp)?);
        crate::create_settings::refuse_others(&asked.settings, is_bucket_key)?;
        self.build(req, backing, asked, &[])
    }
}

fn no_stream_template(stream: &str) -> RestError {
    // ALLOC: error path; one message.
    refuse(
        400,
        "illegal_argument_exception",
        format!("no matching index template found for data stream [{stream}]"),
    )
}

/// `mapping`, a JSON object or none, with `timestamp` mapped as a date
/// under it.
fn with_timestamp(mapping: Option<String>, timestamp: &str) -> Result<String, RestError> {
    use oss_coordinator::templates::merge_mappings;
    use oss_doc::mapping::json::{Json, parse as parse_json};
    // ALLOC: the default mapping of one create, bounded by the name.
    let mut merged = vec![(
        "properties".to_owned(),
        Json::Obj(vec![(
            timestamp.to_owned(),
            Json::Obj(vec![("type".to_owned(), Json::Str("date".to_owned()))]),
        )]),
    )];
    if let Some(text) = mapping {
        let Ok(Json::Obj(own)) = parse_json(text.as_bytes()) else {
            return Err(refuse(400, "malformed_body", "mappings must be an object"));
        };
        merge_mappings(&mut merged, &own);
    }
    Ok(Json::Obj(merged).to_json())
}
