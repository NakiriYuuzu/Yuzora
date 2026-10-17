"""Offline generator tests; scratch artifacts stay under this kit and are removed."""
import ipaddress
import os
from pathlib import Path
import runpy
import shutil
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch

from cryptography import x509

ROOT = Path(__file__).resolve().parent


class MakeCaTests(unittest.TestCase):
    def setUp(self):
        self.scratch = tempfile.TemporaryDirectory(prefix='ca-test-', dir=ROOT)
        self.addCleanup(self.scratch.cleanup)
        self.script = Path(self.scratch.name) / 'make-ca.py'
        shutil.copyfile(ROOT / 'make-ca.py', self.script)
        self.out = self.script.parent / 'out'

    def run_generator(self, *args):
        return subprocess.run([sys.executable, str(self.script), 'test-host', *args],
                              capture_output=True, text=True, timeout=10)

    def test_phone_ip_is_explicit_and_all_leaves_share_constrained_ca(self):
        result = self.run_generator('--counterexample-ip', '192.168.1.20')
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertFalse((self.out / 'ca.key').exists())
        ca = x509.load_pem_x509_certificate((self.out / 'ca.pem').read_bytes())
        extension = ca.extensions.get_extension_for_class(x509.NameConstraints)
        self.assertTrue(extension.critical)
        self.assertEqual(extension.value.permitted_subtrees, [x509.DNSName('test-host.local')])
        self.assertEqual(extension.value.excluded_subtrees, [
            x509.IPAddress(ipaddress.ip_network('0.0.0.0/0')), x509.IPAddress(ipaddress.ip_network('::/0'))])
        for label, san in [('leaf', x509.DNSName('test-host.local')), ('other', x509.DNSName('other.local')),
                           ('ip', x509.IPAddress(ipaddress.ip_address('127.0.0.1'))),
                           ('phone-ip', x509.IPAddress(ipaddress.ip_address('192.168.1.20')))]:
            leaf = x509.load_pem_x509_certificate((self.out / f'{label}.pem').read_bytes())
            self.assertTrue((self.out / f'{label}.key').is_file())
            leaf.verify_directly_issued_by(ca)
            self.assertEqual(list(leaf.extensions.get_extension_for_class(x509.SubjectAlternativeName).value), [san])
        self.assertNotEqual(self.run_generator().returncode, 0, 'Nonempty out must not be overwritten')

    def test_leaf_signing_failure_never_writes_ca_key(self):
        written = []
        original_write = Path.write_bytes
        original_sign = x509.CertificateBuilder.sign
        sign_count = 0

        def record_write(path, data):
            written.append(path.name)
            return original_write(path, data)

        def fail_leaf_signing(builder, *args, **kwargs):
            nonlocal sign_count
            sign_count += 1
            if sign_count > 1:
                raise RuntimeError('mock leaf signing failure')
            return original_sign(builder, *args, **kwargs)

        old_umask = os.umask(0o077)
        try:
            with patch.object(sys, 'argv', [str(self.script), 'test-host']), \
                    patch.object(Path, 'write_bytes', record_write), \
                    patch.object(x509.CertificateBuilder, 'sign', fail_leaf_signing):
                with self.assertRaisesRegex(RuntimeError, 'mock leaf signing failure'):
                    runpy.run_path(str(self.script), run_name='__main__')
        finally:
            os.umask(old_umask)
        self.assertEqual(sign_count, 2)
        self.assertEqual(written, ['ca.pem'])
        self.assertNotIn('ca.key', written)
        self.assertFalse((self.out / 'ca.key').exists())

    def test_invalid_phone_ips_rejected_before_creating_artifacts(self):
        for ip in ('8.8.8.8', '127.0.0.1', '0.0.0.0', '169.254.1.1', '100.64.0.1', '::1', 'not-an-ip'):
            with self.subTest(ip=ip):
                self.assertNotEqual(self.run_generator('--counterexample-ip', ip).returncode, 0)
                self.assertFalse(self.out.exists())


if __name__ == '__main__':
    unittest.main()
