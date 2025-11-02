#!/usr/bin/env bash
# Nightly live smoke test helper for www.zqsdev.com.
#
# Cron setup (runs daily at 08:30 Europe/Paris):
#   1. Ensure the system time zone is set: sudo timedatectl set-timezone Europe/Paris
#   2. Make the script executable: chmod +x scripts/run_live_autotest.sh
#   3. Edit the crontab for the deployment user (crontab -e) and add:
#        30 8 * * * cd /root/zqsdev.com && ./scripts/run_live_autotest.sh >> /root/zqsdev.com/logs/live-smoke.log 2>&1
#      (Create logs/ once: mkdir -p /root/zqsdev.com/logs)
#      This forwards any extra flags via AUTOTEST_FLAGS if needed, e.g.
#        AUTOTEST_FLAGS="--json-output nightly-smoke.json"
#
# Run with --cron-help to print these instructions.

set -euo pipefail

show_cron_help() {
    cat <<'EOF'
ZQSDev Live Smoke Test — Cron Setup
===================================

1. Set the server timezone (needed so 08:30 means Paris time):
     sudo timedatectl set-timezone Europe/Paris

2. Ensure the script is executable inside the repository:
     chmod +x /root/zqsdev.com/scripts/run_live_autotest.sh

3. Configure cron for the deployment user:
     crontab -e
   Add the line:
     30 8 * * * cd /root/zqsdev.com && ./scripts/run_live_autotest.sh >> /root/zqsdev.com/logs/live-smoke.log 2>&1

4. (Optional) To pass flags to the smoke test (for JSON reports, different AI question, etc.),
   set AUTOTEST_FLAGS in the cron line, e.g.:
     AUTOTEST_FLAGS="--json-output nightly-smoke.json" ./scripts/run_live_autotest.sh >> /root/zqsdev.com/logs/live-smoke.log 2>&1

Ensure PUSHOVER_API_TOKEN and PUSHOVER_USER_KEY are available (via environment or .env files)
so alerts fire on failures or skipped runs.
EOF
}

if [[ "${1:-}" == "--cron-help" ]]; then
    show_cron_help
    exit 0
fi

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"
cd "${REPO_ROOT}"

extra_flags="${AUTOTEST_FLAGS:-}"
if [[ $# -gt 0 ]]; then
    extra_flags="$*"
fi

make autotest AUTOTEST_FLAGS="${extra_flags}"
