#!/usr/bin/env python3
"""Push a Lua app to a Mosaico S31 over the USB console, or control apps.

    scripts/push-app.py push <app-dir> [--run] [--follow]
    scripts/push-app.py run <id> | stop | reload | ls | rm <id> [--follow]
    scripts/push-app.py status | reboot [--follow]
    scripts/push-app.py skin-push <skin-dir> [--select] [--follow]
    scripts/push-app.py skin-select <id> | skin-rm <id> | skin-ls

<app-dir> holds launcher.json and the files it names; everything except
dotfiles is sent. The firmware stages the files, swaps them in as
/data/apps/<id> and rescans the launcher; an app that is on screen restarts
with its new files (hot reload). --follow keeps printing the device log.

Needs pyserial (the esptool Python has it). The port is the console port,
so close any other monitor first.
"""

import argparse
import glob
import json
import os
import re
import sys
import time
import zlib

try:
    import serial
except ImportError:
    sys.exit("pyserial is required: pip install pyserial")

PREFIX = "@mosaico "
SAFE_PART = re.compile(r"^[A-Za-z0-9_-][A-Za-z0-9._-]{0,47}$")
ID_RE = re.compile(r"^[A-Za-z0-9_-]{1,63}$")
FILE_MAX = 1024 * 1024


class Device:
    def __init__(self, port, echo):
        self.ser = serial.Serial(port, 115200, timeout=0.2)
        self.ser.dtr = True
        self.echo = echo
        self.buf = b""

    def lines(self, deadline):
        while time.monotonic() < deadline:
            chunk = self.ser.read(4096)
            if chunk:
                self.buf += chunk
            while b"\n" in self.buf:
                line, self.buf = self.buf.split(b"\n", 1)
                yield line.decode("utf-8", "replace").rstrip("\r")

    def reply(self, timeout=20.0):
        for line in self.lines(time.monotonic() + timeout):
            if line.startswith("@ok") or line.startswith("@err"):
                return line
            if self.echo:
                print(line)
        raise SystemExit("no reply from the device (is the port right, is the firmware new enough?)")

    def command(self, text, timeout=20.0, check=True):
        self.ser.write((PREFIX + text + "\n").encode())
        line = self.reply(timeout)
        if check and not line.startswith("@ok"):
            raise SystemExit(f"{text.split()[0]}: {line}")
        return line

    def follow(self):
        try:
            for line in self.lines(float("inf")):
                print(line, flush=True)
        except KeyboardInterrupt:
            pass


def default_port():
    ports = sorted(glob.glob("/dev/cu.usbmodem*")) + sorted(glob.glob("/dev/ttyACM*"))
    if not ports:
        sys.exit("no USB console port found; pass --port")
    return ports[0]


def app_files(root):
    files = []
    for dirpath, dirnames, filenames in os.walk(root):
        dirnames[:] = sorted(d for d in dirnames if not d.startswith("."))
        for name in sorted(filenames):
            if name.startswith("."):
                continue
            path = os.path.join(dirpath, name)
            rel = os.path.relpath(path, root).replace(os.sep, "/")
            parts = rel.split("/")
            if len(parts) > 6 or not all(SAFE_PART.match(p) for p in parts):
                sys.exit(f"{rel}: path not allowed (use [A-Za-z0-9._-], at most 6 levels)")
            size = os.path.getsize(path)
            if size > FILE_MAX:
                sys.exit(f"{rel}: larger than {FILE_MAX} bytes")
            files.append((rel, path))
    return files


def push(dev, root, run):
    with open(os.path.join(root, "launcher.json"), "rb") as f:
        manifest = json.load(f)
    app_id = manifest.get("id") or os.path.basename(os.path.abspath(root))
    if not ID_RE.match(app_id):
        sys.exit(f"bad app id {app_id!r}")
    files = app_files(root)
    dev.command("hello")
    dev.command(f"begin {app_id}")
    total = 0
    started = time.monotonic()
    try:
        for rel, path in files:
            with open(path, "rb") as f:
                data = f.read()
            dev.command(f"file {rel} {len(data)} {zlib.crc32(data):08x}")
            if data:
                dev.ser.write(data)
                line = dev.reply(timeout=30.0)
                if not line.startswith("@ok"):
                    raise SystemExit(f"{rel}: {line}")
            total += len(data)
            print(f"  {rel} ({len(data)} B)")
        dev.command(f"commit {app_id}" + (" run" if run else ""), timeout=30.0)
    except SystemExit:
        dev.command("abort", check=False)
        raise
    print(f"pushed {app_id}: {len(files)} files, {total} B in {time.monotonic() - started:.1f} s")


