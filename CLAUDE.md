# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## What this is

A `trustedProcess` plugin (`local.model-health`) for [Codex Proxy RS](https://github.com/zyycn/codex-proxy-rs) that adds a management page for **candy-question degradation tests** ("降智测试"): each enabled host OpenAI account answers one fixed reasoning puzzle (correct answer `21`) through a chosen client Key, model and reasoning effort, and the page shows per-account accuracy. Up to v0.3.x this plugin ran Meow / ModelTrace fingerprint checks; v0.4.0 removed them entirely. README.md (Chinese) is the authoritative description of product behavior. User-facing strings and error messages are in Chinese, so keep that convention.

## Commands

The host SDK (`gateway-plugin-sdk`) and packaging CLI are pinned to codex-proxy-rs rev `af35cde3744b7afc81c4ad6e1e9c9c76bbbd68b8`. Don't move them to a floating `main`.

```sh
# Full release build (Docker): frontend test+build → per arch: Rust tests+release build in rust:1.99.1-slim-bookworm → package → smoke test
cargo install --git https://github.com/zyycn/codex-proxy-rs.git --rev af35cde3744b7afc81c4ad6e1e9c9c76bbbd68b8 \
  --locked --root .tools/cpr-cli codex-proxy-plugin-cli
CPR_PLUGIN_BIN="$PWD/.tools/cpr-cli/bin/cpr-plugin" ./scripts/build-linux.sh         # amd64 + arm64 → dist/*.tar.gz (+ .sha256)
CPR_PLUGIN_BIN="$PWD/.tools/cpr-cli/bin/cpr-plugin" ./scripts/build-linux.sh arm64   # one arch; per-arch dir .build/linux-target-<arch>
# Preferred for releases (no local disk use): .github/workflows/build.yml runs `build-linux.sh arm64` on ubuntu-24.04-arm.
# Pushing tag v<plugin.json version> publishes/overwrites that Release's assets; workflow_dispatch only uploads an artifact.

# Rust backend (no external services needed; host callbacks are mocked)
cargo test --manifest-path backend/Cargo.toml              # unit tests + backend/tests/protocol.rs
cargo test --manifest-path backend/Cargo.toml <test_name>
# A local toolchain older than rust-version 1.99.1 needs --ignore-rust-version.

# Frontend (Node 24+, pnpm 10.10)
pnpm --dir frontend install --frozen-lockfile
pnpm --dir frontend test                          # node:test, transpiles TS sources directly with `typescript`
node --test frontend/tests/presentation.test.mjs  # single test file
pnpm --dir frontend typecheck
pnpm --dir frontend build
pnpm --dir frontend dev                           # then open http://127.0.0.1:5173/?preview=1 (mock host, DEV only)
#   preview query params: theme=dark, scenario=empty|loading|error|many|slow

# Packaged-artifact smoke test against an offline SDK host fixture (needs Docker)
python3 scripts/package-smoke.py /abs/path/plugin.tar.gz

# Browser QA against the preview (env: PLAYWRIGHT_PATH, CHROME_BINARY, PREVIEW_URL; default http://127.0.0.1:4175/)
node scripts/browser-qa.mjs                        # screenshots go to review/ (gitignored)
```

## Architecture

Two layers. Model traffic and credentials **never** leave the host.

1. **Rust plugin binary** (`backend/`). `main.rs` accepts an SDK session over stdio. `lib.rs::plugin()` builds from `plugin.json` (it is `include_bytes!`'d, so manifest edits affect the binary and tests) and registers one management page. `lib.rs::Service` holds the state mutex, a global execution semaphore (`MAXIMUM_PARALLEL` = 8) and the per-account in-process busy set (`claim` → `Busy` guard released on drop, including host cancellation). `management.rs` holds the route table (`registration()`) and the `dispatch` match. A new route has to be added to both. A `PluginFault` is mapped to an HTTP status plus a `{error:{code,message,send_state}}` body, not a protocol error.
   - `candy.rs`: the prompt (from haowang02/codex-candy-eval, MIT), `EFFORTS` (`none` = omit `reasoning`), request `payload`, grading (`has_standalone_answer`: an ASCII `21` with no adjacent ASCII digit), the Responses-stream `ResponseParser` (final text from `response.completed`, delta fallback only when `output` is missing/empty; failed/incomplete/refusal/empty text → error), and `History`/`Record` (≤20 records per account, oldest first).
   - `host.rs` wraps host callbacks: `host.auth.list/get_runtime`, `host.keys.list`, `host.models.list`, `host.state.get/put/delete` (CAS by version, namespace `candy`, key = account ID), `host.model.execute_stream/stream_read/stream_close`. Every model call explicitly sets `account_id`, `client_key_id`, `provider=openai`, `protocol=openai`. `ask()` bounds the whole request by a deadline and closes the stream on failure.
2. **Vue frontend** (`frontend/`). It uses `@codex-proxy/ui` (BaseTable with selection/expanded rows, BaseMarkdown for answers) and Tailwind 4. It talks to the plugin only through the host bridge `window.codexProxyPlugin` (version 2) → `api/*` routes (`src/api/index.ts`). The page drives batch tests itself: a worker pool of N accounts (N = "并发账号"), each account answering serially; stop/unmount only prevents new requests. The sandboxed iframe has no async clipboard and `localStorage` throws, so defaults come from the newest record rather than browser storage. `vite.config.ts` forces a single IIFE `app.js` + `app.css` with a `defer` script, because the host serves it in a sandboxed iframe. `src/preview.ts` is a mock host loaded only under `import.meta.env.DEV && ?preview=1`, and the production build strips it.

### Routes

- `GET api/bootstrap`: prompt, efforts, limits, keys, and every OpenAI account (including disabled) with record **summaries** (no answer/error text). Pending records of accounts that aren't busy are shown as interrupted (view only). If the body would exceed ~960 KiB, the per-account history shown shrinks (20 → 10 → 5 → 1) and `history_shown` says so.
- `POST api/models {key_id}`, `POST api/candy/record {account_id, record_id}` (full answer/error/request ID), `POST api/candy/clear {account_ids}` (skips busy accounts).
- `POST api/candy/run {account_id, key_id, model, effort}`, one answer:
  1. Validate input, take a semaphore permit (`try_acquire` → 503), claim the account (busy → 409), and re-check the account is enabled OpenAI.
  2. Under the state lock: read history, `recover()` stale pending records into failed/interrupted, push a `pending` record, and **persist it before sending**.
  3. Release the lock and call the model with a 100 s limit (the host caps a management call at 120 s, and model streams are call-scoped and cannot outlive it).
  4. Grade, re-lock, `finish` the record by ID, and CAS-write. Return the record summary plus the account's summaries.

## Invariants to preserve

- Keep account/Key binding explicit, write `pending` before sending, charge interruptions as failed, and never count failed or interrupted answers in accuracy. Don't auto-retry billed requests (neither backend nor page).
- Grading is on the full answer text. Only the stored excerpt (3000 bytes) is truncated.
- Keep responses under the host's 1 MiB management body limit. Answer and error text stay out of list responses.
- `plugin.json` must keep the legacy `runs` namespace declared **byte-identical** (same schema, schemaVersion 1). The host refuses an upgrade that drops a namespace still holding records. The plugin never reads or writes `runs` (tests assert this). For the same reason, rolling back to 0.3.x requires clearing all `candy` records first.
- Changes should come with offline protocol tests (`backend/tests/protocol.rs` uses mocked host callbacks) and browser checks. Don't auto-deploy or trigger real, billed model requests.
- The SDK rev fixes the plugin wire protocol (`PROTOCOL_VERSION`, which is 4 at v3.29.6, while v3.18 to v3.28 hosts speak 2). A host on a different protocol rejects the install with "插件清单或协议版本不兼容，请使用当前 SDK 重新构建". When you move the rev, also update `plugin.json` `engines`, `PROTOCOL_VERSION` in `scripts/package-smoke.py` (it hand-encodes frames: business JSON goes in a `data_bytes` payload prefix), `CPR_REV` in the workflow, and the `rust:<ver>-slim-bookworm` image in `build-linux.sh` and the workflow to the SDK's `rust-version`.
- Release targets are Linux x86_64 and aarch64, glibc 2.36+ (built on bookworm images). This repo's GitHub Releases ship the aarch64 package.
- A version bump touches `plugin.json`, `backend/Cargo.toml` + `Cargo.lock`, `frontend/package.json`, `README.md`, and `NOTICE`.
- Bundled third-party code needs a NOTICE entry (currently: SDK, UI, Vue, VueUse, Lucide, candy-eval prompt/plan names, marked, DOMPurify).
