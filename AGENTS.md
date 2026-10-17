# zqsdev.com Operations Notes

This file captures the details an operator or automation agent needs to keep the backend healthy.

## Systemd service
- Unit file: `/etc/systemd/system/zqs-terminal.service`.
- Runs `/opt/zqsdev/bin/zqs-terminal-server` as the `zqsdev` user with `WorkingDirectory=/opt/zqsdev`.
- Environment is loaded from `/etc/zqsdev/server.env` (contains `OPENAI_API_KEY`, `OPENAI_MODEL=gpt-5.6-luna`, `HOST=127.0.0.1`, `PORT=8787`, `STATIC_DIR=/opt/zqsdev/static`, `AI_BUDGET_PATH=/opt/zqsdev/logs/ai-budget.json`, `RUST_LOG=info`).
- Manage lifecycle with `sudo systemctl status|restart|stop zqs-terminal.service`; logs append to `/opt/zqsdev/backend.log` (mirrored as `./backend.log`) and can also be read with `journalctl -u zqs-terminal.service`.
- The service binds to `127.0.0.1:8787` (reachable only through Nginx) and automatically restarts on failure (`Restart=on-failure`, `RestartSec=5s`). Tail the live log with `make backend-log`.

## Public ingress
- Nginx proxy: `/etc/nginx/sites-enabled/api.zqsdev.com` forwards HTTPS traffic on `api.zqsdev.com` to `http://127.0.0.1:8787`. Keep the `/api/*` prefix when adding new endpoints.
- TLS certificate: managed by Certbot (`/etc/letsencrypt/live/api.zqsdev.com/`), renews automatically.
- If the proxy breaks, reload Nginx with `sudo systemctl reload nginx` after adjustments.
- Netlify rewrite: `/api/*` → `https://api.zqsdev.com/api/:splat`. Re-deploy the site after editing `netlify.toml`.
- Netlify build pipeline: publish-only (no build command). Always run `make build && make test` locally so the committed `static/` assets and `build_id.js` are fresh before pushing.

## Backend releases
The VPS has no git checkout: each release is an immutable directory and the switch is atomic.
- Layout: `/opt/zqsdev/releases/<version>/` (source archive + `DEPLOY_COMMIT`), `/opt/zqsdev/static -> releases/<version>/static`, `/opt/zqsdev/bin/zqs-terminal-server`, `/opt/zqsdev/backups/before-terminal-<version>/` (previous binary + `static-target`), shared Cargo cache `/opt/zqsdev/build-target`.
- Deploy with `make deploy-backend` after committing and pushing (SSH alias `DEPLOY_HOST=ovh` by default, passwordless sudo). It archives `HEAD` with LF endings, uploads it with `scripts/deploy_backend_release.py`, builds as `zqsdev` with `cargo build --locked --release`, backs up the running release, swaps the binary and the `static` symlink, restarts the unit and checks `/api/version` (version + commit). Any failure restores the previous release automatically.
- Manual rollback: copy `backups/before-terminal-<version>/zqs-terminal-server` back to `bin/`, point `static` at the path stored in `static-target`, then `sudo systemctl restart zqs-terminal.service`.
- A release never touches `/etc/zqsdev/server.env` or the budget ledger.

## Workflow notes
- Before handoff, run `make build` and `make test` so the maintainer can refresh the live site with confidence.
- Typical deploy loop: `make build && make test`, commit, `git push` (Netlify redeploys the frontend automatically), then `make deploy-backend` whenever `server/` or `static/data/` changed (the backend loads its data at startup from its own release), and only then run `make autotest AUTOTEST_FLAGS="--no-pushover"` to smoke-test production.
- `make deploy-backend` refuses uncommitted tracked changes because the release is built from `HEAD`; commit the regenerated artifacts first (remember `static/build_id.js` and `static/pkg/**/*`).
- `static/build_id.js` is now tracked; always include the regenerated file produced by `make build` in your commits so Netlify's auto-deploys never fall back to the `?build=dev` cache-buster.
- `make autotest` now fails fast if the deployed `build_id.js` ever falls back to `"dev"`, ensuring production assets are tied to a real commit.
- Extend the automated test suite for every new feature or bugfix fix to keep coverage trending upward.
- Run `python3 scripts/validate_knowledge.py` after editing `static/data/*.json`. Keep historical projects, publications and testimonials. Legacy RAG scripts are retained only for forensic inspection; never run paid upserts as part of a normal release.
- After deploying, run `make autotest AUTOTEST_FLAGS="--no-pushover"` (default base URL `https://zqsdev.com`; `www` redirects and breaks `POST /api/ai`) to ensure `/api/ai` returns factually correct answers with validated `sources` IDs.
- `KNOWLEDGE_UPDATED` in `server/src/ai.rs` must equal `updated_at` in `static/data/profile.json` (a server test enforces it); change both when the curated data is reviewed.
- After editing `static/cv/*.html`, regenerate the PDFs with `python3 scripts/render_cv_pdfs.py` (Chrome + `pip install pymupdf`; `--check` previews the text diff without writing). It reuses the 1254px portrait embedded in the current PDF, which is not a web asset.
- Use the hidden `version` terminal command (or `make version-check`) to verify the frontend and backend are running the same release and to grab the GitHub commit links directly from production.
- If `make version-check` reports an outdated backend commit, check `DEPLOY_COMMIT` in the release that `/opt/zqsdev/static` points to and rerun `make deploy-backend`; the API only reports the embedded `env!(\"CARGO_PKG_VERSION\")` from the running binary.

## Versioning
- The project version lives in `VERSION`, `Cargo.toml`, and `server/Cargo.toml`.
- **Always bump the version number _before every commit_** with `python3 scripts/bump_version.py` or `make bump-version`. Do this prior to edits so both frontend and backend artifacts report the new release identifier and the `version` command stays trustworthy. Immediately stage the updated `VERSION` file (`git add VERSION`) and run `make ensure-version-bumped` to verify the bump is present in your staged changes before committing.

## Full-context AI operations (1.0.18)
- Model default: `gpt-5.6-luna`, Chat Completions, reasoning `none`, max 640 output tokens. One provider call, no automatic retry.
- Data loads once at startup from all seven `static/data/*.json` sources, bounded to 64 KB. Restart after deploying data.
- History is client-supplied, validated, capped to 3 user/assistant pairs and treated as untrusted. `quit`/page reload clears it.
- Requests: max 1,200 characters, 64 KB body, two concurrent provider calls, 16-second server deadline and 20-second browser deadline. Data fetch falls back after 4 seconds.
- Spending is USD, reserved before a call and reconciled once from actual usage including cached reads/writes. Rolling limits: $0.50/min, $2/hour, $2/day, $10/30 days. Ledger: `/opt/zqsdev/logs/ai-budget.json`, private and persistent. Never delete or reset it on deploy. Uncertain failures retain their reservation conservatively.
- Only one server process may own a ledger. Budget file corruption or persistence failure disables AI while preserving `/api/data` and classic commands. Set model prices explicitly if changing model.
- Nginx must overwrite `X-Real-IP`. The app ignores spoofable first `X-Forwarded-For` entries. Through Netlify, limits may group requests behind the same proxy IP; global spending and concurrency limits remain effective.
- Public `sources` identifies sections used by the model; it is attribution, not a guarantee of factual accuracy. Check answer facts in live evals.
- Never publish a budget file or logs in `STATIC_DIR`. Do not delete the old Pinecone index without a separate decision; it is no longer queried.
