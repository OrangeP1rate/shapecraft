//! Picture work done natively for plugins (they run in an interpreter, so decoding and scaling there would be slow).

use image::{DynamicImage, GrayImage, ImageFormat, RgbaImage, imageops::FilterType};

/// The largest side a picture may have (generated pictures are 1-4K).
pub const MAX_SIDE: u32 = 8192;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Picture {
    pub width: u32,
    pub height: u32,
    /// RGBA8, not premultiplied, rows top to bottom.
    pub pixels: Vec<u8>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Mask {
    pub width: u32,
    pub height: u32,
    pub values: Vec<u8>,
}

fn check_size(w: u32, h: u32) -> Result<(), String> {
    if w == 0 || h == 0 || w > MAX_SIDE || h > MAX_SIDE {
        return Err(format!("a {w} × {h} picture is out of range (1-{MAX_SIDE} a side)"));
    }
    Ok(())
}

impl Picture {
    pub fn to_rgba(&self) -> Result<RgbaImage, String> {
        check_size(self.width, self.height)?;
        RgbaImage::from_raw(self.width, self.height, self.pixels.clone())
            .ok_or_else(|| format!("a {} × {} picture needs {} bytes", self.width, self.height, self.width * self.height * 4))
    }

    pub fn from_rgba(img: RgbaImage) -> Self {
        Self { width: img.width(), height: img.height(), pixels: img.into_raw() }
    }
}

impl Mask {
    pub fn to_gray(&self) -> Result<GrayImage, String> {
        check_size(self.width, self.height)?;
        GrayImage::from_raw(self.width, self.height, self.values.clone())
            .ok_or_else(|| format!("a {} × {} mask needs {} bytes", self.width, self.height, self.width * self.height))
    }

    /// The alpha channel of `p`.
    pub fn alpha_of(p: &Picture) -> Self {
        Self { width: p.width, height: p.height, values: p.pixels.as_chunks::<4>().0.iter().map(|px| px[3]).collect() }
    }
}

pub fn decode(bytes: &[u8]) -> Result<Picture, String> {
    let format = image::guess_format(bytes).map_err(|_| "not a PNG, JPEG or WebP picture".to_string())?;
    if !matches!(format, ImageFormat::Png | ImageFormat::Jpeg | ImageFormat::WebP) {
        return Err(format!("{format:?} pictures aren't supported (PNG, JPEG or WebP)"));
    }
    let mut reader = image::ImageReader::with_format(std::io::Cursor::new(bytes), format);
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(MAX_SIDE);
    limits.max_image_height = Some(MAX_SIDE);
    limits.max_alloc = Some(512 << 20);
    reader.limits(limits);
    let img = reader.decode().map_err(|e| format!("the picture couldn't be read ({e})"))?;
    Ok(Picture::from_rgba(img.to_rgba8()))
}

pub fn encode_png(p: &Picture) -> Result<Vec<u8>, String> {
    let img = p.to_rgba()?;
    let mut out = std::io::Cursor::new(Vec::new());
    img.write_to(&mut out, ImageFormat::Png).map_err(|e| e.to_string())?;
    Ok(out.into_inner())
}

pub fn encode_jpeg(p: &Picture, quality: u8) -> Result<Vec<u8>, String> {
    let img = DynamicImage::ImageRgba8(p.to_rgba()?).to_rgb8();
    let mut out = Vec::new();
    let enc = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, quality.clamp(1, 100));
    img.write_with_encoder(enc).map_err(|e| e.to_string())?;
    Ok(out)
}

/// Scales to cover `w` × `h` and crops the overflow around the middle.
pub fn fit(p: &Picture, w: u32, h: u32) -> Result<Picture, String> {
    check_size(w, h)?;
    let img = p.to_rgba()?;
    if img.width() == w && img.height() == h {
        return Ok(p.clone());
    }
    let out = DynamicImage::ImageRgba8(img).resize_to_fill(w, h, FilterType::Lanczos3);
    Ok(Picture::from_rgba(out.to_rgba8()))
}

fn fit_mask(m: &Mask, w: u32, h: u32) -> Result<GrayImage, String> {
    let g = m.to_gray()?;
    if g.width() == w && g.height() == h {
        return Ok(g);
    }
    Ok(DynamicImage::ImageLuma8(g).resize_to_fill(w, h, FilterType::Triangle).to_luma8())
}

/// `top` over `base` by `amount` (255 = all `top`). `base` and `amount` are fitted to `top`'s size first; the
/// result is opaque where `base` is.
pub fn composite(base: &Picture, top: &Picture, amount: &Mask) -> Result<Picture, String> {
    let (w, h) = (top.width, top.height);
    let base = fit(base, w, h)?.to_rgba()?;
    let top_img = top.to_rgba()?;
    let m = fit_mask(amount, w, h)?;
    let mut out = base;
    for ((o, t), a) in out.pixels_mut().zip(top_img.pixels()).zip(m.pixels()) {
        let k = u32::from(a.0[0]);
        for c in 0..3 {
            o.0[c] = ((u32::from(t.0[c]) * k + u32::from(o.0[c]) * (255 - k) + 127) / 255) as u8;
        }
        o.0[3] = o.0[3].max(((u32::from(t.0[3]) * k) / 255) as u8);
    }
    Ok(Picture::from_rgba(out))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn solid(w: u32, h: u32, c: [u8; 4]) -> Picture {
        Picture { width: w, height: h, pixels: c.repeat((w * h) as usize) }
    }

    #[test]
    fn round_trips_png_and_jpeg() -> Result<(), String> {
        let p = solid(4, 3, [10, 200, 30, 255]);
        assert_eq!(decode(&encode_png(&p)?)?, p);
        let j = decode(&encode_jpeg(&p, 90)?)?;
        assert_eq!((j.width, j.height), (4, 3));
        assert!(decode(b"nope").is_err());
        Ok(())
    }

    #[test]
    fn fit_covers_without_stretching() -> Result<(), String> {
        let p = solid(100, 50, [1, 2, 3, 255]);
        let f = fit(&p, 40, 40)?;
        assert_eq!((f.width, f.height), (40, 40));
        assert!(fit(&p, 0, 10).is_err());
        Ok(())
    }

    #[test]
    fn composite_keeps_the_model() -> Result<(), String> {
        // The model (top) where the mask is 255, the generated scene (base) elsewhere, whatever the base's size.
        let scene = solid(20, 20, [0, 0, 255, 255]);
        let model = solid(10, 10, [255, 0, 0, 255]);
        let mut amount = Mask { width: 10, height: 10, values: vec![0; 100] };
        amount.values[0] = 255;
        let out = composite(&scene, &model, &amount)?;
        assert_eq!((out.width, out.height), (10, 10));
        assert_eq!(&out.pixels[0..4], &[255, 0, 0, 255]);
        assert_eq!(&out.pixels[4..8], &[0, 0, 255, 255]);
        Ok(())
    }
}
