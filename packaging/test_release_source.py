import importlib.util
import json
from pathlib import Path
import subprocess
import tempfile
import unittest

spec = importlib.util.spec_from_file_location('release_source', Path(__file__).with_name('check-release-source.py'))
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)

class ReleaseSourceTest(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.root = Path(self.tmp.name)
        self.git('init', '-q')
        self.git('config', 'user.email', 'test@example.invalid')
        self.git('config', 'user.name', 'Test')
        (self.root / 'desktop/src-tauri').mkdir(parents=True)
        (self.root / 'Cargo.toml').write_text('[workspace.package]\nversion="1.2.3"\n')
        (self.root / 'desktop/src-tauri/Cargo.toml').write_text('[package]\nversion="1.2.3"\n')
        for file in ['desktop/package.json', 'desktop/src-tauri/tauri.conf.json']:
            (self.root / file).write_text(json.dumps({'version': '1.2.3'}))
        self.git('add', '.')
        self.git('commit', '-qm', 'merged feature')
        self.git('update-ref', 'refs/remotes/origin/main', 'HEAD')
        self.git('tag', 'v1.2.3')

    def git(self, *args):
        return subprocess.run(['git', '-C', str(self.root), *args], check=True, capture_output=True)

    def test_merged_tag_and_main_validation_pass(self):
        module.validate(self.root, 'refs/tags/v1.2.3')
        module.validate(self.root, 'refs/heads/main')

    def test_unmerged_feature_is_rejected(self):
        self.git('commit', '--allow-empty', '-qm', 'unmerged feature')
        with self.assertRaises(subprocess.CalledProcessError):
            module.validate(self.root, 'refs/tags/v1.2.3')

    def test_other_branch_and_wrong_version_are_rejected(self):
        for ref in ['refs/heads/feature', 'refs/tags/v1.2.4']:
            with self.assertRaises(ValueError): module.validate(self.root, ref)
        (self.root / 'desktop/package.json').write_text('{"version":"1.2.2"}')
        with self.assertRaises(ValueError): module.validate(self.root, 'refs/tags/v1.2.3')

    def test_tag_cannot_point_to_another_commit(self):
        self.git('commit', '--allow-empty', '-qm', 'later merged feature')
        self.git('update-ref', 'refs/remotes/origin/main', 'HEAD')
        with self.assertRaises(ValueError): module.validate(self.root, 'refs/tags/v1.2.3')

if __name__ == '__main__': unittest.main()
