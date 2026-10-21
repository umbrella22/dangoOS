"""Split fox-dango poses into base sprites, swap patches and free-moving symbols.

Usage: python3 assets/fox-dango/tools/fox_parts.py
Needs numpy, pillow, scipy. Reads source/<pose>.png and source/variants/*.png
(aligned edits of a pose that change one thing), writes:

  device/poses/<pose>.png          base pose per expression (symbols removed)
  device/parts/<pose>/<part>_N.png patch (opaque, fixed position) or sprite
                                   (RGBA; alpha marks the symbol's own pixels)
  device/manifest.json             positions, kinds, palette and animation hints
  preview/parts_sheet.png          every pose with its part rectangles

All images share one 128-color palette and are RGB565-exact, so the firmware
can store 8-bit indices plus the palette.
"""
import json
import os
import shutil
import sys

import numpy as np
from PIL import Image, ImageDraw
from scipy import ndimage

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import fox_build as fb  # noqa: E402

ROOT = fb.ROOT
VARIANTS = ROOT / 'source' / 'variants'

# Patch variants: '<pose>__<part>' -> how many changed blobs to keep.
PATCHES = {'blink': 2, 'mouth_closed': 1, 'mouth_wide': 1, 'ear_flick': 1, 'ear_turn': 1,
           'gaze_left': 2, 'gaze_right': 2}
# Symbol layers: what each pose's symbols are called, in reading order.
SYMBOL_NAMES = {
    'puzzled': 'question', 'confused': 'sweat', 'panic': 'sweat', 'angry': 'anger_mark',
    'thinking': 'dot', 'annoyed': 'sigh', 'charging': 'spark', 'sleep': 'z',
}
# Animation hints consumed by the firmware's expression player.
HINTS = {
    'idle': {'breath_ms': 3000, 'breath_pct': 1.5, 'blink': [2500, 6000], 'idle_extras': ['gaze_left', 'gaze_right', 'ear_flick']},
    'sleep': {'breath_ms': 4500, 'breath_pct': 2.5, 'symbols': 'z: rise 24 px and fade, staggered 600 ms'},
    'curious': {'breath_ms': 3000, 'breath_pct': 1.5, 'blink': [2500, 6000]},
    'happy': {'enter': 'pop', 'hop_px': 18, 'hop_ms': 400, 'hops': 3},
    'puzzled': {'breath_ms': 3600, 'breath_pct': 1.2, 'blink': [3000, 6000], 'symbols': 'question: bob 4 px, 1200 ms'},
    'confused': {'breath_ms': 3000, 'breath_pct': 1.5, 'spiral_ms': 100, 'sway_px': 2, 'symbols': 'sweat: blink on/off 500 ms'},
    'surprised': {'enter': 'stretch', 'blink': [3000, 6000]},
    'panic': {'breath_ms': 600, 'breath_pct': 2.0, 'shake_px': 3, 'shake_hz': 11, 'blink': [1500, 3000], 'symbols': 'sweat: fly outward 12 px and respawn; tremble: flicker 120 ms'},
    'angry': {'stomp_ms': 1500, 'shake_px': 2, 'blink': [3000, 6000], 'symbols': 'anger_mark: pulse on/off 400 ms'},
    'satisfied': {'breath_ms': 3600, 'breath_pct': 1.5, 'sway_px': 2},
    'listening': {'breath_ms': 3000, 'breath_pct': 1.5, 'blink': [2500, 6000], 'idle_extras': ['ear_turn']},
    'thinking': {'breath_ms': 3600, 'breath_pct': 1.2, 'blink': [3000, 6000], 'symbols': 'dot: light up one by one, 300 ms each'},
    'speaking': {'mouth_cycle': ['mouth_closed', 'base', 'mouth_wide', 'base'], 'mouth_ms': 120, 'blink': [2500, 5000]},
    'error': {'enter': 'drop', 'shake_every_ms': 2000, 'shake_px': 2},
    'tired': {'breath_ms': 4000, 'breath_pct': 1.5, 'nod_every_ms': 4000, 'nod_px': 6, 'blink': [2000, 4000]},
    'dying': {'breath_ms': 6000, 'breath_pct': 0.8},
    'annoyed': {'sigh_every_ms': 3000, 'blink': [3000, 6000], 'symbols': 'sigh: drift out 10 px and fade'},
    'charging': {'breath_ms': 3000, 'breath_pct': 1.5, 'blink': [2500, 6000], 'symbols': 'spark[0] (bolt): bob 4 px; others twinkle 250 ms'},
    'peering': {'breath_ms': 3600, 'breath_pct': 1.0, 'blink': [3500, 7000], 'lean': 'forward 1.5% squash', 'gaze_scan_ms': 2400},
    'dazzled': {'enter': 'flinch', 'flinch_ms': 360, 'tremble_ms': 900, 'tremble_px': 1},
}


def lum(a):
    a = np.asarray(a).astype(int)
    return (a[..., 0] * 3 + a[..., 1] * 6 + a[..., 2]) // 10


