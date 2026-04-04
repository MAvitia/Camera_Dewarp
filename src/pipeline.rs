use crate::calibration::{self, DjiCalibration};
use crate::remap;
use crossbeam_channel::Sender;
use image::codecs::jpeg::JpegEncoder;
use image::{ImageReader, RgbImage};
use rayon::prelude::*;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

const SUPPORTED_EXTS: &[&str] = &["jpg", "jpeg", "png", "tif", "tiff", "bmp", "webp"];

pub struct BatchSettings {
    pub quality: u8,
    pub alpha: f64,
    pub recursive: bool,
    pub use_gpu: bool,
    pub reverse: Option<bool>,
    pub external_cal: Option<DjiCalibration>,
}

#[derive(Clone)]
pub struct BatchSummary {
    pub total: usize,
    pub processed: usize,
    pub skipped: usize,
    pub elapsed_secs: f64,
}

#[derive(Clone)]
pub struct PreviewData {
    pub original_thumb: Vec<u8>,
    pub dewarped_thumb: Vec<u8>,
    pub thumb_w: u32,
    pub thumb_h: u32,
}

pub enum ProgressMsg {
    CalibrationLoaded(DjiCalibration),
    Processing {
        current: usize,
        total: usize,
        name: String,
    },
    Preview(PreviewData),
    Done(BatchSummary),
    Error(String),
}

pub fn collect_images(input: &Path, recursive: bool) -> Vec<PathBuf> {
    if input.is_file() {
        if is_supported_image(input) {
            return vec![input.to_path_buf()];
        }
        return vec![];
    }

    let mut files: Vec<PathBuf> = if recursive {
        walkdir(input)
    } else {
        std::fs::read_dir(input)
            .into_iter()
            .flatten()
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| p.is_file() && is_supported_image(p))
            .collect()
    };
    files.sort();
    files
}

fn walkdir(dir: &Path) -> Vec<PathBuf> {
    let mut result = Vec::new();
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            let p = entry.path();
            if p.is_dir() {
                result.extend(walkdir(&p));
            } else if is_supported_image(&p) {
                result.push(p);
            }
        }
    }
    result
}

fn is_supported_image(p: &Path) -> bool {
    p.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| SUPPORTED_EXTS.contains(&e.to_ascii_lowercase().as_str()))
}

pub fn batch_dewarp(
    input: &Path,
    output: &Path,
    settings: &BatchSettings,
    progress_tx: Option<Sender<ProgressMsg>>,
) {
    let send = |msg: ProgressMsg| {
        if let Some(tx) = &progress_tx {
            let _ = tx.send(msg);
        }
    };

    let images = collect_images(input, settings.recursive);
    if images.is_empty() {
        send(ProgressMsg::Error("No supported images found.".into()));
        return;
    }

    let (cal, used_external) = match calibration::read_dji_calibration(&images[0]) {
        Some(c) => (c, false),
        None => match &settings.external_cal {
            Some(ext) => (ext.clone(), true),
            None => {
                send(ProgressMsg::Error(format!(
                    "No DJI DewarpData in {} — load a calibration file",
                    images[0].file_name().unwrap_or_default().to_string_lossy()
                )));
                return;
            }
        },
    };

    // Auto-detect: if external cal was needed (no embedded data), images are already
    // dewarped on-device, so default to reverse mode.
    let reverse = settings
        .reverse
        .unwrap_or_else(|| if used_external { true } else { cal.dewarp_flag != 0 });

    send(ProgressMsg::CalibrationLoaded(cal.clone()));

    let first_img = match load_rgb_image(&images[0]) {
        Some(img) => img,
        None => {
            send(ProgressMsg::Error(format!(
                "Cannot read {}",
                images[0].display()
            )));
            return;
        }
    };
    let (w, h) = (first_img.width(), first_img.height());

    let lut = Arc::new(if reverse {
        remap::build_redistort_lut(&cal, w, h)
    } else {
        remap::build_undistort_lut(&cal, w, h, settings.alpha)
    });

    std::fs::create_dir_all(output).ok();

    let total = images.len();
    let quality = settings.quality;
    let lut_ref = Arc::clone(&lut);
    let t0 = Instant::now();

    // Process images in parallel chunks, sending progress after each
    let skipped = std::sync::atomic::AtomicUsize::new(0);
    let completed = std::sync::atomic::AtomicUsize::new(0);

    images.par_iter().for_each(|img_path| {
        let cur = completed.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
        let name = img_path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();

        send(ProgressMsg::Processing {
            current: cur,
            total,
            name: name.clone(),
        });

        let Some(img) = load_rgb_image(img_path) else {
            skipped.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            return;
        };

        let src_data = img.as_raw();
        let dewarped_data = remap::cpu_remap(src_data, w, h, &lut_ref);

        // Build output path
        let rel = if input.is_dir() {
            img_path.strip_prefix(input).unwrap_or(img_path.as_path())
        } else {
            Path::new(img_path.file_name().unwrap())
        };
        let stem = rel.file_stem().unwrap_or_default().to_string_lossy();
        let ext = rel.extension().unwrap_or_default().to_string_lossy();
        let suffix = if reverse { "warped" } else { "dewarped" };
        let out_name = format!("{stem}_{suffix}.{ext}");
        let out_path = output.join(rel.parent().unwrap_or(Path::new(""))).join(&out_name);
        if let Some(parent) = out_path.parent() {
            std::fs::create_dir_all(parent).ok();
        }

        save_jpeg(&dewarped_data, w, h, &out_path, quality, img_path, reverse);

        // Send a thumbnail preview every 10th image or the first one
        if cur == 1 || cur % 10 == 0 {
            let (orig_t, tw, th) = remap::make_thumbnail(src_data, w, h, 300);
            let (dewarp_t, _, _) = remap::make_thumbnail(&dewarped_data, w, h, 300);
            send(ProgressMsg::Preview(PreviewData {
                original_thumb: orig_t,
                dewarped_thumb: dewarp_t,
                thumb_w: tw,
                thumb_h: th,
            }));
        }
    });

    let elapsed = t0.elapsed().as_secs_f64();
    let skipped_count = skipped.load(std::sync::atomic::Ordering::Relaxed);

    send(ProgressMsg::Done(BatchSummary {
        total,
        processed: total - skipped_count,
        skipped: skipped_count,
        elapsed_secs: elapsed,
    }));
}

