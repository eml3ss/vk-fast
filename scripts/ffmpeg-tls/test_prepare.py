#!/usr/bin/env python3
"""Check pinned trust source preparation without building FFmpeg."""
import argparse
import hashlib
import importlib.util
import json
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch


HERE = Path(__file__).resolve().parent
sys.dont_write_bytecode = True
SPEC = importlib.util.spec_from_file_location("prepare", HERE / "prepare.py")
prepare = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(prepare)
SOURCE = None


class TrustPreparation(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory(prefix="vk-fast-ca-test-")
        self.addCleanup(self.directory.cleanup)
        self.root = Path(self.directory.name)
        self.manifest = json.loads((HERE / "trust-source.json").read_text())
        self.pem_name = f"cacert-{self.manifest['ca_version']}.pem"

    def test_pinned_bundle_matches_the_official_hash_and_count(self):
        pem = prepare.checked_bytes(HERE / self.pem_name, self.manifest["ca_sha256"])
        published = (HERE / (self.pem_name + ".sha256")).read_text().split()[0]
        self.assertEqual(published, hashlib.sha256(pem).hexdigest())
        self.assertEqual(len(pem), self.manifest["ca_bytes"])
        self.assertEqual(pem.count(b"-----BEGIN CERTIFICATE-----"), self.manifest["ca_certificates"])

    def test_generated_header_is_exact_original_pem_plus_one_nul(self):
        cc = shutil.which("cc")
        if not cc:
            self.skipTest("C compiler not installed")
        pem = (HERE / self.pem_name).read_bytes()
        header = prepare.render_header(pem, self.manifest)
        self.assertEqual(header, prepare.render_header(pem, self.manifest))
        (self.root / "ca.h").write_bytes(header)
        source = self.root / "check.c"
        source.write_text(
            '#include <stdio.h>\n#include "ca.h"\n'
            'int main(void) { return fwrite(vk_fast_mozilla_ca, 1, '
            'sizeof(vk_fast_mozilla_ca), stdout) != sizeof(vk_fast_mozilla_ca); }\n'
        )
        binary = self.root / "check"
        subprocess.run([cc, "-std=c11", "-Wall", "-Wextra", "-Werror", str(source), "-o", str(binary)], check=True)
        result = subprocess.run([str(binary)], capture_output=True, check=True)
        self.assertEqual(result.stdout, pem + b"\0")

    def isolated_source(self):
        if SOURCE is None:
            self.skipTest("pass --source to check the FFmpeg patch")
        directory = self.root / "source"
        (directory / "libavformat").mkdir(parents=True)
        shutil.copyfile(SOURCE / "libavformat/tls_mbedtls.c", directory / "libavformat/tls_mbedtls.c")
        return directory

    def test_source_preparation_is_offline_idempotent_and_keeps_verification(self):
        source = self.isolated_source()
        target = source / "libavformat/tls_mbedtls.c"
        before = target.read_text()
        with patch.object(prepare.urllib.request, "urlopen", side_effect=AssertionError("unexpected network")):
            first = prepare.prepare(source, self.root / "downloads")
            second = prepare.prepare(source, self.root / "downloads")
        self.assertEqual(first, second)
        after = target.read_text()
        # Existing checks and hostname binding must remain byte-for-byte intact.
        for line in before.splitlines():
            if any(word in line for word in ["shr->verify", "set_hostname", "get_verify_result", "conf_authmode", "VERIFY_OPTIONAL", "VERIFY_NONE"]):
                self.assertIn(line, after)
        self.assertEqual(after.count("mbedtls_x509_crt_parse_file(&tls_ctx->ca_cert, shr->ca_file)"), 1)
        self.assertEqual(after.count("mbedtls_x509_crt_parse(&tls_ctx->ca_cert, vk_fast_mozilla_ca"), 1)

    def test_unknown_source_is_rejected_without_modifying_it(self):
        source = self.isolated_source()
        target = source / "libavformat/tls_mbedtls.c"
        altered = target.read_bytes() + b"\n/* altered */\n"
        target.write_bytes(altered)
        with self.assertRaisesRegex(ValueError, "not the pinned"):
            prepare.prepare(source, self.root / "downloads")
        self.assertEqual(target.read_bytes(), altered)
        self.assertFalse((source / "libavformat/vk_fast_mozilla_ca.h").exists())

    def test_tampered_pem_is_rejected_without_modifying_source(self):
        source = self.isolated_source()
        target = source / "libavformat/tls_mbedtls.c"
        original = target.read_bytes()
        fake = self.root / "trust"
        fake.mkdir()
        shutil.copyfile(HERE / "trust-source.json", fake / "trust-source.json")
        (fake / self.pem_name).write_bytes((HERE / self.pem_name).read_bytes() + b"tampered")
        with patch.object(prepare, "HERE", fake), self.assertRaisesRegex(ValueError, "SHA256 mismatch"):
            prepare.prepare(source, self.root / "downloads")
        self.assertEqual(target.read_bytes(), original)
        self.assertFalse((source / "libavformat/vk_fast_mozilla_ca.h").exists())


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--source", type=Path)
    args, remaining = parser.parse_known_args()
    SOURCE = args.source.resolve() if args.source else None
    unittest.main(argv=[__file__, *remaining])
