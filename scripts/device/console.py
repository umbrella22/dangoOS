#!/usr/bin/env python3
"""Push a Lua app to a Mosaico S31 over the USB console, or control apps.

    scripts/device/console.py bundle-push <bundle-dir> [--run]
    scripts/device/console.py fonts-push <bundle-dir> | fonts-status
    scripts/device/console.py push <app-dir> [--run] [--follow]
    scripts/device/console.py run <id> | stop | reload | ls | rm <id> | rmdata <id> [--follow]
    scripts/device/console.py status | reboot [--follow]
    scripts/device/console.py skin-push <skin-dir> [--select] [--follow]
    scripts/device/console.py skin-select <id> | skin-rm <id> | skin-ls
    scripts/device/console.py brain-push <script.lua> | brain-reload | brain-default | brain-status
    scripts/device/console.py photo-push <image> [--name subdir/name.jpg] [--run]
    scripts/device/console.py recovery | boot-status
    scripts/device/console.py update-status | update-check | update-install | update-cancel
    scripts/device/console.py update-url <https-manifest-url>  # explicit development install
    scripts/device/console.py nand-status | nand-format
    scripts/device/console.py wifi-status | wifi-on | wifi-scan | wifi-disconnect | wifi-off
    scripts/device/console.py wifi-connect <SSID>  # WPA2, local password prompt
    scripts/device/console.py bt-status | bt-on | bt-off
    scripts/device/console.py audio-status | audio-stop
    scripts/device/console.py audio-volume <0..100> | audio-mute <0|1>
    scripts/device/console.py audio-play </data/file.wav>
    scripts/device/console.py audio-record </data/file.wav> [--duration-ms 1000]
    scripts/device/console.py voice-config-push <private-config.json>
    scripts/device/console.py voice-font-push <voice-20.bin>
    scripts/device/console.py voice-status | voice-toggle | voice-cancel | voice-reload
    scripts/device/console.py voice-ask <text> | voice-read
    scripts/device/console.py screen-off <seconds:1..60>

<app-dir> holds launcher.json and the files it names; everything except
dotfiles is sent. The firmware stages the files, swaps them in as
/data/apps/<id> and rescans the launcher; an app that is on screen restarts
with its new files (hot reload). --follow keeps printing the device log.

Needs pyserial (the esptool Python has it). The port is the console port,
so close any other monitor first.
"""

import argparse
import glob
import getpass
import ipaddress
import json
import os
import re
import sys
import time
import zlib
from pathlib import Path
from urllib.parse import urlsplit

try:
    import serial
except ImportError:
    sys.exit("pyserial is required: pip install pyserial")

PREFIX = "@mosaico "
SAFE_PART = re.compile(r"^[A-Za-z0-9_-][A-Za-z0-9._-]{0,47}$")
ID_RE = re.compile(r"^[A-Za-z0-9_-]{1,63}$")
FILE_MAX = 1024 * 1024
BRAIN_MAX = 64 * 1024
CONSOLE_LINE_MAX = 320
VOICE_TEXT_BYTES = 1024


class ConsoleSerial(serial.Serial):
    """Preserve pending console input during pyserial's POSIX port open."""

    def open(self):
        self._preserve_open_input = True
        try:
            super().open()
        finally:
            self._preserve_open_input = False

    def _reset_input_buffer(self):
        if not self._preserve_open_input:
            super()._reset_input_buffer()


