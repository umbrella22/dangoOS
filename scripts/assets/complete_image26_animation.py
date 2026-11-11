#!/usr/bin/env python3
"""Generate geometry-preserving blink, gaze, mouth and occlusion completions."""
import argparse
import json
import subprocess
import sys
from pathlib import Path

from PIL import Image, ImageDraw, ImageFont

from refresh_companion_expressions import ROOT, IDS, MODEL, BRIDGE, fingerprint


def prepare(work):
    manifest = json.loads((ROOT / 'assets/fox-dango/device/manifest.json').read_text())
    cells = [{'id': 'default', 'pose': name, 'edit': 'blink'}
             for name, pose in manifest['poses'].items() if 'blink' in pose['parts']]
    cells += [{'id': identity, 'pose': 'idle', 'edit': 'blink'} for identity in IDS[1:]]
    cells += [{'id': identity, 'pose': 'idle', 'edit': 'remove-hands'} for identity in IDS]
    cells += [{'id': 'default', 'pose': pose, 'edit': edit}
              for pose in ['idle', 'peering'] for edit in ['gaze-left', 'gaze-right']]
    cells += [{'id': 'default', 'pose': 'speaking', 'edit': edit} for edit in ['mouth-closed', 'mouth-wide']]
    cells += [{'id': 'black-blue', 'pose': 'idle', 'edit': 'clean-face'}]
    assert len(cells) == 30
    folder = work / 'animation-completion'; folder.mkdir(exist_ok=True)
    font = ImageFont.truetype('/System/Library/Fonts/Supplemental/Arial.ttf', 16)
    descriptions = {
        'blink': 'Close both eyes naturally. Replace the complete old iris, eye white, highlights and eyelash contour with ONE elegant clean closed eyelid each. Keep the mouth and eyebrows, preserve the expression mood. No open-eye fragments or double lids.',
        'remove-hands': 'REMOVE BOTH round mitten paws completely. Reconstruct the round face outline and hair that were hidden beneath them. Keep the entire upper head, face expression, ears and accessories identical. It becomes just the round mochi head. Do not draw arms, stumps, fingers, or any body. Black background below the reconstructed head.',
        'gaze-left': 'Move ONLY the pupils/irises a few pixels toward the left side of the IMAGE. Keep the eyelids and whites fixed. Keep the head, face, mouth, hands and ears identical.',
        'gaze-right': 'Move ONLY the pupils/irises a few pixels toward the right side of the IMAGE. Keep the eyelids and whites fixed. Keep the head, face, mouth, hands and ears identical.',
        'mouth-closed': 'Change ONLY the mouth to a small softly closed speaking-rest mouth. Remove the old open mouth entirely. Eyes, hands, hair and entire silhouette remain identical.',
        'mouth-wide': 'Change ONLY the mouth to a slightly wider open speaking syllable, with a small pink tongue and a natural upper rim. Cute and small, not screaming. Eyes and everything else remain identical.',
        'clean-face': 'Remove both complete eyes, lashes, brows and mouth. Fill clean skin behind them. Preserve the bangs/hair crossing the face exactly. Keep the paws, head and accessories identical.',
    }
    for batch in range(5):
        items = cells[batch * 6:(batch + 1) * 6]
        board = Image.new('RGB', (1536, 1024))
        draw = ImageDraw.Draw(board)
        for i, item in enumerate(items):
            source = work / 'art' / item['id'] / (item['pose'] + '-device.png')
            im = Image.open(source).convert('RGB').resize((440, 420), Image.Resampling.LANCZOS)
            x, y = i % 3 * 512, i // 3 * 512
            board.paste(im, (x + 36, y + 64))
            draw.text((x + 16, y + 12), f"{i + 1} {item['id']} / {item['pose']} / {item['edit']}", font=font, fill=(140, 150, 164))
            item['source_sha256'] = fingerprint(source)
        board.save(folder / f'{batch:02}-reference.png')
        prompt = '''This is a registered animation-patch editing job, not a redraw or redesign. Modify the provided finished anime sprite sheet ONLY in the specific regions requested below. Preserve every other line, color, hairstyle, accessory, tiny round mitten paw, shading detail, head tilt, silhouette, scale and position exactly. No arms or extra anatomy. The exact character must remain the same. Return exactly 1536x1024, three columns by two rows of 512x512. The sprites stay at exactly their reference positions and sizes in their corresponding cells. Pure solid RGB(0,0,0) background; no labels, text, borders or decorations. Keep all cells separated. Do not magnify the characters to fill the black padding.

Left-to-right then top-to-bottom:
''' + '\n'.join(f"{i + 1}. {item['id']}, {item['pose']}: {descriptions[item['edit']]}" for i, item in enumerate(items))
        (folder / f'{batch:02}-prompt.txt').write_text(prompt + '\n')
        (folder / f'{batch:02}-cells.json').write_text(json.dumps(items, ensure_ascii=False, indent=2) + '\n')


