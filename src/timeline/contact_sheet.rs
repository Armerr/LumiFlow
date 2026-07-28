use anyhow::{Context, Result};
use image::codecs::jpeg::JpegEncoder;
use image::imageops::FilterType;
use image::{GenericImage, Rgb, RgbImage};
use std::fs::{File, OpenOptions};
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

const CELL_SIZE: u32 = 224;
const MAX_COLUMNS: usize = 6;
const JPEG_QUALITY: u8 = 88;

/// Select up to `max_count` evenly spaced positions while retaining both endpoints.
pub fn representative_indices(count: usize, max_count: usize) -> Vec<usize> {
    let selected_count = count.min(max_count);
    match selected_count {
        0 => Vec::new(),
        1 => vec![0],
        selected_count if selected_count == count => (0..count).collect(),
        selected_count => {
            let intervals = selected_count - 1;
            let span = count - 1;
            let quotient = span / intervals;
            let remainder = span % intervals;
            (0..selected_count)
                .map(|index| quotient * index + remainder * index / intervals)
                .collect()
        }
    }
}

/// Render cached thumbnails, in the supplied chronological order, as a JPEG contact sheet.
pub fn render_contact_sheet<P: AsRef<Path>>(thumbnail_paths: &[P], output: &Path) -> Result<()> {
    if thumbnail_paths.is_empty() {
        anyhow::bail!("cannot render a contact sheet without thumbnails");
    }

    let columns = thumbnail_paths.len().min(MAX_COLUMNS) as u32;
    let rows = thumbnail_paths.len().div_ceil(MAX_COLUMNS) as u32;
    let mut sheet = RgbImage::from_pixel(columns * CELL_SIZE, rows * CELL_SIZE, Rgb([0, 0, 0]));

    for (index, thumbnail_path) in thumbnail_paths.iter().enumerate() {
        let path = thumbnail_path.as_ref();
        let thumbnail = image::open(path)
            .with_context(|| format!("failed to decode thumbnail {}", path.display()))?
            .resize(CELL_SIZE, CELL_SIZE, FilterType::Lanczos3)
            .to_rgb8();
        let column = (index % MAX_COLUMNS) as u32;
        let row = (index / MAX_COLUMNS) as u32;
        let x = column * CELL_SIZE + (CELL_SIZE - thumbnail.width()) / 2;
        let y = row * CELL_SIZE + (CELL_SIZE - thumbnail.height()) / 2;
        sheet
            .copy_from(&thumbnail, x, y)
            .context("resized thumbnail did not fit its contact-sheet cell")?;
    }

    write_jpeg_atomically(&sheet, output)
}

fn write_jpeg_atomically(sheet: &RgbImage, output: &Path) -> Result<()> {
    let parent = output.parent().filter(|path| !path.as_os_str().is_empty());
    if let Some(parent) = parent {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("failed to create output directory {}", parent.display()))?;
    }

    let (temporary_path, temporary_file) = create_temporary_file(output)?;
    let write_result = (|| -> Result<()> {
        let mut writer = BufWriter::new(temporary_file);
        JpegEncoder::new_with_quality(&mut writer, JPEG_QUALITY)
            .encode_image(sheet)
            .context("failed to encode contact sheet as JPEG")?;
        writer.flush().context("failed to flush contact sheet")?;
        writer
            .get_ref()
            .sync_all()
            .context("failed to sync contact sheet")?;
        drop(writer);
        std::fs::rename(&temporary_path, output).with_context(|| {
            format!(
                "failed to atomically replace contact sheet {}",
                output.display()
            )
        })?;
        Ok(())
    })();

    if write_result.is_err() {
        let _ = std::fs::remove_file(&temporary_path);
    }
    write_result
}

fn create_temporary_file(output: &Path) -> Result<(PathBuf, File)> {
    let parent = output.parent().unwrap_or_else(|| Path::new(""));
    let filename = output
        .file_name()
        .context("contact-sheet output path has no filename")?
        .to_string_lossy();
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();

    for attempt in 0..100u32 {
        let path = parent.join(format!(
            ".{filename}.{}.{}.tmp",
            std::process::id(),
            nonce + u128::from(attempt)
        ));
        match OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(file) => return Ok((path, file)),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => {
                return Err(error).with_context(|| {
                    format!(
                        "failed to create temporary contact sheet {}",
                        path.display()
                    )
                });
            }
        }
    }

    anyhow::bail!("failed to allocate a unique temporary contact-sheet path")
}

