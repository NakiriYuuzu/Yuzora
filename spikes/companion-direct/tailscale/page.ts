// Throwaway browser-capability probe, not an authenticated gateway or product UI.
const page = `<!doctype html><html lang="zh-Hant"><head><meta charset="utf-8">
<meta name="viewport" content="width=device-width,initial-scale=1">
<link rel="manifest" href="/manifest.webmanifest"><title>Companion Phase 0</title></head>
<body><h1>Companion capability spike</h1><p>僅測瀏覽器能力，未做伺服器 WebAuthn 驗證。請勿輸入真實資料。</p>
<button id="secure">Secure context</button> <button id="sw">Register SW</button>
<button id="create">Create platform passkey</button> <button id="get">Get platform passkey</button>
<button id="create-key">Create security key (NFC / USB-C)</button> <button id="get-key">Get security key</button>
<button id="notification">Notification permission</button> <button id="whoami">Who am I</button>
<pre id="result" aria-live="polite"></pre><script src="/app.js"></script></body></html>`;
const app = `const result = document.querySelector('#result');
const show = value => { result.textContent += JSON.stringify(value, null, 2) + '\\n'; };
const random = n => crypto.getRandomValues(new Uint8Array(n));
const credentials = {};
const decodeFlags = data => {
 if (!data || data.byteLength < 33) return {flags: null, UP: null, UV: null, BE: null, BS: null};
 const flags = new Uint8Array(data)[32];
 return {flags, UP: !!(flags & 1), UV: !!(flags & 4), BE: !!(flags & 8), BS: !!(flags & 16)};
};
const create = async attachment => {
  const c = await navigator.credentials.create({publicKey: {
   challenge: random(32), rp: {id: location.hostname, name: 'Yuzora spike'},
   user: {id: random(16), name: 'spike-user', displayName: 'Spike user'},
   pubKeyCredParams: [-7, -8, -257].map(alg => ({type: 'public-key', alg})),
   authenticatorSelection: {authenticatorAttachment: attachment, userVerification: 'required', residentKey: 'preferred'},
   attestation: 'none', timeout: 120000
  }});
  const transports = c.response.getTransports?.() ?? null;
  credentials[attachment] = {type: 'public-key', id: c.rawId, ...(transports ? {transports} : {})};
  return {operation: 'create', requestedAttachment: attachment, id: c.id, type: c.type,
   ...decodeFlags(c.response.getAuthenticatorData?.()), authenticatorAttachment: c.authenticatorAttachment ?? null,
   transports, note: 'client only; no verification; null flags mean API unavailable'};
};
const get = async attachment => {
  const credential = credentials[attachment];
  if (attachment === 'cross-platform' && !credential) throw new Error('Create security key in this page first; reload clears the target credential');
  const c = await navigator.credentials.get({publicKey: {
   challenge: random(32), rpId: location.hostname, userVerification: 'required', timeout: 120000,
   ...(credential ? {allowCredentials: [credential]} : {})
  }});
  return {operation: 'get', requestedAttachment: attachment, id: c.id,
   ...decodeFlags(c.response.authenticatorData), authenticatorAttachment: c.authenticatorAttachment ?? null,
   note: 'not server verified; without a stored platform ID, get uses discoverable credentials'};
};
const actions = {
 secure: () => ({isSecureContext, origin: location.origin, rpId: location.hostname, standalone: matchMedia('(display-mode: standalone)').matches}),
 sw: async () => ({scope: (await navigator.serviceWorker.register('/sw.js')).scope}),
 create: () => create('platform'),
 get: () => get('platform'),
 'create-key': () => create('cross-platform'),
 'get-key': () => get('cross-platform'),
 notification: async () => ({permission: await Notification.requestPermission()}),
 whoami: async () => (await fetch('/whoami', {cache: 'no-store'})).json()
};
for (const [id, action] of Object.entries(actions)) document.getElementById(id).addEventListener('click', async () => {
 try { show(await action()); } catch(e) { show({name: e.name, message: e.message}); }
});`;
const manifest = {name: 'Companion Phase 0', short_name: 'YZ spike', start_url: '/', display: 'standalone', icons: [192, 512].map(size => ({src: `/icon-${size}.png`, sizes: `${size}x${size}`, type: 'image/png', purpose: 'any'}))};
const routes: Record<string, [string, string]> = {
  '/': ['text/html; charset=utf-8', page],
  '/app.js': ['text/javascript; charset=utf-8', app],
  '/manifest.webmanifest': ['application/manifest+json', JSON.stringify(manifest)],
  '/sw.js': ['text/javascript', "self.addEventListener('install', () => self.skipWaiting()); self.addEventListener('activate', e => e.waitUntil(self.clients.claim())); self.addEventListener('fetch', () => {});"],
};
export function respond(request: Request): Response {
  const headers = new Headers({
    'Content-Security-Policy': "default-src 'none'; script-src 'self'; connect-src 'self'; worker-src 'self'; manifest-src 'self'; img-src 'self'; base-uri 'none'; form-action 'none'; frame-ancestors 'none'",
    'X-Content-Type-Options': 'nosniff', 'Referrer-Policy': 'no-referrer', 'Cache-Control': 'no-store',
  });
  if (request.method !== 'GET') return new Response('Method not allowed', {status: 405, headers});
  const path = new URL(request.url).pathname;
  if (path === '/whoami') {
    headers.set('Content-Type', 'application/json');
    return new Response(JSON.stringify(Object.fromEntries(['Tailscale-User-Login', 'Tailscale-User-Name', 'Host', 'Origin'].map(k => [k, request.headers.get(k)]))), {headers});
  }
  if (path === '/icon-192.png' || path === '/icon-512.png') {
    headers.set('Content-Type', 'image/png');
    return new Response(Bun.file(`${import.meta.dir}${path}`), {headers});
  }
  const route = routes[path];
  if (!route) return new Response('Not found', {status: 404, headers});
  headers.set('Content-Type', route[0]);
  return new Response(route[1], {headers});
}