def blobs(a, b, min_px=15):
    """Changed regions between two placed (unquantized) images, largest first."""
    diff = np.abs(np.asarray(a).astype(int) - np.asarray(b).astype(int)).sum(2) > 60
    diff = ndimage.binary_opening(diff, iterations=1)
    lab, n = ndimage.label(ndimage.binary_dilation(diff, iterations=2))
    out = []
    for i in range(1, n + 1):
        m = lab == i
        if (m & diff).sum() >= min_px:
            out.append(m)
    return sorted(out, key=lambda m: -m.sum())


def bbox(mask, pad=2):
    ys, xs = np.where(mask)
    return (max(xs.min() - pad, 0), max(ys.min() - pad, 0),
            min(xs.max() + pad + 1, fb.CANVAS_W), min(ys.max() + pad + 1, fb.CANVAS_H))


def merge(rects):
    rects = list(rects)
    merged = True
    while merged:
        merged = False
        for i in range(len(rects)):
            for j in range(i + 1, len(rects)):
                a, b = rects[i], rects[j]
                if a[0] < b[2] and b[0] < a[2] and a[1] < b[3] and b[1] < a[3]:
                    rects[i] = (min(a[0], b[0]), min(a[1], b[1]), max(a[2], b[2]), max(a[3], b[3]))
                    del rects[j]
                    merged = True
                    break
            if merged:
                break
    return rects


def spiral_frames(img, n=4):
    """Rotate the two spiral eyes of `confused` in place (nearest, palette-exact).

    Returns `n - 1` frames (the base pose is frame 0) and the two eye boxes.
    """
    a = np.asarray(img).astype(int)
    red = (a[..., 0] > 150) & (a[..., 1] < 110) & (a[..., 2] < 110)
    lab, k = ndimage.label(ndimage.binary_closing(red, iterations=2))
    sizes = ndimage.sum(red, lab, range(1, k + 1))
    boxes = []
    for i in np.argsort(sizes)[-2:]:
        ys, xs = np.where(lab == i + 1)
        cy, cx = ys.mean(), xs.mean()
        r = int(max(np.ptp(ys), np.ptp(xs)) / 2 + 1)
        boxes.append((int(cx - r), int(cy - r), int(cx - r) + 2 * r + 1, int(cy - r) + 2 * r + 1))
    boxes.sort()
    frames = []
    for f in range(1, n):
        out = img.copy()
        for box in boxes:
            crop = img.crop(box)
            r = crop.width // 2
            rot = crop.rotate(-360 * f / n, resample=Image.NEAREST)
            yy, xx = np.mgrid[:crop.height, :crop.width]
            circle = ((yy - r) ** 2 + (xx - r) ** 2) <= (r * 0.86) ** 2
            out.paste(rot, box[:2], Image.fromarray((circle * 255).astype(np.uint8)))
        frames.append(out)
    return frames, boxes