class Device:
    def __init__(self, port, echo):
        self.ser = ConsoleSerial(port=None, baudrate=115200, timeout=0.2,
                                 write_timeout=20.0, exclusive=True)
        self.ser.dtr = False
        self.ser.port = port
        try:
            self.ser.open()
            self.ser.dtr = True
        except BaseException:
            self.ser.close()
            raise
        self.echo = echo
        self.buf = b""

    def lines(self, deadline):
        while time.monotonic() < deadline:
            while b"\n" in self.buf:
                line, self.buf = self.buf.split(b"\n", 1)
                yield line.decode("utf-8", "replace").rstrip("\r")
            # Return available bytes promptly. A large blocking read can collect
            # the final reboot ACK, then lose it when reset interrupts the remainder.
            chunk = self.ser.read(min(4096, self.ser.in_waiting or 1))
            if chunk:
                self.buf += chunk

    def reply(self, timeout=20.0):
        for line in self.lines(time.monotonic() + timeout):
            if line.startswith("@ok") or line.startswith("@err"):
                return line
            if self.echo or line.startswith(("@wifi-ap ", "@voice-heap ", "@voice-error ", "@voice-record ")):
                print(line)
        raise SystemExit("no reply from the device (is the port right, is the firmware new enough?)")

    def command(self, text, timeout=20.0, check=True):
        self.write_all((PREFIX + text + "\n").encode())
        line = self.reply(timeout)
        if check and not line.startswith("@ok"):
            raise SystemExit(f"{text.split()[0]}: {line}")
        return line

    def write_all(self, data):
        offset = 0
        while offset < len(data):
            # NAND file writes apply USB backpressure. Bound each OS write so
            # its timeout measures one chunk rather than an entire skin.
            chunk = data[offset:offset + 4096]
            written = self.ser.write(chunk)
            if not isinstance(written, int) or not 0 < written <= len(chunk):
                raise OSError("serial write made no progress")
            offset += written

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


def voice_ask_command(text):
    data = text.encode("utf-8")
    if not text.strip() or len(data) > VOICE_TEXT_BYTES or "\0" in text:
        raise ValueError("voice text must be nonempty, at most 1024 UTF-8 bytes and contain no NUL")
    command = "voice-ask " + data.hex()
    if len((PREFIX + command).encode("ascii")) > CONSOLE_LINE_MAX:
        raise ValueError("voice text exceeds the console command capacity after hex encoding")
    return command


def read_voice_reply(dev):
    data = bytearray()
    identity = None
    while True:
        line = dev.command(f"voice-read {len(data)}")
        match = re.fullmatch(r"@ok voice-read session=([0-9]+) offset=([0-9]+) total=([0-9]+) hex=([A-Fa-f0-9]*)", line)
        if not match:
            raise SystemExit("invalid voice-read response")
        session, offset, total = map(int, match.group(1, 2, 3))
        if offset != len(data) or (identity is not None and identity != (session, total)):
            raise SystemExit("voice reply changed or returned an invalid offset/length")
        identity = (session, total)
        try:
            chunk = bytes.fromhex(match[4])
        except ValueError:
            raise SystemExit("invalid voice reply hex") from None
        if len(chunk) > 48 or len(data) + len(chunk) > total or (not chunk and len(data) < total):
            raise SystemExit("invalid voice reply chunk length or no progress")
        data.extend(chunk)
        if len(data) == total:
            try:
                return data.decode("utf-8")
            except UnicodeDecodeError:
                raise SystemExit("invalid voice reply UTF-8") from None


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
                dev.write_all(data)
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
    if not isinstance(skin_id, str) or not ID_RE.fullmatch(skin_id) or skin_id == "placeholder":
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
                dev.write_all(data)
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


def push_bundle(dev, root, run=False):
    import resource_bundle
    resource_bundle.application(root)
    push(dev, str(Path(root) / "payload"), run)


def push_fonts(dev, root):
    import resource_bundle
    report = resource_bundle.fonts(root)
    data = (Path(root) / report["file"]).read_bytes()
    status = response_fields(dev.command("fonts-status"))
    if status.get("expected") != report["id"]:
        raise ValueError("UI font bundle does not match the running firmware; update firmware first")
    dev.command("fonts-begin")
    try:
        dev.command(f"file fonts.bin {len(data)} {zlib.crc32(data):08x}")
        dev.write_all(data)
        reply = dev.reply(timeout=60)
        if not reply.startswith("@ok"):
            raise SystemExit(f"fonts: {reply}")
        dev.command("fonts-commit", timeout=60)
    except (SystemExit, OSError):
        dev.command("abort", check=False)
        raise
    print("UI fonts installed. Restart the device to activate them.")


