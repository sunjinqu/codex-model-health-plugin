# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## What this is

A `trustedProcess` plugin (`local.model-health`) for [Codex Proxy RS](https://github.com/zyycn/codex-proxy-rs) that adds a management page for running **Meow** and **ModelTrace** behavioral-fingerprint checks against an existing host OpenAI account and client Key. README.md (Chinese) is the authoritative description of the product behavior and its invariants. User-facing strings and error messages are in Chinese, so keep that convention.

## Commands

The host SDK (`gateway-plugin-sdk`) and packaging CLI are pinned to codex-proxy-rs rev `af35cde3744b7afc81c4ad6e1e9c9c76bbbd68b8`. Don't move them to a floating `main`.

```sh
# Full release build (Docker buildx): frontend → per arch: engine → Rust tests+build with bundled-engine → package → smoke test
cargo install --git https://github.com/zyycn/codex-proxy-rs.git --rev af35cde3744b7afc81c4ad6e1e9c9c76bbbd68b8 \
  --locked --root .tools/cpr-cli codex-proxy-plugin-cli
CPR_PLUGIN_BIN="$PWD/.tools/cpr-cli/bin/cpr-plugin" ./scripts/build-linux.sh         # amd64 + arm64 → dist/*.tar.gz (+ .sha256)
CPR_PLUGIN_BIN="$PWD/.tools/cpr-cli/bin/cpr-plugin" ./scripts/build-linux.sh arm64   # one arch; per-arch dirs .build/{engine,linux-target}-<arch>
# Preferred for releases (no local disk use): .github/workflows/build.yml runs `build-linux.sh arm64` on ubuntu-24.04-arm.
# Pushing tag v<plugin.json version> publishes/overwrites that Release's assets; workflow_dispatch only uploads an artifact.

# Frontend (Node 24+, pnpm 10.10)
pnpm --dir frontend install --frozen-lockfile
pnpm --dir frontend test                          # node:test, transpiles TS sources directly with `typescript`
node --test frontend/tests/accountDisplay.test.mjs  # single test file
pnpm --dir frontend typecheck
pnpm --dir frontend build
pnpm --dir frontend dev                           # then open http://127.0.0.1:5173/?preview=1 (mock host, DEV only)
#   preview query params: theme=dark, scenario=empty|loading|error, updates=slow|error|ahead|behind|diverged

# Rust backend
cargo test --manifest-path backend/Cargo.toml --lib            # unit tests (domain.rs, management.rs); no engine needed
cargo test --manifest-path backend/Cargo.toml <test_name>      # protocol tests: see note below
# Without `bundled-engine`, backend/tests/protocol.rs runs the engine via
# `docker run ... meow-llm-detector:4.5.4 python /fixture/engine/worker.py`, so that local image must exist.
# With `--features bundled-engine` it needs MEOW_ENGINE_BINARY=<path to PyInstaller meow-engine> at compile time.

# Python engine tests (needs numpy/httpx/keyring from vendor/meow/requirements.txt)
python3 -m unittest discover -s engine/tests -p 'test_*.py'
python3 -m unittest discover -s engine/tests -p test_worker.py -k test_payload_preserves_upstream_probe

# Packaged-artifact smoke test against an offline SDK host fixture (add --updates-live to hit the public GitHub API)
python3 scripts/package-smoke.py /abs/path/plugin.tar.gz --continuous --modeltrace --updates

# Browser QA against the preview (env: PLAYWRIGHT_PATH, CHROME_BINARY, PREVIEW_URL; default http://127.0.0.1:4175/)
node scripts/browser-qa.mjs
node scripts/updates-browser-qa.mjs                # screenshots go to review/ (gitignored)
```

## Architecture

There are three processes/layers. Model traffic and credentials **never** leave the host.

1. **Rust plugin binary** (`backend/`). `main.rs` accepts an SDK session over stdio. `lib.rs::plugin()` builds from `plugin.json` (it is `include_bytes!`'d, so manifest edits affect the binary and tests) and registers one management page. `management.rs` holds the route table (`registration()`) and the `dispatch` match. A new route has to be added to both. A `PluginFault` is mapped to an HTTP status plus a `{error:{code,message,send_state}}` body, not a protocol error.
   - `host.rs` wraps host callbacks: `host.auth.list/get_runtime`, `host.keys.list`, `host.models.list`, `host.state.get/put` (CAS by version), `host.model.execute_stream/stream_read/stream_close`. Every model call explicitly sets `account_id`, `client_key_id`, `provider=openai`, `protocol=openai`.
   - `domain.rs` defines `Run` and its state machine (`ready → running → ready|paused|completed`, plus `stopped`), along with `next_job` (all first-pass jobs before retries, bounded by `plan.retry_budget`), `reserve`, `finish_attempt` and `recover`.
   - `updates.rs` runs the manual upstream GitHub commit compare via `host.http.*`. It reads bundled commits from `vendor/meow-source.json` and `vendor/modeltrace/SOURCE.json`, caches successes for 5 minutes, and never caches failures.
2. **Python engine** (`engine/worker.py` + `engine/modeltrace_adapter.py`). It is fully offline and has no network access. It is a long-lived subprocess speaking newline-delimited JSON on stdin/stdout (`{"op": catalog|prepare|payload|evaluate|score, ...}` → `{"ok":true,"data":...}`), with a 2 MiB per-line cap. `engine.rs` spawns it lazily, applies a 20s timeout, and kills/respawns it on any error. In release builds, `engine/Dockerfile` builds it with PyInstaller (onefile, with vendored sources and baselines). Its bytes are embedded into the Rust binary via `include_bytes!(env!("MEOW_ENGINE_BINARY"))` and extracted at startup into a 0700 dir under the host-provided cwd, with `TMPDIR` pointed there because `/tmp` may be `noexec`.
3. **Vue frontend** (`frontend/`). It uses `@codex-proxy/ui` and Tailwind 4. It talks to the plugin only through the host bridge `window.codexProxyPlugin` (version 2) → `api/*` routes (`src/api/index.ts`). `vite.config.ts` forces a single IIFE `app.js` + `app.css` with a `defer` script, because the host serves it in a sandboxed iframe. `src/preview.ts` is a mock host loaded only under `import.meta.env.DEV && ?preview=1`, and the production build strips it.

### Run lifecycle (`api/runs/step`)

1. Take the global execution semaphore (concurrency 1).
2. Lock state, re-validate the account (ID, enabled, platform, and frozen upstream identity; credential-revision bumps from token refresh are not treated as an account change), get the payload from the engine, `reserve` the job, and **persist a `pending` attempt before sending**.
3. Release the state lock during the network call.
4. Re-read the latest run, `finish_attempt`, re-score via the engine, and CAS-write.

A pending attempt found later is charged as `interrupted_unknown_result` on `recover`, so interruptions never become free retries. Failed samples pause the run; only an explicit `resume` continues it.

### Persistence

Runs live in the plugin-private `runs` state namespace, which has 16 slots (schema and limits are in `plugin.json`). The run ID is `"{slot}-{nanos}"` and the slot is parsed from it. A new run takes a free slot, or else replaces the oldest *finished* run. Unfinished runs are never overwritten. Each run's `plan` freezes method, baseline id/version/content hash and jobs, and the engine's `check_plan` rejects a plan whose hash no longer matches. Records without `plan.method` are legacy Meow runs and must keep working (`Method` defaults to `Meow`).

## Invariants to preserve

- `vendor/meow` and `vendor/modeltrace` are verbatim upstream copies pinned to the commits in `vendor/meow-source.json` / `vendor/modeltrace/SOURCE.json` (the latter has per-file sha256s). Don't edit them. Adaptation belongs in `engine/`. Upgrading upstream is a manual vendor-update + rebuild. The "check updates" button only reports, and never downloads anything.
- Keep detection methods and baselines frozen per run. Don't rescore history against new baselines, and don't mix Meow and ModelTrace scores.
- Keep account/Key binding explicit, budget counting conservative, and errored samples out of scoring.
- Changes should come with offline protocol tests (`backend/tests/protocol.rs` uses mocked host callbacks) and browser checks. Don't auto-deploy or trigger real, billed model detections.
- The SDK rev fixes the plugin wire protocol (`PROTOCOL_VERSION`, which is 4 at v3.29.6, while v3.18 to v3.28 hosts speak 2). A host on a different protocol rejects the install with "插件清单或协议版本不兼容，请使用当前 SDK 重新构建". When you move the rev, also update `plugin.json` `engines`, `PROTOCOL_VERSION` in `scripts/package-smoke.py` (it hand-encodes frames: business JSON goes in a `data_bytes` payload prefix), `CPR_REV` in the workflow, and the `rust:<ver>-slim-bookworm` image in `build-linux.sh` and the workflow to the SDK's `rust-version`.
- Release targets are Linux x86_64 and aarch64, glibc 2.36+ (built on bookworm images). This repo's GitHub Releases ship the aarch64 package; x86_64 releases live in the upstream R-lmmortal repo.
- Protocol tests run many plugin instances in one process, so `Engine::bundled()` must keep the per-process sequence in its directory name and `query()` must keep the ETXTBSY spawn retry. Without them, parallel `cargo test` flakes, especially on arm64.
- A version bump touches `plugin.json`, `backend/Cargo.toml` + `Cargo.lock`, `frontend/package.json`, `README.md`, `NOTICE`, and `scripts/package-smoke.py`.
