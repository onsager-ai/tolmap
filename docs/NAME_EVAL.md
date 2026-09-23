# District naming evaluation (#62)

`tolmap build --namer idf` is the default. `--namer model` opts in to Claude Haiku 4.5 through OpenRouter, model id [`anthropic/claude-haiku-4.5`](https://openrouter.ai/anthropic/claude-haiku-4.5). `--namer-model` or `TOLMAP_NAMER_MODEL` overrides the id. The service reads `TOLMAP_NAMER`, defaulting to `idf`; neither Fly nor Railway enables the model.

Model calls require both `OPENROUTER_API_KEY` and `TOLMAP_NAMER_BUDGET_USD`. The key is read only from the environment. There is one request per map for at most 60 cache-miss districts: mainland first by size, then islands of at least eight files. Unconnected districts and other misses use IDF. The request times out after 30 seconds. A complete cache makes no request and produces the same map bytes on every run. The service stores cache entries by repository and membership fingerprint in SQLite; the CLI uses `<out>/<name>.names.json`.

The Rust spend ledger records a conservative reservation **before** each request. It bounds input using UTF-8 request bytes plus 4096 framing tokens, caps output at 1024 tokens, and reserves twice the configured prices. A failed request keeps its reservation. It records response token usage and estimated cost separately. The default price estimates are the [OpenRouter list prices](https://openrouter.ai/anthropic/claude-haiku-4.5) of $1/M input and $5/M output; `TOLMAP_NAMER_INPUT_USD_PER_TOKEN` and `TOLMAP_NAMER_OUTPUT_USD_PER_TOKEN` can override them. An absent, invalid, or exhausted budget falls back to IDF. The ledger is a local file selected by `TOLMAP_NAMER_LEDGER` (or beside the name cache).

The remote evaluation runs all 20 repositories in **one sequential job**, with `TOLMAP_NAMER_BUDGET_USD=5` and one ledger file across all four builds per repository. That removes cross-job reservation races. For an absolute provider-side $5 ceiling even if prices change, use a dedicated [OpenRouter key with a $5 spending limit and no reset](https://openrouter.ai/docs/api/api-reference/api-keys/create-keys). Both limits apply to the whole evaluation; neither is a per-repository allowance. Do not reuse that key for another workload.

The selected pins come from `eval/corpus.toml`:

| Band | Repositories |
|---|---|
| Small | Textualize/rich, celery/celery, scrapy/scrapy, pallets/flask, fastapi/fastapi, gin-gonic/gin, honojs/hono, vuejs/core, pydantic/pydantic |
| Medium | django/django, crawlab-team/crawlab, date-fns/date-fns, etcd-io/etcd, hashicorp/terraform, helm/helm |
| Large | apache/airflow, angular/angular, getsentry/sentry, go-gitea/gitea |
| Ultra | n8n-io/n8n |

For each pin, `eval/name_eval.py` checks out the commit 300 back, builds IDF and model maps, then checks out the pin and warm-starts both modes from their respective earlier map. It reports matched-district rename share using Jaccard ≥ 0.35, name collisions, numbered names, side-by-side names, calls, token counts, estimated cost, and wall time per repository and band. The job fails if a model map has collisions or a higher rename share than IDF. The report, JSON, and spend ledger are uploaded as the `name-eval-results` artifact.

After this workflow is merged, the owner must create the $5-limited OpenRouter key, add it as the repository secret `OPENROUTER_API_KEY`, then manually dispatch **Remote build** with `command=name-eval` and `ref=main`. This PR only builds the harness; it does not dispatch it or enable model naming in hosting.
