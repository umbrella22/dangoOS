#!/usr/bin/env python3
"""Register image completions and export masked PhotoCraft animation layers.

Requires Pillow, NumPy and OpenCV. Generated sheets contain neutral, effort,
release and landing faces in a 2x2 grid. Original paw contours remain separate
from the reconstructed body; editable selections and layers are saved in PSD.
"""
import argparse
import json
import math
import shutil
import subprocess
from pathlib import Path

import cv2
import numpy as np
from PIL import Image, ImageDraw, ImageFilter

from refine_articulated_faces import patch, refine

ROOT = Path(__file__).resolve().parents[2]
SIZE = (352, 336)
IDS = ('default', 'eclipse', 'normal', 'black-blue')
PAWS = {
    'default': [(110, 285, 26, 28), (239, 285, 26, 28)],
    'eclipse': [(117, 309, 23, 23), (235, 309, 23, 23)],
    'normal': [(117, 311, 22, 22), (235, 311, 22, 22)],
    'black-blue': [(115, 310, 23, 23), (238, 310, 23, 23)],
}


def execute(cli, source, commands, destination):
    args = [str(cli), 'run', str(source)]
    for command, params in commands:
        args.extend(['--cmd', command, '--params', json.dumps(params)])
    args.extend(['--out', str(destination)])
    result = subprocess.run(args, capture_output=True, text=True, check=True)
    records = [json.loads(line) for line in result.stdout.splitlines() if line.startswith('{')]
    if any(item.get('ok') is False for item in records):
        raise RuntimeError(result.stdout)
    return records


def silhouette(image):
    """Keep the largest filled silhouette, including dark hair and its outline."""
    a = np.asarray(image.convert('RGBA'))
    visible = ((a[:, :, :3].max(2) > 16) & (a[:, :, 3] > 100)).astype('uint8')
    contours, _ = cv2.findContours(visible, cv2.RETR_EXTERNAL, cv2.CHAIN_APPROX_SIMPLE)
    mask = np.zeros(visible.shape, np.uint8)
    cv2.drawContours(mask, [max(contours, key=cv2.contourArea)], -1, 255, -1)
    return mask


