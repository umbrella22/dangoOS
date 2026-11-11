#!/usr/bin/env python3
"""Pack PhotoCraft parts as a v4 skin and losslessly recompress existing poses.

Pillow and zopfli are authoring dependencies only. Device decoding remains zlib.
All pre-existing image index bytes are verified after repacking. The palette,
pose order, dimensions, placement and animation groups remain identical.
"""
import argparse
import hashlib
import json
import struct
import zlib
from pathlib import Path

from PIL import Image
import zopfli.zlib

HEADER=struct.Struct('<8sIHHHHIIII')
DESC=struct.Struct('<hhHHIII')


def build(source,art,out):
    binary=(source/'fox.bin').read_bytes()
    magic,version,w,h,colors,poses,ml,bl,crc,reserved=HEADER.unpack_from(binary)
    if magic!=b'MSKIN01\0' or version not in (2,3) or reserved or (w,h)!=(352,336):
        raise ValueError('source must be a v2/v3 indexed skin')
    if len(binary)!=HEADER.size+ml+bl or zlib.crc32(binary[HEADER.size:])!=crc:
        raise ValueError('source length/checksum')
    old_meta=binary[HEADER.size:HEADER.size+ml];old_blob=binary[HEADER.size+ml:]
    meta=bytearray(old_meta[:colors*2]);blob=bytearray();cache={};checks=[]
    # Reuse verified compressed streams during artwork iterations. Matching is
    # on complete decoded index bytes, independent of sprite names/coordinates.
    previous={}
    if (out/'fox.bin').exists():
        old=(out/'fox.bin').read_bytes();head=HEADER.unpack_from(old)
        if head[1]==4 and len(old)==HEADER.size+head[6]+head[7] and zlib.crc32(old[HEADER.size:])==head[8]:
            pm=old[HEADER.size:HEADER.size+head[6]];pb=old[HEADER.size+head[6]:];pa=head[4]*2
            def remember():
                nonlocal pa
                d=DESC.unpack_from(pm,pa);pa+=DESC.size
                z=pb[d[4]:d[4]+d[5]];raw=zlib.decompress(z)
                if len(raw)!=d[2]*d[3]:raise ValueError('cached image length')
                previous[raw]=z
            for _ in range(head[5]):
                pa+=2;remember()
                for _ in range(11):
                    n=struct.unpack_from('<H',pm,pa)[0];pa+=2
                    for _ in range(n):remember()
            for part in ['body','left','right','strain','fall','land','relief','ear-left','ear-right','blink']:
                remember()
                if part in ['left','right','ear-left','ear-right']:pa+=4

    def encode(pixels):
        if pixels not in cache:
            compressed=previous.get(pixels)
            if compressed is None:compressed=zopfli.zlib.compress(pixels,numiterations=5)
            if zlib.decompress(compressed)!=pixels:raise ValueError('compression round trip')
            cache[pixels]=(len(blob),len(compressed));blob.extend(compressed)
        return cache[pixels]

    at=colors*2
    def copy_image():
        nonlocal at
        x,y,iw,ih,off,length,flags=DESC.unpack_from(old_meta,at);at+=DESC.size
        pixels=zlib.decompress(old_blob[off:off+length])
        if len(pixels)!=iw*ih:raise ValueError('source image size')
        noff,nlen=encode(pixels);meta.extend(DESC.pack(x,y,iw,ih,noff,nlen,flags))
        checks.append({'sha256':hashlib.sha256(pixels).hexdigest(),'offset':noff,'length':nlen})

    for pose in range(poses):
        if struct.unpack_from('<H',old_meta,at)[0]!=pose:raise ValueError('source pose order')
        meta.extend(old_meta[at:at+2]);at+=2;copy_image()
        for _ in range(11):
            count=struct.unpack_from('<H',old_meta,at)[0]
            meta.extend(old_meta[at:at+2]);at+=2
            for _ in range(count):copy_image()
    if ml-at != (148 if version==3 else 0):raise ValueError('source metadata layout')

    rgb565=struct.unpack_from('<'+'H'*colors,old_meta)
    palette=[]
    for c in rgb565:palette.extend((((c>>11)&31)*255//31,((c>>5)&63)*255//63,(c&31)*255//31))
    palette.extend([0]*(768-len(palette)));ref=Image.new('P',(1,1));ref.putpalette(palette)
    spec=json.loads((art/'rig.json').read_text());art_hashes={}
    for name in ['body','left','right','strain','fall','land','relief','ear-left','ear-right','blink']:
        path=art/(name+'.png');im=Image.open(path).convert('RGBA')
        if im.size!=(w,h):raise ValueError('art canvas')
        art_hashes[name]=hashlib.sha256(path.read_bytes()).hexdigest()
        box=(0,0,w,h) if name=='body' else im.getbbox()
        if box is None:raise ValueError('empty part '+name)
        im=im.crop(box)
        indexed=bytearray(im.convert('RGB').quantize(palette=ref,dither=Image.Dither.NONE).tobytes())
        black=min(range(colors),key=lambda i:sum(v*v for v in palette[i*3:i*3+3]))
        for i,alpha in enumerate(im.getchannel('A').tobytes()):
            if alpha<128:indexed[i]=255
            elif indexed[i]>=colors:indexed[i]=black
        off,length=encode(bytes(indexed));meta.extend(DESC.pack(box[0],box[1],im.width,im.height,off,length,0))
        if name in spec['parts']:
            px,py=spec['parts'][name]['pivot'];px-=box[0];py-=box[1]
            if not (0<=px<im.width and 0<=py<im.height):raise ValueError('pivot '+name)
            meta.extend(struct.pack('<hh',px,py))
    payload=meta+blob
    packed=HEADER.pack(magic,4,w,h,colors,poses,len(meta),len(blob),zlib.crc32(payload),0)+payload
    if len(packed)>1024*1024:raise ValueError('skin transfer limit')
    # Check each pre-existing sprite against the final on-disk blob.
    for record in checks:
        raw=zlib.decompress(blob[record['offset']:record['offset']+record['length']])
        if hashlib.sha256(raw).hexdigest()!=record['sha256']:raise ValueError('existing sprite changed')
    out.mkdir(parents=True,exist_ok=True);(out/'fox.bin').write_bytes(packed)
    info=json.loads((source/'skin.json').read_text())
    info.update(version='2026-11-11-articulated-v3',binary_bytes=len(packed),binary_crc32=zlib.crc32(packed))
    (out/'skin.json').write_text(json.dumps(info,ensure_ascii=False,indent=2)+'\n')
    report={'id':info['id'],'before_bytes':len(binary),'after_bytes':len(packed),'saved_bytes':len(binary)-len(packed),
            'saved_percent':round((len(binary)-len(packed))*100/len(binary),2),
            'original_images_verified':len(checks),'original_pixels_preserved':True,'art_sha256':art_hashes,
            'format':4,'model':spec['model']}
    return report


def main():
    p=argparse.ArgumentParser(description=__doc__)
    p.add_argument('--source',type=Path,required=True);p.add_argument('--art',type=Path,required=True)
    p.add_argument('--output',type=Path,required=True);p.add_argument('--report',type=Path,required=True)
    a=p.parse_args();report=[]
    for identity in ['default','eclipse','normal','black-blue']:
        r=build(a.source/identity,a.art/identity,a.output/identity);report.append(r)
        print(json.dumps({k:v for k,v in r.items() if k!='art_sha256'}),flush=True)
    a.report.write_text(json.dumps(report,ensure_ascii=False,indent=2)+'\n')


if __name__=='__main__':main()