#[cfg(test)]
mod tests {
    use super::{render_contact_sheet, representative_indices};
    use image::{GenericImageView, ImageBuffer, ImageFormat, Rgb};
    use std::path::{Path, PathBuf};
    use std::time::{SystemTime, UNIX_EPOCH};

    struct TestDir(PathBuf);

    impl TestDir {
        fn new() -> Self {
            let nonce = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("system clock should follow Unix epoch")
                .as_nanos();
            let path = std::env::temp_dir().join(format!(
                "lumiflow-contact-sheet-{}-{nonce}",
                std::process::id()
            ));
            std::fs::create_dir_all(&path).expect("test directory should be created");
            Self(path)
        }

        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TestDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn representative_indices_respects_requested_count_and_endpoints() {
        assert_eq!(representative_indices(0, 4), Vec::<usize>::new());
        assert_eq!(representative_indices(4, 0), Vec::<usize>::new());
        assert_eq!(representative_indices(1, 4), vec![0]);
        assert_eq!(representative_indices(4, 8), vec![0, 1, 2, 3]);

        let sampled = representative_indices(17, 6);
        assert_eq!(sampled.len(), 6);
        assert_eq!(sampled.first(), Some(&0));
        assert_eq!(sampled.last(), Some(&16));
    }

    #[test]
    fn representative_indices_supports_maximum_collection_size() {
        assert_eq!(
            representative_indices(usize::MAX, 3),
            vec![0, usize::MAX / 2, usize::MAX - 1]
        );
    }

    #[test]
    fn representative_indices_are_strictly_ordered() {
        for count in 1..40 {
            for max_count in 1..40 {
                let sampled = representative_indices(count, max_count);
                assert!(
                    sampled.windows(2).all(|pair| pair[0] < pair[1]),
                    "indices must increase for count={count}, max_count={max_count}: {sampled:?}"
                );
            }
        }
    }

    #[test]
    fn renders_webp_thumbnails_as_decodable_jpeg_contact_sheet() {
        let dir = TestDir::new();
        let dimensions = [(320, 160), (120, 300), (224, 224), (448, 112)];
        let colors: [Rgb<u8>; 4] = [
            Rgb([220, 30, 30]),
            Rgb([30, 220, 30]),
            Rgb([30, 30, 220]),
            Rgb([220, 180, 30]),
        ];
        let mut paths = Vec::new();

        for (index, ((width, height), color)) in dimensions.into_iter().zip(colors).enumerate() {
            let path = dir.path().join(format!("thumb-{index}.webp"));
            let image = ImageBuffer::from_pixel(width, height, color);
            image
                .save_with_format(&path, ImageFormat::WebP)
                .expect("WebP fixture should encode");
            paths.push(path);
        }

        let output = dir.path().join("sheet.jpg");
        render_contact_sheet(&paths, &output).expect("contact sheet should render");

        let bytes = std::fs::read(&output).expect("contact sheet should be written");
        assert_eq!(&bytes[..2], &[0xff, 0xd8]);
        let sheet = image::load_from_memory_with_format(&bytes, ImageFormat::Jpeg)
            .expect("contact sheet should decode as JPEG");
        assert_eq!(sheet.dimensions(), (4 * 224, 224));

        // Wide and tall thumbnails retain their aspect ratios, leaving dark letterbox bars.
        let wide_bar = sheet.get_pixel(10, 10).0;
        assert!(wide_bar[0] < 20 && wide_bar[1] < 20 && wide_bar[2] < 20);
        let tall_bar = sheet.get_pixel(224 + 10, 112).0;
        assert!(tall_bar[0] < 20 && tall_bar[1] < 20 && tall_bar[2] < 20);

        // Input order is preserved from left to right.
        let centers = [112, 336, 560, 784].map(|x| sheet.get_pixel(x, 112).0);
        assert!(centers[0][0] > centers[0][1] && centers[0][0] > centers[0][2]);
        assert!(centers[1][1] > centers[1][0] && centers[1][1] > centers[1][2]);
        assert!(centers[2][2] > centers[2][0] && centers[2][2] > centers[2][1]);
        assert!(centers[3][0] > centers[3][2] && centers[3][1] > centers[3][2]);
    }
}