def main():
    geom = fb.geometry()
    variants = sorted(p.stem for p in VARIANTS.glob('*.png'))
    ims = fb.load_all(extra=[f'variants/{v}' for v in variants], geom=geom)
    q, pal = fb.quantize(ims)
    rgb = np.asarray(pal.getpalette()[:fb.COLORS * 3], dtype=np.uint8).reshape(-1, 3)
    rgb565 = fb.to565(rgb[None])[0]
    palette = [((int(r) >> 3) << 11) | ((int(g) >> 2) << 5) | (int(b) >> 3) for r, g, b in rgb565]

    out_dir = ROOT / 'device'
    for sub in ('poses', 'parts'):
        shutil.rmtree(out_dir / sub, ignore_errors=True)
        (out_dir / sub).mkdir(parents=True)
    for old in out_dir.glob('*.png'):
        old.unlink()  # flat layout from the first concept round

    manifest = {'canvas': [fb.CANVAS_W, fb.CANVAS_H], 'anchor': 'bottom-centre, panel y offset +16',
                'palette_rgb565': palette, 'poses': {}}
    sheet_items = []
    for key, label in fb.ORDER:
        clean = f'variants/{key}__clean'
        base_key = clean if clean in q else key
        base = q[base_key]
        base.save(out_dir / 'poses' / f'{key}.png')
        entry = {'file': f'poses/{key}.png', 'label': label, 'parts': {}, 'hints': HINTS.get(key, {})}
        rects_for_sheet = []
        pdir = out_dir / 'parts' / key

        # 1) patch variants (blink, mouth, ears, gaze)
        for v in variants:
            pose, _, part = v.partition('__')
            if pose != key or part not in PATCHES:
                continue
            ms = blobs(ims[base_key], ims[f'variants/{v}'])[:PATCHES[part]]
            if not ms:
                print(f'warning: {v} has no visible change')
                continue
            rects = merge(bbox(m) for m in ms)
            items = []
            pdir.mkdir(exist_ok=True)
            for i, r in enumerate(sorted(rects)):
                f = f'parts/{key}/{part}_{i}.png'
                q[f'variants/{v}'].crop(r).save(out_dir / f)
                items.append({'kind': 'patch', 'file': f, 'x': int(r[0]), 'y': int(r[1]),
                              'w': int(r[2] - r[0]), 'h': int(r[3] - r[1])})
                rects_for_sheet.append(('patch', r))
            entry['parts'][part] = items

        # 2) symbols: full pose minus its clean variant, or an additive variant
        sym_src = None
        if base_key != key:
            sym_src, sym_base = key, base_key
        elif f'variants/{key}__zzz' in q:
            sym_src, sym_base = f'variants/{key}__zzz', key
        if sym_src:
            name = SYMBOL_NAMES.get(key, 'symbol')
            ms = blobs(ims[sym_base], ims[sym_src])
            pdir.mkdir(exist_ok=True)
            base_l = lum(q[sym_base])
            src_l = lum(q[sym_src])
            # reading order: left to right, then top to bottom
            ms.sort(key=lambda m: (bbox(m)[0] // 24, bbox(m)[1]))
            if key == 'charging':
                ms.sort(key=lambda m: -m.sum())  # the bolt first
            groups = {}
            qa, qb = np.asarray(q[sym_src]), np.asarray(q[sym_base])
            for m in ms:
                r = bbox(m)
                part = name
                # panic's shake strokes beside the paws are their own layer
                if key == 'panic' and r[1] > fb.CANVAS_H * 0.6:
                    part = 'tremble'
                over_black = bool(base_l[m].mean() < 24)
                if over_black:
                    mask = m & (src_l > 16)
                else:
                    # over the character: keep only pixels that really changed
                    mask = m & ndimage.binary_dilation((qa != qb).any(2), iterations=1)
                mask = mask[r[1]:r[3], r[0]:r[2]]
                rgba = np.dstack([qa[r[1]:r[3], r[0]:r[2]], (mask * 255).astype(np.uint8)])
                items = groups.setdefault(part, [])
                f = f'parts/{key}/{part}_{len(items)}.png'
                Image.fromarray(rgba, 'RGBA').save(out_dir / f)
                items.append({'kind': 'sprite', 'over': 'bg' if over_black else 'body', 'file': f,
                              'x': int(r[0]), 'y': int(r[1]), 'w': int(r[2] - r[0]), 'h': int(r[3] - r[1])})
                rects_for_sheet.append(('sprite', r))
            entry['parts'].update(groups)

        # 3) procedural spiral eyes for confused
        if key == 'confused':
            items = []
            pdir.mkdir(exist_ok=True)
            frames, rects = spiral_frames(base)
            for n, img in enumerate(frames):
                for i, r in enumerate(rects):
                    f = f'parts/{key}/spiral{n + 1}_{i}.png'
                    img.crop(r).save(out_dir / f)
                    items.append({'kind': 'patch', 'frame': n + 1, 'file': f, 'x': r[0], 'y': r[1],
                                  'w': r[2] - r[0], 'h': r[3] - r[1]})
                    if n == 0:
                        rects_for_sheet.append(('patch', r))
            entry['parts']['spiral'] = items

        manifest['poses'][key] = entry
        sheet_items.append((key, label, base, rects_for_sheet))

    with open(out_dir / 'manifest.json', 'w') as fh:
        json.dump(manifest, fh, ensure_ascii=False, indent=1)

    # Diagnostic sheet: green = patch, magenta = sprite.
    cols, cw, ch = 6, fb.CANVAS_W, fb.CANVAS_H + 40
    sheet = Image.new('RGB', (cols * (cw + 12) + 12, ((len(sheet_items) + cols - 1) // cols) * (ch + 12) + 12), (28, 28, 30))
    d = ImageDraw.Draw(sheet)
    font = fb.label_font(22)
    for i, (key, label, base, rects) in enumerate(sheet_items):
        x = 12 + (i % cols) * (cw + 12); y = 12 + (i // cols) * (ch + 12)
        sheet.paste(base, (x, y))
        for kind, r in rects:
            d.rectangle((x + r[0], y + r[1], x + r[2] - 1, y + r[3] - 1),
                        outline=(80, 230, 120) if kind == 'patch' else (240, 80, 220), width=2)
        n = sum(len(v) for v in manifest['poses'][key]['parts'].values())
        d.text((x + 4, y + fb.CANVAS_H + 6), f'{label}  ({n})', fill=(225, 225, 228), font=font)
    sheet.save(ROOT / 'preview' / 'parts_sheet.png')

    total = sum(len(v) for p in manifest['poses'].values() for v in p['parts'].values())
    print(f'{len(manifest["poses"])} poses, {total} part images, manifest written')
    for key, p in manifest['poses'].items():
        if p['parts']:
            print(' ', key, {k: len(v) for k, v in p['parts'].items()})


if __name__ == '__main__':
    main()
