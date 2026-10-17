"""Install a backend release on the production VPS; run as root via `make deploy-backend`.

Usage: sudo python3 deploy_backend_release.py <version> <commit>

Expects /tmp/zqsdev-release-<version>.tar.gz (a `git archive` of <commit>). Builds the
server as `zqsdev`, keeps the previous binary and static target under backups/, swaps the
binary and the `static` symlink atomically, restarts the unit and checks /api/version.
Any failure after the swap restores the previous release. /etc/zqsdev/server.env and the
AI budget ledger in /opt/zqsdev/logs are never touched.
"""
from pathlib import Path
import json, os, shutil, subprocess, sys, time, urllib.request

version, commit = sys.argv[1], sys.argv[2]
base = Path('/opt/zqsdev')
release = base/'releases'/version
cache = base/'build-target'
binary = base/'bin/zqs-terminal-server'
static = base/'static'
backup = base/'backups'/f'before-terminal-{version}'
archive = Path(f'/tmp/zqsdev-release-{version}.tar.gz')

assert len(commit) == 40, 'pass the full commit hash'
assert archive.is_file(), archive
assert static.is_symlink()
previous = static.resolve(strict=True)
assert previous.parent.parent == base/'releases', previous
assert not release.exists() and not backup.exists(), 'release already installed'
release.mkdir(mode=0o755)
subprocess.run(['tar', '-xzf', str(archive), '-C', str(release)], check=True)
assert (release/'VERSION').read_text().strip() == version
(release/'DEPLOY_COMMIT').write_text(commit+'\n')
cache.mkdir(exist_ok=True)
subprocess.run(['chown', '-R', 'zqsdev:zqsdev', str(release), str(cache)], check=True)
subprocess.run(['sudo', '-u', 'zqsdev', 'env', 'ZQS_BUILD_COMMIT='+commit, 'CARGO_TARGET_DIR='+str(cache),
                'cargo', 'build', '--locked', '--release', '--manifest-path', str(release/'server/Cargo.toml')],
               cwd=release, check=True)
backup.mkdir(mode=0o700)
shutil.copy2(binary, backup/'zqs-terminal-server')
(backup/'static-target').write_text(str(previous)+'\n')
replacement = binary.with_suffix('.next')
shutil.copy2(cache/'release/zqs_terminal_server', replacement)
os.chmod(replacement, 0o755)
next_static = base/f'static.next-{version}'
next_static.symlink_to(release/'static', target_is_directory=True)
try:
    os.replace(replacement, binary)
    os.replace(next_static, static)
    subprocess.run(['systemctl', 'restart', 'zqs-terminal'], check=True)
    for attempt in range(20):
        try:
            with urllib.request.urlopen('http://127.0.0.1:8787/api/version', timeout=2) as response:
                deployed = json.load(response)
            assert deployed['version'] == version and deployed['commit'] == commit
            assert deployed['ai_configured']
            print(json.dumps({'deployed': deployed, 'rollback': str(backup), 'configuration_unchanged': True}), flush=True)
            break
        except Exception:
            if attempt == 19:
                raise
            time.sleep(.25)
except Exception:
    shutil.copy2(backup/'zqs-terminal-server', replacement)
    os.replace(replacement, binary)
    if next_static.is_symlink():
        next_static.unlink()
    next_static.symlink_to(previous, target_is_directory=True)
    os.replace(next_static, static)
    subprocess.run(['systemctl', 'restart', 'zqs-terminal'], check=True)
    raise
