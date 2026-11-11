#!/usr/bin/env python3
"""Author image-2.6 expression sheets, refine in PhotoCraft, and pack v4 skins.

All generation inputs and receipts are kept in the requested work directory.
The image provider is invoked only through the configured image bridge.
"""
import argparse
import hashlib
import json
import subprocess
import sys
from pathlib import Path

from PIL import Image, ImageDraw, ImageFont

ROOT = Path(__file__).resolve().parents[2]
IDS = ('default', 'eclipse', 'normal', 'black-blue')
MODEL = 'gpt-image-2.6-sunburst'
SIZE = (352, 336)
POSES = ('idle sleep curious happy puzzled confused surprised panic angry satisfied '
         'listening thinking speaking error tired dying annoyed charging peering dazzled '
         'network-disconnected network-connected bluetooth-disconnected bluetooth-connected music-playing').split()
EXTRAS = ['effort', 'dizzy', 'weary', 'blink', 'clean-face']
LABELS = dict(zip(POSES + EXTRAS, [
    '待机', '睡觉', '好奇', '开心', '疑惑', '晕眩', '惊讶', '慌张', '生气', '满足',
    '聆听', '思考', '说话', '出错', '疲惫', '快没电', '无奈', '充电', '暗处张望', '强光刺眼',
    '网络断开', '网络连接', '蓝牙断开', '蓝牙连接', '音乐播放', '用力单闭眼', '旋转后晕眩',
    '停下后无奈', '自然眨眼', '干净面部底板']))
NAMES = {'default': '心月狐', 'eclipse': '月蚀', 'normal': '常态', 'black-blue': '智谱团子 GLM'}
BRIDGE = Path('/Users/ikaros/.agents/skills/codex-image-gen-bridge/scripts/bridge.py')


