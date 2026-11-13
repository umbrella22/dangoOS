//! Render every packaged expression with the production animator and decoder.
//! cargo run -p os --example expression_preview -- SKIN_ROOT OUTPUT [SKIN_ID]
use os::companion::render::{skin::Skin, FoxAnimator, FoxInput, FoxPose, FoxRenderer, H, W};
use std::{
    fs,
    io::Write,
    path::Path,
    process::{Command, Stdio},
    rc::Rc,
};

const KEYS: [&str; 25] = [
    "idle",
    "sleep",
    "curious",
    "happy",
    "puzzled",
    "confused",
    "surprised",
    "panic",
    "angry",
    "satisfied",
    "listening",
    "thinking",
    "speaking",
    "error",
    "tired",
    "dying",
    "annoyed",
    "charging",
    "peering",
    "dazzled",
    "network-disconnected",
    "network-connected",
    "bluetooth-disconnected",
    "bluetooth-connected",
    "music-playing",
];

fn rgb(pixels: &[u16], output: &mut Vec<u8>) {
    output.clear();
    for c in pixels {
        output.extend_from_slice(&[
            ((c >> 11) * 255 / 31) as u8,
            (((c >> 5) & 63) * 255 / 63) as u8,
            ((c & 31) * 255 / 31) as u8,
        ]);
    }
}

fn main() {
    let args: Vec<_> = std::env::args().collect();
    let root = Path::new(args.get(1).expect("skin root"));
    let output = Path::new(args.get(2).expect("output directory"));
    let ids = ["default", "eclipse", "normal", "black-blue"];
    for id in ids {
        if args
            .get(3)
            .is_some_and(|selected| !selected.starts_with("--") && selected != id)
        {
            continue;
        }
        let skin = Skin::parse(fs::read(root.join(id).join("fox.bin")).unwrap()).unwrap();
        let out = output.join(id);
        fs::create_dir_all(&out).unwrap();
        let mut renderer = FoxRenderer::new();
        let mut pixels = vec![0u16; W * H];
        let mut buffer = Vec::with_capacity(W * H * 3);
        for (pose, key) in FoxPose::ALL.into_iter().zip(KEYS) {
            let mut animator = FoxAnimator::with_skin(pose, Rc::clone(&skin));
            if args.iter().any(|arg| arg == "--stills") {
                renderer.render(&animator.scene(FoxInput::REST), &mut pixels);
                rgb(&pixels, &mut buffer);
                let mut file = fs::File::create(out.join(format!("{key}.ppm"))).unwrap();
                write!(file, "P6\n{W} {H}\n255\n").unwrap();
                file.write_all(&buffer).unwrap();
                continue;
            }
            let mut video = Command::new("ffmpeg")
                .args([
                    "-hide_banner",
                    "-loglevel",
                    "error",
                    "-y",
                    "-f",
                    "rawvideo",
                    "-pix_fmt",
                    "rgb24",
                    "-s",
                    "352x336",
                    "-r",
                    "25",
                    "-i",
                    "-",
                    "-an",
                    "-c:v",
                    "libx264",
                    "-preset",
                    "fast",
                    "-crf",
                    "16",
                    "-pix_fmt",
                    "yuv420p",
                    "-movflags",
                    "+faststart",
                ])
                .arg(out.join(format!("{key}.mp4")))
                .stdin(Stdio::piped())
                .spawn()
                .unwrap();
            let mut input = video.stdin.take().unwrap();
            for frame in 0..165 {
                let scene = animator.scene(FoxInput::REST);
                renderer.render(&scene, &mut pixels);
                rgb(&pixels, &mut buffer);
                input.write_all(&buffer).unwrap();
                if frame == 0 || (key == "idle" && frame % 25 == 0) {
                    let name = if frame == 0 {
                        format!("{key}.ppm")
                    } else {
                        format!("{key}-{frame:03}.ppm")
                    };
                    let mut file = fs::File::create(out.join(name)).unwrap();
                    write!(file, "P6\n{W} {H}\n255\n").unwrap();
                    file.write_all(&buffer).unwrap();
                }
                animator.advance(40);
            }
            drop(input);
            assert!(
                video.wait().unwrap().success(),
                "video export failed: {id}/{key}"
            );
            println!("{id}/{key}: 165 production frames");
        }
    }
}
