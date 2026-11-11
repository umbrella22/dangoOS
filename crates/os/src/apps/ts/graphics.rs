//! Owned native graphics handles. The VM exchanges IDs and copied byte spans.
use super::host::{arguments, bool_value, count, integer, invalid, output, text};
use alloc::{collections::BTreeMap, string::ToString, sync::Arc, vec::Vec};
use app_host::{
    esp_err,
    graphics::{BitmapOptions, Font, Framebuffer, Rect, Rectangles},
    image::{Format, Frame},
};
use serde_json::{json, Value};
use ts_engine::{HostError, HostReply};

pub struct Publication {
    pub pixels: Arc<Vec<u8>>,
    pub rectangles: Rectangles,
    pub width: u16,
}
/// The view borrows a publication that retains its immutable pixel allocation.
pub struct Blit<'a> {
    pub x0: u16,
    pub y0: u16,
    pub x1: u16,
    pub y1: u16,
    pub pixels: *const u8,
    pub stride: usize,
    _owner: &'a [u8],
}
pub struct Blits<'a> {
    frame: &'a Publication,
    rectangles: core::slice::Iter<'a, Rect>,
}
impl<'a> Iterator for Blits<'a> {
    type Item = Blit<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        let r = self.rectangles.next()?;
        let stride = usize::from(self.frame.width) * 2;
        let pixels = &self.frame.pixels[r.y as usize * stride + r.x as usize * 2..];
        Some(Blit {
            x0: r.x as u16,
            y0: r.y as u16,
            x1: (r.x + r.width) as u16,
            y1: (r.y + r.height) as u16,
            pixels: pixels.as_ptr(),
            stride,
            _owner: pixels,
        })
    }
}
impl<'a> IntoIterator for &'a Publication {
    type Item = Blit<'a>;
    type IntoIter = Blits<'a>;