def register_sheet(path, original, identity):
    sheet = Image.open(path).convert('RGBA')
    cell = sheet.width // 2
    cells = [sheet.crop((i % 2 * cell, i // 2 * cell, (i % 2 + 1)*cell, (i // 2 + 1)*cell)) for i in range(4)]
    mask = silhouette(cells[0]); ys, xs = np.where(mask > 0)
    box = (int(xs.min()), int(ys.min()), int(xs.max()+1), int(ys.max()+1))
    om = silhouette(original); oy, ox = np.where(om > 0)
    bottom = 305 if identity == 'default' else 330
    target = (int(ox.min()), int(oy.min()), int(ox.max()+1), bottom)
    width, height = target[2]-target[0], target[3]-target[1]
    aligned = []
    template = cv2.cvtColor(np.asarray(cells[0])[:, :, :3], cv2.COLOR_RGB2GRAY).astype('float32') / 255
    registration = np.zeros(template.shape, np.uint8)
    registration[:int(box[1]+(box[3]-box[1])*.58)] = 255
    for i, im in enumerate(cells):
        pixels = np.asarray(im)
        if i:
            grey = cv2.cvtColor(pixels[:, :, :3], cv2.COLOR_RGB2GRAY).astype('float32') / 255
            matrix = np.eye(2, 3, dtype='float32')
            _, matrix = cv2.findTransformECC(template, grey, matrix, cv2.MOTION_EUCLIDEAN,
                (cv2.TERM_CRITERIA_EPS | cv2.TERM_CRITERIA_COUNT, 100, 1e-5), registration, 5)
            pixels = cv2.warpAffine(pixels, matrix, (cell, cell), flags=cv2.INTER_LINEAR | cv2.WARP_INVERSE_MAP)
        placed = Image.new('RGBA', SIZE)
        placed.paste(Image.fromarray(pixels).crop(box).resize((width,height), Image.Resampling.LANCZOS), target[:2])
        alpha = silhouette(placed)
        placed.putalpha(Image.fromarray(alpha))
        aligned.append(placed)
    return aligned, target


def paw_path(original, shape):
    """Follow the outer ink contour, not a generic ellipse through the artwork."""
    cx,cy,rx,ry = shape
    a = np.asarray(original.convert('RGB')).astype('float32')
    grey = a.mean(2)
    points = []
    for t in np.linspace(0, math.tau, 96, endpoint=False):
        radii = np.linspace(.89, 1.09, 41)
        x = cx + np.cos(t)*rx*radii; y = cy + np.sin(t)*ry*radii
        values = cv2.remap(grey, x.astype('float32')[None], y.astype('float32')[None], cv2.INTER_LINEAR)[0]
        # The inner minimum is the contour; the path runs half a pixel outside.
        score = values + np.abs(radii-.99)*200
        k = int(np.argmin(score)); r = radii[k] + .018
        points.append([round(cx+math.cos(t)*rx*r,3),round(cy+math.sin(t)*ry*r,3)])
    return points


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument('--photocraft', type=Path, required=True)
    p.add_argument('--generation', type=Path, required=True)
    p.add_argument('--output', type=Path, required=True)
    p.add_argument('--eye-clean-sheet', type=Path, help='2x2 image2.5 eye-skin completion; otherwise reuse registered cleanup plates')
    args=p.parse_args(); args.output.mkdir(parents=True,exist_ok=True)
    receipts=[]
    for identity in IDS:
        out=(args.output/identity).resolve();out.mkdir(parents=True,exist_ok=True)
        source=ROOT/('assets/fox-dango/device/poses/idle.png' if identity=='default' else f'assets/dango-family/device/{identity}/idle.png')
        original=Image.open(source).convert('RGBA')
        generation=args.generation/identity
        if identity=='default' and (args.generation/'default-effort').exists() and list((args.generation/'default-effort').glob('*.png')):
            generation=args.generation/'default-effort'
        generated=next(generation.glob('*.png'))
        cells,box=register_sheet(generated,original,identity)
        neutral=cells[0]
        neutral.save(out/'neutral.png')
        for im,name in zip(cells,['neutral','strain','fall','land']): im.save(out/(name+'-registered.png'))
        commands=[('layer.new.layerFromBackground',{}),('layer.renameLayer',{'name':'original'})]
        spec={'source':str(source),'generation':str(generated),'model':'gpt-image-2.5-sunburst','canvas':SIZE,'parts':{}}
        for name,shape in zip(['left','right'],PAWS[identity]):
            points=paw_path(original,shape)
            commands += [('layer.select',{'name':'original'}),('path.set',{'name':name+'-contour','path':{'subpaths':[{'closed':True,'knots':points}]}}),('path.toSelection',{'name':name+'-contour','antiAlias':True}),('layer.new.layerViaCopy',{}),('layer.renameLayer',{'name':name}),('select.deselect',{})]
            spec['parts'][name]={'pivot':[shape[0],shape[1]],'contour':points}
        commands += [('layer.select',{'name':'original'}),('layer.hideLayers',{}),('file.export.layersToFiles',{'dir':str(out/'paws'),'format':'png','trim':False})]
        receipt=execute(args.photocraft,source,commands,out/'original-paw-selections.pcraft')
        for name in ['left','right']:shutil.copyfile(next((out/'paws').glob('*_'+name+'.png')),out/(name+'.png'))
        # The authoring file retains eye and mouth layers individually. Device
        # patches share one bounding box and never contain the jaw silhouette.
        x0,y0,x1,y1=box; w=x1-x0;h=y1-y0
        if identity=='default':
            regions=[(90,193,166,270),(184,193,263,270),(150,253,199,295)]
        else:
            regions=[(108,234,174,300),(182,234,246,300),(153,278,200,322)]
        body=neutral.copy()
        # Ear polygons follow the seam where the front hair occludes the roots.
        ear_polys=[[(x0-2,y0-2),(x0+w*.32,y0+h*.16),(x0+w*.22,y0+h*.30),(x0+w*.11,y0+h*.49),(x0-2,y0+h*.46)],
                   [(x1+2,y0-2),(x1-w*.32,y0+h*.16),(x1-w*.22,y0+h*.30),(x1-w*.11,y0+h*.49),(x1+2,y0+h*.46)]]
        author_layers=[]
        for name,poly in zip(['ear-left','ear-right'],ear_polys):
            mask=Image.new('L',SIZE);ImageDraw.Draw(mask).polygon(poly,fill=255)
            # The hidden root overlaps the head, so a small ear rotation cannot
            # open a black crack along the hair seam.
            a=np.minimum(np.asarray(mask.filter(ImageFilter.MaxFilter(11))),np.asarray(neutral.getchannel('A')))
            ear=neutral.copy();ear.putalpha(Image.fromarray(a));ear.save(out/(name+'.png'))
            ba=np.asarray(body.getchannel('A')).copy();ba[np.asarray(mask)>0]=0;body.putalpha(Image.fromarray(ba))
            bb=ear.getbbox();spec['parts'][name]={'pivot':[round((bb[0]+bb[2])/2),bb[3]-4]}
            author_layers.append(name)
        body.save(out/'body.png')
        spec['face_regions']=regions
        (out/'rig.json').write_text(json.dumps(spec,ensure_ascii=False,indent=2)+'\n')
        patch(cells[2],neutral,regions[2]).save(out/'fall-mouth.png')
        refine(args.photocraft,out,identity,args.eye_clean_sheet)
        receipts.append({'id':identity,'generation':str(generated),'paw_selection':receipt,'layers':20})
        print(identity,'layers',20,flush=True)
    (args.output/'receipt.json').write_text(json.dumps(receipts,ensure_ascii=False,indent=2)+'\n')


if __name__=='__main__': main()
