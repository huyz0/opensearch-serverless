# A document's `knn_vector` value, as the k-NN plugin reads it

## Serves

Parity plan R15, k-NN native, step 2a of four: the mapping is in (`knn-vector-mapping`); this spec reads a document's vector into a value the next steps index; 2b indexes it into the segment, 2c keeps `_source` without it and rebuilds it on fetch, step 3 is the `knn` query. Nothing indexes the field until 2b: a caller that has not opted in is still told the type is unsupported.

## Change

- `FieldValue::Vector(Vec<f32>)`, produced for a `knn_vector` leaf by the document walker. A vector is one array, so the walker reads the whole array when the mapped field is a `knn_vector`, instead of each element as a value of the field.
- The rules are those of `KNNVectorFieldMapper.parseCreateField` for a float vector (k-NN 3.10.0.0), as the recorded `fixtures/compat/knn/knn-documents.json` and `knn-cosine.json` show them:
  - `null`, and an absent field, add nothing; a `null` array is skipped;
  - an array of numbers or numeric strings is read as `f32` (`Float.parseFloat` of the text for a string, Jackson's `getFloatValue` for a number);
  - a number or string that is not an array is read as an array of one, an object as an array of none, so `5` reports `Given: 1`, `{"a":1}` `Given: 0`;
  - the count is checked against the mapping's dimension: `Vector dimension mismatch. Expected: D, Given: N`;
  - a non-numeric string is `For input string: "a"`, a number that overflows `f32` or a string that parses to it is `KNN vector values cannot be infinity`, `NaN` is `KNN vector values cannot be NaN`;
  - an array element that is a boolean, `null` or an array is Jackson's `not numeric, cannot use numeric value accessors`, reported twice as for every coercion;
  - a string value that is not an array is read as base64 first, which is how the plugin refuses `"1,2,3"`: `Invalid base64 encoding for vector field [v]: Illegal base64 character 2c`, caused by `Illegal base64 character 2c`;
  - a cosine field refuses a vector of zeros (`knn-cosine.json`);
  - every refusal is a `mapper_parsing_exception` `failed to parse field [v] of type [knn_vector] in document with id '<id>'. Preview of field's value: '<what the plugin previews>'`, caused by the cause above.
- The vector is read only for a mapping parsed with `ParseOptions { knn: Some(_) }`, so nothing changes for any caller until 2b.
- A field's `ignore_malformed` is not a `knn_vector` parameter; a vector the plugin refuses is refused whatever the setting.

## Acceptance criteria

- AC-1: every `PUT _doc` step of `fixtures/compat/knn/knn-documents.json` and `knn-cosine.json` replays against `Document::parse`: an accepted document yields the vector the plugin would index, a refused one carries the plugin's exception types, messages and preview down its `caused_by` chain.
- AC-2: a vector in a nested object (`o.v`), and a field next to other fields, parse; the documents' other fields are unchanged.
- AC-3: a document parsed with a mapping that has not opted in is refused as unsupported, as before.
- AC-4: an accepted vector's components equal what `Float.parseFloat` or Jackson gives for the same text, including `-0.0`, a subnormal and a 17-digit decimal.
- AC-5: a vector of the mapping's dimension is read without a request to the object store and with memory bounded by the dimension (16,000) times 4 bytes.

## S3 request budget

None: document parsing makes no request.

## Switch

`ParseOptions::knn`, as in the mapping spec; off in every caller until 2b.
