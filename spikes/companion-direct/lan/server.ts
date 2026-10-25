import { respond } from '../tailscale/page';

export function isAllowedBindAddress(hostname: string | undefined): hostname is string {
  if (!hostname || !/^(25[0-5]|2[0-4]\d|1\d\d|[1-9]?\d)(\.(25[0-5]|2[0-4]\d|1\d\d|[1-9]?\d)){3}$/.test(hostname)) return false;
  const octets = hostname.split('.').map(Number);
  return hostname === '127.0.0.1' || octets[0] === 10 ||
    (octets[0] === 172 && octets[1] >= 16 && octets[1] <= 31) ||
    (octets[0] === 192 && octets[1] === 168);
}

if (import.meta.main) {
  // Default is loopback. Only the user may opt in to a specific private LAN address.
  const args = Bun.argv.slice(2);
  const bindIndex = args.indexOf('--bind');
  const hostname = bindIndex === -1 ? '127.0.0.1' : args[bindIndex + 1];
  if (!isAllowedBindAddress(hostname)) {
    throw new Error('Use 127.0.0.1 or an explicit RFC1918 IPv4 address; no wildcard listener');
  }
  const modes = ['--counterexample', '--ip-counterexample', '--phone-ip-counterexample'].filter(flag => args.includes(flag));
  if (modes.length > 1) throw new Error('Choose only one counterexample leaf');
  const leaf = modes[0] === '--counterexample' ? 'other' : modes[0] === '--ip-counterexample' ? 'ip' : modes[0] === '--phone-ip-counterexample' ? 'phone-ip' : 'leaf';
  const server = Bun.serve({hostname, port: 47443, fetch: respond,
    tls: {cert: Bun.file(`${import.meta.dir}/out/${leaf}.pem`), key: Bun.file(`${import.meta.dir}/out/${leaf}.key`)} });
  console.log(`Capability spike only: ${server.url} (${leaf})`);
  for (const signal of ['SIGINT', 'SIGTERM'] as const) process.on(signal, () => { server.stop(true); process.exit(0); });
}
