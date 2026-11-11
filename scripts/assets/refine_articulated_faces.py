#!/usr/bin/env python3
"""Clean eye plates in PhotoCraft and paint consistent blink/dizzy patches."""
import argparse
import json
import math
import shutil
import subprocess
from pathlib import Path

import numpy as np
from PIL import Image, ImageDraw, ImageFilter

SIZE = (352, 336)
IDS = ('default', 'eclipse', 'normal', 'black-blue')


def execute(cli, source, commands, destination):
    args = [str(cli), 'run', str(source)]
    for command, params in commands:
        args.extend(['--cmd', command, '--params', json.dumps(params)])
    args.extend(['--out', str(destination)])
    result = subprocess.run(args, capture_output=True, text=True)
    if result.returncode:
        raise RuntimeError(f'PhotoCraft failed for {destination}: {result.stderr[-1600:]}')
    records = [json.loads(line) for line in result.stdout.splitlines() if line.startswith('{')]
    if any(record.get('ok') is False for record in records):
        raise RuntimeError(result.stdout)
    return records


def stroke(name, points, color, size, closed=False):
    return [
        ('path.set', {'name': name, 'path': {'subpaths': [{'closed': closed, 'knots': points}]}}),
        ('path.stroke', {'name': name, 'tool': 'brush', 'size': size, 'hardness': 1, 'opacity': 100, 'color': color}),
    ]


def patch(image, neutral, region):
    mask = Image.new('L', SIZE)
    ImageDraw.Draw(mask).rounded_rectangle(region, radius=8, fill=255)
    mask = mask.filter(ImageFilter.GaussianBlur(0.6))
    # Bake the seam once into RGB. A second feather during compositing would
    # expose the old iris beneath a partially transparent replacement plate.
    pixels = Image.composite(image, neutral, mask)
    pixels.putalpha(mask.point(lambda v: 255 if v else 0))
    return pixels


