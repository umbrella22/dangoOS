"""Device-faithful animation preview, composed only from device/ assets.

Usage: python3 assets/fox-dango/tools/fox_anim.py   (numpy + pillow; ffmpeg for MP4)
Run fox_parts.py first. Writes preview/animation_parts.{mp4,gif}: a 3x3 grid of
expressions, each driven by the hints in device/manifest.json.

Composition order per frame is the one the firmware should use:
  1. base pose (RLE)            2. patches (blink / mouth / ears / gaze / spiral)
  3. sprites over the body      4. whole-canvas nearest-neighbour warp (breath, stomp)
  5. sprites over the background, moved / blinked / stipple-faded after the warp
Fades are an ordered-dither stipple on the sprite mask (no alpha blending), so a
palette-index renderer can do them with one compare per pixel.
"""
import json, math, os, shutil, subprocess, sys, tempfile
import numpy as np
from PIL import Image, ImageDraw

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import fox_build as fb  # noqa: E402

DEV = fb.ROOT / 'device'
M = json.load(open(DEV / 'manifest.json'))
W, H = M['canvas']
FPS, SECONDS = 20, 8
BAYER = (np.array([[0, 8, 2, 10], [12, 4, 14, 6], [3, 11, 1, 9], [15, 7, 13, 5]]) + 0.5) / 16


def rgb(path):
    return np.asarray(Image.open(DEV / path).convert('RGB'))


POSE = {k: rgb(p['file']) for k, p in M['poses'].items()}
PART = {}
for k, p in M['poses'].items():
    for name, items in p['parts'].items():
        for it in items:
            im = Image.open(DEV / it['file'])
            a = np.asarray(im.convert('RGBA'))
            it['rgb'] = a[..., :3]
            it['mask'] = a[..., 3] > 127 if it['kind'] == 'sprite' else np.ones(a.shape[:2], bool)
        PART[(k, name)] = items


def parts(key, name, frame=None):
    items = PART.get((key, name), [])
    return [i for i in items if frame is None or i.get('frame') == frame]


def paste(canvas, it, dx=0, dy=0, scale=1.0, fade=1.0):
    src, mask = it['rgb'], it['mask']
    if scale != 1.0:
        h, w = mask.shape
        nh, nw = max(1, round(h * scale)), max(1, round(w * scale))
        yi = np.clip((np.arange(nh) / scale).astype(int), 0, h - 1)
        xi = np.clip((np.arange(nw) / scale).astype(int), 0, w - 1)
        src, mask = src[yi][:, xi], mask[yi][:, xi]
        dx -= (nw - w) // 2; dy -= (nh - h) // 2
    x0, y0 = it['x'] + int(dx), it['y'] + int(dy)
    h, w = mask.shape
    if fade < 1.0:
        yy, xx = np.mgrid[y0:y0 + h, x0:x0 + w]
        mask = mask & (BAYER[yy % 4, xx % 4] < fade)
    cx0, cy0 = max(x0, 0), max(y0, 0)
    cx1, cy1 = min(x0 + w, W), min(y0 + h, H)
    if cx1 <= cx0 or cy1 <= cy0:
        return
    sm = mask[cy0 - y0:cy1 - y0, cx0 - x0:cx1 - x0]
    region = canvas[cy0:cy1, cx0:cx1]
    region[sm] = src[cy0 - y0:cy1 - y0, cx0 - x0:cx1 - x0][sm]


def warp(a, sx, sy, dx=0, dy=0):
    """Nearest-neighbour scale about the bottom centre (row/column LUT on device)."""
    ay, ax = H - 1, W / 2
    sy_idx = np.round(ay - (ay - (np.arange(H) - dy)) / sy).astype(int)
    sx_idx = np.round(ax + (np.arange(W) - dx - ax) / sx).astype(int)
    vy = (sy_idx >= 0) & (sy_idx < H); vx = (sx_idx >= 0) & (sx_idx < W)
    out = np.zeros_like(a)
    sub = a[np.clip(sy_idx, 0, H - 1)][:, np.clip(sx_idx, 0, W - 1)]
    out[np.ix_(vy, vx)] = sub[np.ix_(vy, vx)]
    return out


