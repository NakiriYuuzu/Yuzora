"""Pure preflight path tests; mock subprocess, never invoke Herdr."""
import importlib.util
import json
from pathlib import Path
import tempfile
from types import SimpleNamespace
import unittest
from unittest.mock import patch

ROOT = Path(__file__).resolve().parent
spec = importlib.util.spec_from_file_location('isolated_probe', ROOT / 'probe.py')
probe = importlib.util.module_from_spec(spec)
spec.loader.exec_module(probe)


class PreflightTests(unittest.TestCase):
    def test_realpath_rejects_symlink_escape_for_each_routing_field(self):
        with tempfile.TemporaryDirectory(prefix='preflight-test-', dir=ROOT) as scratch:
            base = Path(scratch)
            root = base / 'root'
            root.mkdir()
            outside = base / 'outside'
            outside.mkdir()
            (root / 'escape').symlink_to(outside)
            fixtures = base / 'fixtures'
            fixtures.mkdir()
            session = {'name': 'default', 'running': False,
                       'socket_path': str(root / 'socket'), 'session_dir': str(root / 'session')}
            with patch.object(probe, 'T', root), patch.object(probe, 'FIX', fixtures):
                with patch.object(probe.subprocess, 'run', return_value=SimpleNamespace(stdout=json.dumps({'sessions': [session]}))):
                    self.assertEqual(probe.preflight()['sessions'], [session])
                for field in ('socket_path', 'session_dir'):
                    escaped = {**session, field: str(root / 'escape' / 'target')}
                    with self.subTest(field=field), patch.object(probe.subprocess, 'run', return_value=SimpleNamespace(stdout=json.dumps({'sessions': [escaped]}))):
                        with self.assertRaises(SystemExit) as raised:
                            probe.preflight()
                        self.assertEqual(raised.exception.code, escaped)


if __name__ == '__main__':
    unittest.main()
