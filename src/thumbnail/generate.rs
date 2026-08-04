use anyhow::Context;
use image::GenericImageView;
use std::path::Path;

/// Supported image formats for decoding.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ImageFormat {
    Jpeg,
    Png,
    WebP,
    Gif,
    Heic,
    Heif,
    Avif,
    Tiff,
}

impl ImageFormat {
    pub fn from_path(path: &Path) -> Option<Self> {
        match path.extension()?.to_str()?.to_lowercase().as_str() {
            "jpg" | "jpeg" => Some(Self::Jpeg),
            "png" => Some(Self::Png),
            "webp" => Some(Self::WebP),
            "gif" => Some(Self::Gif),
            "heic" => Some(Self::Heic),
            "heif" => Some(Self::Heif),
            "avif" => Some(Self::Avif),
            "tif" | "tiff" => Some(Self::Tiff),
            _ => None,
        }
    }

    #[allow(dead_code)]
    pub fn extension(&self) -> &str {
        match self {
            Self::Jpeg => "jpg",
            Self::Png => "png",
            Self::WebP => "webp",
            Self::Gif => "gif",
            Self::Heic => "heic",
            Self::Heif => "heif",
            Self::Avif => "avif",
            Self::Tiff => "tiff",
        }
    }
}

/// Decoded image buffer with dimensions.
pub struct DecodedImage {
    pub width: u32,
    pub height: u32,
    pub data: Vec<u8>, // RGBA
}

/// Decode an image file into RGBA buffer.
/// Uses `image` crate for standard formats; `libheif-rs` for HEIC (if feature enabled).
pub fn decode_image(path: &Path) -> anyhow::Result<DecodedImage> {
    let format = ImageFormat::from_path(path).context("unsupported image format")?;

    match format {
        #[cfg(feature = "heic")]
        ImageFormat::Heic | ImageFormat::Heif | ImageFormat::Avif => decode_heic(path),
        #[cfg(not(feature = "heic"))]
        ImageFormat::Heic | ImageFormat::Heif | ImageFormat::Avif => {
            anyhow::bail!("HEIC support not compiled in (enable 'heic' feature)")
        }
        _ => {
            let img =
                image::open(path).with_context(|| format!("failed to open image: {:?}", path))?;
            let (width, height) = img.dimensions();
            let data = img.to_rgba8().into_raw();
            Ok(DecodedImage {
                width,
                height,
                data,
            })
        }
    }
}

/// Decode HEIC/HEIF/AVIF using libheif-rs.
#[cfg(feature = "heic")]
fn decode_heic(path: &Path) -> anyhow::Result<DecodedImage> {
    use libheif_rs::{Channel, ColorSpace, HeifContext, LibHeif, RgbChroma, StreamReader};

    let _libheif = LibHeif::new();

    let file =
        std::fs::read(path).with_context(|| format!("failed to read HEIC file: {:?}", path))?;

    let reader = StreamReader::new(file)?;
    let ctx = HeifContext::read_from_reader(Box::new(reader))?;
    let handle = ctx.primary_image_handle()?;

    let width = handle.width();
    let height = handle.height();

    let image = handle.decode(ColorSpace::Rgb(RgbChroma::Rgba), false)?;
    let planes = image.planes();
    let plane = &planes.interleaved.unwrap();

    Ok(DecodedImage {
        width,
        height,
        data: plane.data.clone(),
    })
}

/// Generate a WebP thumbnail from a source image.
/// `target_width` = desired width in pixels; height is proportional.
/// `quality` = WebP quality 0-100.
pub fn generate_thumbnail(
    source: &Path,
    target_width: u32,
    quality: f32,
) -> anyhow::Result<Vec<u8>> {
    let decoded = decode_image(source)?;

    const MAX_BYTES: usize = 15 * 1024;
    let image = image::RgbaImage::from_raw(decoded.width, decoded.height, decoded.data)
        .context("failed to create image from decoded data")?;
    let mut width = target_width.min(decoded.width);
    let mut encoder_quality = quality;

    loop {
        let height = ((decoded.height as f64 * width as f64 / decoded.width as f64).round() as u32).max(1);
        let resized = if width == decoded.width {
            image.clone()
        } else {
            image::imageops::resize(&image, width, height, image::imageops::FilterType::Lanczos3)
        };
        let encoded = webp::Encoder::from_rgba(resized.as_raw(), width, height)
            .encode(encoder_quality)
            .to_vec();
        if encoded.len() <= MAX_BYTES || width <= 48 {
            return Ok(encoded);
        }
        width = ((width as f32 * 0.8).round() as u32).max(48);
        encoder_quality = (encoder_quality - 10.0).max(20.0);
    }
}

/// Get image dimensions without full decode.
pub fn get_dimensions(path: &Path) -> anyhow::Result<(u32, u32)> {
    let format = ImageFormat::from_path(path).context("unsupported image format")?;

    match format {
        #[cfg(feature = "heic")]
        ImageFormat::Heic | ImageFormat::Heif | ImageFormat::Avif => {
            use libheif_rs::{HeifContext, LibHeif, StreamReader};
            let _libheif = LibHeif::new();
            let file = std::fs::read(path)?;
            let reader = StreamReader::new(file)?;
            let ctx = HeifContext::read_from_reader(Box::new(reader))?;
            let handle = ctx.primary_image_handle()?;
            Ok((handle.width(), handle.height()))
        }
        #[cfg(not(feature = "heic"))]
        ImageFormat::Heic | ImageFormat::Heif | ImageFormat::Avif => {
            Ok((0, 0)) // dimensions unknown without HEIC support
        }
        _ => {
            let reader = image::ImageReader::open(path)?.with_guessed_format()?;
            let (w, h) = reader.into_dimensions()?;
            Ok((w, h))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_detection() {
        assert_eq!(
            ImageFormat::from_path(Path::new("photo.jpg")),
            Some(ImageFormat::Jpeg)
        );
        assert_eq!(
            ImageFormat::from_path(Path::new("photo.JPEG")),
            Some(ImageFormat::Jpeg)
        );
        assert_eq!(
            ImageFormat::from_path(Path::new("photo.heic")),
            Some(ImageFormat::Heic)
        );
        assert_eq!(ImageFormat::from_path(Path::new("video.mp4")), None);
    }
}
