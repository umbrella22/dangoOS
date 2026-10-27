#!/usr/bin/env python3
"""Publish a tag only after downloading and verifying all draft Release assets."""
import argparse
import json
from pathlib import Path
import subprocess
import tempfile

from package import ROOT, require, verify


def gh(*args, check=True):
    result = subprocess.run(["gh", *map(str, args)], capture_output=True, text=True)
    if check and result.returncode:
        raise ValueError(f"gh {args[0]} failed: {result.stderr.strip()}")
    return result


def publish(directory, repository):
    manifest = verify(directory)
    tag, commit = manifest["tag"], manifest["source_commit"]
    ref = json.loads(gh("api", f"repos/{repository}/git/ref/tags/{tag}").stdout)["object"]
    if ref["type"] == "tag":
        ref = json.loads(gh("api", f"repos/{repository}/git/tags/{ref['sha']}").stdout)["object"]
    require(ref["type"] == "commit" and ref["sha"] == commit, "remote tag does not identify artifact source")
    notes = ROOT / "scripts/release/notes" / f"{tag}.md"
    require(notes.is_file(), "release notes missing for tag")
    # The REST by-tag endpoint excludes drafts. gh release view resolves both
    # drafts and published releases, including a draft left by an interrupted run.
    inspect = ("release", "view", tag, "--repo", repository, "--json", "isDraft,assets,url")
    existing = gh(*inspect, check=False)
    if existing.returncode:
        require("release not found" in existing.stderr.lower(), f"cannot inspect release: {existing.stderr}")
        gh("release", "create", tag, "--repo", repository, "--draft", "--verify-tag",
           "--title", f"dangoOS {tag}", "--notes-file", notes)
        release = json.loads(gh(*inspect).stdout)
    else:
        release = json.loads(existing.stdout)
    if release["isDraft"]:
        files = sorted(path for path in directory.iterdir() if path.is_file())
        expected = {path.name for path in files}
        require(all(asset["name"] in expected for asset in release["assets"]), "draft has unexpected assets")
        gh("release", "upload", tag, *files, "--repo", repository, "--clobber")
    with tempfile.TemporaryDirectory(prefix="dango-release-download-") as temporary:
        destination = Path(temporary)
        gh("release", "download", tag, "--repo", repository, "--dir", destination)
        downloaded = verify(destination)
        require(downloaded == manifest and
                (destination / "SHA256SUMS").read_bytes() == (directory / "SHA256SUMS").read_bytes(),
                "downloaded release differs from build artifacts")
    if release["isDraft"]:
        flags = ["--prerelease"] if "-" in tag else ["--latest"]
        gh("release", "edit", tag, "--repo", repository, "--draft=false", *flags)
    release = json.loads(gh(*inspect).stdout)
    require(not release["isDraft"], "release is still a draft")
    print(json.dumps({"status": "published-and-verified", "url": release["url"],
                      "tag": tag, "source_commit": commit, "assets": len(release["assets"])}))


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("directory", type=Path)
    parser.add_argument("--repo", required=True)
    args = parser.parse_args()
    try:
        publish(args.directory, args.repo)
    except (ValueError, OSError, subprocess.CalledProcessError) as error:
        parser.exit(2, f"publication rejected: {error}\n")