    fn into_iter(self) -> Self::IntoIter {
        Blits {
            frame: self,
            rectangles: self.rectangles.iter(),
        }
    }
}
pub struct Graphics {
    pub screen: Option<Framebuffer>,
    fonts: BTreeMap<u32, (Font, usize)>,
    images: BTreeMap<u32, Frame>,
    next: u32,
    pub ui: bool,
    limit: usize,
    publication: Option<Arc<Vec<u8>>>,
    publication_allocations: usize,
    publication_copied_bytes: u64,
}
impl Graphics {
    /// File payloads stay native; the JS completion contains only a checked ID.
    pub fn file_request(&mut self, request: &mut ts_engine::HostRequest) -> Result<(), HostError> {
        if request.operation != "image.save_file" {
            return Ok(());
        }
        let a = arguments(&request.args)?;
        count(&a, 3)?;
        let path = text(&a[1])?;
        if !path.to_ascii_lowercase().ends_with(".jpg")
            && !path.to_ascii_lowercase().ends_with(".jpeg")
        {
            return Err(invalid("image output must be .jpg or .jpeg"));
        }
        let id = self.call("image.jpeg", &json!([a[0], a[2]]).to_string(), &[])?;
        let id: u32 =
            serde_json::from_str(&id.json).map_err(|_| invalid("invalid image identifier"))?;
        request.bytes = self
            .images
            .remove(&id)
            .ok_or_else(|| invalid("closed image handle"))?
            .pixels;
        request.operation = "storage.write".into();
        request.args = json!([path]).to_string();
        Ok(())
    }
    pub fn file_reply(
        &mut self,
        operation: &str,
        reply: Result<HostReply, HostError>,
    ) -> Result<HostReply, HostError> {
        let reply = reply?;
        match operation {
            "image.load_file" => self.call("image.decode", "[]", &reply.bytes),
            "font.load_file" => self.call("font.decode", "[]", &reply.bytes),
            _ => Ok(reply),
        }
    }
    pub fn new(limit: usize) -> Self {
        Self {
            screen: None,
            fonts: BTreeMap::new(),
            images: BTreeMap::new(),
            next: 1,
            ui: false,
            limit,
            publication: None,
            publication_allocations: 0,
            publication_copied_bytes: 0,
        }
    }
    fn used(&self) -> usize {
        self.screen.as_ref().map_or(0, Framebuffer::bytes)
            + self.publication.as_ref().map_or(0, |pixels| pixels.len())
            + self.fonts.values().map(|v| v.1).sum::<usize>()
            + self.images.values().map(|v| v.pixels.len()).sum::<usize>()
    }
    fn reserve(&self, bytes: usize) -> Result<(), HostError> {
        if self.used().saturating_add(bytes) > self.limit {
            Err(HostError::new(
                esp_err::NO_MEM,
                "native graphics budget exceeded",
            ))
        } else {
            Ok(())
        }
    }
    fn id(&mut self) -> Result<u32, HostError> {
        let id = self.next;
        self.next = self
            .next
            .checked_add(1)
            .ok_or_else(|| invalid("graphics handle space exhausted"))?;
        Ok(id)
    }
    fn screen(&mut self) -> Result<&mut Framebuffer, HostError> {
        self.screen
            .as_mut()
            .ok_or_else(|| invalid("display is closed"))
    }
    fn image(&self, id: &Value) -> Result<&Frame, HostError> {
        self.images
            .get(&integer(id)?)
            .ok_or_else(|| invalid("closed image handle"))
    }
    fn font(&self, id: &Value) -> Result<Option<&Font>, HostError> {
        if id.is_null() {
            Ok(None)
        } else {
            Ok(Some(
                &self
                    .fonts
                    .get(&integer(id)?)
                    .ok_or_else(|| invalid("closed font handle"))?
                    .0,
            ))
        }
    }
    pub fn present(&mut self, args: &str) -> Result<Publication, HostError> {
        let a = arguments(args)?;
        count(&a, 1)?;
        let full = bool_value(&a[0])?;
        let (rectangles, width, length) = {
            let screen = self.screen()?;
            (
                output(screen.rectangles(full))?,
                screen.width,
                screen.pixels().len(),
            )
        };
        if !rectangles.is_empty() {
            let source = self.screen.as_ref().unwrap().pixels();
            if let Some(bytes) = self
                .publication
                .as_mut()
                .and_then(Arc::get_mut)
                .filter(|bytes| bytes.len() == length)
            {
                let stride = usize::from(width) * 2;
                for rect in &rectangles {
                    let start = rect.x as usize * 2;
                    let count = rect.width as usize * 2;
                    for row in rect.y as usize..(rect.y + rect.height) as usize {
                        let offset = row * stride + start;
                        bytes[offset..offset + count]
                            .copy_from_slice(&source[offset..offset + count]);
                        self.publication_copied_bytes =
                            self.publication_copied_bytes.saturating_add(count as u64);
                    }
                }
            } else {
                // An outstanding UI consumer keeps its immutable snapshot.
                // Allocate a new snapshot only when exclusive reuse is impossible.
                self.reserve(length)?;
                let mut bytes = output(app_host::graphics::buffer(length))?;
                bytes.copy_from_slice(self.screen.as_ref().unwrap().pixels());
                self.publication = Some(Arc::new(bytes));
                self.publication_allocations = self.publication_allocations.saturating_add(1);
                self.publication_copied_bytes =
                    self.publication_copied_bytes.saturating_add(length as u64);
            }
        }
        Ok(Publication {
            pixels: self
                .publication
                .clone()
                .unwrap_or_else(|| Arc::new(Vec::new())),
            rectangles,
            width,
        })
    }
    pub fn presented(&mut self) {
        if let Some(s) = self.screen.as_mut() {
            s.presented();
        }
    }
    pub fn draw(&mut self, draw: ts_engine::Draw) -> Result<(), HostError> {
        output(self.screen()?.draw(
            draw.operation,
            draw.coordinates,
            draw.color,
            draw.start,
            draw.end,
        ))
    }
    pub fn call(&mut self, op: &str, args: &str, bytes: &[u8]) -> Result<HostReply, HostError> {
        let a = arguments(args)?;
        if !bytes.is_empty()
            && !matches!(
                op,
                "font.decode" | "image.decode" | "image.raw" | "display.blit"
            )
        {
            return Err(invalid("operation accepts no bytes"));
        }
        let value = match op {
            "display.open" => {
                count(&a, 3)?;
                if self.ui || self.screen.is_some() {
                    return Err(invalid("screen already leased"));
                }
                let w = small(&a[0])?;
                let h = small(&a[1])?;
                let n = integer(&a[2])?;
                if !(1..=2).contains(&n) {
                    return Err(invalid("framebuffer count must be 1 or 2"));
                }
                self.reserve(usize::from(w) * usize::from(h) * 2 * n as usize)?;
                self.screen = Some(output(Framebuffer::new(w, h, n as u8))?);
                json!({"width":w,"height":h,"bpp":16,"framebuffer_count":n})
            }
            "display.close" => {
                count(&a, 0)?;
                self.screen = None;
                self.publication = None;
                Value::Null
            }
            "display.info" => {
                count(&a, 0)?;
                let s = self.screen()?;
                json!({"width":s.width,"height":s.height,"bpp":16,"framebuffer_count":s.count})
            }
            "display.stats" => {
                count(&a, 0)?;
                json!({"native_bytes":self.used(),"limit_bytes":self.limit,"fonts":self.fonts.len(),"images":self.images.len(),
                    "publication_bytes":self.publication.as_ref().map_or(0,|pixels|pixels.len()),
                    "publication_allocations":self.publication_allocations,
                    "publication_copied_bytes":self.publication_copied_bytes})
            }
            "display.begin" => {
                count(&a, 1)?;
                let clear = if a[0].is_null() {
                    None
                } else {
                    Some(integer(&a[0])?)
                };
                output(self.screen()?.begin(clear))?;
                Value::Null
            }
            "display.save" => {
                count(&a, 0)?;
                output(self.screen()?.save())?;
                Value::Null
            }
            "display.restore" => {
                count(&a, 0)?;
                output(self.screen()?.restore())?;
                Value::Null
            }
            "display.translate" => {
                count(&a, 2)?;
                output(self.screen()?.translate(signed(&a[0])?, signed(&a[1])?))?;
                Value::Null
            }
            "display.clip" => {
                count(&a, 4)?;
                output(self.screen()?.clip(
                    signed(&a[0])?,
                    signed(&a[1])?,
                    signed(&a[2])?,
                    signed(&a[3])?,
                ))?;
                Value::Null
            }
            "display.draw" => {
                count(&a, 5)?;
                let v = a[1]
                    .as_array()
                    .filter(|v| v.len() == 6)
                    .ok_or_else(|| invalid("six shape coordinates required"))?;
                let mut coords = [0; 6];
                for (i, c) in coords.iter_mut().enumerate() {
                    *c = signed(&v[i])?;
                }
                self.draw(ts_engine::Draw {
                    operation: signed(&a[0])?,
                    coordinates: coords,
                    color: integer(&a[2])?,
                    start: number(&a[3])?,
                    end: number(&a[4])?,
                })?;
                Value::Null
            }
            "display.measure" => {
                count(&a, 3)?;
                let (w, h) = output(Framebuffer::measure(
                    text(&a[0])?,
                    signed(&a[1])?,
                    self.font(&a[2])?,
                ))?;
                json!({"width":w,"height":h})
            }
            "display.text" => {
                count(&a, 6)?;
                let font = if a[5].is_null() {
                    None
                } else {
                    Some(
                        &self
                            .fonts
                            .get(&integer(&a[5])?)
                            .ok_or_else(|| invalid("closed font handle"))?
                            .0,
                    )
                };
                let s = self
                    .screen
                    .as_mut()
                    .ok_or_else(|| invalid("display is closed"))?;
                output(s.text(
                    signed(&a[0])?,
                    signed(&a[1])?,
                    text(&a[2])?,
                    signed(&a[3])?,
                    integer(&a[4])?,
                    font,
                ))?;
                Value::Null
            }
            "font.decode" => {
                count(&a, 0)?;
                self.reserve(bytes.len() + 64)?;
                let f = output(Font::decode(bytes))?;
                let id = self.id()?;
                self.fonts.insert(id, (f, bytes.len() + 64));
                json!(id)
            }
            "font.close" => {
                count(&a, 1)?;
                self.fonts.remove(&integer(&a[0])?);
                Value::Null
            }
            "image.decode" => {
                count(&a, 0)?;
                self.reserve(decode_cost(bytes)?)?;
                let f = output(Frame::decode(bytes))?;
                let id = self.id()?;
                self.images.insert(id, f);
                json!(id)
            }
            "image.raw" => {
                count(&a, 3)?;
                self.reserve(bytes.len())?;
                let mut pixels = output(app_host::graphics::buffer(bytes.len()))?;
                pixels.copy_from_slice(bytes);
                let f = output(Frame::raw(
                    small(&a[0])?,
                    small(&a[1])?,
                    output(Format::parse(text(&a[2])?))?,
                    pixels,
                ))?;
                let id = self.id()?;
                self.images.insert(id, f);
                json!(id)
            }
            "image.info" => {
                count(&a, 1)?;
                let f = self.image(&a[0])?;
                json!({"width":f.width,"height":f.height,"format":f.format.name(),"bytes":f.pixels.len()})
            }
            "image.close" => {
                count(&a, 1)?;
                self.images.remove(&integer(&a[0])?);
                Value::Null
            }
            "image.bytes" => {
                count(&a, 1)?;
                return Ok(HostReply {
                    json: "null".into(),
                    bytes: self.image(&a[0])?.pixels.clone(),
                });
            }
            "image.jpeg" => {
                count(&a, 2)?;
                let f = self.image(&a[0])?;
                let quality = integer(&a[1])?;
                if !(1..=100).contains(&quality) {
                    return Err(invalid("JPEG quality must be 1..100"));
                }
                let scratch = usize::from(f.width) * usize::from(f.height) * 3 + 16384;
                self.reserve(scratch)?;
                let available = (self.limit - self.used() - scratch).min(app_host::image::FILE_MAX);
                let rgb = output(f.convert(Format::Rgb888))?;
                let pixels = output(app_host::jpeg::encode_rgb888_limited(
                    f.width,
                    f.height,
                    &rgb.pixels,
                    quality as u8,
                    available,
                ))?;
                let f = output(Frame::raw(f.width, f.height, Format::Jpeg, pixels))?;
                let id = self.id()?;
                self.images.insert(id, f);
                json!(id)
            }
            "image.convert" | "image.resize" => {
                count(&a, if op == "image.convert" { 2 } else { 5 })?;
                let f = self.image(&a[0])?;
                let format = output(Format::parse(text(&a[1])?))?;
                let (w, h) = if op == "image.resize" {
                    (small(&a[2])?, small(&a[3])?)
                } else {
                    (f.width, f.height)
                };
                self.reserve(
                    usize::from(f.width) * usize::from(f.height) * 3
                        + usize::from(w)
                            * usize::from(h)
                            * if matches!(format, Format::Rgb888 | Format::Bgr888) {
                                3
                            } else {
                                2
                            },
                )?;
                let f = if op == "image.resize" {
                    output(f.resize(w, h, format, bool_value(&a[4])?))?
                } else if matches!(format, Format::Jpeg | Format::Mjpeg) {
                    let scratch = usize::from(f.width) * usize::from(f.height) * 3 + 16384;
                    self.reserve(scratch)?;
                    let available =
                        (self.limit - self.used() - scratch).min(app_host::image::FILE_MAX);
                    let rgb = output(f.convert(Format::Rgb888))?;
                    let pixels = output(app_host::jpeg::encode_rgb888_limited(
                        w,
                        h,
                        &rgb.pixels,
                        90,
                        available,
                    ))?;
                    output(Frame::raw(w, h, format, pixels))?
                } else {
                    output(f.convert(format))?
                };
                let id = self.id()?;
                self.images.insert(id, f);
                json!(id)
            }
            "display.image" => {
                count(&a, 4)?;
                let id = integer(&a[0])?;
                let image = self
                    .images
                    .get(&id)
                    .ok_or_else(|| invalid("closed image handle"))?;
                let converted;
                let f = if matches!(
                    image.format,
                    Format::Rgb565 | Format::Rgb888 | Format::Bgr888
                ) {
                    image
                } else {
                    self.reserve(usize::from(image.width) * usize::from(image.height) * 5)?;
                    converted = output(image.convert(Format::Rgb565))?;
                    &converted
                };
                let (x, y, options) = bitmap_options(&a[1..])?;
                let s = self
                    .screen
                    .as_mut()
                    .ok_or_else(|| invalid("display is closed"))?;
                output(s.bitmap(
                    x,
                    y,
                    f.width,
                    f.height,
                    bitmap_format(f.format)?,
                    &f.pixels,
                    options,
                ))?;
                Value::Null
            }
            "display.blit" => {
                count(&a, 6)?;
                let (x, y, options) = bitmap_options(&a[3..])?;
                let format = output(Format::parse(text(&a[2])?))?;
                output(self.screen()?.bitmap(
                    x,
                    y,
                    small(&a[0])?,
                    small(&a[1])?,
                    bitmap_format(format)?,
                    bytes,
                    options,
                ))?;
                Value::Null
            }
            _ => {
                return Err(HostError::new(
                    esp_err::NOT_SUPPORTED,
                    "unknown graphics operation",
                ))
            }
        };
        Ok(HostReply::json(value.to_string()))
    }
}
fn signed(v: &Value) -> Result<i32, HostError> {
    v.as_i64()
        .and_then(|n| i32::try_from(n).ok())
        .ok_or_else(|| invalid("int32 required"))
}
fn small(v: &Value) -> Result<u16, HostError> {
    u16::try_from(integer(v)?).map_err(|_| invalid("uint16 required"))
}
fn number(v: &Value) -> Result<f64, HostError> {
    v.as_f64()
        .filter(|v| v.is_finite())
        .ok_or_else(|| invalid("finite number required"))
}
fn bitmap_format(f: Format) -> Result<i32, HostError> {
    match f {
        Format::Rgb565 => Ok(0),
        Format::Bgr888 => Ok(1),
        Format::Rgb888 => Ok(2),
        _ => Err(invalid("unsupported display format")),
    }
}
fn bitmap_options(a: &[Value]) -> Result<(i32, i32, BitmapOptions), HostError> {
    let o = a[2]
        .as_array()
        .filter(|v| v.len() == 8)
        .ok_or_else(|| invalid("eight bitmap options required"))?;
    Ok((
        signed(&a[0])?,
        signed(&a[1])?,
        BitmapOptions {
            mode: signed(&o[0])?,
            width: signed(&o[1])?,
            height: signed(&o[2])?,
            source_x: signed(&o[3])?,
            source_y: signed(&o[4])?,
            source_width: signed(&o[5])?,
            source_height: signed(&o[6])?,
            opacity: u8::try_from(integer(&o[7])?)
                .map_err(|_| invalid("opacity must be 0..255"))?,
        },
    ))
}
fn decode_cost(bytes: &[u8]) -> Result<usize, HostError> {
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") && bytes.len() >= 24 {
        let w = u32::from_be_bytes(bytes[16..20].try_into().unwrap()) as usize;
        let h = u32::from_be_bytes(bytes[20..24].try_into().unwrap()) as usize;
        w.checked_mul(h)
            .and_then(|v| v.checked_mul(16))
            .and_then(|v| v.checked_add(65536))
            .ok_or_else(|| invalid("image dimensions overflow"))
    } else {
        Ok(bytes.len())
    }
}
