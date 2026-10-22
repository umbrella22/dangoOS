"""Compare real image-module output to independent Pillow/pixel fixtures.

Run after n5_image_probe: python3 scripts/n5-check-images.py <output-dir>.
Requires Pillow; this script performs no serial or device operations.
"""
from pathlib import Path
import argparse, hashlib, io, json, statistics, struct
from PIL import Image
parser=argparse.ArgumentParser(description=__doc__)
parser.add_argument('output',type=Path)
parser.add_argument('--fixtures',type=Path,default=Path(__file__).resolve().parents[1]/'crates/mosaico-lua/testdata/n5-images')
args=parser.parse_args()
fixtures=args.fixtures
out=args.output
root=out.parent
receipt=json.loads((fixtures/'receipt.json').read_text())
for name,digest in receipt['sha256'].items():
    assert hashlib.sha256((fixtures/name).read_bytes()).hexdigest()==digest,name
rows=[]
def pack565(rgb):
    return b''.join(struct.pack('<H',((r>>3)<<11)|((g>>2)<<5)|(b>>3)) for r,g,b in rgb)
def read565(path):
    data=path.read_bytes()
    return [(((v>>11)&31)*255//31,((v>>5)&63)*255//63,(v&31)*255//31) for (v,) in struct.iter_unpack('<H',data)]
for name in ['pixels','alpha']:
    rgb=receipt['png_rgb'] if name=='pixels' else [[(c*a+127)//255 for c in [r,g,b]] for r,g,b,a in receipt['png_rgba']]
    assert (out/(name+'.rgb')).read_bytes()==bytes(c for px in rgb for c in px),name+' RGB'
    assert (out/(name+'.rgb565')).read_bytes()==pack565(rgb),name+' RGB565'
    rows.append({'case':name,'rgb':'exact','rgb565':'exact'})
for name in ['orientation.jpg','grayscale.jpeg']:
    image=Image.open(fixtures/name).convert('RGB')
    width,height=image.size
    ref=list(image.getdata())
    for half in [False,True]:
        suffix='.half.rgb565' if half else '.rgb565'
        expected=ref
        if half:
            expected=[tuple((sum(ref[(y*2+dy)*width+x*2+dx][c] for dy in range(2) for dx in range(2))+2)//4 for c in range(3)) for y in range(height//2) for x in range(width//2)]
        actual=read565(out/(name+suffix))
        assert len(actual)==len(expected),(name,half,'dimensions')
        delta=[abs(a-b) for px,refpx in zip(actual,expected) for a,b in zip(px,refpx)]
        mae=statistics.mean(delta)
        severe=sum(max(abs(a-b) for a,b in zip(px,refpx))>32 for px,refpx in zip(actual,expected))/len(actual)
        assert mae<=6 and severe<=0.02,(name,half,mae,severe)
        rows.append({'case':name+suffix,'pixels':len(actual),'mean_channel_error':round(mae,4),'fraction_pixels_over_32':severe})
for name in ['orientation.jpg','grayscale.jpeg']:
    ref=read565(out/(name+'.rgb565'))
    dimensions=Image.open(fixtures/name).size
    oracle=Image.new('RGB',dimensions);oracle.putdata(ref)
    stream=io.BytesIO();oracle.save(stream,format='JPEG',quality=90,subsampling=2);stream.seek(0)
    expected=list(Image.open(stream).convert('RGB').getdata())
    for suffix in ['.encoded.jpg','.saved.jpg']:
        encoded=Image.open(out/(name+suffix))
        assert encoded.format=='JPEG' and encoded.size==dimensions,(name,suffix,'format/size')
        actual=list(encoded.convert('RGB').getdata())
        delta=[abs(a-b) for px,refpx in zip(actual,ref) for a,b in zip(px,refpx)]
        mae=statistics.mean(delta)
        severe=sum(max(abs(a-b) for a,b in zip(px,refpx))>48 for px,refpx in zip(actual,ref))/len(actual)
        oracle_mae=statistics.mean(abs(a-b) for px,refpx in zip(actual,expected) for a,b in zip(px,refpx))
        oracle_severe=sum(max(abs(a-b) for a,b in zip(px,refpx))>16 for px,refpx in zip(actual,expected))/len(actual)
        assert mae<=8 and oracle_mae<=2 and oracle_severe<=0.01,(name,suffix,mae,oracle_mae,oracle_severe)
        rows.append({'case':name+suffix,'pixels':len(actual),'mean_channel_error_vs_rgb565':round(mae,4),'fraction_pixels_over_48_vs_rgb565':severe,'mean_channel_error_vs_pillow_q90_420':round(oracle_mae,4),'fraction_pixels_over_16_vs_pillow_q90_420':oracle_severe})
(root/'pixel-results.json').write_text(json.dumps(rows,indent=2)+'\n')
print(json.dumps(rows,indent=2))
