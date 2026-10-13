# Chaos parity — forge-1 (forge, 1 replica(s)) — 20261012-210431

- target `http://localhost:8088`; 6 kills every 15s per phase; k6 read mix 10 VUs + git-fetch 4 VUs, 10 s timeout
- image: `sha256:f4569c39ced7f1e6816119bd008ae6f80e42e63d2b59cdccb8ea8eed7775ac84`

## Phase A1-graceful — `kubectl delete pod ` every 15s

| kill | pod | all 1 ready again after |
|---|---|---|
| 1 | forge-5456bf96d8-mwj47 | 4s |
| 2 | forge-5456bf96d8-7pk96 | 3s |
| 3 | forge-5456bf96d8-z4qdp | 4s |
| 4 | forge-5456bf96d8-4ffp4 | 4s |
| 5 | forge-5456bf96d8-wbzhm | 3s |
| 6 | forge-5456bf96d8-pb9bv | 3s |

| k6 script | requests | failed | error rate | p50 ms | p95 ms | p99 ms |
|---|---|---|---|---|---|---|
| api-read | 83036 | 0 | 0% | 9.4 | 39.2 | 51.5 |
| git-fetch | 31768 | 2 | 0.006% | 7 | 35.8 | 48.4 |

- overall HTTP error rate: **.001%** (2 / 114804)
- availability probe (GET /users/bench every 0.5 s): longest 0.0s, total 0.0s of 0 probes failed
- git integrity loop: 100 iterations, pushes ok/failed **100/0**, clones ok/failed **100/0**, fsck failures **0**, acked commits missing **0**

## Phase A2-force — `kubectl delete pod --grace-period=0 --force` every 15s

| kill | pod | all 1 ready again after |
|---|---|---|
| 1 | forge-5456bf96d8-qrlwl | 4s |
| 2 | forge-5456bf96d8-r22fg | 4s |
| 3 | forge-5456bf96d8-2dqbq | 4s |
| 4 | forge-5456bf96d8-5wr7v | 5s |
| 5 | forge-5456bf96d8-bc2sr | 4s |
| 6 | forge-5456bf96d8-6b2vk | 3s |

| k6 script | requests | failed | error rate | p50 ms | p95 ms | p99 ms |
|---|---|---|---|---|---|---|
| api-read | 67007 | 661 | 0.986% | 9.5 | 40.7 | 55.4 |
| git-fetch | 26645 | 287 | 1.077% | 7.1 | 35.8 | 49.3 |

- overall HTTP error rate: **1.012%** (948 / 93652)
- availability probe (GET /users/bench every 0.5 s): longest 3.0s, total 3.5s of 7 probes failed
- git integrity loop: 92 iterations, pushes ok/failed **87/5**, clones ok/failed **89/3**, fsck failures **0**, acked commits missing **0**

Container restarts at the end: 0 
