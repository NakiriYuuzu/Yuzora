"""Loopback-only HTTP/TLS checks; no browser or trust-store modification."""
from pathlib import Path
import subprocess
import time

ROOT = Path(__file__).resolve().parent.parent
OUT = ROOT / 'lan/out'
name = (OUT / 'hostname.txt').read_text().strip()
results = []


def command(args, expected=0, error=None):
    p = subprocess.run(args, capture_output=True, text=True, timeout=10)
    results.append('$ ' + ' '.join(args) + f'\nexit={p.returncode}\n' + p.stdout + p.stderr)
    if not p.returncode == expected:
        raise SystemExit(results[-1])
    if error and error not in p.stdout + p.stderr:
        raise SystemExit(results[-1])
    return p


try:
    if (OUT / 'ca.key').exists():
        raise SystemExit('CA signing key must already be deleted')
    command(['curl', '--version'])
    command(['openssl', 'version'])
    cases = [('leaf', name, None, None),
             ('other', 'other.local', '--counterexample', 'permitted subtree violation'),
             ('ip', '127.0.0.1', '--ip-counterexample', 'excluded subtree violation')]
    for leaf, host, flag, error in cases:
        command(['openssl', 'verify', '-CAfile', str(OUT / 'ca.pem'),
                 '-verify_ip' if leaf == 'ip' else '-verify_hostname', host, str(OUT / f'{leaf}.pem')],
                expected=2 if error else 0, error=error)
        server = subprocess.Popen(['bun', str(ROOT / 'lan/server.ts'), *([flag] if flag else [])],
                                  stdout=subprocess.PIPE, stderr=subprocess.PIPE)
        try:
            time.sleep(1)
            if not server.poll() is None:
                raise SystemExit('Server failed to bind')
            command(['curl', '--noproxy', '*', '--fail', '--silent', '--show-error', '--max-time', '5',
                     '--cacert', str(OUT / 'ca.pem'), '--resolve', f'{host}:47443:127.0.0.1',
                     f'https://{host}:47443/'], expected=60 if error else 0, error=error)
        finally:
            server.terminate()
            server.communicate(timeout=5)
            results.append(f'{leaf} loopback server stopped; exit={server.returncode}')
    results.append('PASS: OpenSSL verify and this curl TLS backend both accept the permitted DNS leaf, '
                   'reject other.local (permitted subtree violation), and reject matching IP SAN 127.0.0.1 '
                   '(excluded subtree violation). Versions/backends above; no phone verifier tested. '
                   'All loopback servers stopped. ca.key absent.')
finally:
    (ROOT / 'lan/SELFTEST.txt').write_text('\n'.join(results) + '\n')
