#!/usr/bin/env python3
"""Check served bytes, redirects and version scoping without release binaries."""
import hashlib
import subprocess
import sys
import tempfile
import time
import unittest
from pathlib import Path
from urllib.error import HTTPError
from urllib.request import Request, urlopen


class ReleaseMirrorTest(unittest.TestCase):
    def test_release_layout_preserves_exact_bytes_and_rejects_other_paths(self):
        with tempfile.TemporaryDirectory() as temporary:
            directory = Path(temporary)
            version = "0.2.16"
            payloads = {f"Kiln-{version}-windows-{arch}.zip": f"fixture {arch}\x00\xff".encode("latin1") for arch in ("x64", "arm64")}
            manifest = "".join(f"{hashlib.sha256(data).hexdigest()}  {name}\n" for name, data in payloads.items())
            payloads["SHA256SUMS.txt"] = manifest.encode("ascii")
            for name, data in payloads.items():
                (directory / name).write_bytes(data)
            (directory / "not-an-asset.txt").write_text("must not be served")
            port_file = directory / "port"
            process = subprocess.Popen([sys.executable, str(Path(__file__).with_name("release-archive-mirror.py")), str(directory), str(port_file), "--version", version], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
            try:
                deadline = time.monotonic() + 5
                while not port_file.exists():
                    if process.poll() is not None or time.monotonic() > deadline:
                        self.fail("mirror failed to start")
                    time.sleep(0.02)
                base = f"http://127.0.0.1:{port_file.read_text()}"
                with urlopen(base + "/releases/latest", timeout=3) as response:
                    self.assertEqual(response.url, base + f"/releases/tag/v{version}")
                    self.assertEqual(response.read(), b"Kiln 0.2.16 verification mirror")
                for name, data in payloads.items():
                    url = base + f"/releases/download/v{version}/{name}"
                    with urlopen(url, timeout=3) as response:
                        self.assertEqual(response.read(), data)
                    with urlopen(Request(url, method="HEAD"), timeout=3) as response:
                        self.assertEqual(int(response.headers["Content-Length"]), len(data))
                for path in ["/", "/not-an-asset.txt", "/releases/tag/v0.2.15", f"/releases/download/v{version}/../not-an-asset.txt", "/releases/download/v0.2.15/SHA256SUMS.txt"]:
                    with self.assertRaises(HTTPError) as error:
                        urlopen(base + path, timeout=3)
                    self.assertEqual(error.exception.code, 404)
            finally:
                process.terminate()
                process.wait(timeout=5)

    def test_invalid_version_is_rejected_before_serving(self):
        result = subprocess.run([sys.executable, str(Path(__file__).with_name("release-archive-mirror.py")), ".", "unused-port", "--version", "../0.2.16"], capture_output=True, text=True)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("stable numeric version", result.stderr)


if __name__ == "__main__":
    unittest.main()