def push_skin(dev, root, select):
    with open(os.path.join(root, "skin.json"), "rb") as f:
        manifest = json.load(f)
    skin_id = manifest.get("id")
    if not isinstance(skin_id, str) or not ID_RE.fullmatch(skin_id) or skin_id == "default":
        sys.exit(f"bad skin id {skin_id!r}")
    files = app_files(root)
    if {rel for rel, _ in files} != {"skin.json", "fox.bin"}:
        sys.exit("skin directory must contain exactly skin.json and fox.bin")
    dev.command("hello")
    dev.command(f"skin-begin {skin_id}")
    try:
        for rel, path in files:
            with open(path, "rb") as f:
                data = f.read()
            dev.command(f"file {rel} {len(data)} {zlib.crc32(data):08x}")
            if data:
                dev.ser.write(data)
                line = dev.reply(timeout=30.0)
                if not line.startswith("@ok"):
                    raise SystemExit(f"{rel}: {line}")
            print(f"  {rel} ({len(data)} B)")
        dev.command(f"skin-commit {skin_id}", timeout=30.0)
    except SystemExit:
        dev.command("abort", check=False)
        raise
    if select:
        dev.command(f"skin-select {skin_id}", timeout=30.0)
    print(f"pushed skin {skin_id}" + (" and selected" if select else ""))


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("action", choices=["push", "run", "stop", "reload", "ls", "rm", "status", "reboot", "skin-push", "skin-select", "skin-rm", "skin-ls"])
    ap.add_argument("target", nargs="?", help="app directory (push) or app id (run, rm)")
    ap.add_argument("--select", action="store_true", help="select after skin-push")
    ap.add_argument("--run", action="store_true", help="open the app after pushing")
    ap.add_argument("--follow", action="store_true", help="keep printing the device log")
    ap.add_argument("--port", help="console port (default: the first /dev/cu.usbmodem*)")
    ap.add_argument("--verbose", "-v", action="store_true", help="print device log lines while waiting")
    args = ap.parse_args()

    needs_target = args.action in ("push", "run", "rm", "skin-push", "skin-select", "skin-rm")
    if needs_target != bool(args.target):
        ap.error(f"{args.action} {'needs' if needs_target else 'takes no'} target")

    if args.select and args.action != "skin-push":
        ap.error("--select requires skin-push")
    if args.action in ("skin-select", "skin-rm") and not ID_RE.fullmatch(args.target):
        ap.error("invalid exact skin id")
    dev = Device(args.port or default_port(), args.verbose)
    if args.action == "skin-push":
        push_skin(dev, args.target, args.select)
    elif args.action in ("skin-select", "skin-rm"):
        print(dev.command(f"{args.action} {args.target}", timeout=30.0))
    elif args.action == "skin-ls":
        print(dev.command("skin-ls"))
    elif args.action == "push":
        push(dev, args.target, args.run)
    elif args.action == "ls":
        print(dev.command("ls")[len("@ok ls"):].strip() or "(no apps in /data/apps)")
    elif args.action == "status":
        print(dev.command("status")[len("@ok status"):].strip())
    elif args.action in ("run", "rm"):
        dev.command(f"{args.action} {args.target}")
    elif args.action == "reboot":
        # The device drops off USB as it resets, often before the reply.
        try:
            dev.command("reboot", timeout=3.0)
        except (serial.SerialException, OSError, SystemExit):
            pass
        print("rebooting")
        return
    else:
        dev.command(args.action)
    if args.follow:
        dev.follow()


if __name__ == "__main__":
    main()
