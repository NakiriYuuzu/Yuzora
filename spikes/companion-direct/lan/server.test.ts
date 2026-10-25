import { expect, test } from 'bun:test';
import { isAllowedBindAddress } from './server';

test('bind accepts only loopback and canonical RFC1918 dotted quads', () => {
  for (const address of ['127.0.0.1', '10.0.0.0', '10.255.255.255', '172.16.0.0',
    '172.31.255.255', '192.168.0.0', '192.168.255.255']) {
    expect(isAllowedBindAddress(address)).toBe(true);
  }
});

test('bind rejects noncanonical, malformed, wildcard and non-RFC1918 addresses', () => {
  for (const address of [undefined, '', '172.017.0.1', '010.0.0.1', '10.00.0.1',
    '10.0.0.01', '0x0a.0.0.1', '10.0x0.0.1', '10..0.1', '10.0.0.', '.10.0.1',
    '10.0.1', '10.0.0.1.2', '10.0.0.256', '10.-1.0.1', '10.+1.0.1', '10.1e1.0.1',
    ' 10.0.0.1', '10.0.0.1 ', '10.0.0.1\n', '0.0.0.0', '8.8.8.8',
    '172.15.255.255', '172.32.0.0', '192.167.255.255', '192.169.0.0',
    '100.64.0.1', '169.254.1.1', '127.0.0.2', '::1', '::ffff:10.0.0.1', 'localhost']) {
    expect(isAllowedBindAddress(address)).toBe(false);
  }
});
