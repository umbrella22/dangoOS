#!/usr/bin/env python3
"""Export production display_pipeline replays to PNG/GIF and a portable viewer.

Requires Pillow. The viewer has its own playback cadence; it is not a device
FPS measurement. Read only the PPM files named in the current replay receipt.
"""

import argparse
import base64
import io
import json
from pathlib import Path

from PIL import Image, ImageDraw, ImageFont


VIEWER = r"""<!doctype html>
<html lang="zh-CN"><meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>原生 LVGL 显示回放</title>
<style>
:root{color-scheme:dark;font:15px/1.6 system-ui,sans-serif;background:#11151a;color:#e3e8ed}
*{box-sizing:border-box}body{max-width:1100px;margin:auto;padding:30px}
h1{font-size:25px;margin:0 0 6px}p{color:#acb8c5;margin:6px 0 22px}
main{display:grid;grid-template-columns:minmax(280px,480px) 1fr;gap:32px}
canvas{display:block;width:100%;height:auto;background:#000;border:1px solid #34414e}
label{display:block;margin-bottom:16px}select,button{font:inherit;background:#202a35;color:inherit;border:1px solid #4b5c6d;border-radius:5px;padding:7px 10px}
select{margin-left:8px}button{cursor:pointer}input[type=range]{width:100%;accent-color:#bdf079}
dl{display:grid;grid-template-columns:130px 1fr;gap:8px;margin:18px 0}dt{color:#acb8c5}dd{margin:0;font-variant-numeric:tabular-nums}
pre{font-size:12px;white-space:pre-wrap;color:#b3c7db}small{color:#acb8c5}
.pass{color:#bdf079}a{color:#aed8ff}@media(max-width:780px){main{grid-template-columns:1fr}body{padding:18px}}
</style>
<h1>原生 LVGL · RGB888 局部刷新回放</h1>
<p>生产设置页、指针输入与像素编码在 host 运行。内存面板逐帧与完整画面比对；DMA、TE 和屏幕扫描需接设备验证。</p>
<main><div><canvas id="panel" width="480" height="480"></canvas>
<input id="position" type="range" min="0" value="0" aria-label="回放帧">
<button id="play">播放</button> <small>展示速度 80 ms/帧，与设备帧率无关</small></div>
<div><label>像素格式<select id="format"><option value="rgb888">RGB888</option><option value="rgb565">RGB565</option></select></label>
<label>场景<select id="scene"><option value="audio">音量拖动</option><option value="push">双页 push</option><option value="brightness">亮度拖动</option></select></label>
<label><input id="damage" type="checkbox"> 标出提交的更新范围</label>
<div class="pass" id="checks"></div><dl>
<dt>帧位置</dt><dd id="frame"></dd><dt>像素提交量</dt><dd id="bytes"></dd>
<dt>40 MHz 纯数据</dt><dd id="wire"></dd><dt>DMA 分块数</dt><dd id="windows"></dd>
<dt>连续来源字节</dt><dd id="direct"></dd><dt>host 绘制耗时</dt><dd id="host"></dd>
</dl><small>纯数据时间 = 字节数 ÷ 20 MB/s，未计入命令、DMA、TE 等开销。连续来源表示编码后的来源可直接借用，实际 DMA 行为待设备验证。</small>
<pre id="event"></pre><small>静态页：松手动画结束后，每个设置场景另运行 10 轮，像素提交均为 0。push 的新页只绘制一次，中间帧仅由旧、新页面的条带构成。</small>
</div></main>
<script id="reports" type="application/json">__REPORTS__</script>
<script>
const reports=JSON.parse(document.getElementById('reports').textContent);
const el=id=>document.getElementById(id), ctx=el('panel').getContext('2d');
let frames=[], index=0, timer=null, generation=0;
function show(){
 const f=frames[index], token=++generation, img=new Image();
 el('position').value=index;
 el('frame').textContent=`${index+1} / ${frames.length} · ${f.file}`;
 el('bytes').textContent=`${f.bytes.toLocaleString()} B`;
 el('wire').textContent=`${(f.wire_only_us_at_40mhz/1000).toFixed(3)} ms（理论）`;
 el('windows').textContent=f.windows;
 el('direct').textContent=`${f.direct_bytes.toLocaleString()} B`;
 el('host').textContent=f.host_render_us===null?'—（此帧仅组合条带）':`${f.host_render_us.toLocaleString()} μs（主机）`;
 el('event').textContent=JSON.stringify(f.event,null,2);
 img.onload=()=>{if(token!==generation)return;ctx.drawImage(img,0,0);
  if(el('damage').checked){ctx.strokeStyle='#bdf079';ctx.lineWidth=2;
   for(const [x0,y0,x1,y1] of f.damage)ctx.strokeRect(x0+1,y0+1,x1-x0-1,y1-y0-1);}};
 img.src=f.png;
}
function select(){
 const r=reports[el('format').value];
 frames=r.frames.filter(f=>f.scene===el('scene').value);index=Math.min(index,frames.length-1);
 el('position').max=frames.length-1;
 el('checks').textContent=`${r.full_frame_comparisons} / ${r.frames.length} 帧完整像素对照通过`;
 show();
}
el('format').onchange=select;
el('scene').onchange=()=>{index=0;select();};
el('position').oninput=()=>{index=Number(el('position').value);show();};
el('damage').onchange=show;
el('play').onclick=()=>{if(timer){clearInterval(timer);timer=null;el('play').textContent='播放';}
 else{el('play').textContent='暂停';timer=setInterval(()=>{index=(index+1)%frames.length;show();},80);}};
document.addEventListener('keydown',e=>{if(e.key==='ArrowLeft'||e.key==='ArrowRight'){
 e.preventDefault();index=Math.max(0,Math.min(frames.length-1,index+(e.key==='ArrowRight'?1:-1)));show();}});
select();
</script></html>
"""


