# Blend/prune variants

`node-relative` is the default for `build`, `dump-blend`, the service, and
`polyglot-report`. The project owner approved the switch on 2026-09-23 after
issue #57's measurements showed that a single globally dominant edge can make
the old `absolute` floor discard nearly every other link. The local threshold
keeps the strongest structure around each endpoint while retaining the
existing top-14 cap and union semantics. `--prune-variant absolute` and the
other measured routes remain available for reproduction and comparison.

The owner chose “Switch anyway” after the pre-set retention rule failed on
sqlalchemy: warm-start retention moved from 0.9881 to 0.9643, a −0.0238 change
against a maximum allowed regression of 0.005. The full measurements and the
explicit override are recorded in finding 23. This is a measured default
choice with a known cost, rather than a claim that node-relative wins every
repository.

The calibration below used the nine acceptance repositories at the exact
commits and source roots in `data/fixtures.toml`. It ran on GitHub-hosted
standard runners in [Remote build 35733565035](https://github.com/onsager-ai/tolmap/actions/runs/35733565035), not on the development laptop. `dump-blend` records the sorted mass-normalised distribution before max-rescaling, so both constants are derived from the graph the partitioner actually receives.

| fixture | candidate edges | share below today's 0.02 floor | percentile rank of 0.02 (type 7) | pre-rescale maximum |
|---|---:|---:|---:|---:|
| django | 3,929 | 0% | 0% | 0.00181176425411413 |
| flask | 248 | 0% | 0% | 0.0284377393461295 |
| httpx | 210 | 0% | 0% | 0.0182977196378017 |
| vue | 1,895 | 0% | 0% | 0.00271543092129483 |
| celery | 2,466 | 0.0811030% | **0.0419289047681%** | 0.00406601705566762 |
| scrapy | 3,734 | 0.0535619% | 0.0470118032790% | 0.00229715111119034 |
| rich | 1,350 | 0.4444444% | 0.443812846609% | 0.00531922726877363 |
| prometheus | 6,456 | 1.1926890% | 1.19053950470% | 0.00163553469129302 |
| sqlalchemy | 7,257 | 82.3342979% | 82.3388989600% | 0.00879535914089830 |

`percentile` uses the median of the nine inverse empirical CDF positions for
0.02: `p = 0.0004192890476807616`. Quantiles use the deterministic type-7
definition, `h = p(n-1)` with linear interpolation between the adjacent
sorted weights. Celery is the median fixture by this measure, and evaluating
its type-7 quantile at `p` returns exactly 0.02. Ties remain kept because the
prune comparison is still `weight >= floor`.

`node-relative` retains the existing top-14 cap and uses 0.02 as a fraction,
not an absolute global number: an edge qualifies from a node when it is at
least 2% of that node's strongest incident edge. The final edge set is the
union of the qualifying per-node sets, matching the established prune union
semantics.

`pre-rescale` applies one absolute floor to the mass-normalised weights before
the global-maximum rescale. Today's median below-floor share is 2/3,734 =
0.0535619%. Because nine finite edge distributions form a step function, no
numeric floor produces that exact median: the two adjacent attainable medians
are 0.0405515% and 0.0811030%. Exhaustively evaluating the distributions'
breakpoints selects `0.00007752604982072655`, the nearer value (0.0405515%,
an absolute difference of 0.0130104 percentage points). This documents the
discrete nearest match instead of claiming unattainable exact equality.
