import sys, os, json, random
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from lib import *
K = "/_plugins/_knn"
T = "compat-knntrain"
random.seed(7)
vecs = [[round(random.uniform(-1, 1), 3) for _ in range(3)] for _ in range(300)]
nd = "".join(json.dumps(x) + "\n" for i, v in enumerate(vecs) for x in ({"index": {"_id": str(i)}}, {"v": v}))
def train(mid, method, **kw):
    b = {"training_index": T, "training_field": "v", "dimension": 3, "method": method}; b.update(kw); return b
ivf = {"name": "ivf", "engine": "faiss", "space_type": "l2", "parameters": {"nlist": 4, "nprobes": 2}}
steps = [
 step("train ivf", "POST", K + "/models/compat-m1/_train", train("m1", ivf, description="first")),
 step("train answers while training", "GET", K + "/models/compat-m1", poll=("/state", "created", 120)),
 step("get model", "GET", K + "/models/compat-m1"),
 step("get model with filter_path", "GET", K + "/models/compat-m1", query={"filter_path": "model_id,state,dimension"}),
 step("get model head", "HEAD", K + "/models/compat-m1"),
 step("train same id again", "POST", K + "/models/compat-m1/_train", train("m1", ivf)),
 step("train with generated id", "POST", K + "/models/_train", train("x", ivf), capture={"gen": "/model_id"}),
 step("get generated model", "GET", K + "/models/{gen}", poll=("/state", "created", 120)),
 step("delete generated model", "DELETE", K + "/models/{gen}"),
 step("train hnsw pq", "POST", K + "/models/compat-m2/_train", train("m2", {"name": "hnsw", "engine": "faiss", "space_type": "l2", "parameters": {"m": 8, "ef_construction": 64, "encoder": {"name": "pq", "parameters": {"m": 3, "code_size": 8}}}})),
 step("hnsw pq model", "GET", K + "/models/compat-m2", poll=("/state", "created", 120)),
 step("train ivf pq", "POST", K + "/models/compat-m3/_train", train("m3", {"name": "ivf", "engine": "faiss", "space_type": "innerproduct", "parameters": {"nlist": 2, "encoder": {"name": "pq", "parameters": {"m": 3, "code_size": 8}}}})),
 step("ivf pq model", "GET", K + "/models/compat-m3", poll=("/state", "created", 120)),
 step("train too few vectors for pq", "POST", K + "/models/compat-m4/_train", train("m4", ivf, max_training_vector_count=2)),
 step("train limited model", "GET", K + "/models/compat-m4", poll=("/state", "failed", 60)),
 step("train missing index", "POST", K + "/models/compat-m5/_train", {"training_index": "compat-nope", "training_field": "v", "dimension": 3, "method": ivf}),
 step("train missing field", "POST", K + "/models/compat-m5/_train", {"training_index": T, "training_field": "nope", "dimension": 3, "method": ivf}),
 step("train wrong dimension", "POST", K + "/models/compat-m5/_train", {"training_index": T, "training_field": "v", "dimension": 5, "method": ivf}),
 step("train without method", "POST", K + "/models/compat-m5/_train", {"training_index": T, "training_field": "v", "dimension": 3}),
 step("train method not needing training", "POST", K + "/models/compat-m5/_train", train("m5", {"name": "hnsw", "engine": "faiss", "space_type": "l2"})),
 step("train lucene", "POST", K + "/models/compat-m5/_train", train("m5", {"name": "hnsw", "engine": "lucene", "space_type": "l2"})),
 step("train unknown parameter", "POST", K + "/models/compat-m5/_train", train("m5", {"name": "ivf", "engine": "faiss", "space_type": "l2", "parameters": {"nope": 1}})),
 step("train nlist zero", "POST", K + "/models/compat-m5/_train", train("m5", {"name": "ivf", "engine": "faiss", "space_type": "l2", "parameters": {"nlist": 0}})),
 step("train missing training_index", "POST", K + "/models/compat-m5/_train", {"training_field": "v", "dimension": 3, "method": ivf}),
 step("train missing dimension", "POST", K + "/models/compat-m5/_train", {"training_index": T, "training_field": "v", "method": ivf}),
 step("train bad id", "POST", K + "/models/Compat%20Bad/_train", train("m", ivf)),
 step("bad id model state", "GET", K + "/models/Compat%20Bad", poll=("/state", "created", 120)),
 step("delete bad id model", "DELETE", K + "/models/Compat%20Bad"),
 step("train with description too long", "POST", K + "/models/compat-m6/_train", train("m6", ivf, description="d" * 400)),
 step("description model state", "GET", K + "/models/compat-m6", poll=("/state", "created", 120)),
 step("train with search_size", "POST", K + "/models/compat-m7/_train", train("m7", ivf, search_size=50, max_training_vector_count=100)),
 step("search_size model state", "GET", K + "/models/compat-m7", poll=("/state", "created", 120)),
 step("train with space_type in body", "POST", K + "/models/compat-m8/_train", train("m8", ivf, space_type="l2")),
 step("space_type model state", "GET", K + "/models/compat-m8", poll=("/state", "created", 120)),
 step("get missing model", "GET", K + "/models/compat-nope"),
 step("search models", "POST", K + "/models/_search", {"query": {"match_all": {}}, "sort": [{"model_id": "asc"}]}, query={"_source_excludes": "model_blob"}),
 step("search models by state", "POST", K + "/models/_search", {"query": {"term": {"state": "created"}}, "sort": [{"model_id": "asc"}], "_source": {"excludes": ["model_blob"]}}),
 step("create index from model", "PUT", "/compat-knnmodel", {"settings": {"index.knn": True, "number_of_shards": 1, "number_of_replicas": 0}, "mappings": {"properties": {"v": {"type": "knn_vector", "model_id": "compat-m1"}}}}),
 step("mapping from model", "GET", "/compat-knnmodel/_mapping"),
 step("index into model index", "POST", "/compat-knnmodel/_bulk", raw="".join(json.dumps(x) + "\n" for i, v in enumerate(vecs[:20]) for x in ({"index": {"_id": str(i)}}, {"v": v})), query={"refresh": "true"}, ctype="application/x-ndjson"),
 step("search model index", "POST", "/compat-knnmodel/_search", {"size": 3, "query": {"knn": {"v": {"vector": vecs[0], "k": 3}}}, "_source": False}),
 step("search model index with nprobes", "POST", "/compat-knnmodel/_search", {"size": 3, "query": {"knn": {"v": {"vector": vecs[0], "k": 3, "method_parameters": {"nprobes": 4}}}}, "_source": False}),
 step("search model index with ef_search on ivf", "POST", "/compat-knnmodel/_search", {"size": 3, "query": {"knn": {"v": {"vector": vecs[0], "k": 3, "method_parameters": {"ef_search": 4}}}}, "_source": False}),
 step("model of a wrong dimension", "PUT", "/compat-knnmodel2", {"settings": {"index.knn": True}, "mappings": {"properties": {"v": {"type": "knn_vector", "model_id": "compat-nope"}}}}),
 step("delete model in use", "DELETE", K + "/models/compat-m1"),
 step("delete index using the model", "DELETE", "/compat-knnmodel"),
 step("delete model", "DELETE", K + "/models/compat-m1"),
 step("delete model again", "DELETE", K + "/models/compat-m1"),
 step("get deleted model", "GET", K + "/models/compat-m1"),
 step("delete the others", "DELETE", K + "/models/compat-m2"),
 step("delete m3", "DELETE", K + "/models/compat-m3"),
 step("delete m4", "DELETE", K + "/models/compat-m4"),
 step("delete m6", "DELETE", K + "/models/compat-m6"),
 step("delete m7", "DELETE", K + "/models/compat-m7"),
 step("delete m8", "DELETE", K + "/models/compat-m8"),
 step("stats with models", "GET", K + "/stats/training_requests,training_errors,model_index_status"),
]
VOL = ["index_uuid", "model_blob", "timestamp", "took", "_seq_no", "_primary_term", "training_index_size_in_kb", "last_updated_time", "node_id", "training_node_assignment", "training_requests", "training_errors", "training_memory_usage", "training_memory_usage_percentage", "indexing_from_model_degraded", "graph_memory_usage"]
cl = [("DELETE", "/compat-knnmodel*", None, {"ignore_unavailable": "true"})] + [("DELETE", K + "/models/compat-m%d" % i) for i in range(1, 9)] + [("DELETE", K + "/models/Compat%20Bad")]
run(sys.argv[1:], "k-NN model training and models", "Training faiss models (_train), the model routes and an index mapped from a model, as the stock plugin 3.10.0 answers them. Training is asynchronous; a poll step keeps the first answer that shows the wanted state.",
    steps, volatile=VOL, cleanup=cl,
    setup=[("DELETE", "/" + T, None, {"ignore_unavailable": "true"}), ("PUT", "/" + T, {"settings": {"index.knn": True, "number_of_shards": 1, "number_of_replicas": 0}, "mappings": {"properties": {"v": {"type": "knn_vector", "dimension": 3}}}}),
           ("POST", "/" + T + "/_bulk", None, {"refresh": "true"}, nd)],
    setup_note=f"index {T}: 300 documents with a 3-dimensional vector v from a fixed seed")
