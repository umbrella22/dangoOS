#!/usr/bin/env python3
"""Attach a computer Responses agent and validated skills over the USB console."""
import argparse
import json
from pathlib import Path
import sys

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from agent.bridge import serve
from console import Device, default_port


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--port")
    parser.add_argument("--config", type=Path, required=True, help="private Responses configuration; never copied to the device")
    parser.add_argument("--skills", type=Path, default=Path.home() / ".local/share/dango-agent/skills")
    parser.add_argument("--capabilities", type=int, choices=range(1, 8), default=7, help="explicit grants: status=1, volume=2, brightness=4")
    parser.add_argument("--once", help="submit one text task, then detach")
    parser.add_argument("--receipt", type=Path)
    args = parser.parse_args()
    try:
        config = json.loads(args.config.read_text())
    except (OSError, ValueError):
        parser.error("cannot read the private JSON configuration")
    device = Device(args.port or default_port(), False)
    def completed(receipt):
        if args.receipt:
            args.receipt.parent.mkdir(parents=True, exist_ok=True)
            args.receipt.write_text(json.dumps(receipt, ensure_ascii=False, indent=2) + "\n")
            args.receipt.chmod(0o600)
        print(json.dumps(receipt, ensure_ascii=False), flush=True)
    try:
        serve(device, config, args.skills, args.capabilities, args.once, completed)
    except KeyboardInterrupt:
        pass
    except Exception as error:
        raise SystemExit(type(error).__name__ + ": " + str(error)) from None
    finally:
        device.ser.close()


if __name__ == "__main__":
    main()