fn load_rgb_image(path: &Path) -> Option<RgbImage> {
    ImageReader::open(path)
        .ok()?
        .decode()
        .ok()
        .map(|img| img.into_rgb8())
}

/// Extract metadata segments (EXIF, XMP, ICC profile, comments) from a source JPEG.
/// Returns raw segment bytes including markers and length headers.
/// Skips APP0 (JFIF) since the encoder provides its own.
fn extract_jpeg_metadata(source: &Path) -> Vec<Vec<u8>> {
    let data = match std::fs::read(source) {
        Ok(d) => d,
        Err(_) => return Vec::new(),
    };
    if data.len() < 4 || data[0] != 0xFF || data[1] != 0xD8 {
        return Vec::new();
    }

    let mut segments = Vec::new();
    let mut pos = 2;

    while pos + 3 < data.len() {
        if data[pos] != 0xFF {
            break;
        }

        let marker = data[pos + 1];

        if marker == 0xDA || marker == 0xD9 {
            break;
        }

        if marker == 0x00 || (0xD0..=0xD7).contains(&marker) || marker == 0xFF {
            pos += 1;
            continue;
        }

        if pos + 3 >= data.len() {
            break;
        }
        let seg_len = ((data[pos + 2] as usize) << 8) | (data[pos + 3] as usize);
        let total_len = 2 + seg_len;

        if pos + total_len > data.len() {
            break;
        }

        // Keep APP1-APP15 (EXIF, XMP, ICC, etc.) and COM; skip APP0 (JFIF)
        if (marker >= 0xE1 && marker <= 0xEF) || marker == 0xFE {
            segments.push(data[pos..pos + total_len].to_vec());
        }

        pos += total_len;
    }

    segments
}

/// Patch the DewarpFlag value inside the XMP APP1 metadata segment.
/// Searches for `DewarpFlag="X"` and replaces X with the new value.
/// Only works when old and new values have the same byte length.
fn patch_dewarp_flag(segments: &mut [Vec<u8>], new_value: &[u8]) {
    let needle = b"DewarpFlag=\"";
    for seg in segments.iter_mut() {
        if let Some(pos) = seg.windows(needle.len()).position(|w| w == needle) {
            let val_start = pos + needle.len();
            if let Some(val_len) = seg[val_start..].iter().position(|&b| b == b'"') {
                if val_len == new_value.len() {
                    seg[val_start..val_start + val_len].copy_from_slice(new_value);
                } else if new_value.len() < val_len {
                    // Pad with spaces if new value is shorter (keeps segment length valid)
                    for i in 0..val_len {
                        seg[val_start + i] = if i < new_value.len() {
                            new_value[i]
                        } else {
                            b' '
                        };
                    }
                }
            }
        }
    }
}

fn save_jpeg(data: &[u8], width: u32, height: u32, path: &Path, quality: u8, source: &Path, reverse: bool) {
    let mut jpeg_buf = Vec::new();
    {
        let mut encoder = JpegEncoder::new_with_quality(std::io::Cursor::new(&mut jpeg_buf), quality);
        if encoder
            .encode(data, width, height, image::ExtendedColorType::Rgb8)
            .is_err()
        {
            return;
        }
    }

    let mut metadata = extract_jpeg_metadata(source);
    if metadata.is_empty() {
        let _ = std::fs::write(path, &jpeg_buf);
        return;
    }

    if reverse {
        patch_dewarp_flag(&mut metadata, b"0");
    } else {
        patch_dewarp_flag(&mut metadata, b"1");
    }

    let meta_size: usize = metadata.iter().map(|s| s.len()).sum();
    let mut output = Vec::with_capacity(jpeg_buf.len() + meta_size);

    // SOI
    output.extend_from_slice(&[0xFF, 0xD8]);

    // Keep the encoder's APP0/JFIF if present
    let mut skip = 2;
    if jpeg_buf.len() > 5 && jpeg_buf[2] == 0xFF && jpeg_buf[3] == 0xE0 {
        let seg_len = ((jpeg_buf[4] as usize) << 8) | (jpeg_buf[5] as usize);
        let end = 2 + 2 + seg_len;
        if end <= jpeg_buf.len() {
            output.extend_from_slice(&jpeg_buf[2..end]);
            skip = end;
        }
    }

    // Inject original metadata segments (EXIF with GPS, XMP, ICC, etc.)
    for seg in &metadata {
        output.extend_from_slice(seg);
    }

    // Append remaining encoder data (quantization tables, Huffman tables, image data)
    output.extend_from_slice(&jpeg_buf[skip..]);

    let _ = std::fs::write(path, &output);
}
