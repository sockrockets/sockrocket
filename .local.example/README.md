# Local secrets

Copy to the repo root as `.local/` and fill in real values:

```bash
cp -R .local.example .local
```

`.local/` is listed in `.gitignore` and must never be committed.

| File | Used by |
|------|---------|
| `router.env` | `tools/ssh_askpass.sh`, `tools/router_login_test.sh` |
| `live.env` | ignored live probes under `crates/sockrocket-core/tests/` |
