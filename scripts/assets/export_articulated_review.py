#!/usr/bin/env python3
"""Present native balance_preview frames with device and screen viewpoints."""
import argparse
import json
import shutil
import subprocess
from pathlib import Path

from PIL import Image, ImageDraw, ImageFont

NAMES={'default':'心月狐','eclipse':'月蚀','normal':'常态','black-blue':'智谱团子'}
CASES={'idle':'安静地待着','slow':'慢转 · 小手挪步','rescue':'及时停下 · 抓稳了','fall':'快转 · 失手落下','heavy':'剧烈转动 · 重重落地'}
FACES={'Neutral':'正常','Effort':'单闭眼，用力抓住','Dizzy':'转得有点晕','Weary':'停下了，有点无奈'}
STAGES={'Resting':'站稳了','Scooting':'小手挪一下，身体跟过来','Struggling':'抓住一侧，悬着晃一晃','Falling':'哎呀，没抓住','Landing':'落地压扁，软软弹回','Recovering':'缓一缓，慢慢恢复'}
FONT='/System/Library/Fonts/STHeiti Light.ttc'


def font(size): return ImageFont.truetype(FONT,size)


def frame(image, trace, identity, case):
    canvas=Image.new('RGB',(960,600),(16,18,24));d=ImageDraw.Draw(canvas)
    d.text((30,20),NAMES[identity]+'  /  '+CASES[case],font=font(25),fill=(243,236,224))
    d.text((30,66),'设备转动的示意视角',font=font(16),fill=(158,163,179))
    d.text((522,66),'屏幕原始画面',font=font(16),fill=(158,163,179))
    screen=Image.new('RGBA',(368,352),(0,0,0,0));sd=ImageDraw.Draw(screen)
    sd.rounded_rectangle((0,0,367,351),radius=16,fill=(0,0,0,255),outline=(79,84,100,255),width=2)
    screen.paste(image,(8,8));sd.ellipse((181,2,187,6),fill=(224,178,110,255))
    screen=screen.resize((316,302),Image.Resampling.LANCZOS)
    world=screen.rotate(trace['device_angle'],Image.Resampling.BICUBIC,expand=True)
    canvas.paste(world,(245-world.width//2,306-world.height//2),world)
    canvas.paste(image,(540,138))
    d.rounded_rectangle((535,133,896,479),radius=8,outline=(60,64,77),width=1)
    d.text((30,530),STAGES[trace['stage']],font=font(22),fill=(233,191,131))
    if 'face' in trace:d.text((522,495),'表情：'+FACES[trace['face']],font=font(17),fill=(185,192,209))
    d.text((30,568),'模拟 IMU 输入 · 实际 Rust 渲染帧 · 25 fps 导出',font=font(13),fill=(122,130,148))
    d.text((837,536),f"{trace['at_ms']/1000:.2f} s",font=font(16),fill=(161,166,182))
    return canvas


def main():
    p=argparse.ArgumentParser(description=__doc__);p.add_argument('--root',type=Path,required=True)
    a=p.parse_args();root=a.root.resolve();out=root/'review';media=out/'media';media.mkdir(parents=True,exist_ok=True)
    for identity in NAMES:
        for case in CASES:
            source=root/'frames'/identity/case;trace=json.loads((source/'trace.json').read_text())
            dest=media/(identity+'-'+case+'.mp4')
            cmd=['ffmpeg','-hide_banner','-loglevel','error','-y','-f','rawvideo','-pix_fmt','rgb24','-s','960x600','-r','25','-i','-','-an','-c:v','libx264','-preset','fast','-crf','19','-pix_fmt','yuv420p','-movflags','+faststart',str(dest)]
            proc=subprocess.Popen(cmd,stdin=subprocess.PIPE)
            for n in [0]*8+list(range(len(trace)))+[len(trace)-1]*15:
                f=trace[n];im=Image.open(source/f"{f['frame']:04}.ppm").convert('RGB')
                result=frame(im,f,identity,case);proc.stdin.write(result.tobytes())
                if identity=='default' and case=='fall' and f['at_ms']==1080:result.save(out/'poster.png')
            proc.stdin.close()
            if proc.wait()!=0:raise RuntimeError('video export failed: '+str(dest))
            print(identity,case,flush=True)
    for case in ['fall','heavy','slow']:
        subprocess.run(['ffmpeg','-hide_banner','-loglevel','error','-y','-i',str(media/('default-'+case+'.mp4')),'-filter_complex','[0:v]fps=12.5,scale=768:-1,split[a][b];[a]palettegen=max_colors=160[p];[b][p]paletteuse=dither=bayer:bayer_scale=3','-loop','0',str(out/(case+'-preview.gif'))],check=True)
    if (root/'glm-expression-sequence.png').exists():
        shutil.copyfile(root/'glm-expression-sequence.png',out/'glm-expression-sequence.png')
        shutil.copyfile(root/'blink-cleanup-comparison.png',out/'blink-cleanup-comparison.png')
    layers=out/'layers';layers.mkdir(exist_ok=True)
    for identity in NAMES:
        source=Path('assets/companion-articulated')/identity
        for ext in ['psd','pcraft']:shutil.copyfile(source/(identity+'-articulated.'+ext),layers/(identity+'.'+ext))
    storage=json.loads((root/'storage.json').read_text())
    rows=''.join(f"<tr><td>{NAMES[r['id']]}</td><td>{r['before_bytes']/1024:.1f} KiB</td><td>{r['after_bytes']/1024:.1f} KiB</td><td>−{r['saved_percent']}%</td></tr>" for r in storage)
    html='''<!doctype html><html lang="zh-CN"><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><title>团子 · 找平衡</title>
<style>*{box-sizing:border-box}body{margin:0;background:#101218;color:#ece8e1;font:16px -apple-system,BlinkMacSystemFont,"PingFang SC",sans-serif}main{max-width:1080px;margin:0 auto;padding:40px 32px 72px}header{border-bottom:1px solid #303440;padding-bottom:24px;margin-bottom:24px}.eyebrow{color:#d6af79;font-size:13px;letter-spacing:3px}h1{font-size:36px;letter-spacing:2px;margin:12px 0}p{color:#a9afbf;line-height:1.8}nav{display:flex;gap:10px;flex-wrap:wrap;margin:20px 0}button,select{font:inherit;color:#c7cbd6;background:#1a1e28;border:1px solid #343a48;border-radius:8px;padding:10px 16px;cursor:pointer}button.on{background:#dfbc87;color:#20232b;border-color:#dfbc87}button:hover,select:hover{border-color:#ae956f}video{display:block;width:100%;background:#000;border-radius:12px;border:1px solid #303541}.tools{display:flex;justify-content:space-between;align-items:center;gap:16px;margin:18px 0 34px}.tools p{font-size:13px;margin:0}h2{font-size:21px;margin:38px 0 12px}table{width:100%;border-collapse:collapse;font-variant-numeric:tabular-nums}td,th{text-align:left;border-bottom:1px solid #2d323f;padding:13px 6px}th{font-size:13px;color:#8e97ad;font-weight:500}td:last-child{color:#dcbd90}a{color:#e0bf8e;text-underline-offset:4px}.downloads{display:flex;gap:24px;flex-wrap:wrap;line-height:2}footer{margin-top:38px;font-size:13px;color:#8892a7}img{width:100%;border:1px solid #303541;border-radius:10px}@media(max-width:600px){main{padding:24px 16px}h1{font-size:28px}button{padding:9px 12px;font-size:14px}.tools{align-items:flex-start;flex-direction:column}td,th{font-size:13px}}</style>
<main><header><div class="eyebrow">DANGO / MOTION STUDY</div><h1>团子，自己找平衡。</h1><p>小手挪一下，身体晃一下。转动时先单闭眼，再变晕；停下后露出无奈，慢慢恢复正常。</p></header>
<nav id="characters"><button class="on" data-id="default">心月狐</button><button data-id="eclipse">月蚀</button><button data-id="normal">常态</button><button data-id="black-blue">智谱团子</button></nav>
<nav id="cases"><button data-case="idle">待机</button><button data-case="slow">慢慢挪</button><button data-case="rescue">及时停下</button><button class="on" data-case="fall">失手落地</button><button data-case="heavy">重重落地</button></nav>
<video id="video" controls autoplay muted loop playsinline poster="poster.png" src="media/default-fall.mp4"></video>
<div class="tools"><p>左边示意设备转动，右边查看原始屏幕内容。可用慢速观察小手、倾斜和落地的变化。</p><select id="speed" aria-label="播放速度"><option value="1">正常速度</option><option value="0.5">0.5 倍速</option><option value="0.25">0.25 倍速</option></select></div>
<h2>保留圆润的小手和原来的气质</h2><p>慢挪时双手交替轻轻伸长；抓握时一侧小手拉住，身体随之摆动。强烈转动后的落地会压得更扁，再弹回原来的样子。无奈之后直接恢复正常表情。</p>
CLEANUP<h2>可编辑分层</h2><p>每只团子提供 20 个图层的 PhotoCraft 与 PSD 文件，包含身体、双手、耳朵，以及独立的眼睛与嘴部变化。</p><div class="downloads">LAYERS</div>
<h2>资源包体积</h2><p>在新动作图层之外，四只角色各 25 种完整姿态，连同局部动画共 191 张既有图片的像素校验一致。下表以分层前的动作版（v3）为基准；无损重压缩与相同图片复用降低了体积。</p><table><thead><tr><th>团子</th><th>动作版 v3</th><th>分层版 v4</th><th>节省</th></tr></thead><tbody>ROWS</tbody></table>
<footer>本页展示主机模拟输入驱动实际 Runtime、动画器和渲染器的结果。实体传感器方向、设备帧率与操作手感尚待设备验证。v4 皮肤需要配套的新固件。</footer></main>
<script>let id='default',kind='fall';const video=document.querySelector('#video'),speed=document.querySelector('#speed');function update(){video.src=`media/${id}-${kind}.mp4`;video.load();video.playbackRate=Number(speed.value);video.play().catch(()=>{})}for(const [selector,key] of [['#characters','id'],['#cases','case']])document.querySelector(selector).addEventListener('click',e=>{const b=e.target.closest('button');if(!b)return;document.querySelectorAll(selector+' button').forEach(x=>x.classList.toggle('on',x===b));if(key==='id')id=b.dataset.id;else kind=b.dataset.case;update()});speed.addEventListener('change',()=>video.playbackRate=Number(speed.value));</script></html>'''
    links=''.join(f'<span>{name}：<a href="layers/{identity}.psd">PSD</a> · <a href="layers/{identity}.pcraft">PhotoCraft</a></span>' for identity,name in NAMES.items())
    cleanup = '<h2>表情顺序与眼部清理</h2><p>单闭眼 → 晕眩 → 停下后无奈 → 正常。GLM 的旧眉线和眼角残片已清理，嘴部补丁也收窄到嘴部范围。</p><img src="glm-expression-sequence.png" alt="GLM 的四个表情阶段"><p>闭眼补丁清理前后：</p><img src="blink-cleanup-comparison.png" alt="眼白和旧睫毛清理前后">' if (out/'glm-expression-sequence.png').exists() else ''
    (out/'index.html').write_text(html.replace('ROWS',rows).replace('LAYERS',links).replace('CLEANUP',cleanup))
    shutil.copyfile(root/'storage.json',out/'storage.json')


if __name__=='__main__':main()
