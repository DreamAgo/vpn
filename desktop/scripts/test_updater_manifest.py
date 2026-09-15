import importlib.util
import tempfile
import unittest
from pathlib import Path

spec = importlib.util.spec_from_file_location(
    "updater_manifest", Path(__file__).with_name("generate-updater-manifest.py")
)
updater = importlib.util.module_from_spec(spec)
spec.loader.exec_module(updater)


class ManifestTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.tag = "v0.1.16"
        for target, pattern in updater.ASSETS.items():
            asset = self.root / pattern.format(tag=self.tag)
            asset.write_bytes(b"test package")
            Path(str(asset) + ".sig").write_text(f"signature-{target}\n")

    def generate(self):
        return updater.generate_manifest(self.root, self.tag, "https://updates.example/updates/")

    def test_platforms_formats_and_signatures(self):
        manifest = self.generate()
        self.assertEqual(manifest["version"], "0.1.16")
        expected = {
            "windows-x86_64": ".exe",
            "darwin-x86_64": ".app.tar.gz",
            "darwin-aarch64": ".app.tar.gz",
        }
        self.assertEqual(set(manifest["platforms"]), set(expected))
        for target, extension in expected.items():
            item = manifest["platforms"][target]
            self.assertTrue(item["url"].startswith("https://updates.example/updates/vpn-gui-"))
            self.assertTrue(item["url"].endswith(extension))
            self.assertEqual(item["signature"], f"signature-{target}")

    def test_each_platform_requires_package_and_signature(self):
        for pattern in updater.ASSETS.values():
            asset = self.root / pattern.format(tag=self.tag)
            for path in [asset, Path(str(asset) + ".sig")]:
                original = path.read_bytes()
                for missing in [True, False]:
                    with self.subTest(path=path.name, missing=missing):
                        if missing:
                            path.unlink()
                        else:
                            path.write_bytes(b"")
                        with self.assertRaises(ValueError):
                            self.generate()
                        path.write_bytes(original)


if __name__ == "__main__":
    unittest.main()
