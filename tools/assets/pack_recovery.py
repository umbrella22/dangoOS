#!/usr/bin/env python3
"""Pack the 126 image-bridge redraws into the independently decoded RAN1 format.

Geometry is redrawn per frame. A fixed approved RECOVERY wordmark replaces the
reference lettering during its original reveal interval, avoiding spelling
flicker. The waiting caption is drawn by the screen and is not repeated in RAN1.
Requires Pillow; no provider calls or credentials are used by this packer.
"""
import argparse
import hashlib
import json
from pathlib import Path
import struct
import zlib
from PIL import Image, ImageDraw, ImageFilter

SIZE = (480, 480)
LEVELS = (0, 16, 40, 72, 112, 160, 216, 255)
PALETTE = [(v, v, v) for v in LEVELS] + [(v, round(v * .30), round(v * .33)) for v in LEVELS]
RGB565 = [((r >> 3) << 11) | ((g >> 2) << 5) | (b >> 3) for r, g, b in PALETTE]
NEAREST = [min(range(8), key=lambda n: abs(LEVELS[n] - value)) for value in range(256)]


def indexed(image):
    output = bytearray(480 * 480)
    for i, (r, g, b) in enumerate(image.convert("RGB").get_flattened_data()):
        red = r > g + 30 and r > b + 25
        light = r if red else round(.299 * r + .587 * g + .114 * b)
        output[i] = NEAREST[light] + (8 if red else 0)
    result = Image.frombytes("P", SIZE, bytes(output))
    result.putpalette([c for rgb in PALETTE for c in rgb] + [0] * (768 - 48))
    packed = bytes((output[i] << 4) | output[i + 1] for i in range(0, len(output), 2))
    return result, packed


def rcs(image):
    _, raw = indexed(image)
    return b"RCS1" + struct.pack("<HH16H", *SIZE, *RGB565) + zlib.compress(raw, 9)