def breath(t, key):
    h = M['poses'][key]['hints']
    ms, pct = h.get('breath_ms', 3000), h.get('breath_pct', 1.5) / 100
    s = math.sin(2 * math.pi * t * 1000 / ms)
    return 1 - pct * 0.5 * s, 1 + pct * s


def blinking(t, times, dur=0.15):
    return any(0 <= t - b < dur for b in times)


def frame(key, t):
    c = POSE[key].copy()
    sx, sy = breath(t, key)
    dx = dy = 0
    late = []  # sprites drawn after the warp: (item, dx, dy, scale, fade)

    if key == 'idle':
        if blinking(t, [1.0, 4.4, 7.2]):
            [paste(c, i) for i in parts(key, 'blink')]
        elif 1.8 <= t < 2.9:
            [paste(c, i) for i in parts(key, 'gaze_left')]
        elif 3.1 <= t < 4.2:
            [paste(c, i) for i in parts(key, 'gaze_right')]
        if any(0 <= t - e < 0.12 for e in (5.4, 5.65)):
            [paste(c, i) for i in parts(key, 'ear_flick')]

    elif key == 'thinking':
        lit = int(t * 1000 // 300) % 5          # 0..3 dots, then a beat of 4 = all lit
        for n, i in enumerate(parts(key, 'dot')):
            late.append((i, 0, 0, 1.0, 1.0 if n < lit else 0.0))
        if blinking(t, [2.6, 6.1]):
            [paste(c, i) for i in parts(key, 'blink')]

    elif key == 'speaking':
        talking = (t % 4.0) < 2.8
        if talking:
            cycle = M['poses'][key]['hints']['mouth_cycle']
            step = cycle[int(t * 1000 // M['poses'][key]['hints']['mouth_ms']) % len(cycle)]
            if step != 'base':
                [paste(c, i) for i in parts(key, step)]
        else:
            [paste(c, i) for i in parts(key, 'mouth_closed')]
        if blinking(t, [3.2, 7.0]):
            [paste(c, i) for i in parts(key, 'blink')]

    elif key == 'sleep':
        for n, i in enumerate(parts(key, 'z')):
            ph = ((t - n * 0.6) % 2.4) / 2.4
            late.append((i, ph * 6, -ph * 24, 1.0, min(1.0, 3 * (1 - ph)) if ph > 0.05 else 0.0))

    elif key == 'panic':
        dx = round(3 * math.sin(2 * math.pi * 11 * t))
        for n, i in enumerate(parts(key, 'sweat')):
            ph = ((t + n * 0.13) % 0.7) / 0.7
            vx = (i['x'] + i['w'] / 2 - W / 2); vy = (i['y'] + i['h'] / 2 - H * 0.55)
            d = math.hypot(vx, vy) or 1
            late.append((i, vx / d * 12 * ph, vy / d * 12 * ph, 1.0, 1.0 - ph * ph))
        on = int(t * 1000 // 120) % 2 == 0
        for i in parts(key, 'tremble'):
            late.append((i, dx + (1 if on else -1), 0, 1.0, 1.0 if on else 0.6))
        if blinking(t, [1.6, 4.2, 6.5], 0.1):
            [paste(c, i) for i in parts(key, 'blink')]

    elif key == 'charging':
        sp = parts(key, 'spark')
        if sp:
            late.append((sp[0], 0, round(4 * math.sin(2 * math.pi * t / 1.2)), 1.0, 1.0))
            for n, i in enumerate(sp[1:]):
                ph = (t * 1000 / 250 + n * 1.7) % 4
                late.append((i, 0, 0, 1.0, 1.0 if ph < 2.6 else 0.0))
        if blinking(t, [2.2, 5.8]):
            [paste(c, i) for i in parts(key, 'blink')]

    elif key == 'confused':
        f = int(t * 1000 // M['poses'][key]['hints']['spiral_ms']) % 4
        if f:
            [paste(c, i) for i in parts(key, 'spiral', f)]
        dx = round(2 * math.sin(2 * math.pi * t / 2.0))
        on = int(t * 1000 // 500) % 2 == 0
        for i in parts(key, 'sweat'):
            (late.append((i, dx, 0, 1.0, 1.0)) if i['over'] == 'bg' else paste(c, i)) if on else None

    elif key == 'angry':
        big = int(t * 1000 // 400) % 2 == 0
        for i in parts(key, 'anger_mark'):
            paste(c, i, scale=1.18 if big else 0.92)
        st = (t % 1.5)
        if st < 0.3:
            s = 0.06 * math.sin(math.pi * st / 0.3)
            sy -= s; sx += s * 0.6
            dx = round(2 * math.sin(2 * math.pi * 14 * st))
        if blinking(t, [3.4, 6.9]):
            [paste(c, i) for i in parts(key, 'blink')]

    elif key == 'puzzled':
        for i in parts(key, 'question'):
            late.append((i, 0, round(-4 * (0.5 - 0.5 * math.cos(2 * math.pi * t / 1.2))), 1.0, 1.0))
        if blinking(t, [2.0, 5.5]):
            [paste(c, i) for i in parts(key, 'blink')]

    out = warp(c, sx, sy, dx, dy)
    for i, ddx, ddy, sc, fd in late:
        if fd > 0:
            paste(out, i, ddx, ddy, sc, fd)
    return out


GRID = ['idle', 'thinking', 'speaking', 'sleep', 'panic', 'charging', 'confused', 'angry', 'puzzled']
CELL, GAP, LBL = 360, 12, 34
font = fb.label_font(22)


def compose(t):
    cols = 3
    sheet = Image.new('RGB', (cols * (CELL + GAP) + GAP, 3 * (CELL + LBL + GAP) + GAP), (28, 28, 30))
    d = ImageDraw.Draw(sheet)
    for n, key in enumerate(GRID):
        x = GAP + (n % cols) * (CELL + GAP); y = GAP + (n // cols) * (CELL + LBL + GAP)
        sheet.paste(fb.panel(Image.fromarray(frame(key, t))).resize((CELL, CELL), Image.LANCZOS), (x, y))
        d.text((x + 6, y + CELL + 4), M['poses'][key]['label'], fill=(225, 225, 228), font=font)
    return sheet


if __name__ == '__main__':
    frames = [compose(f / FPS) for f in range(FPS * SECONDS)]
    prev = fb.ROOT / 'preview'
    w, h = frames[0].size
    small = [fr.resize((w // 3, h // 3), Image.LANCZOS) for fr in frames[::2]]
    small[0].save(prev / 'animation_parts.gif', save_all=True, append_images=small[1:],
                  duration=2000 // FPS, loop=0, optimize=True)
    frames[int(FPS * 1.05)].save(prev / 'animation_parts_still.png')
    if shutil.which('ffmpeg'):
        tmp = tempfile.mkdtemp()
        for n, fr in enumerate(frames):
            fr.save(f'{tmp}/{n:04d}.png')
        subprocess.run(['ffmpeg', '-y', '-loglevel', 'error', '-framerate', str(FPS), '-i', f'{tmp}/%04d.png',
                        '-vf', 'pad=ceil(iw/2)*2:ceil(ih/2)*2', '-c:v', 'libx264', '-pix_fmt', 'yuv420p',
                        '-crf', '18', str(prev / 'animation_parts.mp4')], check=True)
        shutil.rmtree(tmp)
    print(len(frames), 'frames', frames[0].size, os.path.getsize(prev / 'animation_parts.gif') // 1024, 'KiB gif')
