# Response shapes of the stock anomaly detection plugin, as written down here

From the plugin's published documentation examples; provisional until a stock run is recorded (AC-15 of the spec).

- **Create or replace** (`POST|PUT /detectors[/{id}]`): `{"_id":"<id>","_version":1,"_seq_no":0,"_primary_term":1,"anomaly_detector":{...}}`.
- **Detector** (`anomaly_detector`): `name`, `description`, `time_field`, `indices` (array), `filter_query` (a query object, `match_all` when absent), `detection_interval` and `window_delay` (`{"period":{"interval":10,"unit":"Minutes"}}`), `shingle_size` (8 when absent), `schema_version`, `feature_attributes` (`[{"feature_id","feature_name","feature_enabled","aggregation_query":{"<name>":{"<agg>":{"field":"<f>"}}}}]`), `ui_metadata`, `last_update_time` (ms), and for a high-cardinality detector `category_field` (array) and `detector_type`.
- **Get** (`GET /detectors/{id}`): the create shape with `anomaly_detector`; with `job=true` also `anomaly_detector_job` (`name`, `schedule.interval {start_time, period, unit}`, `window_delay`, `enabled`, `enabled_time`, `disabled_time`, `last_update_time`, `lock_duration_seconds`); with `task=true` also `realtime_detection_task` and `historical_analysis_task`. A missing detector is 404 with an `error` of type `status_exception`.
- **Start** answers `{"_id","_version","_seq_no","_primary_term"}`; **stop** answers the detector's id in the plugin's message form. To record before slice 4 builds them.
- **Preview**: `{"anomaly_result":[{"detector_id":null,"anomaly_grade":0.0,"anomaly_score":0.0,"confidence":0.0,"feature_data":[{"feature_id","feature_name","data"}],"data_start_time","data_end_time","entity":[{"name","value"}]}],"anomaly_detector":{...}}`.
- **Result document** in the result index: `detector_id`, `schema_version`, `data_start_time`, `data_end_time`, `execution_start_time`, `execution_end_time`, `anomaly_score`, `anomaly_grade`, `confidence`, `feature_data`, `entity`, `model_id`, `task_id`, and `error` when the run failed.
- **Search** (`/detectors/_search`, `/detectors/results/_search`): an ordinary `_search` body and response over the detector set or the result index.
- **Profile**: `{"state":"RUNNING","error":"","init_progress":{"percentage":"100%","estimated_minutes_left":0,"needed_shingles":0},"total_entities":n,"models":[...]}`.
- **Stats** (`GET /stats`): `anomaly_detectors_index_status`, `anomaly_detection_state_status`, `detector_count`, `single_entity_detector_count`, `multi_entity_detector_count`, and under `nodes.<id>`: `ad_execute_request_count`, `ad_execute_failure_count`, `ad_hc_execute_request_count`, `model_count`.

Still to write down before the slices that build them: `_validate` bodies, `_topAnomalies`, `tasks/_search`, the 400 and 409 error bodies.
