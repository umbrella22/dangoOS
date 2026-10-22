#!/usr/bin/env python3
"""Bounded N1 console session using the existing push-app API (no flashing).

Commands run in order, then --push uploads and runs an app, then logs are
collected for --seconds. Each serial write has a 20 second timeout.
Command replies retain push-app's 20/30 second limits;
--wait bounds connection retries only, --seconds bounds the final log window.
Exit: 0 session completed (not Lua/physical acceptance), 1 setup/protocol error,
2 serial/I/O error or invalid CLI, 130 interrupted. Logs are created exclusively.
"""

import argparse
import contextlib
import datetime
import importlib.util
import math
from pathlib import Path
import sys
import time

DEFAULT_PORT = "/dev/cu.usbmodemmosaico_s31_r11"


def duration(value):
    try:
        result = float(value)
    except ValueError:
        raise argparse.ArgumentTypeError("must be finite and nonnegative") from None
    if not math.isfinite(result) or result < 0:
        raise argparse.ArgumentTypeError("must be finite and nonnegative")
    return result


class Tee:
    def __init__(self, console, log):
        self.console, self.log = console, log

    def write(self, text):
        self.console.write(text)
        self.log.write(text)
        self.flush()
        return len(text)

    def flush(self):
        self.console.flush()
        self.log.flush()


def stamp(label):
    print(label + "=" + datetime.datetime.now(datetime.timezone.utc).isoformat(), flush=True)


def load_api():
    # Avoid leaving import artifacts in the repository, including on failure.
    sys.dont_write_bytecode = True
    spec = importlib.util.spec_from_file_location("push_app", Path(__file__).resolve().with_name("push-app.py"))
    api = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(api)
    return api


def session(args):
    dev = None
    stamp("session_started")
    try:
        api = load_api()
        deadline = time.monotonic() + args.wait
        while True:
            try:
                dev = api.Device(args.port, True)
                break
            except (api.serial.SerialException, OSError):
                remaining = deadline - time.monotonic()
                if remaining <= 0:
                    raise
                time.sleep(min(0.1, remaining))
        # Bound command and payload writes without changing the shared API.
        dev.ser.write_timeout = 20.0
        stamp("connected")
        print("PORT " + args.port)
        for command in args.command:
            print("COMMAND " + command)
            # check=True is essential: @err and missing replies raise SystemExit.
            print(dev.command(command, check=True))
        if args.push:
            api.push(dev, args.push, True)
        for line in dev.lines(time.monotonic() + args.seconds):
            print(line)
        print("SESSION_OK (transport only; inspect probe results separately)")
        return 0
    except KeyboardInterrupt:
        print("INTERRUPTED", file=sys.stderr)
        return 130
    except SystemExit as error:
        # push-app reports protocol/import failures with SystemExit strings.
        print("PROTOCOL_OR_SETUP_ERROR " + str(error), file=sys.stderr)
        return 1
    except Exception as error:
        print(type(error).__name__ + ": " + str(error), file=sys.stderr)
        return 2 if isinstance(error, OSError) or type(error).__name__ == "SerialException" else 1
    finally:
        try:
            if dev is not None:
                dev.ser.close()
        finally:
            stamp("session_finished")


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--port", default=DEFAULT_PORT, help="runtime console port (default: %(default)s)")
    parser.add_argument("--log", required=True, help="new log path; existing files are never overwritten")
    parser.add_argument("--seconds", type=duration, default=40, help="final log window seconds (default: 40)")
    parser.add_argument("--wait", type=duration, default=60, help="port connection retry seconds (default: 60; 0 tries once)")
    parser.add_argument("--push", help="app directory to upload and run after commands")
    parser.add_argument("--command", action="append", default=[], help="console command; repeat to run in order")
    args = parser.parse_args(argv)
    if any(not command.strip() or "\n" in command or "\r" in command for command in args.command):
        parser.error("--command must contain one nonempty protocol command")
    try:
        with open(args.log, "x", encoding="utf-8") as log:
            with contextlib.redirect_stdout(Tee(sys.stdout, log)), contextlib.redirect_stderr(Tee(sys.stderr, log)):
                return session(args)
    except OSError as error:
        print("LOG_OR_IO_ERROR " + str(error), file=sys.stderr)
        return 2


if __name__ == "__main__":
    sys.exit(main())
