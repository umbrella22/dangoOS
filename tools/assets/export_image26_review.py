#!/usr/bin/env python3
"""Export an offline review of native expression and motion renders."""
import argparse
import json
import shutil
import subprocess
from pathlib import Path

from PIL import Image, ImageDraw, ImageFont

from refresh_companion_expressions import IDS, POSES, LABELS, NAMES
from export_articulated_review import frame, CASES


def export(work):
    out = work / 'review'; out.mkdir(exist_ok=True)
    font = ImageFont.truetype('/System/Library/Fonts/STHeiti Light.ttc', 19)
    comparison = Image.new('RGB', (4 * 352, 2 * 372), (18, 21, 28)); draw = ImageDraw.Draw(comparison)
    for n, identity in enumerate(IDS):
        images = out / 'images' / identity; images.mkdir(parents=True, exist_ok=True)
        videos = out / 'media' / identity; videos.mkdir(parents=True, exist_ok=True)
        for pose in POSES:
            for kind, root in [('before', 'before-native'), ('after', 'expressions')]:
                im = Image.open(work / root / identity / (pose + '.ppm')).convert('RGB')
                im.save(images / (pose + '-' + kind + '.png'))
                if pose == 'idle':
                    y = 0 if kind == 'before' else 372
                    comparison.paste(im, (n * 352, y + 30))
                    draw.text((n * 352 + 10, y + 5), NAMES[identity] + (' · 原版' if kind == 'before' else ' · image 2.6'), font=font, fill=(226, 229, 239))
            shutil.copyfile(work / 'expressions' / identity / (pose + '.mp4'), videos / (pose + '.mp4'))
        sheet = Image.new('RGB', (1200, 5 * 253), (18, 21, 28)); d = ImageDraw.Draw(sheet)
        for i, pose in enumerate(POSES):
            im = Image.open(images / (pose + '-after.png')); im.thumbnail((234, 223))
            x, y = i % 5 * 240, i // 5 * 253
            sheet.paste(im, (x + (240 - im.width) // 2, y + 27))
            d.text((x + 8, y + 5), LABELS[pose], font=font, fill=(230, 232, 238))
        sheet.save(out / (identity + '-all-expressions.png'))
        for case in CASES:
            source = work / 'frames' / identity / case
            trace = json.loads((source / 'trace.json').read_text())
            dest = videos / ('balance-' + case + '.mp4')
            proc = subprocess.Popen(['ffmpeg', '-hide_banner', '-loglevel', 'error', '-y', '-f', 'rawvideo',
                '-pix_fmt', 'rgb24', '-s', '960x600', '-r', '25', '-i', '-', '-an', '-c:v', 'libx264',
                '-preset', 'fast', '-crf', '18', '-pix_fmt', 'yuv420p', '-movflags', '+faststart', str(dest)], stdin=subprocess.PIPE)
            for row in trace:
                im = Image.open(source / f"{row['frame']:04}.ppm").convert('RGB')
                proc.stdin.write(frame(im, row, identity, case).tobytes())
            proc.stdin.close()
            if proc.wait(): raise RuntimeError('Motion video export: ' + str(dest))
        layers = out / 'layers' / identity; layers.mkdir(parents=True, exist_ok=True)
        for kind in ['articulated', 'expressions']:
            for ext in ['psd', 'pcraft']:
                shutil.copyfile(work / 'components' / identity / (identity + '-' + kind + '.' + ext), layers / (kind + '.' + ext))
        for ext in ['psd', 'pcraft']:
            source = work / 'components' / identity / ('confused-animation.' + ext)
            if source.exists(): shutil.copyfile(source, layers / source.name)
        skin = out / 'skins' / identity; skin.mkdir(parents=True, exist_ok=True)
        for name in ['fox.bin', 'skin.json']: shutil.copyfile(work / 'skins' / identity / name, skin / name)
    comparison.save(out / 'family-comparison.png')
    storage = json.loads((work / 'storage.json').read_text())
    old, new = sum(x['before_bytes'] for x in storage), sum(x['after_bytes'] for x in storage)
    rows = ''.join(f"<tr><td>{NAMES[x['id']]}</td><td>{x['before_bytes']/1024:.1f} KiB</td><td>{x['after_bytes']/1024:.1f} KiB</td><td>{'减少' if x['saved_bytes'] >= 0 else '增加'} {abs(x['saved_percent']):.2f}%</td></tr>" for x in storage)
    data = json.dumps({'ids': IDS, 'names': NAMES, 'poses': POSES, 'labels': LABELS, 'cases': CASES, 'revisions': {x['id']: x['sha256'][:12] for x in storage}}, ensure_ascii=False)
    html = '''<!doctype html><html lang="zh-CN"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><title>团子 · image 2.6 表情工坊</title>
<style>:root{color-scheme:dark;--ink:#f0ece6;--muted:#a4acbb;--line:#303746;--gold:#e1bb80}*{box-sizing:border-box}body{margin:0;background:#11141b;color:var(--ink);font:16px/1.65 -apple-system,BlinkMacSystemFont,"PingFang SC",sans-serif}main{max-width:1120px;margin:auto;padding:44px 32px 72px}header{border-bottom:1px solid var(--line);padding-bottom:24px}.eyebrow{color:var(--gold);font-size:12px;letter-spacing:3px}h1{font-size:38px;line-height:1.25;letter-spacing:1px;margin:13px 0}h2{font-size:23px;margin:38px 0 12px}p{color:var(--muted);margin:10px 0}button,select{font:inherit;color:var(--ink);background:#1c222e;border:1px solid #384153;border-radius:7px;padding:9px 15px;cursor:pointer}button:hover,select:hover{border-color:var(--gold)}button[aria-pressed=true]{background:var(--gold);color:#1b1c22;border-color:var(--gold)}nav,.tools{display:flex;flex-wrap:wrap;gap:10px;margin:22px 0}.tools{align-items:center;justify-content:space-between}label{color:var(--muted);font-size:14px}.comparison{display:grid;grid-template-columns:1fr 1fr;gap:20px}.pane{margin:0;min-width:0}figcaption{display:flex;justify-content:space-between;color:var(--muted);font-size:14px;margin:0 0 9px}.stage{background:#000;border:1px solid var(--line);border-radius:10px;overflow:hidden;display:grid;place-items:center;aspect-ratio:352/336}.stage img,.stage video{display:block;width:100%;height:100%;object-fit:contain}.pixel img,.pixel video{image-rendering:pixelated}.grid{display:grid;grid-template-columns:repeat(5,1fr);gap:12px;margin-top:18px}.thumb{padding:0;overflow:hidden;text-align:left;background:#0a0b0f;border-color:#2b3341}.thumb img{display:block;width:100%;aspect-ratio:352/336;object-fit:contain}.thumb span{display:block;padding:6px 9px;font-size:13px}.thumb[aria-pressed=true]{background:#2d2b28;color:var(--gold)}.motion{width:100%;display:block;background:#000;border:1px solid var(--line);border-radius:10px}.downloads{display:flex;flex-wrap:wrap;gap:14px 24px}a{color:#e4c18d;text-underline-offset:4px}table{border-collapse:collapse;width:100%;font-variant-numeric:tabular-nums}td,th{padding:12px 8px;border-bottom:1px solid var(--line);text-align:left}th{color:var(--muted);font-size:13px;font-weight:500}td:last-child{color:var(--gold)}.metric{color:var(--gold);font-size:22px}.wide{display:block;width:100%;border:1px solid var(--line);border-radius:8px}footer{border-top:1px solid var(--line);padding-top:20px;margin-top:40px;color:#8994a8;font-size:13px}details{margin-top:26px}summary{cursor:pointer;color:#b8c1d0}@media(max-width:700px){main{padding:27px 16px 48px}h1{font-size:28px}.comparison{gap:10px}.grid{grid-template-columns:repeat(3,1fr);gap:8px}nav{gap:7px}button{font-size:14px;padding:8px 11px}.tools{align-items:flex-start;gap:12px}th,td{font-size:13px;padding:10px 4px}}</style></head>
<body><main><header><div class="eyebrow">DANGO / EXPRESSION ATELIER</div><h1>熟悉的小团子，更清楚的表情。</h1><p>四只角色，100 种常规表情。圆润的脸、干净的发束、独立的小手；点选下方表情，和原版一起看。</p></header>
<nav id="characters" aria-label="选择团子"></nav><div class="tools"><label>当前表情 <select id="pose" aria-label="选择表情"></select></label><label>播放速度 <select id="speed" aria-label="播放速度"><option value="1">正常</option><option value="0.5">0.5 倍</option><option value="0.25">0.25 倍</option></select></label><button id="pixel" aria-pressed="false">查看像素边缘</button></div>
<div class="comparison" id="comparison"><figure class="pane"><figcaption><span>更新前</span><span>原表情</span></figcaption><div class="stage"><img id="before" alt="更新前的团子表情"></div></figure><figure class="pane"><figcaption><span>image 2.6 · 精修后</span><span id="current-label"></span></figcaption><div class="stage"><video id="after" controls autoplay muted loop playsinline></video></div></figure></div>
<div class="grid" id="expressions" aria-label="全部 25 种表情"></div>
<h2>挪小手，转一圈，再稳稳落下</h2><p>用力时先单闭眼，再变晕；停下后露出无奈，最后恢复正常。小手独立挪动，落地时身体压扁再弹回。</p><nav id="cases" aria-label="选择动作"></nav><video class="motion" id="motion" controls autoplay muted loop playsinline></video>
<h2>这次修整的细节</h2><p>心月狐改用更宽圆的脸型和柔和成束的头发。四套头发统一清理错位亮斑和涂抹感，保留清晰的明暗层次；背景光晕清除。蓝牙和 Wi-Fi 符号按统一路径精修，连接状态的勾、叉独立分层。四只团子的眩晕圈圈眼都能独立旋转。</p><details><summary>展开四只团子的待机对照</summary><img class="wide" src="family-comparison.png" alt="四只团子的更新前后待机对照"></details>
<h2>设备资源容量</h2><p>比较本轮开始时已认可的分层版与本轮最终皮肤包，包含全部表情和找平衡部件。</p><p class="metric">TOTAL</p><table><thead><tr><th>团子</th><th>更新前</th><th>精修后</th><th>减少</th></tr></thead><tbody>ROWS</tbody></table><p>每只角色使用自己的最多 128 色 RGB565 调色板。图片按索引无抖动量化，再无损压缩并复用相同数据；每包均低于设备 1 MiB 限制。视频和可编辑原稿不计入设备资源容量。</p>
<h2>可编辑原稿与皮肤</h2><div class="downloads" id="downloads"></div>
<footer>表情视频与动作画面来自项目实际 Rust 动画器、皮肤解码器和渲染器。动作使用模拟传感器输入；25 fps 是导出采样率。尚未进行设备刷写或实机显示验收。皮肤沿用 v4 格式。</footer></main>
<script>const data=DATA;const params=new URLSearchParams(location.search);let id=data.ids.includes(params.get('id'))?params.get('id'):'default',pose=data.poses.includes(params.get('pose'))?params.get('pose'):'idle',kind='heavy';const asset=path=>path+'?v='+data.revisions[id];const q=s=>document.querySelector(s),after=q('#after'),motion=q('#motion');for(const key of data.ids){const b=document.createElement('button');b.textContent=data.names[key];b.dataset.id=key;b.onclick=()=>{id=key;update()};q('#characters').append(b)}for(const key of data.poses){const o=document.createElement('option');o.value=key;o.textContent=data.labels[key];q('#pose').append(o);const b=document.createElement('button');b.className='thumb';b.dataset.pose=key;b.innerHTML='<img alt=""><span></span>';b.querySelector('span').textContent=data.labels[key];b.onclick=()=>{pose=key;updateExpression()};q('#expressions').append(b)}for(const [key,label]of Object.entries(data.cases)){const b=document.createElement('button');b.textContent=label;b.dataset.case=key;b.onclick=()=>{kind=key;updateMotion()};q('#cases').append(b)}function play(v,path){v.src=asset(path);v.load();v.playbackRate=Number(q('#speed').value);v.play().catch(()=>{})}function updateExpression(){q('#pose').value=pose;q('#before').src=asset(`images/${id}/${pose}-before.png`);after.poster=asset(`images/${id}/${pose}-after.png`);q('#current-label').textContent=data.labels[pose];play(after,`media/${id}/${pose}.mp4`);document.querySelectorAll('[data-pose]').forEach(b=>b.setAttribute('aria-pressed',String(b.dataset.pose===pose)))}function updateMotion(){play(motion,`media/${id}/balance-${kind}.mp4`);document.querySelectorAll('[data-case]').forEach(b=>b.setAttribute('aria-pressed',String(b.dataset.case===kind)))}function update(){document.querySelectorAll('[data-id]').forEach(b=>b.setAttribute('aria-pressed',String(b.dataset.id===id)));document.querySelectorAll('[data-pose]').forEach(b=>{const img=new Image();img.alt=data.names[id]+' · '+data.labels[b.dataset.pose];img.decoding='async';img.src=asset(`images/${id}/${b.dataset.pose}-after.png`);b.querySelector('img').replaceWith(img)});q('#downloads').innerHTML=`<a href="${id}-all-expressions.png">${data.names[id]} · 全表情图</a><a href="layers/${id}/expressions.psd">表情 PSD</a><a href="layers/${id}/articulated.psd">动作 PSD</a><a href="layers/${id}/articulated.pcraft">PhotoCraft 动作工程</a><a href="skins/${id}/fox.bin">设备皮肤</a><a href="skins/${id}/skin.json">皮肤清单</a>`;if(id!=='default')q('#downloads').innerHTML+=`<a href="layers/${id}/confused-animation.psd">圈圈眼动画 PSD</a><a href="layers/${id}/confused-animation.pcraft">圈圈眼 PhotoCraft 工程</a>`;updateExpression();updateMotion()}q('#pose').onchange=e=>{pose=e.target.value;updateExpression()};q('#speed').onchange=e=>{after.playbackRate=motion.playbackRate=Number(e.target.value)};q('#pixel').onclick=()=>{const on=q('#comparison').classList.toggle('pixel');q('#pixel').setAttribute('aria-pressed',String(on))};update();</script></body></html>'''
    total = f'{old/1024:.1f} → {new/1024:.1f} KiB · 减少 {(old-new)/1024:.1f} KiB（{(old-new)*100/old:.2f}%）'
    html = html.replace('<th>减少</th>', '<th>变化</th>').replace('最多 128 色', f"最多 {max(x['palette_budget'] for x in storage)} 色")
    (out / 'index.html').write_text(html.replace('TOTAL', total).replace('ROWS', rows).replace('DATA', data))
    shutil.copyfile(work / 'storage.json', out / 'storage.json')


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--work', type=Path, required=True)
    export(parser.parse_args().work.resolve())