def read_voice_resource(path, font=False):
    limit = 5 * 1024 * 1024 if font else 8192
    with open(path, "rb") as source:
        data = source.read(limit + 1)
    if not data or len(data) > limit:
        raise ValueError("voice resource is empty or exceeds its size limit")
    if font:
        if len(data) < 16 or data[:6] != b"MFNT\x01\x04":
            raise ValueError("voice font requires the generated MFNT resource")
    else:
        # Do not include JSON errors or field values in diagnostics: this file
        # contains credentials. Firmware validates the complete configuration.
        try:
            config = json.loads(data)
            if config.get("schema") != 1:
                raise ValueError()
            for name, scheme in [("asr", "wss"), ("chat", "https")]:
                endpoint = config[name]
                url = urlsplit(endpoint["url"])
                if url.scheme != scheme or not url.hostname or url.username is not None or url.fragment:
                    raise ValueError()
                if not all(isinstance(endpoint[key], str) and endpoint[key] for key in ("api_key", "model")):
                    raise ValueError()
        except (ValueError, TypeError, KeyError, AttributeError, UnicodeError):
            raise ValueError("invalid voice configuration; check schema, encrypted URLs, keys and models") from None
    return data


def push_voice_resource(dev, data, font=False):
    kind = "font" if font else "config"
    filename = "font.bin" if font else "config.json"
    dev.command(f"voice-{kind}-begin")
    try:
        dev.command(f"file {filename} {len(data)} {zlib.crc32(data):08x}")
        dev.write_all(data)
        reply = dev.reply(timeout=120)
        if not reply.startswith("@ok"):
            raise SystemExit(f"voice resource upload failed: {reply}")
        print(dev.command(f"voice-{kind}-commit", timeout=120))
    except (SystemExit, OSError):
        dev.command("abort", check=False)
        raise
    if not font:
        print(dev.command("voice-reload", check=False))


def read_brain(path):
    with open(path, "rb") as f:
        data = f.read(BRAIN_MAX + 1)
    if not 0 < len(data) <= BRAIN_MAX:
        raise SystemExit("brain script must contain 1..65536 bytes")
    if str(path).endswith(".etb"):
        data = b"etb1:" + data.hex().encode("ascii")
        if len(data) > BRAIN_MAX:
            raise SystemExit("brain bytecode envelope exceeds 64 KiB")
    try:
        data.decode("utf-8")
    except UnicodeDecodeError:
        raise SystemExit("brain script must be UTF-8") from None
    return data


def push_brain(dev, data):
    dev.command("hello")
    dev.command("brain-begin")
    filename = "source.etb.hex" if data.startswith(b"etb1:") else "source.lua"
    try:
        dev.command(f"file {filename} {len(data)} {zlib.crc32(data):08x}")
        dev.ser.write(data)
        line = dev.reply(timeout=30.0)
        if not line.startswith(f"@ok file {filename}"):
            raise SystemExit(f"brain upload: {line}")
        # Firmware replies only after candidate init, first tick, durable write,
        # and the UI-owned VM swap. A syntax/timeout/write error keeps the old VM.
        line = dev.command("brain-commit", timeout=30.0)
    except (SystemExit, OSError, serial.SerialException):
        try:
            dev.command("abort", check=False, timeout=2.0)
        except (SystemExit, OSError, serial.SerialException):
            pass
        raise
    print(f"{line}; {len(data)} B crc={zlib.crc32(data):08x}")


def read_photo(path, name=None):
    rel = name if name is not None else os.path.basename(path)
    parts = rel.split("/")
    if len(parts) > 6 or not all(SAFE_PART.fullmatch(p) for p in parts):
        raise SystemExit("photo name must use [A-Za-z0-9._-], at most 6 levels")
    if os.path.splitext(rel)[1].lower() not in (".jpg", ".jpeg", ".png", ".bin"):
        raise SystemExit("photo must be JPG, JPEG, PNG or LVGL BIN")
    with open(path, "rb") as f:
        data = f.read(FILE_MAX + 1)
    if not 0 < len(data) <= FILE_MAX:
        raise SystemExit(f"photo must contain 1..{FILE_MAX} bytes")
    return rel, data