def prepare_rig(work):
    for identity in IDS:
        folder = work / 'rig-completion' / identity; folder.mkdir(parents=True, exist_ok=True)
        base = Image.open(work / 'art' / identity / 'idle-device.png').convert('RGB').resize((440, 420), Image.Resampling.LANCZOS)
        board = Image.new('RGB', (1536, 1024))
        names = ['neutral', 'effort', 'dizzy', 'weary', 'blink', 'clean-face']
        for i in range(6): board.paste(base, (i % 3 * 512 + 36, i // 3 * 512 + 64))
        board.save(folder / '00-reference.png')
        prompt = '''Create six precisely registered expression layers from this single approved anime mochi-head character. This is an animation rig paint-completion task. Exactly 1536x1024, 3 columns by 2 rows, each cell 512x512. Preserve the exact input head shape, size, position, hair strands, ear silhouette, accessories, palette and shading in ALL six cells. No labels or text. Pure solid RGB(0,0,0) background, all art contained in its own cell.

IN EVERY CELL: REMOVE BOTH small round paws completely and seamlessly reconstruct the small areas of jaw/cheek and hair hidden underneath them. Draw ONLY the round head, hair, ears and existing hair accessories. There are NO hands, arms, limbs, stump shapes, neck, torso or shoulders anywhere. Do NOT add new floating objects. The detached mitten paws will be animated separately. The head should have the same unobstructed curved underside in all six cells. Do not change the ear poses or head tilt between cells.

Only the facial expression varies, left-to-right then top-to-bottom:
1. Neutral: same original face and open eyes.
2. Effort: the IMAGE LEFT eye is squeezed shut into a small > shape, the right eye stays open, concentrating. Slightly tense tiny mouth. One closed eye only.
3. Dizzy: two clean spiral eyes; remove ALL underlying old irises, lids, highlights and lashes. A small wavy mouth. No extra brows or stars.
4. Weary: softly half-lidded eyes, quietly resigned, small flat mouth. Mild helplessness, NOT angry or sad. Complete new lids and brows with no old lines behind them.
5. Blink: both eyes naturally closed, one elegant simple eyelid line each, no irises, highlights or eye whites. Keep the original neutral mouth.
6. Clean face: remove ALL eyes, eyelashes, eyebrows AND the mouth, fill with clean skin for a repair plate; keep bangs crossing the face exactly intact.

Keep skin color identical in all cells, no speckles or grain. No eyelashes left behind after an eye is replaced. Clean concise anime lines, identical hair and accessories in all cells. Hair must preserve the reference's MATTE two-or-three-tone cel shading exactly: no white gloss streaks, misplaced shine spots, smeared brush texture, glowing hair or new highlights. Geometry registration and the complete absence of paws/arms are essential.'''
        (folder / '00-prompt.txt').write_text(prompt + '\n')
        (folder / '00-cells.json').write_text(json.dumps([{'id': identity, 'edit': name, 'pose': 'idle'} for name in names], indent=2) + '\n')


def generate(work, batch, identity=None):
    folder = work / 'animation-completion' if identity is None else work / 'rig-completion' / identity
    result = folder / f'{batch:02}-result.json'
    if result.exists():
        return
    started = folder / f'{batch:02}-started.json'
    if started.exists():
        raise RuntimeError('Unconfirmed completion request; inspect it before resending')
    reference = folder / f'{batch:02}-reference.png'
    started.write_text(json.dumps({'model': MODEL, 'reference_sha256': fingerprint(reference)}))
    proc = subprocess.run([sys.executable, str(BRIDGE), '--mode', 'edit', '--model', MODEL,
        '--size', '4:3', '--image', str(reference), '--output-dir', str(folder / f'{batch:02}-output'),
        (folder / f'{batch:02}-prompt.txt').read_text()], capture_output=True, text=True)
    (folder / f'{batch:02}-response.log').write_text(proc.stdout + proc.stderr)
    if proc.returncode:
        raise RuntimeError(f'Completion {batch} failed; inspect its response log')
    receipt = json.loads(proc.stdout)
    if not receipt.get('file'):
        raise RuntimeError('No local image returned')
    receipt['sha256'] = fingerprint(Path(receipt['file']))
    receipt['cells'] = json.loads((folder / f'{batch:02}-cells.json').read_text())
    result.write_text(json.dumps(receipt, ensure_ascii=False, indent=2) + '\n')
    print('animation completion', batch, receipt['file'], flush=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('command', choices=['prepare', 'generate', 'prepare-rig', 'generate-rig'])
    parser.add_argument('--work', type=Path, required=True)
    parser.add_argument('--batch', type=int, choices=range(5))
    parser.add_argument('--id', choices=IDS)
    args = parser.parse_args(); work = args.work.resolve()
    if args.command == 'prepare': prepare(work)
    elif args.command == 'prepare-rig': prepare_rig(work)
    elif args.command == 'generate-rig':
        if args.id is None: parser.error('generate-rig requires --id')
        generate(work, 0, args.id)
    elif args.batch is None: parser.error('generate requires --batch')
    else: generate(work, args.batch)


if __name__ == '__main__': main()