def register_clean_sheet(sheet_path, out, identity, neutral):
    import cv2
    sheet = Image.open(sheet_path).convert('RGB')
    cell = sheet.width // 2
    index = IDS.index(identity)
    image = sheet.crop((index%2*cell,index//2*cell,(index%2+1)*cell,(index//2+1)*cell))
    image = image.resize((512,512),Image.Resampling.LANCZOS).crop((10,22,502,492)).resize(SIZE,Image.Resampling.LANCZOS)
    reference = Image.new('RGB',SIZE);reference.paste(neutral,mask=neutral.getchannel('A'))
    template = cv2.cvtColor(np.asarray(reference),cv2.COLOR_RGB2GRAY).astype('float32')/255
    moving = cv2.cvtColor(np.asarray(image),cv2.COLOR_RGB2GRAY).astype('float32')/255
    mask = np.zeros((SIZE[1],SIZE[0]),np.uint8)
    mask[:190 if identity=='default' else 235] = 255
    matrix = np.eye(2,3,dtype='float32')
    _,matrix = cv2.findTransformECC(template,moving,matrix,cv2.MOTION_AFFINE,
        (cv2.TERM_CRITERIA_EPS|cv2.TERM_CRITERIA_COUNT,150,1e-6),mask,5)
    aligned = cv2.warpAffine(np.asarray(image),matrix,SIZE,flags=cv2.INTER_LINEAR|cv2.WARP_INVERSE_MAP)
    Image.fromarray(aligned).save(out/'eyes-clean-registered.png')


def refine(cli, out, identity, clean_sheet=None):
    neutral = Image.open(out/'neutral.png').convert('RGBA')
    spec = json.loads((out/'rig.json').read_text())
    regions = [(90,193,166,270),(184,193,263,270),(157,254,198,283)] if identity=='default' else [(98,234,177,302),(179,234,254,302),(160,283,196,311)]
    if identity=='black-blue':
        # GLM's neutral brows sit above the other characters' eye plates. Cover
        # them too, so expressive brows do not stack over the old straight ones.
        regions[:2]=[(98,218,177,302),(179,218,254,302)]
    if clean_sheet:
        register_clean_sheet(clean_sheet,out,identity,neutral)
    plate = out/'eyes-clean-registered.png'
    if not plate.exists():
        raise ValueError('a registered eye cleanup plate or --clean-sheet is required')
    ink = '#761e25' if identity in ('default','eclipse') else '#352a40'
    subpaths=[]
    for l,t,r,b in regions[:2]:
        subpaths.append({'closed':True,'knots':[[l,t],[r,t],[r,b],[l,b]]})
    commands=[
        ('layer.renameLayer',{'name':'neutral-reference'}),
        ('file.placeEmbedded',{'path':str(plate.resolve()),'fit':False,'smartObject':False,'center':[176,168]}),
        ('layer.renameLayer',{'name':'eye-skin-repair'}),
        ('path.set',{'name':'eye-repair-areas','path':{'subpaths':subpaths}}),
        ('path.toSelection',{'name':'eye-repair-areas','antiAlias':True,'feather':2.0}),
        ('layer.layerMask.revealSelection',{}),
        ('select.deselect',{}),
    ]
    records = execute(cli,out/'neutral.png',commands,out/'eyes-clean.pcraft')
    subprocess.run([str(cli),'convert',str(out/'eyes-clean.pcraft'),str(out/'eyes-clean.png')],check=True,capture_output=True)
    centers = [(132,231),(221,231)] if identity=='default' else [(138,271),(217,271)]
    if identity=='black-blue': centers=[(137,273),(218,273)]
    blink_commands=[];dizzy_commands=[];wink_commands=[]
    for n,(cx,cy) in enumerate(centers):
        half = 21 if identity=='default' else 17
        points=[(cx-half+2*half*i/40,cy-3*math.sin(math.pi*i/40)) for i in range(41)]
        blink_commands += stroke(f'closed-eye-{n}',points,ink,2.2)
        if n==0:
            wink_commands=stroke('effort-wink',[(cx-half,cy-8),(cx+half*.75,cy),(cx-half,cy+9)],ink,3.4 if identity=='default' else 2.8)
        radius=13 if identity=='default' else 10
        spiral=[]
        for i in range(121):
            t=i/120
            angle=t*math.tau*2.1
            r=1.0+t*radius
            spiral.append((cx+math.cos(angle)*r,cy+math.sin(angle)*r))
        dizzy_commands += stroke(f'dizzy-eye-{n}',spiral,ink,2.0)
    for name,actions in [('blink',blink_commands),('dizzy',dizzy_commands),('wink',wink_commands)]:
        records += execute(cli,out/'eyes-clean.png',actions,out/(name+'-clean.png'))
    for name in ['strain','land']:
        image=Image.open(out/(name+'-registered.png')).convert('RGBA')
        combined=Image.new('RGBA',SIZE)
        for region,feature in zip(regions,['eye-left','eye-right','mouth']):
            source = Image.open(out/'wink-clean.png').convert('RGBA') if name=='strain' and feature=='eye-left' else image
            part=patch(source,neutral,region)
            part.save(out/(name+'-'+feature+'.png'))
            combined.alpha_composite(part)
        combined.save(out/(name+'.png'))
    for name,source in [('blink','blink-clean.png'),('fall','dizzy-clean.png')]:
        image=Image.open(out/source).convert('RGBA')
        combined=Image.new('RGBA',SIZE)
        for region,feature in zip(regions,['eye-left','eye-right','mouth']):
            if feature=='mouth' and name=='fall':
                part=patch(Image.open(out/'fall-registered.png').convert('RGBA'),neutral,region)
            else:
                part=patch(image,neutral,region)
            part.save(out/(name+'-'+feature+'.png'))
            combined.alpha_composite(part)
        combined.save(out/(name+'.png'))
    # The relaxed half-lidded face persists from stop through recovery. There
    # is no extra closed-eye smile between it and the restored neutral face.
    for suffix in ['', '-eye-left','-eye-right','-mouth']:
        shutil.copyfile(out/('land'+suffix+'.png'),out/('relief'+suffix+'.png'))
    spec['faces']={'strain':'effort-wink','fall':'dizzy','land':'weary-after-stop','relief':'weary-after-stop','blink':'clean-closed-eyes'}
    spec['face_regions']=regions
    spec['cleanup']='image2.5 eye-skin completion, PhotoCraft path masks and antialiased eye strokes'
    (out/'rig.json').write_text(json.dumps(spec,ensure_ascii=False,indent=2)+'\n')
    names=['ear-left','ear-right']+[state+'-'+part for state in ['strain','fall','land','relief','blink'] for part in ['eye-left','eye-right','mouth']]+['left','right']
    commands=[('layer.renameLayer',{'name':'body'})]
    for name in names:
        commands += [('file.placeEmbedded',{'path':str((out/(name+'.png')).resolve()),'fit':False,'smartObject':False,'center':[176,168]}),('layer.renameLayer',{'name':name})]
        if name.startswith(('strain','fall','land','relief','blink')):commands.append(('layer.hideLayers',{}))
    records += execute(cli,out/'body.png',commands,out/(identity+'-articulated.pcraft'))
    subprocess.run([str(cli),'convert',str(out/(identity+'-articulated.pcraft')),str(out/(identity+'-articulated.psd'))],check=True,capture_output=True)
    (out/'face-cleanup-receipt.json').write_text(json.dumps(records,ensure_ascii=False,indent=2)+'\n')


if __name__=='__main__':
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--photocraft',type=Path,required=True)
    parser.add_argument('--art',type=Path,required=True)
    parser.add_argument('--clean-sheet',type=Path)
    args=parser.parse_args()
    for identity in IDS:
        refine(args.photocraft,(args.art/identity).resolve(),identity,args.clean_sheet)
        print(identity,'PhotoCraft eye cleanup and face layers exported',flush=True)