def push_photo(dev, rel, data, run=False):
    dev.command("hello")
    dev.command("photo-begin")
    command_mode = True
    try:
        # A missing ready/file receipt leaves the parser's binary-body state
        # unknown. Text cleanup could then become part of the photo payload.
        command_mode = False
        line = dev.command(f"file {rel} {len(data)} {zlib.crc32(data):08x}", check=False)
        if line != f"@ok ready {rel}":
            command_mode = line.startswith("@err ")
            raise SystemExit(f"photo ready: {line}")
        dev.write_all(data)
        line = dev.reply(timeout=30.0)
        command_mode = line == f"@ok file {rel}" or line.startswith(f"@err file {rel}: ")
        if line != f"@ok file {rel}":
            raise SystemExit(f"photo upload: {line}")
        line = dev.command(f"photo-commit {rel}", timeout=30.0)
        if line != f"@ok photo-commit {rel}":
            raise SystemExit(f"photo commit: {line}")
    except (SystemExit, OSError, serial.SerialException):
        if command_mode:
            try:
                dev.command("abort", check=False, timeout=2.0)
            except (SystemExit, OSError, serial.SerialException):
                pass
        raise
    print(f"{line}; {len(data)} B -> /data/appdata/photo/photos/{rel}")
    if run:
        dev.command("run photo")


def response_fields(text):
    return dict(word.split("=", 1) for word in text.split() if "=" in word)


def screen_off(dev, seconds):
    """Keep CDC connected for the bounded lease and always request restoration."""
    try:
        print(dev.command(f"screen-off {seconds}", timeout=10.0))
        deadline = time.monotonic() + seconds + 3.0
        last_state = None
        while time.monotonic() < deadline:
            status = dev.command("power-status")
            state = response_fields(status).get("state")
            if state != last_state:
                print(status)
                last_state = state
            if state == "active":
                return
            if state == "fault":
                raise SystemExit("Panel restore failed; inspect power-status and retry screen-on.")
            time.sleep(0.2)
        raise SystemExit("Screen standby lease exceeded; requesting screen-on.")
    finally:
        # Also runs on Ctrl-C. The firmware separately restores on disconnect.
        print(dev.command("screen-on", timeout=10.0))




def wifi_connect_command(ssid, password):
    """Encode credentials without placing the password in argv or error text."""
    network = ssid.encode("utf-8")
    key = password.encode("utf-8")
    if not 1 <= len(network) <= 32:
        raise ValueError("SSID requires 1..32 UTF-8 bytes")
    if not 8 <= len(key) <= 63 or any(not 32 <= byte <= 126 for byte in key):
        raise ValueError("WPA2 password requires 8..63 printable ASCII bytes")
    return f"wifi-connect {network.hex()} {key.hex()}"


