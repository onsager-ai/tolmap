# Evaluation corpus summary

Each cell is the per-repository median [p10–p90]. Failed repositories are counted but excluded from metric distributions.

The px²/file *min* columns are dominated by a geometric artifact, not zoom: a district with too few member files (empirically almost always <=12) never clears the >=12-point marching-squares contour filter in src/blobs.rs and gets no polygon at any zoom, which floors that repo's minimum at 0.0 regardless of everything else in the map (see docs/FINDINGS.md finding 18). The *share of files under floor* rows are the robust statistic: the fraction of all mapped files sitting in a mainland/island district whose px²/file is below the 30 px²/file floor (#48) at fit zoom, including these zero-blob districts (their file-dot budget is genuinely 0, which is what "under floor" means for them too).

| metric | small | medium | large | ultra |
|---|---:|---:|---:|---:|
| repositories (built/failed) | 49/0 | 40/1 | 28/2 | 11/1 |
| files | 51 [19–224] | 704 [353–1,517] | 4,050 [2,145–6,799] | 11,991 [8,757–39,964] |
| districts | 5 [3–8] | 16 [11–50] | 52 [21–364] | 330 [87–3,411] |
| mainland | 5 [3–8] | 13 [9–22] | 18 [9–25] | 16 [3–27] |
| islands | 0 [0–0] | 1 [0–5] | 16 [2–230] | 309 [45–1,867] |
| unconnected | 0 [0–0] | 1 [0–11] | 7 [0–63] | 13 [0–335] |
| landmarks | 10 [7–13] | 21 [15–55] | 56 [27–371] | 338 [90–2,548] |
| modularity q | 0.254 [-0.003–0.454] | 0.563 [0.439–0.706] | 0.716 [0.511–0.865] | 0.823 [0.692–0.987] |
| kept edges | 133 [21–1,234] | 4,986 [512–27,090] | 21,976 [5,428–168,774] | 41,758 [28,709–160,031] |
| zero-edge files | 1 [0–18] | 36 [2–462] | 142 [15–1,264] | 338 [6–25,620] |
| 390×700 px²/file min | 127.4 [37.4–250.8] | 33.3 [19.9–58.1] | 6.7 [0.0–21.7] | 0.0 [0.0–0.0] |
| 390×700 px²/file district median | 189.5 [123.8–286.7] | 60.2 [28.9–99.0] | 12.2 [5.9–28.4] | 2.0 [0.0–5.5] |
| 1440×900 px²/file min | 904.2 [251.6–1435.6] | 223.8 [141.4–395.9] | 51.1 [0.0–101.3] | 0.0 [0.0–0.0] |
| 1440×900 px²/file district median | 1338.7 [858.5–1820.7] | 356.0 [207.0–748.8] | 91.4 [40.3–162.2] | 16.8 [0.0–38.9] |
| 390×700 share of files under floor | 0.0% [0.0%–0.0%] | 0.0% [0.0%–64.3%] | 99.6% [30.9%–99.9%] | 99.8% [60.9%–100.0%] |
| 1440×900 share of files under floor | 0.0% [0.0%–0.0%] | 0.0% [0.0%–0.0%] | 0.0% [0.0%–4.9%] | 42.0% [2.6%–97.0%] |
| build seconds | 0.3 [0.2–0.9] | 2.9 [1.5–8.2] | 19.5 [8.4–52.9] | 81.6 [42.1–380.9] |
| peak RSS MB | 20.8 [17.5–34.1] | 114.2 [42.9–254.5] | 519.8 [213.3–1266.9] | 1786.7 [838.9–5305.8] |