def read_report(path: Path):
    report = json.loads(path.read_text())
    assert report["size"] == [480, 480]
    assert report["device_measurements"] is False
    assert report["full_frame_comparisons"] == len(report["frames"])
    images = []
    for frame in report["frames"]:
        image = Image.open(path.parent / frame["file"]).convert("RGB")
        assert image.size == (480, 480)
        images.append(image)
        stream = io.BytesIO()
        image.save(stream, format="PNG")
        frame["png"] = "data:image/png;base64," + base64.b64encode(stream.getvalue()).decode()
    return report, images


def contact_sheet(report, images, destination):
    def extremum(scene, largest):
        choices = [
            (i, f["event"]["native_slider_value"])
            for i, f in enumerate(report["frames"])
            if f["scene"] == scene and "native_slider_value" in f["event"]
        ]
        return (max if largest else min)(choices, key=lambda pair: pair[1])[0]

    pushes = [i for i, f in enumerate(report["frames"]) if f["scene"] == "push"]
    selections = [
        (extremum("audio", False), "Audio / low"),
        (extremum("audio", True), "Audio / high"),
        (min(pushes, key=lambda i: abs(report["frames"][i]["event"]["offset"] - 240)), "Push / intermediate"),
        (extremum("brightness", False), "Brightness / low"),
        (extremum("brightness", True), "Brightness / high"),
        (pushes[-1], "Launcher / push complete"),
    ]
    def font_at(size):
        for name in ["/System/Library/Fonts/Supplemental/Arial.ttf", "DejaVuSans.ttf"]:
            try:
                return ImageFont.truetype(name, size)
            except OSError:
                pass
        return ImageFont.load_default(size=size)

    font, small = font_at(18), font_at(15)
    sheet = Image.new("RGB", (1020, 818), "#11151a")
    draw = ImageDraw.Draw(sheet)
    draw.text((20, 16), f"RGB888 / native LVGL host replay / {report['full_frame_comparisons']} pixel comparisons passed", font=font, fill="#e3e8ed")
    draw.text((20, 44), "Memory panel output. Transport times are analytical; no device measurements.", font=small, fill="#acb8c5")
    for n, (index, label) in enumerate(selections):
        x, y = 20 + n % 3 * 334, 82 + n // 3 * 365
        sheet.paste(images[index].resize((320, 320), Image.Resampling.LANCZOS), (x, y))
        draw.text((x, y + 325), label, font=font, fill="#e3e8ed")
        frame = report["frames"][index]
        draw.text((x, y + 346), f"{frame['bytes']:,} B / {frame['windows']} windows", font=small, fill="#acb8c5")
    sheet.save(destination)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--rgb888", type=Path, required=True, help="RGB888 replay.json")
    parser.add_argument("--rgb565", type=Path, required=True, help="RGB565 replay.json")
    parser.add_argument("--out", type=Path, required=True)
    args = parser.parse_args()
    args.out.mkdir(parents=True, exist_ok=True)
    reports, images = {}, {}
    for name, path in [("rgb888", args.rgb888), ("rgb565", args.rgb565)]:
        reports[name], images[name] = read_report(path)
    assert reports["rgb888"]["format"] == "Bgr888"
    assert reports["rgb565"]["format"] == "Rgb565Be"
    payload = json.dumps(reports, ensure_ascii=False, separators=(",", ":")).replace("<", "\\u003c")
    (args.out / "index.html").write_text(VIEWER.replace("__REPORTS__", payload))
    contact_sheet(reports["rgb888"], images["rgb888"], args.out / "contact.png")
    images["rgb888"][20].save(args.out / "audio.png")
    images["rgb888"][80].save(args.out / "brightness.png")
    # GIF is a convenience preview; PNGs in the viewer retain native color precision.
    frames = [image.resize((384, 384), Image.Resampling.LANCZOS) for image in images["rgb888"]]
    frames[0].save(args.out / "replay.gif", save_all=True, append_images=frames[1:], duration=80, loop=0)
    print(f"Exported {len(frames)} frames and portable viewer to {args.out}")


if __name__ == "__main__":
    main()
