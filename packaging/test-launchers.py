"""Exercise the native package's launcher install using a release payload.

Reads native-packages.yaml without compiling or launching the app. Ruby
provides the existing YAML parser; no Python packages.
"""
import json
from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parent.parent


def yaml(path):
    return json.loads(subprocess.check_output([
        "ruby", "-rjson", "-ryaml", "-e",
        "puts JSON.generate(YAML.safe_load_file(ARGV.fetch(0), aliases: false))", str(path),
    ], text=True))


def payload(root):
    (root / "packaging/applications").mkdir(parents=True)
    (root / "packaging/icons").mkdir(parents=True)
    shutil.copyfile(ROOT / "packaging/applications/spotifast.desktop",
                    root / "packaging/applications/spotifast.desktop")
    shutil.copyfile(ROOT / "packaging/icons/spotifast.svg", root / "packaging/icons/spotifast.svg")
    (root / "target/release").mkdir(parents=True)
    for name in ["spotifast", "target/release/spotifast"]:
        shutil.copyfile(shutil.which("true"), root / name)
    for name in ["LICENSE", "README.md"]:
        shutil.copyfile(ROOT / name, root / name)
    shutil.copytree(ROOT / "contrib/omarchy", root / "contrib/omarchy")


class LauncherInstallTest(unittest.TestCase):
    def test_native_manifest_installs_the_launcher_and_icon(self):
        config = yaml(ROOT / "native-packages.yaml")
        entries = [c for c in config["nfpm"]["contents"] if "/packaging/" in c["src"]]
        self.assertEqual(len(entries), 2)
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            payload(root / "spotifast-v9.8.7-linux-amd64")
            for entry in entries:
                pattern = entry["src"].replace("@PAYLOAD@/", "").replace("@VERSION@", "9.8.7").replace("@TARGET@", "linux-amd64")
                files = list(root.glob(pattern))
                self.assertEqual(len(files), 1)
                self.assertEqual(files[0].stem, "spotifast")
                self.assertTrue(entry["dst"].endswith("/"))


if __name__ == "__main__":
    unittest.main()
