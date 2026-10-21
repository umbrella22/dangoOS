"""Quantize fox-dango expressions for the 480x480 panel and build previews.

Usage: python3 assets/fox-dango/tools/fox_build.py
Reads source/<key>.png (1254x1254 masters on black) and writes
preview/expressions_480.png. Also the shared library for fox_parts.py (which
writes device/) and fox_anim.py: canvas geometry, 128-colour RGB565 palette, panel.
"""
import os
from pathlib import Path
import numpy as np
from PIL import Image, ImageDraw, ImageFont

ROOT = Path(__file__).resolve().parent.parent
ORDER = [  # (key, label); order matches BallExpression, then extras
    ('idle', '待机 Idle'),
    ('sleep', '睡眠 Sleep'),
    ('curious', '好奇 Curious'),
    ('happy', '开心 Happy'),
    ('puzzled', '疑惑 Puzzled'),
    ('confused', '困惑 Confused'),
    ('surprised', '惊讶 Surprised'),
    ('panic', '慌张 Panic'),
    ('angry', '生气 Angry'),
    ('satisfied', '满意 Satisfied'),
    ('listening', '聆听 Listening'),
    ('thinking', '思考 Thinking'),
    ('speaking', '说话 Speaking'),
    ('error', '出错 Error'),
    ('tired', '疲惫 Tired'),
    ('dying', '濒死 Dying'),
    ('annoyed', '不耐烦 Annoyed'),
    ('charging', '充电 Charging'),
]
CANVAS_W, CANVAS_H = 352, 336   # device canvas; bottom-anchored, headroom for hops
COLORS = 128

def src(key):
    return Image.open(ROOT / 'source' / f'{key}.png').convert('RGB')

def to565(a):
    r = (a[..., 0] >> 3); g = (a[..., 1] >> 2); b = (a[..., 2] >> 3)
    out = np.stack([(r << 3) | (r >> 2), (g << 2) | (g >> 4), (b << 3) | (b >> 2)], -1)
    return out.astype(np.uint8)

def geometry():
    """One crop + scale for every expression: the union of the 18 poses' content."""
    boxes = [np.argwhere(np.asarray(src(k)).max(2) > 24) for k, _ in ORDER]
    y0 = min(b[:, 0].min() for b in boxes); y1 = max(b[:, 0].max() for b in boxes)
    x0 = min(b[:, 1].min() for b in boxes); x1 = max(b[:, 1].max() for b in boxes)
    w, h = x1 - x0 + 1, y1 - y0 + 1
    return x0, y0, w, h, min(CANVAS_W / w, (CANVAS_H - 24) / h)


def place(im, geom):
    """Map a 1254x1254 master onto the bottom-anchored device canvas."""
    x0, y0, w, h, s = geom
    c = im.crop((x0, y0, x0 + w, y0 + h)).resize((round(w * s), round(h * s)), Image.LANCZOS)
    canvas = Image.new('RGB', (CANVAS_W, CANVAS_H))
    canvas.paste(c, ((CANVAS_W - c.width) // 2, CANVAS_H - c.height))
    return canvas


def load_all(extra=(), geom=None):
    """Poses (and optional extra masters, e.g. 'variants/idle__blink') on the canvas."""
    geom = geom or geometry()
    keys = [k for k, _ in ORDER] + list(extra)
    return {k: place(src(k), geom) for k in keys}


def quantize(ims):
    """Shared palette over every image; returns RGB565-exact images and the palette."""
    keys = list(ims)
    mosaic = Image.new('RGB', (CANVAS_W * len(keys), CANVAS_H))
    for i, k in enumerate(keys):
        mosaic.paste(ims[k], (i * CANVAS_W, 0))
    pal = mosaic.quantize(COLORS, method=Image.Quantize.MAXCOVERAGE)
    q = {}
    for k in keys:
        idx = ims[k].quantize(palette=pal, dither=Image.Dither.NONE)
        q[k] = Image.fromarray(to565(np.asarray(idx.convert('RGB'))))
    return q, pal


def panel(img, offset=(0, 0)):
    """Place a canvas on a 480x480 rounded black panel (16 px below centre)."""
    P = 480
    p = Image.new('RGB', (P, P))
    p.paste(img, ((P - img.width) // 2 + offset[0], (P - img.height) // 2 + 16 + offset[1]))
    mask = Image.new('L', (P, P)); ImageDraw.Draw(mask).rounded_rectangle((0, 0, P - 1, P - 1), 72, fill=255)
    bg = Image.new('RGB', (P, P), (12, 12, 13)); bg.paste(p, (0, 0), mask)
    return bg

def label_font(size):
    for f in ['/usr/share/fonts/opentype/noto/NotoSerifCJK-Bold.ttc',
              '/System/Library/Fonts/PingFang.ttc', '/System/Library/Fonts/STHeiti Medium.ttc']:
        if os.path.exists(f):
            return ImageFont.truetype(f, size)
    return ImageFont.load_default()


if __name__ == '__main__':
    ims = load_all()
    q, pal = quantize(ims)
    # Device files (poses, parts, manifest) come from fox_parts.py; this script
    # only renders the overview sheet of the full, symbol-baked poses.
    (ROOT / 'preview').mkdir(exist_ok=True)
    font = label_font(30)
    cols, cell = 6, 480
    rows = (len(ORDER) + cols - 1) // cols
    sheet = Image.new('RGB', (cols * (cell + 24) + 24, rows * (cell + 70) + 24), (28, 28, 30))
    d = ImageDraw.Draw(sheet)
    for i, (k, label) in enumerate(ORDER):
        x = 24 + (i % cols) * (cell + 24); y = 24 + (i // cols) * (cell + 70)
        sheet.paste(panel(q[k]), (x, y))
        tw = d.textlength(label, font=font)
        d.text((x + (cell - tw) / 2, y + cell + 12), label, fill=(225, 225, 228), font=font)
    sheet = sheet.resize((sheet.width // 2, sheet.height // 2), Image.LANCZOS)
    sheet.save(ROOT / 'preview' / 'expressions_480.png')
    print('wrote preview/expressions_480.png')
