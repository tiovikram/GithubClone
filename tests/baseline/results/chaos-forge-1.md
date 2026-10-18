# Chaos parity — forge-1 (forge, 1 replica(s)) — 20261018-015633

- target `http://localhost:8088`; 6 kills every 15s per phase; k6 read mix 10 VUs + git-fetch 4 VUs, 10 s timeout
- image: `sha256:e4ceea76db8982f014649f39ba8f30bae252f622f43dad2af11bba063c81cb0c`

## Phase A1-graceful — `kubectl delete pod ` every 15s

| kill | pod | all 1 ready again after |
|---|---|---|
| 1 | forge-7d8c59b98c-b6tb8 | 4s |
| 2 | forge-7d8c59b98c-z57mj | 4s |
| 3 | forge-7d8c59b98c-9749b | 4s |
| 4 | forge-7d8c59b98c-mzwbd | 4s |
| 5 | forge-7d8c59b98c-k6dvs | 3s |
| 6 | forge-7d8c59b98c-bjfbh | 4s |

| k6 script | requests | failed | error rate | p50 ms | p95 ms | p99 ms |
|---|---|---|---|---|---|---|
| api-read | 77942 | 0 | 0% | 9.8 | 43.5 | 57.7 |
| git-fetch | 31270 | 2 | 0.006% | 7.1 | 39.3 | 54.2 |

- overall HTTP error rate: **.001%** (2 / 109212)
- availability probe (GET /users/bench every 0.5 s): longest 0.0s, total 0.0s of 0 probes failed
- git integrity loop: 96 iterations, pushes ok/failed **96/0**, clones ok/failed **96/0**, fsck failures **0**, acked commits missing **0**

## Phase A2-force — `kubectl delete pod --grace-period=0 --force` every 15s

| kill | pod | all 1 ready again after |
|---|---|---|
| 1 | forge-7d8c59b98c-mscc9 | 4s |
| 2 | forge-7d8c59b98c-vsnz8 | 5s |
| 3 | forge-7d8c59b98c-lhzgw | 3s |
| 4 | forge-7d8c59b98c-fjw8n | 4s |
| 5 | forge-7d8c59b98c-9pqbb | 4s |
| 6 | forge-7d8c59b98c-6fvns | 4s |

| k6 script | requests | failed | error rate | p50 ms | p95 ms | p99 ms |
|---|---|---|---|---|---|---|
| api-read | 61755 | 454 | 0.735% | 9.9 | 46.2 | 64.3 |
| git-fetch | 26018 | 231 | 0.888% | 7 | 40.5 | 57 |

- overall HTTP error rate: **.780%** (685 / 87773)
- availability probe (GET /users/bench every 0.5 s): longest 3.1s, total 5.5s of 11 probes failed
- git integrity loop: 89 iterations, pushes ok/failed **82/7**, clones ok/failed **85/4**, fsck failures **0**, acked commits missing **0**

Container restarts at the end: 0 
