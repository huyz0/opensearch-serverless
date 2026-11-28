//! The switches a create reads when it parses a mapping.

use super::IndexService;

impl IndexService {
    /// Accepts a `nested` object in a created index's mapping when `on`
    /// (`serverless.nested.enabled`); off, it is refused as unsupported.
    #[must_use]
    pub fn with_nested(mut self, on: bool) -> Self {
        self.nested = on;
        self
    }

    /// Accepts a `geo_point` field in a created index's mapping when `on`
    /// (`serverless.geospatial.index_fields`); off, it is refused as unsupported.
    #[must_use]
    pub fn with_geo(mut self, on: bool) -> Self {
        self.geo = on;
        self
    }
}
