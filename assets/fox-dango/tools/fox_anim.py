"""Device-faithful animation prototype: pose sprites + blink patch + procedural motion.

Usage: python3 assets/fox-dango/tools/fox_anim.py  (needs numpy, pillow, scipy;
ffmpeg optional for the MP4). Writes preview/animation_prototype.{gif,mp4}.
Every transform is nearest-neighbour on the quantized sprite, as on device.
"""
import math, os, shutil, subprocess, sys, tempfile
import numpy as np
from PIL import Image
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import fox_build as fb

ims = fb.load_all(extra=['idle_blink'])
ims['blink'] = ims.pop('idle_blink')
q, _ = fb.quantize(ims)
A = {k: np.asarray(v) for k, v in q.items()}
H, W = fb.CANVAS_H, fb.CANVAS_W

# Blink patch: the two largest changed blobs between idle and its aligned
# blink edit, measured before quantization so dither noise does not count.
from scipy import ndimage
raw = np.abs(np.asarray(ims['idle']).astype(int) - np.asarray(ims['blink']).astype(int)).sum(2) > 50
raw = ndimage.binary_opening(raw, iterations=2)
lab, n = ndimage.label(ndimage.binary_dilation(raw, iterations=4))
sizes = ndimage.sum(raw, lab, range(1, n + 1))
keep = np.argsort(sizes)[-2:] + 1
ys, xs = np.where(np.isin(lab, keep))
y0, y1, x0, x1 = ys.min() - 2, ys.max() + 3, xs.min() - 2, xs.max() + 3
BLINK_BOX = (x0, y0, x1, y1)
print('blink patch', BLINK_BOX, f'{(x1-x0)}x{(y1-y0)} px')

def with_blink(key):
    a = A[key].copy()
    a[y0:y1, x0:x1] = A['blink'][y0:y1, x0:x1]
    return a

def warp(a, sx, sy, dx=0, dy=0):
    """Nearest-neighbour scale about the bottom centre, like a row/column LUT on device."""
    ay, ax = H - 1, W / 2
    yy = np.arange(H); xx = np.arange(W)
    sy_idx = np.round(ay - (ay - (yy - dy)) / sy).astype(int)
    sx_idx = np.round(ax + (xx - dx - ax) / sx).astype(int)
    vy = (sy_idx >= 0) & (sy_idx < H); vx = (sx_idx >= 0) & (sx_idx < W)
    out = np.zeros_like(a)
    sub = a[np.clip(sy_idx, 0, H - 1)][:, np.clip(sx_idx, 0, W - 1)]
    out[np.ix_(vy, vx)] = sub[np.ix_(vy, vx)]
    return out

FPS = 20
def spring(t, amp=0.12, tau=0.10, period=0.24):
    return amp * math.exp(-t / tau) * math.cos(2 * math.pi * t / period)

# Timeline: (start_s, key, mode)
TL = [(0.0, 'idle', 'calm'), (2.6, 'happy', 'hop'), (4.6, 'idle', 'calm'),
      (6.6, 'panic', 'shake'), (8.2, 'idle', 'calm'), (9.6, 'sleep', 'sleep'), (12.0, None, None)]
BLINKS = [1.3, 5.6, 9.0]
frames = []
total = TL[-1][0]
for f in range(int(total * FPS)):
    t = f / FPS
    i = max(j for j in range(len(TL) - 1) if TL[j][0] <= t)
    t0, key, mode = TL[i]; lt = t - t0
    # breathing: period and depth depend on the state
    period, depth = {'calm': (3.0, 0.015), 'hop': (2.0, 0.012), 'shake': (0.6, 0.02), 'sleep': (4.5, 0.025)}[mode]
    sy = 1 + depth * math.sin(2 * math.pi * lt / period)
    sx = 1 - depth * 0.5 * math.sin(2 * math.pi * lt / period)
    dx = dy = 0
    # switch "pop": squash on entry, swap pose at peak squash, spring back
    if i > 0 and lt < 0.6:
        s = spring(lt)
        sy -= s; sx += s * 0.6
    if mode == 'hop':
        ph = (lt - 0.35) % 0.55
        if 0.35 < lt < 1.8 and ph < 0.4:
            dy = -int(18 * math.sin(math.pi * ph / 0.4))
        elif 0.35 < lt < 1.8 and ph < 0.55:
            s = 0.08 * math.sin(math.pi * (ph - 0.4) / 0.15); sy -= s; sx += s * 0.6
    if mode == 'shake':
        dx = int(round(3 * math.sin(2 * math.pi * 11 * lt)))
    # before the swap peak, keep showing the previous pose
    shown = key if (i == 0 or lt >= 0.05) else TL[i - 1][1]
    blinking = any(0 <= t - b < 0.15 for b in BLINKS) and shown == 'idle'
    a = with_blink('idle') if blinking else A[shown]
    frames.append(fb.panel(Image.fromarray(warp(a, sx, sy, dx, dy))))

out = fb.ROOT / 'preview' / 'animation_prototype.gif'
small = [fr.resize((360, 360), Image.NEAREST) for fr in frames]
small[0].save(out, save_all=True, append_images=small[1:], duration=1000 // FPS, loop=0, optimize=True)
if shutil.which('ffmpeg'):
    tmp = tempfile.mkdtemp()
    for n, fr in enumerate(frames):
        fr.save(f'{tmp}/{n:04d}.png')
    subprocess.run(['ffmpeg', '-y', '-loglevel', 'error', '-framerate', str(FPS), '-i', f'{tmp}/%04d.png',
                    '-c:v', 'libx264', '-pix_fmt', 'yuv420p', '-crf', '16',
                    str(fb.ROOT / 'preview' / 'animation_prototype.mp4')], check=True)
    shutil.rmtree(tmp)
print(len(frames), 'frames,', os.path.getsize(out) // 1024, 'KiB gif')
