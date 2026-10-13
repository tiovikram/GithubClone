# Chaos parity — forgejo-1 (forgejo, 1 replica(s)) — 20261012-224530

- target `http://localhost:8088`; 6 kills every 15s per phase; k6 read mix 10 VUs + git-fetch 4 VUs, 10 s timeout
- image: `sha256:4d4bd83c10b2fd04b982adc141eb65deaf5875ca33471016ecc3a7fe58566373`

## Phase A1-graceful — `kubectl delete pod ` every 15s

| kill | pod | all 1 ready again after |
|---|---|---|
| 1 | forgejo-7c5794f6dd-jt7ql | 9s |
| 2 | forgejo-7c5794f6dd-psbzk | 10s |
| 3 | forgejo-7c5794f6dd-wmhl2 | 9s |
| 4 | forgejo-7c5794f6dd-48wrq | 10s |
| 5 | forgejo-7c5794f6dd-n252c | 10s |
| 6 | forgejo-7c5794f6dd-k2tgt | 9s |

| k6 script | requests | failed | error rate | p50 ms | p95 ms | p99 ms |
|---|---|---|---|---|---|---|
| api-read | 40604 | 32090 | 79.032% | 1.3 | 107.2 | 405.3 |
| git-fetch | 25441 | 11604 | 45.611% | 8.7 | 47.7 | 73.4 |

- overall HTTP error rate: **66.157%** (43694 / 66045)
- availability probe (GET /users/bench every 0.5 s): longest 4.1s, total 11.5s of 23 probes failed
- git integrity loop: 122 iterations, pushes ok/failed **72/50**, clones ok/failed **72/50**, fsck failures **0**, acked commits missing **0**

## Phase A2-force — `kubectl delete pod --grace-period=0 --force` every 15s

| kill | pod | all 1 ready again after |
|---|---|---|
| 1 | forgejo-7c5794f6dd-9rdbh | 5s |
| 2 | forgejo-7c5794f6dd-8jzj2 | 5s |
| 3 | forgejo-7c5794f6dd-skfw5 | 4s |
| 4 | forgejo-7c5794f6dd-r7rv9 | 5s |
| 5 | forgejo-7c5794f6dd-w4x6r | 5s |
| 6 | forgejo-7c5794f6dd-f9h9z | 5s |

| k6 script | requests | failed | error rate | p50 ms | p95 ms | p99 ms |
|---|---|---|---|---|---|---|
| api-read | 8868 | 238 | 2.684% | 54.1 | 377.4 | 2000.7 |
| git-fetch | 14437 | 198 | 1.371% | 18 | 51.2 | 71.3 |

- overall HTTP error rate: **1.870%** (436 / 23305)
- availability probe (GET /users/bench every 0.5 s): longest 3.0s, total 4.0s of 8 probes failed
- git integrity loop: 82 iterations, pushes ok/failed **76/6**, clones ok/failed **78/4**, fsck failures **0**, acked commits missing **0**

Container restarts at the end: 0 
