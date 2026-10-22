//! Independent image consumer: writes real module output for external pixel
//! comparison. Its input corpus and Pillow expectations are supplied by caller.
use mosaico_lua::{GLOBAL_ALLOCATOR, Host, Limits, Step, Vm};
use std::{env, fs, path::Path};

struct Output;
impl Host for Output {
    fn print(&mut self, line: &[u8]) {
        println!("{}", String::from_utf8_lossy(line));
    }
}
fn literal(path: &Path) -> String {
    let mut quoted = String::from("\"");
    for b in path.to_str().expect("UTF-8 path").bytes() {
        if (32..127).contains(&b) && b != b'"' && b != b'\\' {
            quoted.push(b as char);
        } else {
            quoted.push_str(&format!("\\{b:03}"));
        }
    }
    quoted.push('"');
    quoted
}
fn main() {
    let args: Vec<_> = env::args().collect();
    assert_eq!(args.len(), 3, "n5_image_probe <fixture-dir> <output-dir>");
    let input = fs::canonicalize(&args[1]).unwrap();
    fs::create_dir_all(&args[2]).unwrap();
    let output = fs::canonicalize(&args[2]).unwrap();
    let script = format!(
        r#"
local image=require('image')
local input,output={},{}
local function save(name,bytes)
  local f <close> = assert(io.open(output..'/'..name,'wb'))
  assert(f:write(bytes))
end
for _,name in ipairs({{'pixels','alpha'}}) do
  local frame <close> = image.load_file(input..'/'..name..'.png')
  local i=frame:info();assert(i.width==3 and i.height==2 and i.pixel_format=='RGB3')
  save(name..'.rgb',frame:data())
  local resized <close> = image.resize(frame,{{width=3,height=2,filter='nearest'}})
  frame:release()
  assert(not frame:info().valid and not pcall(frame.data,frame))
  save(name..'.rgb565',resized:data())
end
for _,case in ipairs({{{{'orientation.jpg',96,64}},{{'grayscale.jpeg',32,24}}}}) do
  local frame <close> = image.load_file(input..'/'..case[1])
  local i=frame:info();assert(i.width==case[2] and i.height==case[3])
  local full <close> = image.resize(frame,{{width=case[2],height=case[3],filter='nearest'}})
  save(case[1]..'.rgb565',full:data())
  local encoded <close> = image.convert(full,image.JPEG)
  assert(encoded:info().pixel_format=='JPEG')
  save(case[1]..'.encoded.jpg',encoded:data())
  image.save_file(output..'/'..case[1]..'.saved.jpg',full)
  local half <close> = image.resize(frame,{{width=case[2]//2,height=case[3]//2,filter='bilinear'}})
  save(case[1]..'.half.rgb565',half:data())
end
for _,name in ipairs({{'corrupt.jpg','truncated.jpg','truncated.png'}}) do
  assert(not pcall(function()
    local frame <close> = image.load_file(input..'/'..name)
    local resized <close> = image.resize(frame,{{width=8,height=8}})
  end),name..' was accepted')
end
local base <close> = image.load_file(input..'/pixels.png')
assert(not pcall(image.resize,base,{{width=65535,height=65535}}))
collectgarbage('collect')
print('IMAGE_PROBE_COMPLETE')
"#,
        literal(&input),
        literal(&output)
    );
    let mut vm = Vm::new(Box::new(Output), Limits::default(), &GLOBAL_ALLOCATOR, 0).unwrap();
    vm.load("independent-image-probe", script.as_bytes())
        .unwrap();
    for _ in 0..100_000 {
        match vm.resume() {
            Step::Slice => {}
            Step::Done => {
                println!(
                    "heap_used={} heap_peak={}",
                    vm.memory_used(),
                    vm.memory_peak()
                );
                drop(vm);
                println!("IMAGE_DISCARD_COMPLETE");
                return;
            }
            other => panic!("image probe: {other:?}"),
        }
    }
    panic!("image probe did not finish");
}
