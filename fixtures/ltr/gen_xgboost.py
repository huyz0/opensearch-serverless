"""Generates fixtures/ltr/xgboost-*-model.json: a model XGBoost saved, and xgboost-*-scores.txt:
the scores it gave on fixed vectors, one "v1 v2 ... -> score" a line (nan = missing). Run: python gen_xgboost.py (xgboost 2.1.4, numpy)."""
import json, math
import numpy as np
import xgboost as xgb

rng = np.random.RandomState(7)
X = rng.rand(200, 4).astype(np.float32)
X[rng.rand(*X.shape) < 0.1] = np.nan
y_reg = np.nan_to_num(X[:, 0]) * 3 + np.nan_to_num(X[:, 1]) - np.nan_to_num(X[:, 2]) * 2
y_bin = (y_reg > np.median(y_reg)).astype(np.float32)
probe = np.array([[0.1, 0.9, 0.3, 0.5], [0.8, 0.2, 0.7, 0.1], [np.nan, 0.5, 0.5, 0.5],
                  [0.5, np.nan, np.nan, 0.2], [np.nan] * 4, [0.0, 0.0, 0.0, 0.0],
                  [1.0, 1.0, 1.0, 1.0], [0.33, 0.66, 0.99, 0.12]], dtype=np.float32)
for objective, y in [("reg:squarederror", y_reg), ("reg:logistic", y_bin), ("binary:logistic", y_bin)]:
    params = {"objective": objective, "max_depth": 3, "eta": 0.3, "tree_method": "exact", "seed": 1, "nthread": 1}
    booster = xgb.train(params, xgb.DMatrix(X, label=y), num_boost_round=6)
    path = f"xgboost-{objective.replace(':', '-')}"
    booster.save_model(f"{path}-model.json")
    scores = [float(s) for s in booster.predict(xgb.DMatrix(probe))]
    with open(f"{path}-scores.txt", "w") as out:
        for row, score in zip(probe.tolist(), scores):
            out.write(" ".join("nan" if math.isnan(v) else repr(v) for v in row) + " -> " + repr(score) + "\n")
