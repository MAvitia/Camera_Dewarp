use crate::calibration::{self, DjiCalibration};
use crate::remap;
use crossbeam_channel::Sender;
use image::codecs::jpeg::JpegEncoder;
use image::{ImageReader, RgbImage};
use rayon::prelude::*;
use std::io::BufWriter;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

const SUPPORTED_EXTS: &[&str] = &["jpg", "jpeg", "png", "tif", "tiff", "bmp", "webp"];

pub struct BatchSettings {
    pub quality: u8,
    pub alpha: f64,
    pub recursive: bool,
    pub use_gpu: bool,
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

    let cal = match calibration::read_dji_calibration(&images[0]) {
        Some(c) => c,
        None => {
            send(ProgressMsg::Error(format!(
                "No DJI DewarpData in {}",
                images[0].file_name().unwrap_or_default().to_string_lossy()
            )));
            return;
        }
    };

    if cal.dewarp_flag != 0 {
        send(ProgressMsg::Error(format!(
            "Images already dewarped (DewarpFlag={})",
            cal.dewarp_flag
        )));
        return;
    }

    send(ProgressMsg::CalibrationLoaded(cal.clone()));

    // Read first image to get dimensions
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

    let lut = Arc::new(remap::build_undistort_lut(&cal, w, h, settings.alpha));

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
        let out_name = format!("{stem}_dewarped.{ext}");
        let out_path = output.join(rel.parent().unwrap_or(Path::new(""))).join(&out_name);
        if let Some(parent) = out_path.parent() {
            std::fs::create_dir_all(parent).ok();
        }

        save_jpeg(&dewarped_data, w, h, &out_path, quality);

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

fn save_jpeg(data: &[u8], width: u32, height: u32, path: &Path, quality: u8) {
    let file = match std::fs::File::create(path) {
        Ok(f) => f,
        Err(_) => return,
    };
    let writer = BufWriter::new(file);
    let mut encoder = JpegEncoder::new_with_quality(writer, quality);
    let _ = encoder.encode(data, width, height, image::ExtendedColorType::Rgb8);
}
