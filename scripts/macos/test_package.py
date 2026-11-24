"""Packaging control-flow tests; mocked Apple services are not notarization evidence."""
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import unittest

SOURCE = Path(__file__).resolve().parent
SIGNING = {
    "LANLINK_MAC_APP_IDENTITY": "Developer ID Application: Test (TEST)",
    "LANLINK_MAC_INSTALLER_IDENTITY": "Developer ID Installer: Test (TEST)",
    "LANLINK_MAC_NOTARY_PROFILE": "test-profile",
    "LANLINK_MAC_REQUIRE_NOTARIZATION": "1",
}
STUB = '''#!/usr/bin/env python3
import json, os, pathlib, sys
name = pathlib.Path(sys.argv[0]).name
args = sys.argv[1:]
with open(os.environ['COMMAND_LOG'], 'a') as log:
    log.write(json.dumps([name, *args]) + '\\n')
if os.environ.get('FAIL_COMMAND') == name + ' ' + ' '.join(args[:2]):
    sys.exit(1)
if name == 'node': print('0.2.0')
if name == 'cargo' and '--manifest-path' in args:
    output=pathlib.Path('target/desktop-shell/aarch64-apple-darwin/release/lanlink-desktop-shell')
    output.parent.mkdir(parents=True,exist_ok=True)
    output.touch()
if name == 'xcrun' and args[0] == 'notarytool': print(json.dumps({'status': os.environ.get('NOTARY_STATUS', 'Accepted')}))
if name == 'xcrun' and args[0] == 'clang': pathlib.Path(args[-1]).touch()
if name in ('pkgbuild', 'ditto'): pathlib.Path(args[-1]).touch()
'''


class PackagingTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix="lanlink-package-")
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        shutil.copytree(SOURCE, self.root / "scripts/macos")
        for path in ("apps/web/dist", "assets/branding", "target/aarch64-apple-darwin/release", "bin"):
            (self.root / path).mkdir(parents=True)
        for path in ("LICENSE", "assets/branding/lanlink.icns", "target/aarch64-apple-darwin/release/lanlink-agent"):
            (self.root / path).touch()
        for name in ("node", "npm", "cargo", "xcrun", "codesign", "plutil", "pkgbuild", "ditto", "shasum", "spctl", "pkgutil"):
            stub = self.root / "bin" / name
            stub.write_text(STUB)
            stub.chmod(0o755)
        self.env = {key: value for key, value in os.environ.items() if not key.startswith("LANLINK_MAC_") and key != "LANLINK_RELEASE_VERSION"}
        self.env.update(PATH=str(self.root / "bin") + os.pathsep + os.environ["PATH"], COMMAND_LOG=str(self.root / "commands.jsonl"))

    def run_package(self, **env):
        result = subprocess.run(["/bin/bash", "scripts/macos/package.sh"], cwd=self.root, env={**self.env, **env}, capture_output=True, text=True)
        calls = [json.loads(line) for line in (self.root / "commands.jsonl").read_text().splitlines()]
        return result, calls

    def test_development_package_keeps_layout_without_notary(self):
        result, calls = self.run_package()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertFalse(any("notarytool" in call for call in calls))
        app = self.root / "dist/macos-aarch64-apple-darwin/LANLink.app"
        helper = app / "Contents/Helpers/LANLink Workbench.app"
        self.assertTrue((helper / "Contents/MacOS/lanlink-desktop-shell").is_file())
        self.assertTrue((helper / "Contents/Resources/LANLink.icns").is_file())
        self.assertTrue((self.root / "dist/macos-aarch64-apple-darwin/LANLink.app/Contents/Resources/launch.sh").is_file())
        self.assertEqual([c for c in calls if c[0] == "ditto"][-1][-2:], ["dist/macos-aarch64-apple-darwin", "dist/LANLink-0.2.0-aarch64-apple-darwin.zip"])

    def test_preview_carries_full_agent_version_without_changing_bundle_version(self):
        result, _ = self.run_package(LANLINK_RELEASE_VERSION="0.2.0-preview.6")
        self.assertEqual(result.returncode, 0, result.stderr)
        plist = (self.root / "dist/macos-aarch64-apple-darwin/LANLink.app/Contents/Info.plist").read_text()
        self.assertIn("<key>LANLinkAgentVersion</key><string>0.2.0-preview.6</string>", plist)
        self.assertIn("<key>CFBundleShortVersionString</key><string>0.2.0</string>", plist)

    def test_partial_or_required_signing_fails_before_build(self):
        for config in ({"LANLINK_MAC_REQUIRE_NOTARIZATION": "1"}, {"LANLINK_MAC_APP_IDENTITY": SIGNING["LANLINK_MAC_APP_IDENTITY"]}, {**SIGNING, "LANLINK_MAC_APP_IDENTITY": "-"}):
            with self.subTest(config=config):
                result, calls = self.run_package(**config)
                self.assertNotEqual(result.returncode, 0)
                self.assertFalse(any(call[0] == "npm" for call in calls))

    def test_signed_release_staples_before_archiving(self):
        result, calls = self.run_package(**SIGNING)
        self.assertEqual(result.returncode, 0, result.stderr)
        submits = [c for c in calls if c[:3] == ["xcrun", "notarytool", "submit"]]
        self.assertEqual(len(submits), 2)
        self.assertTrue(all("--wait" in c and "test-profile" in c for c in submits))
        staples = [i for i, c in enumerate(calls) if c[:3] == ["xcrun", "stapler", "staple"]]
        package = next(i for i, c in enumerate(calls) if c[0] == "pkgbuild")
        archive = max(i for i, c in enumerate(calls) if c[0] == "ditto")
        self.assertLess(staples[0], package)
        self.assertLess(package, staples[1])
        self.assertLess(staples[1], archive)
        self.assertTrue(all("runtime" in c and "--timestamp" in c for c in calls if c[:2] == ["codesign", "--force"]))
        self.assertIn(SIGNING["LANLINK_MAC_INSTALLER_IDENTITY"], calls[package])
        seals = [c[-1] for c in calls if c[:2] == ["codesign", "--force"]]
        self.assertEqual(seals[-2:], [
            "dist/macos-aarch64-apple-darwin/LANLink.app/Contents/Helpers/LANLink Workbench.app",
            "dist/macos-aarch64-apple-darwin/LANLink.app",
        ])

    def test_nonaccepted_notarization_prevents_final_archive(self):
        result, calls = self.run_package(**SIGNING, NOTARY_STATUS="Invalid")
        self.assertNotEqual(result.returncode, 0)
        self.assertFalse(any(c[0] == "pkgbuild" for c in calls))

    def test_notarization_failure_prevents_final_archive(self):
        result, calls = self.run_package(**SIGNING, FAIL_COMMAND="xcrun notarytool submit")
        self.assertNotEqual(result.returncode, 0)
        self.assertFalse(any(c[0] == "pkgbuild" for c in calls))
        self.assertFalse(any(c[0] == "ditto" and c[-1].startswith("dist/LANLink-") for c in calls))


@unittest.skipUnless(sys.platform == "darwin", "native macOS launcher")
class NativeLauncherTests(unittest.TestCase):
    def test_native_launcher_resolves_spaced_bundle_and_propagates_exit(self):
        with tempfile.TemporaryDirectory(prefix="LANLink launcher ") as directory:
            contents = Path(directory) / "LANLink.app/Contents"
            (contents / "MacOS").mkdir(parents=True)
            (contents / "Resources").mkdir()
            executable = contents / "MacOS/LANLink"
            subprocess.run(["xcrun", "clang", "-Wall", "-Wextra", "-Werror", str(SOURCE / "launcher.c"), "-o", str(executable)], check=True)
            script = contents / "Resources/launch.sh"
            script.write_text('#!/bin/bash\nprintf "%s" "$0"\nexit 23\n')
            result = subprocess.run([str(executable)], cwd="/", capture_output=True, text=True)
            self.assertEqual(result.returncode, 23)
            self.assertEqual(result.stdout, str(script.resolve()))
            script.write_text('#!/bin/bash\nprintf "%s" "${1:-native}"\n')
            normal = subprocess.run([str(executable)], cwd="/", capture_output=True, text=True, check=True)
            fallback = subprocess.run([str(executable), "--browser"], cwd="/", capture_output=True, text=True, check=True)
            self.assertEqual(normal.stdout, "native")
            self.assertEqual(fallback.stdout, "--browser")


if __name__ == "__main__":
    unittest.main()
