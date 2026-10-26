# Hub 字体

`gen_hub_font.py` 把 Noto Sans SC 光栅化成 Hub 页面用的子集字体，输出到
`crates/os/src/assets/hub-*.bin`，由 `ui/hub_font.rs` 读取（RFC 0006 第 2.2 节）。

| 输出 | 字重 | 字号 | 内容 |
| --- | --- | --- | --- |
| `hub-20.bin` | Regular | 20 px | 正文、提示 |
| `hub-24.bin` | Regular | 24 px | 卡片数值、按钮 |
| `hub-30.bin` | Medium | 30 px | 页面标题 |
| `hub-digits-64.bin` | Regular | 64 px | 计时器数字，只含 `0-9:` |

字符集自动收集：ASCII 可见字符，加上 `src/hub/**` 和 `src/ui/hub_view.rs` 里出现的全部中文。
改了页面里的中文就要重新生成，否则新字显示成方框。

```sh
python3 assets/fonts/gen_hub_font.py \
    --regular NotoSansSC-Regular.otf --medium NotoSansSC-Medium.otf
```

源字体每个约 8 MB，不提交；从
<https://github.com/notofonts/noto-cjk/tree/main/Sans/SubsetOTF/SC> 下载。

许可：Noto Sans SC 使用 SIL Open Font License 1.1，子集字体随固件分发时保留此说明即可。
