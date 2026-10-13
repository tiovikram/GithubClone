# Chaos parity — forge-3 (forge, 3 replica(s)) — 20261012-220039

- target `http://localhost:8088`; 6 kills every 15s per phase; k6 read mix 10 VUs + git-fetch 4 VUs, 10 s timeout
- image: `sha256:f4569c39ced7f1e6816119bd008ae6f80e42e63d2b59cdccb8ea8eed7775ac84`

## Phase A1-graceful — `kubectl delete pod ` every 15s

| kill | pod | all 3 ready again after |
|---|---|---|
| 1 | forge-5456bf96d8-lpxcv | 3s |
| 2 | forge-5456bf96d8-rz5b2 | 6s |
| 3 | forge-5456bf96d8-mt6bh | 3s |
| 4 | forge-5456bf96d8-jd5ch | 5s |
| 5 | forge-5456bf96d8-r5lsm | 4s |
| 6 | forge-5456bf96d8-bgpvm | 4s |

| k6 script | requests | failed | error rate | p50 ms | p95 ms | p99 ms |
|---|---|---|---|---|---|---|
| api-read | 124230 | 0 | 0% | 7.7 | 20.6 | 29.9 |
| git-fetch | 40308 | 0 | 0% | 7.1 | 20.1 | 29.8 |

- overall HTTP error rate: **0%** (0 / 164538)
- availability probe (GET /users/bench every 0.5 s): longest 0.0s, total 0.0s of 0 probes failed
- git integrity loop: 95 iterations, pushes ok/failed **95/0**, clones ok/failed **95/0**, fsck failures **0**, acked commits missing **0**

## Phase A2-force — `kubectl delete pod --grace-period=0 --force` every 15s

| kill | pod | all 3 ready again after |
|---|---|---|
| 1 | forge-5456bf96d8-czfjv | 4s |
| 2 | forge-5456bf96d8-4p8vs | 3s |
| 3 | forge-5456bf96d8-4t8f5 | 3s |
| 4 | forge-5456bf96d8-jvrgf | 6s |
| 5 | forge-5456bf96d8-qsf7l | 4s |
| 6 | forge-5456bf96d8-94jz9 | 5s |

| k6 script | requests | failed | error rate | p50 ms | p95 ms | p99 ms |
|---|---|---|---|---|---|---|
| api-read | 104280 | 0 | 0% | 8.9 | 25.4 | 36.2 |
| git-fetch | 46979 | 2 | 0.004% | 6 | 17 | 27.3 |

- overall HTTP error rate: **.001%** (2 / 151259)
- availability probe (GET /users/bench every 0.5 s): longest 0.0s, total 0.0s of 0 probes failed
- git integrity loop: 96 iterations, pushes ok/failed **96/0**, clones ok/failed **96/0**, fsck failures **0**, acked commits missing **0**

Container restarts at the end: 0 0 0 
