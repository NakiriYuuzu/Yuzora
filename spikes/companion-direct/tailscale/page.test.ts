import { expect, test } from 'bun:test';
import { respond } from './page';

test('capability page has same-origin scripts, strict CSP and explicit API checks', async () => {
  const page = respond(new Request('https://spike.local/'));
  expect(page.headers.get('Content-Security-Policy')).toContain("default-src 'none'");
  expect(page.headers.get('Content-Security-Policy')).not.toContain('unsafe-inline');
  expect(await page.text()).toContain('<script src="/app.js">');
  const script = await respond(new Request('https://spike.local/app.js')).text();
  expect(script).toContain('rpId: location.hostname');
  expect(script).toContain("userVerification: 'required'");
  expect(script).toContain("residentKey: 'preferred'");
  expect(script).toContain('[-7, -8, -257]');
  expect(script).toContain("attestation: 'none'");
  expect(() => new Function(script)).not.toThrow(); // syntax only, never invokes browser APIs
});

test('whoami preserves headers as JSON data, not executable HTML', async () => {
  const response = respond(new Request('https://spike.local/whoami', {headers: {
    'Tailscale-User-Login': 'someone@example.invalid', 'Tailscale-User-Name': '<script>evil</script>',
    Host: 'spike.local:8443', Origin: 'https://spike.local:8443',
  }}));
  expect(response.headers.get('Cache-Control')).toBe('no-store');
  expect(await response.json()).toEqual({'Tailscale-User-Login': 'someone@example.invalid',
    'Tailscale-User-Name': '<script>evil</script>', Host: 'spike.local:8443', Origin: 'https://spike.local:8443'});
});

test('manifest, SW and icons are same-origin; unknown paths/methods fail', async () => {
  const manifest = await respond(new Request('https://spike.local/manifest.webmanifest')).json();
  expect(manifest.display).toBe('standalone');
  for (const icon of manifest.icons) {
    const response = respond(new Request(`https://spike.local${icon.src}`));
    expect(response.headers.get('Content-Type')).toBe('image/png');
    expect((await response.arrayBuffer()).byteLength).toBeGreaterThan(100);
  }
  expect(respond(new Request('https://spike.local/sw.js')).status).toBe(200);
  expect(respond(new Request('https://spike.local/secret.key')).status).toBe(404);
  expect(respond(new Request('https://spike.local/', {method: 'POST'})).status).toBe(405);
});

test('registration decodes byte 32 and reports attachment/transports; get targets the chosen key', async () => {
  const script = await respond(new Request('https://spike.local/app.js')).text();
  const created: any[] = [], requested: any[] = [];
  const platformId = new Uint8Array([1]).buffer, keyId = new Uint8Array([2]).buffer;
  const data = (flags: number) => { const bytes = new Uint8Array(37); bytes[32] = flags; return bytes.buffer; };
  const navigator = {credentials: {
    create: async (options: any) => {
      created.push(options.publicKey);
      const key = options.publicKey.authenticatorSelection.authenticatorAttachment === 'cross-platform';
      return {id: key ? 'key' : 'platform', rawId: key ? keyId : platformId, type: 'public-key',
        authenticatorAttachment: key ? 'cross-platform' : 'platform', response: {
          getAuthenticatorData: () => data(key ? 5 : 29), getTransports: () => key ? ['nfc', 'usb'] : ['internal'],
        }};
    },
    get: async (options: any) => {
      requested.push(options.publicKey);
      return {id: 'key', authenticatorAttachment: 'cross-platform', response: {authenticatorData: data(5)}};
    },
  }};
  const document = {querySelector: () => ({}), getElementById: () => ({addEventListener() {}})};
  // Plain JS unit test with fake APIs; no browser, device or authenticator is invoked.
  const actions = new Function('navigator', 'document', 'location', 'crypto', script + '\nreturn actions;')(
    navigator, document, {hostname: 'spike.local'}, crypto);
  await expect(actions['get-key']()).rejects.toThrow('Create security key');
  expect(requested).toHaveLength(0);
  expect(await actions.create()).toMatchObject({flags: 29, UP: true, UV: true, BE: true, BS: true,
    authenticatorAttachment: 'platform', transports: ['internal']});
  expect(await actions['create-key']()).toMatchObject({flags: 5, UP: true, UV: true, BE: false, BS: false,
    authenticatorAttachment: 'cross-platform', transports: ['nfc', 'usb']});
  expect(created.map(c => c.authenticatorSelection)).toEqual([
    {authenticatorAttachment: 'platform', userVerification: 'required', residentKey: 'preferred'},
    {authenticatorAttachment: 'cross-platform', userVerification: 'required', residentKey: 'preferred'},
  ]);
  expect(await actions['get-key']()).toMatchObject({UP: true, UV: true, BE: false, BS: false});
  expect(requested[0].allowCredentials).toEqual([{type: 'public-key', id: keyId, transports: ['nfc', 'usb']}]);
  await actions.get();
  expect(requested[1].allowCredentials).toEqual([{type: 'public-key', id: platformId, transports: ['internal']}]);
});

test('missing registration diagnostics are unknown, never falsely reported as BE=0', async () => {
  const script = await respond(new Request('https://spike.local/app.js')).text();
  const actions = new Function('navigator', 'document', 'location', 'crypto', script + '\nreturn actions;')(
    {credentials: {create: async () => ({rawId: new ArrayBuffer(1), response: {}})}},
    {querySelector: () => ({}), getElementById: () => ({addEventListener() {}})}, {hostname: 'spike.local'}, crypto);
  expect(await actions.create()).toMatchObject({flags: null, UP: null, UV: null, BE: null, BS: null,
    authenticatorAttachment: null, transports: null});
});
