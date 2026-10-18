# Chaos parity — forge-3 (forge, 3 replica(s)) — 20261018-023333

- target `http://localhost:8088`; 6 kills every 15s per phase; k6 read mix 10 VUs + git-fetch 4 VUs, 10 s timeout
- image: `sha256:e4ceea76db8982f014649f39ba8f30bae252f622f43dad2af11bba063c81cb0c`

## Phase A1-graceful — `kubectl delete pod ` every 15s

| kill | pod | all 3 ready again after |
|---|---|---|
| 1 | forge-7d8c59b98c-wtlk7 | 4s |
| 2 | forge-7d8c59b98c-z7wfw | 4s |
| 3 | forge-7d8c59b98c-xrldg | 5s |
| 4 | forge-7d8c59b98c-lb9tw | 4s |
| 5 | forge-7d8c59b98c-nh886 | 5s |
| 6 | forge-7d8c59b98c-jxchv | 5s |

| k6 script | requests | failed | error rate | p50 ms | p95 ms | p99 ms |
|---|---|---|---|---|---|---|
| api-read | 112374 | 0 | 0% | 8.8 | 20.6 | 27.9 |
| git-fetch | 43824 | 1 | 0.002% | 6.7 | 16.8 | 23.9 |

- overall HTTP error rate: **0%** (1 / 156198)
- availability probe (GET /users/bench every 0.5 s): longest 0.0s, total 0.0s of 0 probes failed
- git integrity loop: 90 iterations, pushes ok/failed **90/0**, clones ok/failed **90/0**, fsck failures **0**, acked commits missing **0**

## Phase A2-force — `kubectl delete pod --grace-period=0 --force` every 15s

| kill | pod | all 3 ready again after |
|---|---|---|
| 1 | forge-7d8c59b98c-62lkr | 3s |
| 2 | forge-7d8c59b98c-2xncg | 5s |
| 3 | forge-7d8c59b98c-c4trj | 4s |
| 4 | forge-7d8c59b98c-2tl9z | 3s |
| 5 | forge-7d8c59b98c-4ffzr | 5s |
| 6 | forge-7d8c59b98c-shbmw | 3s |

| k6 script | requests | failed | error rate | p50 ms | p95 ms | p99 ms |
|---|---|---|---|---|---|---|
| api-read | 106327 | 0 | 0% | 8.8 | 23.4 | 33.4 |
| git-fetch | 44505 | 0 | 0% | 6.5 | 17.3 | 25.9 |

- overall HTTP error rate: **0%** (0 / 150832)
- availability probe (GET /users/bench every 0.5 s): longest 0.0s, total 0.0s of 0 probes failed
- git integrity loop: 91 iterations, pushes ok/failed **91/0**, clones ok/failed **91/0**, fsck failures **0**, acked commits missing **0**

Container restarts at the end: 0 0 0 