def fingerprint(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def original(identity, name, work):
    if identity == 'default' and (work / 'style.json').exists():
        identity = json.loads((work / 'style.json').read_text()).get('default_geometry', identity)
    if name in POSES:
        folder = ROOT / ('assets/fox-dango/device/poses' if identity == 'default'
                         else f'assets/dango-family/device/{identity}')
        return Image.open(folder / (name + '.png')).convert('RGB')
    folder = work / 'before/articulated' / identity
    im = Image.open(folder / 'neutral.png').convert('RGBA')
    if name in ('effort', 'dizzy', 'weary', 'blink'):
        file = {'effort': 'strain', 'dizzy': 'fall', 'weary': 'land', 'blink': 'blink'}[name]
        im.alpha_composite(Image.open(folder / (file + '.png')).convert('RGBA'))
    else:
        # The model completes a clean skin plate while preserving bangs.
        im = Image.open(folder / 'eyes-clean.png').convert('RGBA')
    for hand in ('left', 'right'):
        im.alpha_composite(Image.open(folder / (hand + '.png')).convert('RGBA'))
    background = Image.new('RGB', SIZE)
    background.paste(im, mask=im.getchannel('A'))
    return background


def prepare(work):
    font = ImageFont.truetype('/System/Library/Fonts/Supplemental/Arial.ttf', 17)
    descriptions = {
        'default': 'cream-white hair, pale pink and red accents, warm burgundy eyes, fox ears and the exact original small red hair ornament',
        'eclipse': 'silver-white hair, red eyes, black-and-red gothic headband and ribbons, exact original hair ornament',
        'normal': 'ivory-white hair, blue-violet eyes, white lace maid headband, navy hair ornaments',
        'black-blue': 'charcoal-black hair with cobalt-blue tips and streaks, grey-violet eyes, white lace maid headband, small Z hair clip and bell',
    }
    rows = []
    for identity in IDS:
        out = work / 'generation' / identity
        out.mkdir(parents=True, exist_ok=True)
        for batch in range(5):
            names = (POSES + EXTRAS)[batch * 6:(batch + 1) * 6]
            board = Image.new('RGB', (1536, 1024), (0, 0, 0))
            draw = ImageDraw.Draw(board)
            for index, name in enumerate(names):
                x, y = index % 3 * 512, index // 3 * 512
                im = original(identity, name, work).resize((440, 420), Image.Resampling.LANCZOS)
                board.paste(im, (x + 36, y + 64))
                draw.text((x + 22, y + 16), f'{index + 1:02} {name}', font=font, fill=(151, 159, 178))
            reference = out / f'{batch:02}-reference.png'
            board.save(reference)
            prompt = f'''Refine this exact six-cell character expression sheet for a tiny embedded-screen anime mascot. This is a faithful second-pass cleanup and charm upgrade of an existing approved character, not a redesign.

OUTPUT LAYOUT: exactly 1536x1024, exactly three columns and two rows of 512x512 cells. Keep the character in every cell in the same position, size, head tilt, flattened or tall silhouette, ear pose, hand placement, and icon arrangement as its matching input cell. Pure solid black RGB(0,0,0) background. No border, no floor, no shadow outside the character, no typography and no cell labels in the output. Every ear, hair strand, little hand and floating symbol must be wholly inside its cell, with at least 20 pixels of black padding. Keep the bottom margin. Do not combine or repeat cells.

CHARACTER IDENTITY: {NAMES[identity]}. {descriptions[identity]}. Preserve hairstyle, accessories, original palette, round head proportions and recognizability. It is only a soft round mochi head with ears and two small round mitten paws. No torso, neck, shoulders, arms, elbows, fingers, legs, or extra limbs. Detached tiny paws are intentional. Do not connect paws with arms. Do not enlarge eyes into a different character.

ART DIRECTION: polished cute Japanese chibi anime key animation; more expressive and alive through clean asymmetry of brows, clear little mouth shapes, expressive irises and restrained blush. Confident fine continuous ink contours, smooth compact cel-shaded color areas, a few intentional soft gradients, uncluttered bright eye highlights, cleaner hair grouping and accessory edges. Avoid grain, stippling, stippled blush, noisy texture, paint smears, photorealism or 3D. Keep tiny-screen readability and compact indexed-color storage. Eyelashes must belong to the new eyelid: never leave a second old lid or brow behind. Closed eyes have one intentional clean line each, no leftover open-eye reflections or floating lash fragments. Mouths are small and anatomically consistent in this stylization. Angry fangs, when present, are one small upper tooth pointing down from the upper-right mouth rim, never a lower tooth.

EXPRESSION ORDER, left-to-right then top-to-bottom:
''' + '\n'.join(f'{i + 1}. {name}: {LABELS[name]}; preserve the matching reference expression and all its semantic symbols.' for i, name in enumerate(names)) + '''

For tired / dying / error keep the low wide squashed body; dying is almost closed sleepy eyes, not X eyes. Keep Wi-Fi versus Bluetooth symbols distinct and retain the small success check versus disconnected cross accurately. For effort: only the left eye is squeezed shut, right eye open and concentrating. For dizzy: two clean spiral eyes, not ordinary eyes behind spirals. For weary: mild half-lidded resignation and a tiny flat mouth, not anger. For blink: relaxed closed lids, no iris, white eyeball or highlight. For clean-face: remove BOTH entire eyes, brows and mouth, leave smooth intact skin, retain bangs and hair that cross the face; it is a paint repair plate, not an expression.

Please redraw all six matching cells carefully and keep them perfectly separated. The character design and proportions must remain consistent across the sheet.'''
            (out / f'{batch:02}-prompt.txt').write_text(prompt + '\n')
            rows.append({'id': identity, 'batch': batch, 'expressions': names,
                         'reference': str(reference.relative_to(work)), 'reference_sha256': fingerprint(reference),
                         'model': MODEL})
    (work / 'generation-plan.json').write_text(json.dumps(rows, ensure_ascii=False, indent=2) + '\n')


def generate(work, identity, batch):
    folder = work / 'generation' / identity
    result = folder / f'{batch:02}-result.json'
    if result.exists():
        receipt = json.loads(result.read_text())
        if receipt.get('file') and Path(receipt['file']).exists():
            print(identity, batch, 'already generated', flush=True)
            return
        raise RuntimeError('Existing result needs inspection before resending')
    prompt = (folder / f'{batch:02}-prompt.txt').read_text()
    args = [sys.executable, str(BRIDGE), '--mode', 'edit', '--model', MODEL,
            '--size', '4:3', '--image', str(folder / f'{batch:02}-reference.png'),
            '--output-dir', str(folder / f'{batch:02}-output'), prompt]
    started = folder / f'{batch:02}-started.json'
    if started.exists():
        raise RuntimeError('An earlier unconfirmed request exists; inspect it before resending')
    started.write_text(json.dumps({'model': MODEL, 'reference_sha256': fingerprint(folder / f'{batch:02}-reference.png')}))
    proc = subprocess.run(args, capture_output=True, text=True)
    (folder / f'{batch:02}-response.log').write_text(proc.stdout + proc.stderr)
    if proc.returncode:
        raise RuntimeError(f'{identity} batch {batch} failed; inspect its response log')
    receipt = json.loads(proc.stdout)
    if not receipt.get('file'):
        raise RuntimeError('Generation returned no local image; inspect its result')
    receipt['sha256'] = fingerprint(Path(receipt['file']))
    receipt['expressions'] = (POSES + EXTRAS)[batch * 6:(batch + 1) * 6]
    result.write_text(json.dumps(receipt, ensure_ascii=False, indent=2) + '\n')
    print(identity, batch, receipt['file'], flush=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('command', choices=['prepare', 'generate'])
    parser.add_argument('--work', type=Path, required=True)
    parser.add_argument('--id', choices=IDS)
    parser.add_argument('--batch', type=int, choices=range(5))
    args = parser.parse_args()
    work = args.work.resolve()
    if args.command == 'prepare':
        prepare(work)
    else:
        if args.id is None or args.batch is None:
            parser.error('generate requires --id and --batch')
        generate(work, args.id, args.batch)


if __name__ == '__main__':
    main()