def compose(image, reference, master, start_ms):
    scene = image.convert("RGB").resize(SIZE, Image.Resampling.LANCZOS)
    if reference.convert("L").getextrema()[1] == 0:
        return Image.new("RGB", SIZE)
    draw = ImageDraw.Draw(scene)
    draw.rectangle((0, 380, 479, 479), fill="black")
    if 690 <= start_ms < 1380:
        # The reference builds its large word across the entire canvas before
        # contracting it beside the globe. The first glyph is well left of
        # the final wordmark; a mask of only the final bounds leaks old letters.
        draw.rectangle((0, 165, 479, 282), fill="black")
        word = master.crop((208, 183, 440, 225))
        contraction = min(1., max(0., (start_ms - 1290) / 90))
        width = round(352 + (232 - 352) * contraction)
        height = round(64 + (42 - 64) * contraction)
        x = round(64 + (208 - 64) * contraction)
        y = round(177 + (183 - 177) * contraction)
        word = word.resize((width, height), Image.Resampling.LANCZOS)
        fraction = min(1., (1 + (start_ms - 690) // 75) / 8)
        visible = round(word.width * fraction)
        scene.paste(word.crop((0, 0, visible, word.height)), (x, y))
    elif start_ms >= 1380:
        # The reference's foreign letters and sub-caption occupy this region.
        # Their replacement uses one stable bitmap, revealed left to right.
        draw.rectangle((208, 173, 459, 272), fill="black")
        word = master.crop((208, 183, 440, 225))
        scene.paste(word, (208, 183))
    return scene


def geometry_bounds(image):
    """Find the connected moon/globe, excluding the separate orbiting stars."""
    # Join antialiasing gaps in a contour before measuring it. This affects
    # registration only; the original redrawn pixels remain the rendered image.
    mask = image.convert("L").point(lambda v: 255 if v >= 70 else 0).filter(ImageFilter.MaxFilter(5))
    pixels = bytearray(mask.tobytes())
    components = []
    for start, value in enumerate(pixels):
        if not value:
            continue
        pixels[start] = 0
        stack = [start]
        x0 = x1 = start % 480
        y0 = y1 = start // 480
        area = 0
        while stack:
            at = stack.pop()
            x, y = at % 480, at // 480
            x0, x1, y0, y1 = min(x0, x), max(x1, x), min(y0, y), max(y1, y)
            area += 1
            for yy in range(max(0, y - 1), min(480, y + 2)):
                for xx in range(max(0, x - 1), min(480, x + 2)):
                    other = yy * 480 + xx
                    if pixels[other]:
                        pixels[other] = 0
                        stack.append(other)
        components.append((y1 - y0 + 1, area, (x0, y0, x1 + 1, y1 + 1)))
    if not components:
        return None
    tallest = max(components)
    # During construction the globe consists of disconnected points.
    return tallest[2] if tallest[0] >= 32 else mask.getbbox()


def register(image, reference, start_ms):
    scene = image.convert("RGB").resize(SIZE, Image.Resampling.LANCZOS)
    if start_ms < 1950:
        return scene
    ref = Image.new("RGB", SIZE)
    ref.paste(reference.convert("RGB").resize((480, 336), Image.Resampling.LANCZOS), (0, 40))
    # Some redraws recenter the globe. A fixed left crop truncates those shapes
    # and yields the wrong anchor. Match the whole connected shape instead.
    ImageDraw.Draw(ref).rectangle((236, 173, 459, 272), fill="black")
    # Reference typography starts at x=236 after the contraction. Collect all
    # left-side construction fragments, including disconnected top/bottom arcs.
    a = ref.crop((0, 0, 236, 380)).convert("L").point(lambda v: 255 if v >= 70 else 0).getbbox()
    b = (scene.convert("L").point(lambda v: 255 if v >= 70 else 0).getbbox()
         if start_ms < 2010 else geometry_bounds(scene))
    if a and b:
        scale = (a[3] - a[1]) / (b[3] - b[1])
        if not .65 <= scale <= 1.5:
            raise ValueError(f"redraw needs visual repair: scale {scale:.3f}")
        cx, cy = (a[0] + a[2]) / 2, (a[1] + a[3]) / 2
        sx, sy = (b[0] + b[2]) / 2, (b[1] + b[3]) / 2
        scene = scene.transform(SIZE, Image.Transform.AFFINE,
                                (1 / scale, 0, sx - cx / scale,
                                 0, 1 / scale, sy - cy / scale),
                                Image.Resampling.BICUBIC)
    return scene


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--frames", type=Path)
    parser.add_argument("--reference", type=Path)
    parser.add_argument("--timing", type=Path)
    parser.add_argument("--master", required=True, type=Path)
    parser.add_argument("--assets", required=True, type=Path)
    parser.add_argument("--preview", type=Path)
    parser.add_argument("--static-only", action="store_true")
    parser.add_argument("--derive-rejected", action="store_true", help="explicitly interpolate isolated provider-rejected frames; receipts remain distinct from API results")
    args = parser.parse_args()
    master = Image.open(args.master).convert("RGB").resize(SIZE)
    args.assets.mkdir(parents=True, exist_ok=True)
    static = rcs(master)
    (args.assets / "static.rcs").write_bytes(static)
    waiting = master.crop((0, 390, 480, 424)).convert("L").point(lambda v: 255 if v >= 80 else 0).convert("1")
    (args.assets / "waiting.rct").write_bytes(b"RCT1" + struct.pack("<HHH", 480, 34, 390) + waiting.tobytes())
    if args.static_only:
        print(json.dumps({"static_bytes": len(static), "palette_rgb565": RGB565})); return
    if not all((args.frames, args.reference, args.timing, args.preview)):
        parser.error("full packing requires --frames --reference --timing --preview")
    timing = json.loads(args.timing.read_text())
    if timing["frame_count"] != 126 or len(timing["frames"]) != 126:
        raise ValueError("expected all 126 original composited frames")
    args.preview.mkdir(parents=True, exist_ok=True)
    images, compressed, durations, checksums, provenance, derived = [], [], [], [], [], []
    def generated(n):
        receipt_path=args.frames/f"result-{n:04}.json"
        if not receipt_path.exists() or not receipt_path.stat().st_size: return None
        receipt=json.loads(receipt_path.read_text());path=Path(receipt.get("file", ""))
        return (receipt_path,path) if path.is_file() else None
    for frame in timing["frames"]:
        n = frame["index"]
        source = Image.open(args.reference / f"{n:04}.png")
        original=generated(n)
        if original:
            receipt_path,path=original
            geometry=register(Image.open(path),source,frame["start_ms"])
            origin={"kind":"provider-redraw","input_receipt":str(receipt_path),"generated_sha256":hashlib.sha256(path.read_bytes()).hexdigest()}
        else:
            error=args.frames/f"error-{n:04}.log"
            if not args.derive_rejected or not error.exists() or "moderation_blocked" not in error.read_text():
                raise ValueError(f"frame {n} is not generated; no frame was silently substituted")
            if n==0 or n==125 or not generated(n-1) or not generated(n+1):
                raise ValueError(f"frame {n} needs a dedicated repair: adjacent successful redraws unavailable")
            neighbors=[];hashes=[]
            for neighbor in (n-1,n+1):
                _,path=generated(neighbor);ref=Image.open(args.reference/f"{neighbor:04}.png")
                neighbors.append(register(Image.open(path),ref,timing["frames"][neighbor]["start_ms"]))
                hashes.append(hashlib.sha256(path.read_bytes()).hexdigest())
            left,right=timing["frames"][n-1]["start_ms"],timing["frames"][n+1]["start_ms"]
            geometry=Image.blend(neighbors[0],neighbors[1],(frame["start_ms"]-left)/(right-left))
            origin={"kind":"neighbor-interpolation","neighbors":[n-1,n+1],"source_hashes":hashes,"provider_error":str(error)}
            derived.append(n)
        scene = compose(geometry, source, master, frame["start_ms"])
        image, packed = indexed(scene)
        images.append(image); compressed.append(zlib.compress(packed, 9))
        durations.append(frame["duration_ms"]); checksums.append(zlib.crc32(packed))
        provenance.append({"index": n, **origin,
                           "duration_ms": frame["duration_ms"], "packed_sha256": hashlib.sha256(packed).hexdigest()})
    header = b"RAN1" + struct.pack("<HHHHI16H", 480, 480, 126, 16, 115200, *RGB565)
    offset = len(header) + 16 * len(images)
    index = bytearray()
    for data, duration, crc in zip(compressed, durations, checksums):
        index.extend(struct.pack("<IIII", offset, len(data), duration, crc)); offset += len(data)
    bundle = header + index + b"".join(compressed)
    (args.assets / "animation.ran").write_bytes(bundle)
    args.preview.mkdir(parents=True, exist_ok=True)
    # Preview the device caption too; the RAN1 payload itself still omits it.
    presented = []
    for image in images:
        shown = image.convert("RGB")
        shown.paste(master.crop((0, 390, 480, 424)), (0, 390))
        presented.append(shown)
    presented[0].save(args.preview / "recovery.gif", save_all=True, append_images=presented[1:],
                   duration=durations, loop=0, disposal=2, optimize=False)
    presented[-1].save(args.preview / "final.png")
    contact = Image.new("RGB", (960, 16 * 140), "#141414")
    labels = ImageDraw.Draw(contact)
    for n, image in enumerate(images):
        x, y = n % 8 * 120, n // 8 * 140
        contact.paste(image.convert("RGB").resize((120, 120)), (x, y + 20))
        labels.text((x + 2, y + 3), f"{n:03} {durations[n]}ms", fill="white")
    contact.save(args.preview / "contact.jpg")
    receipt = {"frames": len(images), "provider_redraws":len(images)-len(derived),"interpolated_frames":derived,
               "duration_ms": sum(durations), "bundle_bytes": len(bundle), "static_bytes": len(static),
               "work_bytes": 115200, "band_bytes": 480 * 32 * 2,
               "sha256": hashlib.sha256(bundle).hexdigest(), "model": "gpt-image-2.5-sunburst",
               "typography": "approved fixed bitmap, composited along original reveal interval",
               "blank_frame": "the empty reference frame remains empty after composition", "inputs": provenance}
    (args.preview / "provenance.json").write_text(json.dumps(receipt, indent=2) + "\n")
    print(json.dumps({k: v for k, v in receipt.items() if k != "inputs"}, indent=2))


if __name__ == "__main__":
    main()
