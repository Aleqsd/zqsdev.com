# zqsdev.com Operations Notes

This file captures the details an operator or automation agent needs to keep the backend healthy.

## Systemd service
- Unit file: `/etc/systemd/system/zqs-terminal.service`.
- Runs `/opt/zqsdev/bin/zqs-terminal-server` as the `zqsdev` user with `WorkingDirectory=/opt/zqsdev`.
- Environment is loaded from `/etc/zqsdev/server.env` (contains `OPENAI_API_KEY`, `GROQ_API_KEY`, `GOOGLE_API_KEY`, `HOST=0.0.0.0`, `PORT=8787`, `STATIC_DIR=/opt/zqsdev/static`, `RUST_LOG=info`).
- Manage lifecycle with `sudo systemctl status|restart|stop zqs-terminal.service`; logs append to `/opt/zqsdev/backend.log` (mirrored as `./backend.log`) and can also be read with `journalctl -u zqs-terminal.service`.
- The service binds to TCP `0.0.0.0:8787` and automatically restarts on failure (`Restart=on-failure`, `RestartSec=5s`). Tail the live log with `make backend-log`.

## Public ingress
- Nginx proxy: `/etc/nginx/sites-enabled/api.zqsdev.com` forwards HTTPS traffic on `api.zqsdev.com` to `http://127.0.0.1:8787`. Keep the `/api/*` prefix when adding new endpoints.
- TLS certificate: managed by Certbot (`/etc/letsencrypt/live/api.zqsdev.com/`), renews automatically.
- If the proxy breaks, reload Nginx with `sudo systemctl reload nginx` after adjustments.
- Netlify rewrite: `/api/*` → `https://api.zqsdev.com/api/:splat`. Re-deploy the site after editing `netlify.toml`.

## Update workflow
Run `make update` from the repo root to:
1. `git pull --rebase` the repository.
2. Rebuild the WebAssembly bundle and proxy binaries (`make build`).
3. Restart the systemd unit (`sudo systemctl restart zqs-terminal.service`).

## Workflow notes
- Before handoff, run `make build` and `make test` so the maintainer can refresh the live site with confidence.
- Extend the automated test suite for every new feature or bugfix fix to keep coverage trending upward.

## Versioning
- The project version lives in `VERSION`, `Cargo.toml`, and `server/Cargo.toml`.
- Bump versions with `python3 scripts/bump_version.py` (defaults to patch) once per handoff, before the first `make build`/`make test`, and reuse that version for any reruns.