def update_url_command(value):
    if not value.startswith("https://") or any(ord(c) <= 32 or ord(c) >= 127 for c in value) or any(c in value for c in "#\\"):
        raise ValueError("update-url requires an HTTPS URL without credentials or control characters")
    url = urlsplit(value)
    if url.username is not None or url.password is not None or not url.hostname or len(url.hostname) > 253 or not re.fullmatch(r"[A-Za-z0-9.-]+", url.hostname) or url.port == 0:
        raise ValueError("invalid update host or port")
    command = f"update-url {value}"
    if len((PREFIX + command).encode()) > 320:
        raise ValueError("update URL exceeds the console command capacity")
    return command


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    camera = ("camera-status", "camera-start", "camera-stop", "camera-capture")
    audio = ("audio-status", "audio-stop", "audio-play", "audio-record", "audio-volume", "audio-mute")
    voice = ("voice-status", "voice-toggle", "voice-cancel", "voice-reload", "voice-config-push", "voice-font-push", "voice-ask", "voice-read", "voice-page")
    wireless = ("wifi-status", "wifi-on", "wifi-scan", "wifi-connect", "wifi-disconnect", "wifi-off", "wifi-saved", "wifi-saved-connect", "wifi-forget", "wifi-auto", "bt-status", "bt-on", "bt-off")
    updates = ("update-status", "update-check", "update-install", "update-cancel", "update-url")
    updates += ("update-proxy",)
    ap.add_argument("action", choices=["bundle-push", "fonts-push", "fonts-status", "push", "run", "stop", "reload", "ls", "rm", "rmdata", "status", "reboot", "skin-push", "skin-select", "skin-rm", "skin-ls", "brain-push", "brain-reload", "brain-default", "brain-status", "photo-push", "recovery", "boot-status", "nand-info", "nand-status", "nand-format", "power-status", "screen-off", "screen-on", *wireless, *camera, *updates, *audio, *voice])
    ap.add_argument("target", nargs="?", help="app/skin directory, app/skin id, brain script or photo file")
    ap.add_argument("--name", help="photo path relative to Album photos (photo-push only)")
    ap.add_argument("--select", action="store_true", help="select after skin-push")
    ap.add_argument("--run", action="store_true", help="open the app after pushing")
    ap.add_argument("--follow", action="store_true", help="keep printing the device log")
    ap.add_argument("--port", help="console port (default: the first /dev/cu.usbmodem*)")
    ap.add_argument("--verbose", "-v", action="store_true", help="print device log lines while waiting")
    ap.add_argument("--duration-ms", type=int, default=1000, help="audio-record duration in milliseconds")
    args = ap.parse_args()

    needs_target = args.action in ("bundle-push", "fonts-push", "push", "run", "rm", "rmdata", "skin-push", "skin-select", "skin-rm", "brain-push", "photo-push", "screen-off", "wifi-connect", "wifi-saved-connect", "wifi-forget", "wifi-auto", "update-url", "update-proxy", "audio-play", "audio-record", "audio-volume", "audio-mute", "voice-config-push", "voice-font-push", "voice-ask", "voice-page")
    if needs_target != bool(args.target):
        ap.error(f"{args.action} {'needs' if needs_target else 'takes no'} target")
    if args.action == "screen-off" and (not re.fullmatch(r"[0-9]+", args.target) or not 1 <= int(args.target) <= 60):
        ap.error("screen-off requires 1..60 seconds")
    if args.select and args.action != "skin-push":
        ap.error("--select requires skin-push")
    if args.action in ("skin-select", "skin-rm") and not ID_RE.fullmatch(args.target):
        ap.error("invalid exact skin id")
    if args.name is not None and args.action != "photo-push":
        ap.error("--name requires photo-push")
    wifi_command = None
    if args.action == "wifi-connect":
        try:
            wifi_command = wifi_connect_command(args.target, getpass.getpass("WPA2 password (saved on device after successful connection): "))
        except ValueError as error:
            ap.error(str(error))
    elif args.action in ("wifi-saved-connect", "wifi-forget"):
        network = args.target.encode("utf-8")
        if not 1 <= len(network) <= 32:
            ap.error("SSID requires 1..32 UTF-8 bytes")
        wifi_command = f"{args.action} {network.hex()}"
    elif args.action == "wifi-auto":
        if args.target not in ("on", "off"):
            ap.error("wifi-auto requires on or off")
        wifi_command = f"wifi-auto {args.target}"
    if args.action in ("bundle-push", "fonts-push"):
        import resource_bundle
        try:
            (resource_bundle.application if args.action == "bundle-push" else resource_bundle.fonts)(args.target)
        except (ValueError, KeyError, TypeError, OSError) as error:
            ap.error(str(error))
    brain_data = read_brain(args.target) if args.action == "brain-push" else None
    voice_command = args.action
    if args.action == "voice-page" and (not re.fullmatch(r"[0-9]+",args.target) or int(args.target)>0xffffffff):
        ap.error("voice-page requires a NAND byte offset")
    if args.action == "voice-ask":
        try:
            voice_command = voice_ask_command(args.target)
        except ValueError as error:
            ap.error(str(error))
    voice_data = None
    if args.action in ("voice-config-push", "voice-font-push"):
        try:
            voice_data = read_voice_resource(args.target, args.action == "voice-font-push")
        except (ValueError, OSError) as error:
            ap.error(str(error))
    photo = read_photo(args.target, args.name) if args.action == "photo-push" else None
    update_command = args.action
    if args.action == "update-url":
        try:
            update_command = update_url_command(args.target)
        except ValueError as error:
            ap.error(str(error))
    if args.action == "update-proxy":
        try:
            if args.target != "off":
                host, port = args.target.rsplit(":", 1)
                address = ipaddress.IPv4Address(host)
                if address.is_unspecified or address.is_multicast or host == "255.255.255.255" or not port.isascii() or not port.isdigit() or not 1 <= int(port) <= 65535:
                    raise ValueError("invalid HTTP proxy endpoint")
            update_command = f"update-proxy {args.target}"
        except ValueError:
            ap.error("update-proxy requires IPv4:port or off (HTTP, no authentication)")
    dev = Device(args.port or default_port(), args.verbose)
    if args.action in updates:
        try:
            print(dev.command(update_command))
            if args.follow:
                dev.follow()
        finally:
            dev.ser.close()
        return
    if args.action in voice:
        try:
            if voice_data is not None:
                push_voice_resource(dev, voice_data, args.action == "voice-font-push")
            elif args.action == "voice-page":
                print(dev.command("voice-page "+args.target))
            elif args.action == "voice-read":
                print(read_voice_reply(dev))
            else:
                print(dev.command(voice_command, timeout=30.0))
            if args.follow:
                dev.follow()
        finally:
            dev.ser.close()
        return
    if args.action in camera:
        try:
            print(dev.command(args.action, timeout=30.0))
            if args.follow:
                dev.follow()
        finally:
            dev.ser.close()
        return
    if args.action in audio:
        try:
            command=args.action
            if args.target is not None:
                if any(c.isspace() for c in args.target):
                    ap.error("audio target cannot contain whitespace")
                command += f" {args.target}"
            if args.action == "audio-record":
                command += f" {args.duration_ms}"
            print(dev.command(command, timeout=45.0))
            if args.follow:
                dev.follow()
        finally:
            dev.ser.close()
        return
    if args.action in wireless:
        try:
            command = wifi_command or args.action
            print(dev.command(command, timeout=60.0))
            if args.follow:
                dev.follow()
        finally:
            dev.ser.close()
        return
    if args.action in ("nand-info", "nand-status", "nand-format", "power-status", "screen-on", "screen-off"):
        try:
            if args.action == "screen-off":
                screen_off(dev, int(args.target))
            elif args.action == "nand-format":
                print(dev.command("nand-format", timeout=60.0))
            else:
                print(dev.command(args.action, timeout=10.0))
            if args.follow:
                dev.follow()
        finally:
            dev.ser.close()
        return
    if args.action in ("recovery", "boot-status"):
        try:
            print(dev.command(args.action, timeout=30.0))
        finally:
            dev.ser.close()
        return
    elif args.action == "photo-push":
        try:
            push_photo(dev, *photo, run=args.run)
            if args.follow:
                dev.follow()
        finally:
            dev.ser.close()
        return
    elif args.action == "brain-push":
        push_brain(dev, brain_data)
    elif args.action in ("brain-reload", "brain-default", "brain-status"):
        print(dev.command(args.action, timeout=30.0))
    elif args.action == "skin-push":
        try:
            push_skin(dev, args.target, args.select)
        finally:
            dev.ser.close()
    elif args.action in ("skin-select", "skin-rm"):
        print(dev.command(f"{args.action} {args.target}", timeout=30.0))
    elif args.action == "skin-ls":
        print(dev.command("skin-ls"))
    elif args.action in ("bundle-push", "fonts-push", "fonts-status"):
        try:
            if args.action == "bundle-push":
                push_bundle(dev, args.target, args.run)
            elif args.action == "fonts-push":
                push_fonts(dev, args.target)
            else:
                print(dev.command("fonts-status"))
            if args.follow:
                dev.follow()
        except (ValueError, KeyError, TypeError, OSError) as error:
            ap.exit(2, f"resource upload failed: {error}\n")
        finally:
            dev.ser.close()
        return
    elif args.action == "push":
        push(dev, args.target, args.run)
    elif args.action == "ls":
        print(dev.command("ls")[len("@ok ls"):].strip() or "(no apps in /data/apps)")
    elif args.action == "status":
        print(dev.command("status")[len("@ok status"):].strip())
    elif args.action in ("run", "rm", "rmdata"):
        dev.command(f"{args.action} {args.target}", timeout=30.0)
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
