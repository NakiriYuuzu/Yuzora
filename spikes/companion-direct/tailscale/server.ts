import { respond } from './page';
const server = Bun.serve({hostname: '127.0.0.1', port: 47110, fetch: respond});
console.log(`Capability spike only: ${server.url}`);
for (const signal of ['SIGINT', 'SIGTERM'] as const) process.on(signal, () => { server.stop(true); process.exit(0); });
