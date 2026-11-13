#!/usr/bin/env python3
"""Redraw each reference frame through the configured image bridge, sequentially.

An existing successful receipt is never resubmitted. A failed or ambiguous
request stops the batch for diagnosis. PIL is a host-side asset dependency.
"""
import argparse
import json
from pathlib import Path
import subprocess
import sys
import time
from PIL import Image, ImageDraw


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--reference", required=True, type=Path)
    parser.add_argument("--master", required=True, type=Path)
    parser.add_argument("--output", required=True, type=Path)
    parser.add_argument("--frames", required=True, help="comma-separated indices or start:end")
    parser.add_argument("--bridge", type=Path, default=Path.home() / ".agents/skills/codex-image-gen-bridge/scripts/bridge.py")
    parser.add_argument("--model", default="gpt-image-2.5-sunburst")
    parser.add_argument("--bridge-python", default="python3", help="stdlib Python used by the provider bridge")
    parser.add_argument("--tls-retries", type=int, default=3, help="retries only a diagnosed TLS handshake EOF before HTTP is sent")
    args = parser.parse_args()
    if ":" in args.frames:
        first, end = map(int, args.frames.split(":")); frames = range(first, end)
    else:
        frames = [int(v) for v in args.frames.split(",")]
    output = args.output.resolve(); output.mkdir(parents=True, exist_ok=True)
    master = Image.open(args.master).convert("RGB").resize((480, 480))
    ImageDraw.Draw(master).rectangle((208, 182, 440, 225), fill="black")
    ImageDraw.Draw(master).rectangle((0, 380, 479, 479), fill="black")
    for n in frames:
        receipt = output / f"result-{n:04}.json"
        if receipt.exists():
            if receipt.stat().st_size:
                result = json.loads(receipt.read_text())
                if result.get("file") and Path(result["file"]).is_file():
                    print(f"frame {n}: already generated", flush=True); continue
            error = output / f"error-{n:04}.log"
            diagnostic = error.read_text() if error.exists() else ""
            if "do_handshake" in diagnostic and "SSLEOFError" in diagnostic:
                # TLS failed before HTTP headers or the billable POST body.
                receipt.rename(output / f"result-{n:04}-tls-failed-{time.time_ns()}.json")
                error.rename(output / f"error-{n:04}-tls-failed-{time.time_ns()}.log")
            else:
                raise SystemExit(f"frame {n}: previous incomplete request; diagnose before resubmitting")
        source = Image.open(args.reference / f"{n:04}.png").convert("RGB")
        source = source.resize((480, 336), Image.Resampling.LANCZOS)
        frame = Image.new("RGB", (480, 480), "black"); frame.paste(source, (0, 40))
        # Fixed typography is composited later, using the reference's reveal
        # mask. Generative per-frame spelling would make the word shimmer.
        ImageDraw.Draw(frame).rectangle((236, 183, 416, 266), fill="black")
        board = Image.new("RGB", (960, 512), "#111111")
        board.paste(frame, (0, 32)); board.paste(master, (480, 32))
        draw = ImageDraw.Draw(board)
        draw.text((8, 8), f"LEFT: exact animation frame {n}/125, 480x480", fill="white")
        draw.text((488, 8), "RIGHT: line style only; do not copy its objects", fill="white")
        reference = output / f"reference-{n:04}.png"; board.save(reference)
        prompt = f"""Redraw the LEFT 480x480 animation canvas only, animation frame {n} of 125.
Preserve the LEFT frame's exact shape positions, sizes, contours, opacity, construction phase and black empty areas. RIGHT is only a white/coral line-art style reference, not the current animation state. Do not add a moon, star or orbit that is absent on LEFT. Do not finish incomplete shapes. Match every flicker and sparse geometric placeholder from LEFT. The blank area on the right is deliberately blank for device-composited RECOVERY typography: draw no text or lettering anywhere. Output a single square scene of only the LEFT canvas, with no reference panels, labels, footer or border. Pure black background, crisp monochrome celestial geometric line art. Keep coordinates registered to the LEFT 480x480 canvas. No camera movement or new visual interpretation."""
        (output / f"prompt-{n:04}.txt").write_text(prompt)
        error = output / f"error-{n:04}.log"
        for attempt in range(args.tls_retries + 1):
            with receipt.open("w") as stdout, error.open("w") as stderr:
                result = subprocess.run([args.bridge_python, str(args.bridge), "--mode", "edit", "--image", str(reference),
                                         "--model", args.model, "--size", "1:1", "--output-dir", str(output / f"frame-{n:04}"), prompt],
                                        stdout=stdout, stderr=stderr)
            if result.returncode == 0:
                break
            diagnostic = error.read_text()
            if attempt < args.tls_retries and "do_handshake" in diagnostic and "SSLEOFError" in diagnostic:
                stamp = time.time_ns()
                receipt.rename(output / f"result-{n:04}-tls-failed-{stamp}.json")
                error.rename(output / f"error-{n:04}-tls-failed-{stamp}.log")
                print(f"frame {n}: TLS handshake ended before HTTP; retry {attempt + 1}/{args.tls_retries}", flush=True)
                time.sleep(min(2 ** (attempt + 1), 30))
                continue
            raise SystemExit(f"frame {n}: provider error; batch stopped for diagnosis")
        value = json.loads(receipt.read_text())
        if not value.get("file"):
            raise SystemExit(f"frame {n}: image URL needs explicit asset download before continuing")
        print(f"frame {n}: generated", flush=True)


if __name__ == "__main__":
    main()
