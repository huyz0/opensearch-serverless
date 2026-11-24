# A node says whether it is ready and can be drained

## Serves

Parity plan R12.2, the readiness and drain half: the shell's `SearchReadiness`, `ServerlessReadyHandler` and `ServerlessDrainHandler` (`SearchDrain` without its hand-over). Handing warm blocks to peers needs the hot-set copy (R12.1) and is not here.

## Change

- `oss-node` gains `lifecycle`: a `Readiness` that is *ready* from the moment the node listens and *draining* once told to. Draining is final for the process.
- `GET /_serverless/ready` answers 200 `{"ready":true,"state":"ready"}` while ready and 503 `{"ready":false,"state":"draining"}` once draining: the probe a Kubernetes ingress reads. It makes no object-store request.
- `POST /_serverless/drain` starts draining and answers once the lease has had time to say so: `{"drained":true,"took_millis":N}`. A second call answers at once with `{"drained":false,"already":true}`. It waits `serverless.search.drain.settle` (`OSS_DRAIN_SETTLE_MS`, default two thirds of the lease TTL, the shell's). The default is longer than the renewal interval (a third of the TTL), so a renewal has dropped the role by the time it answers; a setting below the renewal interval can answer before it has. It is meant for a `preStop` hook.
- A draining node's lease is written without the `search` role from its next renewal, so no coordinator ranks it as a reader or pushes it a tail; its `ingest` role and every shard it owns are untouched, and it still answers any request that reaches it.
- Nothing runs until `POST /_serverless/drain` is called, so nothing changes for a node that is never drained.
- Not here, listed: waiting for warm blocks before ready (`warm_on_join`) and handing them to peers on drain (R12.1), refusing a search this node coordinates from a local reader while draining, and draining on SIGTERM.

## Acceptance criteria

- AC-1: a node is ready after it starts: `GET /_serverless/ready` answers 200 with `ready` true.
- AC-2: after `POST /_serverless/drain` the same GET answers 503 with `state` `draining`, and stays so.
- AC-3: the drain answers `drained` true on the first call and `already` true on every later one, and the second call does not wait.
- AC-4: the lease a draining node writes names `ingest` and not `search`; the lease of a node never drained names both.
- AC-5: through a running node, after a drain a second node's membership sees the first without the `search` role, and the first still owns its shards and answers a write and a read.
- AC-6: the drain's wait is the setting's, and is bounded: the default is two thirds of the lease TTL up to a minute, and a setting above one minute is refused at start.
- AC-7: while a drain is waiting, `GET /_serverless/ready` already answers 503, and the drain's `took_millis` is at least the setting.

## S3 request budget

None added. The role goes out in the lease renewal the node already makes every `lease_ttl / 3`; the drain forces none.

## Switch

None needed: the endpoints are inert until called, and a drain is the operator's decision, not a default.
