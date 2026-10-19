# Shared viewer host composition v1

The same production Home, indexing progress, map and CM6 source reader can be
served by a local service or an authenticated host. A host may insert a JSON
script with `id="tolmap-host"`, `type="application/json"` and this shape before
the module script:

```json
{"version":1,"private":true,"workspace":"Personal workspace","repositories":[{"value":"opaque-repository-id","label":"owner/repository"}],"actions":[{"label":"Connections and history","path":"/workspace"},{"label":"Sign out","path":"/logout","method":"post"}]}
```

Serialize it safely for HTML (escape `<` as `\u003c`). It selects authorized
repository options instead of the public repository input. POST `/api/index`
still returns the public `IndexResponse`; progress uses the generated
`JobSnapshot` contract and real worker stage events. Unknown counts and ETA stay
null. Completion may change the job's slug to an immutable snapshot route.

Private profiles disable bundled catalogue/map fallback and public issue links
that would include private repository or error context. No profile means the
existing OSS behavior. The profile supplies presentation, never authority:
the hosting server must validate sessions, grants, provider permission, immutable
revision, cache hits, streams, source and downloads independently.

The map remains full viewport, with the existing Home navigation and camera,
selection, source-reader and visibility-gate behavior. There is no second
reader or map implementation in the hosting application.
