import sys, os, json, math
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from lib import *
AD = "/_plugins/_anomaly_detection"
SRC = "compat-adr-src"
CUSTOM = "opensearch-ad-plugin-result-compat"
LIVE = "compat-adr-live"
import time
T0 = 1700000000000
MIN = 60000
N = 600
END = T0 + N * MIN
# two hosts, three cities; host h0 spikes at minutes 400..403, host h1 at 500; city c2 is a third of the rows
rows = []
for i in range(N):
    for h in range(2):
        spike = (h == 0 and 400 <= i < 404) or (h == 1 and i == 500)
        rows.append({"ts": T0 + i * MIN, "v": round(10 + math.sin(i / 5) + (30 if spike else 0), 4), "host": "h%d" % h, "city": "c%d" % ((i + h) % 3)})
nd = "".join(json.dumps(x) + "\n" for r in rows for x in ({"index": {}}, r))

now = int(time.time() // 60 * 60 * 1000)
live = "".join(json.dumps(x) + "\n" for i in range(240) for h in range(2) for x in ({"index": {}}, {"ts": now - (240 - i) * MIN, "v": round(10 + math.sin(i / 5), 4), "host": "h%d" % h}))


def det(name, **kw):
    d = {"name": name, "description": "x", "time_field": "ts", "indices": [SRC],
         "feature_attributes": [{"feature_name": "f", "feature_enabled": True, "aggregation_query": {"f": {"avg": {"field": "v"}}}}],
         "filter_query": {"match_all": {}}, "detection_interval": {"period": {"interval": 1, "unit": "Minutes"}}, "window_delay": {"period": {"interval": 1, "unit": "Minutes"}}}
    d.update(kw)
    return d


HIST = {"start_time": T0, "end_time": END}
TOP = {"size": 3, "category_field": ["host"], "start_time_ms": T0, "end_time_ms": END}
PROFILE_TYPES = "state,error,coordinating_node,shingle_size,total_size_in_bytes,models,init_progress,total_entities,ad_task"
DONE = ("/historical_analysis_task/state", "FINISHED", 90)


def top(name, detector, body, query=None, **kw):
    return step(name, "POST", AD + "/detectors/{%s}/results/_topAnomalies" % detector, body, query, **kw)


steps = [
 step("create single entity detector", "POST", AD + "/detectors", det("compat-r1"), capture={"s": "/_id"}),
 step("create detector by one category field", "POST", AD + "/detectors", det("compat-rh", category_field=["host"]), capture={"h": "/_id"}),
 step("create detector by two category fields", "POST", AD + "/detectors", det("compat-rh2", category_field=["host", "city"]), capture={"h2": "/_id"}),
 step("create detector with a custom result index", "POST", AD + "/detectors", det("compat-rc", category_field=["host"], result_index=CUSTOM), capture={"c": "/_id"}),
 step("create detector without results", "POST", AD + "/detectors", det("compat-r0", category_field=["host"]), capture={"e": "/_id"}),
 step("get one category field detector", "GET", AD + "/detectors/{h}"),
 step("get two category fields detector", "GET", AD + "/detectors/{h2}"),
 # preview and validate
 step("preview one category field detector", "POST", AD + "/detectors/_preview", {"period_start": T0 + 300 * MIN, "period_end": T0 + 520 * MIN, "detector": det("compat-pv", category_field=["host"])}),
 step("preview category detector by id", "POST", AD + "/detectors/{h}/_preview", {"period_start": T0 + 300 * MIN, "period_end": T0 + 520 * MIN}),
 step("preview single entity by id", "POST", AD + "/detectors/{s}/_preview", {"period_start": T0 + 300 * MIN, "period_end": T0 + 520 * MIN}),
 step("preview two category fields", "POST", AD + "/detectors/_preview", {"period_start": T0 + 300 * MIN, "period_end": T0 + 520 * MIN, "detector": det("compat-pv2", category_field=["host", "city"])}),
 step("validate category detector", "POST", AD + "/detectors/_validate", det("compat-rv", category_field=["host"])),
 step("validate category detector for the model", "POST", AD + "/detectors/_validate/model", det("compat-rv", indices=[LIVE], category_field=["host"])),
 step("validate single entity for the model", "POST", AD + "/detectors/_validate/model", det("compat-rv", indices=[LIVE])),
 step("validate category field of the wrong type", "POST", AD + "/detectors/_validate", det("compat-rv", category_field=["v"])),
 step("validate category field of an unknown field", "POST", AD + "/detectors/_validate", det("compat-rv", category_field=["nope"])),
 step("validate with the same name as a detector", "POST", AD + "/detectors/_validate", det("compat-r1")),
 step("run detector by id", "POST", AD + "/detectors/{s}/_run", {"period_start": T0 + 300 * MIN, "period_end": T0 + 520 * MIN}),
 step("run category detector by id", "POST", AD + "/detectors/{h}/_run", {"period_start": T0 + 300 * MIN, "period_end": T0 + 520 * MIN}),
 # nothing has run yet
 step("profile of a detector with no results", "GET", AD + "/detectors/{e}/_profile", query={"_all": "true"}),
 step("top anomalies of a detector with no results", "POST", AD + "/detectors/{e}/results/_topAnomalies", TOP),
 step("results search with no result index", "POST", AD + "/detectors/results/_search", {"size": 0, "query": {"match_all": {}}}),
 step("results search of an unknown result index", "POST", AD + "/detectors/results/_search/" + CUSTOM + "-nope", {"size": 0}),
 # historical analysis over the whole source
 step("start historical analysis of the single entity detector", "POST", AD + "/detectors/{s}/_start", HIST, capture={"st": "/_id"}),
 step("get detector once the single entity task ends", "GET", AD + "/detectors/{s}", query={"task": "true"}, poll=DONE),
 step("start historical analysis of the one field detector", "POST", AD + "/detectors/{h}/_start", HIST, capture={"ht": "/_id"}),
 step("get detector once the one field task ends", "GET", AD + "/detectors/{h}", query={"task": "true"}, poll=DONE),
 step("start historical analysis of the two field detector", "POST", AD + "/detectors/{h2}/_start", HIST, capture={"h2t": "/_id"}),
 step("get detector once the two field task ends", "GET", AD + "/detectors/{h2}", query={"task": "true"}, poll=DONE),
 step("start historical analysis of the custom result index detector", "POST", AD + "/detectors/{c}/_start", HIST, capture={"ct": "/_id"}),
 step("get detector once the custom result index task ends", "GET", AD + "/detectors/{c}", query={"task": "true"}, poll=DONE),
 step("start historical analysis with start after end", "POST", AD + "/detectors/{e}/_start", {"start_time": END, "end_time": T0}),
 step("start historical analysis with no data in range", "POST", AD + "/detectors/{e}/_start", {"start_time": T0 - 1000 * MIN, "end_time": T0 - 500 * MIN}, capture={"empty": "/_id"}),
 step("start historical analysis with a missing end", "POST", AD + "/detectors/{e}/_start", {"start_time": T0}),
 # profile, every type
 step("profile all of the single entity detector", "GET", AD + "/detectors/{s}/_profile", query={"_all": "true"}),
 step("profile all of the one field detector", "GET", AD + "/detectors/{h}/_profile", query={"_all": "true"}),
 step("profile all of the two field detector", "GET", AD + "/detectors/{h2}/_profile", query={"_all": "true"}),
 step("profile every type by name", "GET", AD + "/detectors/{h}/_profile/" + PROFILE_TYPES),
 step("profile state", "GET", AD + "/detectors/{h}/_profile/state"),
 step("profile error", "GET", AD + "/detectors/{h}/_profile/error"),
 step("profile coordinating node", "GET", AD + "/detectors/{h}/_profile/coordinating_node"),
 step("profile shingle size", "GET", AD + "/detectors/{h}/_profile/shingle_size"),
 step("profile total size in bytes", "GET", AD + "/detectors/{h}/_profile/total_size_in_bytes"),
 step("profile models", "GET", AD + "/detectors/{h}/_profile/models"),
 step("profile init progress", "GET", AD + "/detectors/{h}/_profile/init_progress"),
 step("profile total entities", "GET", AD + "/detectors/{h}/_profile/total_entities"),
 step("profile ad task", "GET", AD + "/detectors/{h}/_profile/ad_task"),
 step("profile of one entity", "GET", AD + "/detectors/{h}/_profile", query={"_all": "true", "entity": "host:h0"}),
 step("start historical analysis again over a short range", "POST", AD + "/detectors/{s}/_start", {"start_time": T0, "end_time": T0 + 10 * MIN}, capture={"short": "/_id"}),
 step("profile ad task of the single entity detector", "GET", AD + "/detectors/{s}/_profile/ad_task", poll=("/ad_task/ad_task/state", "FAILED", 60)),
 step("profile with the type in the query", "GET", AD + "/detectors/{h}/_profile", query={"type": "state,models"}),
 step("profile by POST", "POST", AD + "/detectors/{h}/_profile"),
 # tasks
 step("search tasks of one detector", "POST", AD + "/detectors/tasks/_search", {"size": 0, "query": {"term": {"detector_id": "{h}"}}, "aggs": {"types": {"terms": {"field": "task_type"}}, "states": {"terms": {"field": "state"}}}}),
 step("search tasks by type", "POST", AD + "/detectors/tasks/_search", {"size": 0, "query": {"match_all": {}}, "aggs": {"types": {"terms": {"field": "task_type"}}, "states": {"terms": {"field": "state"}}}}),
 step("search tasks of the latest only", "POST", AD + "/detectors/tasks/_search", {"size": 0, "query": {"bool": {"filter": [{"term": {"is_latest": True}}, {"term": {"detector_id": "{h2}"}}]}}, "aggs": {"types": {"terms": {"field": "task_type"}}}}),
 step("search tasks by GET", "GET", AD + "/detectors/tasks/_search", {"size": 0, "query": {"term": {"detector_id": "{s}"}}}),
 step("search tasks with a bad query", "POST", AD + "/detectors/tasks/_search", {"query": {"nope": {}}}),
 # result documents
 step("count results of the single entity detector", "POST", AD + "/detectors/results/_search", {"size": 0, "track_total_hits": True, "query": {"term": {"detector_id": "{s}"}}}),
 step("count results of the custom result index detector", "POST", AD + "/detectors/results/_search/" + CUSTOM, {"size": 0, "track_total_hits": True, "query": {"term": {"detector_id": "{c}"}}}),
 step("count results of the custom result index detector in the default index", "POST", AD + "/detectors/results/_search", {"size": 0, "track_total_hits": True, "query": {"term": {"detector_id": "{c}"}}}),
 step("count results of the one field detector", "POST", AD + "/detectors/results/_search", {"size": 0, "track_total_hits": True, "query": {"term": {"detector_id": "{h}"}}}),
 step("count results of the two field detector", "POST", AD + "/detectors/results/_search", {"size": 0, "track_total_hits": True, "query": {"term": {"detector_id": "{h2}"}}}),
 step("anomalies of the single entity detector", "POST", AD + "/detectors/results/_search", {"size": 6, "query": {"bool": {"filter": [{"term": {"detector_id": "{s}"}}, {"range": {"anomaly_grade": {"gt": 0}}}]}}, "sort": [{"data_start_time": "asc"}]}),
 step("first results of the single entity detector", "POST", AD + "/detectors/results/_search", {"size": 3, "query": {"term": {"detector_id": "{s}"}}, "sort": [{"data_start_time": "asc"}]}),
 step("anomalies of the custom result index detector", "POST", AD + "/detectors/results/_search/" + CUSTOM, {"size": 8, "query": {"bool": {"filter": [{"term": {"detector_id": "{c}"}}, {"range": {"anomaly_grade": {"gt": 0}}}]}}, "sort": [{"data_start_time": "asc"}]}),
 step("results of one entity", "POST", AD + "/detectors/results/_search/" + CUSTOM, {"size": 2, "query": {"bool": {"filter": [{"term": {"detector_id": "{c}"}}, {"nested": {"path": "entity", "query": {"bool": {"filter": [{"term": {"entity.name": "host"}}, {"term": {"entity.value": "h1"}}]}}}}]}}, "sort": [{"data_start_time": "asc"}]}),
 step("anomalies of the two field detector", "POST", AD + "/detectors/results/_search", {"size": 6, "query": {"bool": {"filter": [{"term": {"detector_id": "{h2}"}}, {"range": {"anomaly_grade": {"gt": 0}}}]}}, "sort": [{"data_start_time": "asc"}]}),
 step("results grouped by entity", "POST", AD + "/detectors/results/_search/" + CUSTOM, {"size": 0, "query": {"term": {"detector_id": "{c}"}}, "aggs": {"by_entity": {"nested": {"path": "entity"}, "aggs": {"names": {"terms": {"field": "entity.name"}, "aggs": {"values": {"terms": {"field": "entity.value"}}}}}}, "max_grade": {"max": {"field": "anomaly_grade"}}}}),
 step("results by task id", "POST", AD + "/detectors/results/_search", {"size": 0, "track_total_hits": True, "query": {"term": {"task_id": "{st}"}}}),
 step("results with the plugin's dashboards query", "POST", AD + "/detectors/results/_search", {"size": 0, "query": {"bool": {"filter": [{"term": {"detector_id": "{s}"}}, {"range": {"anomaly_grade": {"gt": 0}}}, {"range": {"data_start_time": {"gte": T0, "lte": END}}}]}}, "aggs": {"max_anomaly_grade": {"max": {"field": "anomaly_grade"}}}}),
 step("results search with only the custom result index", "POST", AD + "/detectors/results/_search/" + CUSTOM, {"size": 0, "track_total_hits": True, "query": {"match_all": {}}}, {"only_query_custom_result_index": "true"}),
 step("results search with only the custom index and none named", "POST", AD + "/detectors/results/_search", {"size": 0, "query": {"match_all": {}}}, {"only_query_custom_result_index": "true"}),
 step("results search by GET", "GET", AD + "/detectors/results/_search", {"size": 0, "track_total_hits": True, "query": {"term": {"detector_id": "{s}"}}}),
 step("results search with a bad query", "POST", AD + "/detectors/results/_search", {"query": {"nope": {}}}),
 step("results search with a bad sort", "POST", AD + "/detectors/results/_search", {"sort": [{"nope_field": "asc"}]}),
 step("results by detector", "GET", AD + "/detectors/{s}/results"),
 step("results by detector with a body", "POST", AD + "/detectors/{s}/results", {"size": 1}),
 step("results by detector with underscore", "GET", AD + "/detectors/{s}/_results"),
 step("results of a missing detector", "GET", AD + "/detectors/nope/results"),
 # top anomalies: the stock plugin's script reads _source, which a compound-file segment refuses in this build
 top("top anomalies of an unmerged result index", "h", TOP, {"historical": "true"}),
 top("top anomalies of the single entity detector", "s", TOP, {"historical": "true"}),
 top("top anomalies of a detector with a custom result index", "c", TOP, {"historical": "true"}),
 step("results index to plain segments", "PUT", "/.opendistro-anomaly-results*/_settings", {"index.compound_format": False}, {"expand_wildcards": "all"}),
 step("results index merged", "POST", "/.opendistro-anomaly-results*/_forcemerge", None, {"max_num_segments": "1", "expand_wildcards": "all"}),
 step("top anomalies by severity", "POST", AD + "/detectors/{h}/results/_topAnomalies", dict(TOP, order="severity"), {"historical": "true"}),
 top("top anomalies of a historical task", "h", dict(TOP, order="severity", task_id="{ht}"), {"historical": "true"}),
 top("top anomalies by occurrence", "h", dict(TOP, order="occurrence"), {"historical": "true"}),
 top("top anomalies with the default order", "h", TOP, {"historical": "true"}),
 top("top anomalies with size one", "h", dict(TOP, size=1), {"historical": "true"}),
 top("top anomalies with size ten", "h", dict(TOP, size=10), {"historical": "true"}),
 top("top anomalies with a small range", "h", dict(TOP, start_time_ms=T0 + 398 * MIN, end_time_ms=T0 + 404 * MIN), {"historical": "true"}),
 top("top anomalies with a range of no anomalies", "h", dict(TOP, start_time_ms=T0, end_time_ms=T0 + 100 * MIN), {"historical": "true"}),
 top("top anomalies with a range with no data", "h", dict(TOP, start_time_ms=T0 - 5000 * MIN, end_time_ms=T0 - 4000 * MIN), {"historical": "true"}),
 top("top anomalies of two category fields", "h2", dict(TOP, category_field=["host", "city"]), {"historical": "true"}),
 top("top anomalies of two category fields by one", "h2", dict(TOP, category_field=["city"]), {"historical": "true"}),
 top("top anomalies of two category fields by host", "h2", dict(TOP, category_field=["host"]), {"historical": "true"}),
 top("top anomalies by occurrence of two category fields", "h2", dict(TOP, category_field=["host", "city"], order="occurrence"), {"historical": "true"}),
 top("top anomalies with the task of another detector", "h2", dict(TOP, task_id="{ht}", category_field=["host", "city"]), {"historical": "true"}),
 top("top anomalies without a category field", "h", {"size": 3, "start_time_ms": T0, "end_time_ms": END}, {"historical": "true"}),
 top("top anomalies of a category field the detector lacks", "h", dict(TOP, category_field=["city"]), {"historical": "true"}),
 top("top anomalies of an unknown category field", "h", dict(TOP, category_field=["nope"]), {"historical": "true"}),
 top("top anomalies with an empty category field", "h", dict(TOP, category_field=[]), {"historical": "true"}),
 top("top anomalies without a start", "h", {"size": 3, "category_field": ["host"], "end_time_ms": END}, {"historical": "true"}),
 top("top anomalies without an end", "h", {"size": 3, "category_field": ["host"], "start_time_ms": T0}, {"historical": "true"}),
 top("top anomalies without a size", "h", {"category_field": ["host"], "start_time_ms": T0, "end_time_ms": END}, {"historical": "true"}),
 top("top anomalies with an end before the start", "h", dict(TOP, start_time_ms=END, end_time_ms=T0), {"historical": "true"}),
 top("top anomalies with size zero", "h", dict(TOP, size=0), {"historical": "true"}),
 top("top anomalies with a negative size", "h", dict(TOP, size=-1), {"historical": "true"}),
 top("top anomalies with a size over the limit", "h", dict(TOP, size=10001), {"historical": "true"}),
 top("top anomalies with a bad order", "h", dict(TOP, order="nope"), {"historical": "true"}),
 top("top anomalies with a string size", "h", dict(TOP, size="three"), {"historical": "true"}),
 top("top anomalies with a string time", "h", dict(TOP, start_time_ms="yesterday"), {"historical": "true"}),
 top("top anomalies with an unknown key", "h", dict(TOP, nope=1), {"historical": "true"}),
 top("top anomalies with a bad body", "h", None, {"historical": "true"}, raw="{"),
 top("top anomalies with no body", "h", None, {"historical": "true"}),
 top("top anomalies with an empty object", "h", {}, {"historical": "true"}),
 top("top anomalies with a task id of nothing", "h", dict(TOP, task_id="nope"), {"historical": "true"}),
 top("top anomalies with a bad historical value", "h", TOP, {"historical": "maybe"}),
 top("top anomalies without a historical task", "h", TOP, {"historical": "false"}),
 step("top anomalies of a missing detector id", "POST", AD + "/detectors/nope/results/_topAnomalies", TOP, {"historical": "true"}),
 step("top anomalies by GET", "GET", AD + "/detectors/{h}/results/_topAnomalies", TOP, {"historical": "true"}),
 step("top anomalies with the historical flag in the path", "POST", AD + "/detectors/{h}/results/_topAnomalies/true", TOP),
 step("top anomalies of an unrelated route", "POST", AD + "/detectors/results/_topAnomalies", TOP),

 # real time: the detector job runs every minute over a source that ends now
 step("create real time detector", "POST", AD + "/detectors", det("compat-rl", indices=[LIVE], category_field=["host"], window_delay={"period": {"interval": 1, "unit": "Minutes"}}), capture={"l": "/_id"}),
 step("start real time detector", "POST", AD + "/detectors/{l}/_start"),
 step("profile total entities of a running detector", "GET", AD + "/detectors/{l}/_profile/total_entities", poll=("/total_entities", 2, 300)),
 step("top anomalies of a real time detector", "POST", AD + "/detectors/{l}/results/_topAnomalies", {"size": 3, "category_field": ["host"], "start_time_ms": 0, "end_time_ms": 4102444800000}),
 step("top anomalies of a real time detector by occurrence", "POST", AD + "/detectors/{l}/results/_topAnomalies", {"size": 3, "category_field": ["host"], "start_time_ms": 0, "end_time_ms": 4102444800000, "order": "occurrence"}),
 step("top anomalies of a real time detector as historical", "POST", AD + "/detectors/{l}/results/_topAnomalies", {"size": 3, "category_field": ["host"], "start_time_ms": 0, "end_time_ms": 4102444800000}, {"historical": "true"}),
 step("stop real time detector", "POST", AD + "/detectors/{l}/_stop"),
 step("delete real time detector", "DELETE", AD + "/detectors/{l}"),
 # stats and counts after the runs
 step("stats by name after the runs", "GET", AD + "/stats/detector_count,hc_detector_count,single_entity_detector_count,anomaly_detectors_index_status"),
 step("detector count after the runs", "GET", AD + "/detectors/count"),
 step("search detectors after the runs", "POST", AD + "/detectors/_search", {"size": 10, "query": {"match_all": {}}, "sort": [{"name.keyword": "asc"}], "_source": ["name", "detector_type", "category_field", "result_index"]}),
 # deleting
 step("delete detector with results", "DELETE", AD + "/detectors/{s}"),
 step("results of a deleted detector remain", "POST", AD + "/detectors/results/_search", {"size": 0, "track_total_hits": True, "query": {"term": {"detector_id": "{s}"}}}),
 step("tasks of a deleted detector remain", "POST", AD + "/detectors/tasks/_search", {"size": 0, "track_total_hits": True, "query": {"term": {"detector_id": "{s}"}}}),
 step("top anomalies of a deleted detector", "POST", AD + "/detectors/{s}/results/_topAnomalies", TOP, {"historical": "true"}),
 step("delete results by query", "DELETE", AD + "/detectors/results", {"query": {"term": {"detector_id": "{h2}"}}}),
 step("count results after the delete", "POST", AD + "/detectors/results/_search", {"size": 0, "track_total_hits": True, "query": {"term": {"detector_id": "{h2}"}}}, poll=("/hits/total/value", 0, 20)),
 step("delete one field detector", "DELETE", AD + "/detectors/{h}"),
 step("delete two field detector", "DELETE", AD + "/detectors/{h2}"),
 step("delete detector without results", "DELETE", AD + "/detectors/{e}"),
]
VOL = ["took", "_seq_no", "_primary_term", "last_update_time", "feature_id", "enabled_time", "disabled_time", "execution_start_time", "execution_end_time", "task_progress", "init_progress", "estimated_minutes_left", "started_by", "user", "coordinating_node", "worker_node", "task_id", "model_id", "node_id", "detector_task_id", "_score", "last_updated_time", "id", "ad_models_memory_usage", "ad_executing_batch_task_count", "ad_total_batch_task_execution_count", "ad_canceled_batch_task_count", "ad_batch_task_failure_count", "model_size_in_bytes", "profile_time", "ad_execute_request_count", "ad_execute_failure_count", "ad_hc_execute_request_count", "ad_hc_execute_failure_count", "total_size_in_bytes", "last_active_timestamp", "last_checkpoint_time", "model_size", "current_piece", "start_time", "_id", "anomaly_score", "threshold", "anomaly_grade", "data", "model_count"]
STATE = [".opendistro-anomaly-detectors", ".opendistro-anomaly-detector-jobs", ".opendistro-anomaly-detection-state", ".opendistro-anomaly-checkpoints"]
cl = [("DELETE", "/" + SRC, None, {"ignore_unavailable": "true"}),
      ("DELETE", "/" + LIVE, None, {"ignore_unavailable": "true"}),
      ("DELETE", "/.opendistro-anomaly-results*", None, {"ignore_unavailable": "true", "expand_wildcards": "all"}),
      ("DELETE", "/" + CUSTOM + "*", None, {"ignore_unavailable": "true", "expand_wildcards": "all"})] + \
     [("POST", "/" + i + "/_delete_by_query", {"query": {"match_all": {}}}, {"refresh": "true", "ignore_unavailable": "true"}) for i in STATE]
run(sys.argv[1:], "anomaly detection results and top anomalies",
    "The stock Anomaly Detection plugin after historical analysis has written result documents: result search, per-detector result routes, `_topAnomalies` (orders, sizes, category fields, errors), profile of every type, tasks, preview and validate of category detectors, stats and delete. Generated ids are placeholders.",
    steps, volatile=VOL, cleanup=cl, patterns=[(r"\d{4}\.\d{2}\.\d{2}", "<date>")],
    setup=[("PUT", "/" + SRC, {"settings": {"number_of_shards": 1, "number_of_replicas": 0}, "mappings": {"properties": {"ts": {"type": "date"}, "v": {"type": "double"}, "host": {"type": "keyword"}, "city": {"type": "keyword"}}}}),
           ("POST", "/" + SRC + "/_bulk", None, {"refresh": "true"}, nd),
           ("PUT", "/" + LIVE, {"settings": {"number_of_shards": 1, "number_of_replicas": 0}, "mappings": {"properties": {"ts": {"type": "date"}, "v": {"type": "double"}, "host": {"type": "keyword"}}}}),
           ("POST", "/" + LIVE + "/_bulk", None, {"refresh": "true"}, live)],
    setup_note=f"index {SRC}: {N} minutes from epoch ms {T0}, two documents a minute (host h0 and h1, city c0..c2 rotating), v a sine around 10, host h0 spiking by 30 at minutes 400 to 403 and host h1 at minute 500")
