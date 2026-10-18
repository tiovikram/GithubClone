# Chaos parity — forgejo-1 (forgejo, 1 replica(s)) — 20261018-031145

- target `http://localhost:8088`; 6 kills every 15s per phase; k6 read mix 10 VUs + git-fetch 4 VUs, 10 s timeout
- image: `sha256:4d4bd83c10b2fd04b982adc141eb65deaf5875ca33471016ecc3a7fe58566373`

## Phase A1-graceful — `kubectl delete pod ` every 15s

| kill | pod | all 1 ready again after |
|---|---|---|
| 1 | forgejo-7c5794f6dd-9sthh | 9s |
| 2 | forgejo-7c5794f6dd-s2km4 | 8s |
| 3 | forgejo-7c5794f6dd-98mdd | 9s |
| 4 | forgejo-7c5794f6dd-5vckk | 9s |
| 5 | forgejo-7c5794f6dd-jd6z6 | 9s |
| 6 | forgejo-7c5794f6dd-dvljn | 10s |

| k6 script | requests | failed | error rate | p50 ms | p95 ms | p99 ms |
|---|---|---|---|---|---|---|
| api-read | 39327 | 30540 | 77.657% | 1.3 | 112.8 | 401.3 |
| git-fetch | 25795 | 11762 | 45.598% | 8.7 | 47.3 | 72.3 |

- overall HTTP error rate: **64.958%** (42302 / 65122)
- availability probe (GET /users/bench every 0.5 s): longest 4.6s, total 14.5s of 29 probes failed
- git integrity loop: 121 iterations, pushes ok/failed **70/51**, clones ok/failed **72/49**, fsck failures **0**, acked commits missing **0**

## Phase A2-force — `kubectl delete pod --grace-period=0 --force` every 15s

| kill | pod | all 1 ready again after |
|---|---|---|
| 1 | forgejo-7c5794f6dd-fpqgd | 5s |
| 2 | forgejo-7c5794f6dd-ztnhk | 5s |
| 3 | forgejo-7c5794f6dd-zvfj5 | 5s |
| 4 | forgejo-7c5794f6dd-ppmv7 | 5s |
| 5 | forgejo-7c5794f6dd-9xc5f | 5s |
| 6 | forgejo-7c5794f6dd-9cnld | 4s |

| k6 script | requests | failed | error rate | p50 ms | p95 ms | p99 ms |
|---|---|---|---|---|---|---|
| api-read | 8903 | 315 | 3.538% | 53.7 | 388.5 | 2000.7 |
| git-fetch | 14493 | 226 | 1.559% | 18.1 | 52.3 | 73.6 |

- overall HTTP error rate: **2.312%** (541 / 23396)
- availability probe (GET /users/bench every 0.5 s): longest 0.5s, total 3.0s of 6 probes failed
- git integrity loop: 79 iterations, pushes ok/failed **73/6**, clones ok/failed **75/4**, fsck failures **0**, acked commits missing **0**

Container restarts at the end: 0 
