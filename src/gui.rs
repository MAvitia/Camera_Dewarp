use crate::calibration::{self, DjiCalibration};
use crate::pipeline::{self, BatchSettings, PreviewData, ProgressMsg};
use crate::remap;
use crossbeam_channel::{Receiver, Sender};
use eframe::egui;

const VERSION: &str = env!("CARGO_PKG_VERSION");

struct DewarpGui {
    input_folder: String,
    output_folder: String,
    quality: u8,
    crop_edges: bool,
    use_gpu: bool,
    recursive: bool,
    dewarp_enabled: bool,
    reverse_mode: bool,

    calibration: Option<DjiCalibration>,
    cal_from_file: bool,
    image_count: usize,
    needs_cal_file: bool,

    preview_original: Option<egui::TextureHandle>,
    preview_dewarped: Option<egui::TextureHandle>,

    processing: bool,
    progress_current: usize,
    progress_total: usize,
    progress_name: String,
    status_msg: String,
    images_per_sec: f64,
    process_start: Option<std::time::Instant>,

    progress_rx: Option<Receiver<ProgressMsg>>,

    show_about: bool,
    show_calibration: bool,
}

impl Default for DewarpGui {
    fn default() -> Self {
        Self {
            input_folder: String::new(),
            output_folder: String::new(),
            quality: 95,
            crop_edges: true,
            use_gpu: false,
            recursive: false,
            dewarp_enabled: true,
            reverse_mode: false,
            calibration: None,
            cal_from_file: false,
            image_count: 0,
            needs_cal_file: false,
            preview_original: None,
            preview_dewarped: None,
            processing: false,
            progress_current: 0,
            progress_total: 0,
            progress_name: String::new(),
            status_msg: "Ready".into(),
            images_per_sec: 0.0,
            process_start: None,
            progress_rx: None,
            show_about: false,
            show_calibration: false,
        }
    }
}

impl DewarpGui {
    fn load_input_folder(&mut self, ctx: &egui::Context) {
        if self.input_folder.is_empty() {
            return;
        }

        let path = std::path::Path::new(&self.input_folder);
        let images = pipeline::collect_images(path, self.recursive);
        self.image_count = images.len();

        if images.is_empty() {
            self.status_msg = "No supported images in folder.".into();
            self.calibration = None;
            return;
        }

        let embedded = calibration::read_dji_calibration(&images[0]);

        if let Some(cal) = embedded {
            self.calibration = Some(cal);
            self.cal_from_file = false;
            self.needs_cal_file = false;
        } else if self.cal_from_file && self.calibration.is_some() {
            // Keep the previously loaded calibration file
            self.needs_cal_file = false;
        } else {
            self.calibration = None;
            self.cal_from_file = false;
            self.needs_cal_file = true;
            self.reverse_mode = true;
            self.status_msg = format!(
                "{} images — no embedded calibration. Load a calibration file to proceed.",
                self.image_count
            );
            return;
        }

        self.reverse_mode = self.calibration.as_ref().is_some_and(|c| c.dewarp_flag != 0);

        let mode_label = if self.reverse_mode { "REVERSE" } else { "Dewarp" };
        self.status_msg = format!("{} images found  |  Mode: {mode_label}", self.image_count);
        self.generate_preview(&images[0], ctx);
    }

    fn generate_preview(&mut self, image_path: &std::path::Path, ctx: &egui::Context) {
        let Some(cal) = &self.calibration else { return };

        let Ok(reader) = image::ImageReader::open(image_path) else {
            return;
        };
        let Ok(img) = reader.decode() else { return };
        let rgb = img.into_rgb8();
        let (w, h) = (rgb.width(), rgb.height());

        let (orig_thumb, tw, th) = remap::make_thumbnail(rgb.as_raw(), w, h, 400);

        let lut = if self.reverse_mode {
            remap::build_redistort_lut(cal, tw, th)
        } else {
            let alpha = if self.crop_edges { 0.0 } else { 1.0 };
            remap::build_undistort_lut(cal, tw, th, alpha)
        };
        let dewarped_thumb = remap::cpu_remap(&orig_thumb, tw, th, &lut);

        self.preview_original = Some(ctx.load_texture(
            "preview-orig",
            egui::ColorImage::from_rgb([tw as usize, th as usize], &orig_thumb),
            egui::TextureOptions::LINEAR,
        ));
        self.preview_dewarped = Some(ctx.load_texture(
            "preview-dewarp",
            egui::ColorImage::from_rgb([tw as usize, th as usize], &dewarped_thumb),
            egui::TextureOptions::LINEAR,
        ));
    }

    fn start_processing(&mut self) {
        if self.input_folder.is_empty() || self.processing {
            return;
        }

        let input = std::path::PathBuf::from(&self.input_folder);
        let suffix = if self.reverse_mode { "warped" } else { "dewarped" };
        let output = if self.output_folder.is_empty() {
            let name = input.file_name().unwrap_or_default().to_string_lossy();
            input
                .parent()
                .unwrap_or(&input)
                .join(format!("{name}_{suffix}"))
        } else {
            std::path::PathBuf::from(&self.output_folder)
        };

        if self.output_folder.is_empty() {
            self.output_folder = output.to_string_lossy().to_string();
        }

        let reverse = self.reverse_mode;
        let ext_cal = if self.cal_from_file {
            self.calibration.clone()
        } else {
            None
        };
        let settings = BatchSettings {
            quality: self.quality,
            alpha: if self.crop_edges { 0.0 } else { 1.0 },
            recursive: self.recursive,
            use_gpu: self.use_gpu,
            reverse: Some(reverse),
            external_cal: ext_cal,
        };

        let (tx, rx): (Sender<ProgressMsg>, Receiver<ProgressMsg>) =
            crossbeam_channel::unbounded();

        self.progress_rx = Some(rx);
        self.processing = true;
        self.progress_current = 0;
        self.progress_total = 0;
        self.process_start = Some(std::time::Instant::now());

        std::thread::spawn(move || {
            pipeline::batch_dewarp(&input, &output, &settings, Some(tx));
        });
    }

    fn poll_progress(&mut self, ctx: &egui::Context) {
        let rx = match &self.progress_rx {
            Some(rx) => rx.clone(),
            None => return,
        };

        let mut finished = false;

        while let Ok(msg) = rx.try_recv() {
            match msg {
                ProgressMsg::CalibrationLoaded(cal) => {
                    self.calibration = Some(cal);
                }
                ProgressMsg::Processing {
                    current,
                    total,
                    name,
                } => {
                    self.progress_current = current;
                    self.progress_total = total;
                    self.progress_name = name;
                    if let Some(start) = self.process_start {
                        let elapsed = start.elapsed().as_secs_f64();
                        if elapsed > 0.5 {
                            self.images_per_sec = current as f64 / elapsed;
                        }
                    }
                }
                ProgressMsg::Preview(preview) => {
                    self.update_preview_textures(&preview, ctx);
                }
                ProgressMsg::Done(summary) => {
                    self.processing = false;
                    self.status_msg = format!(
                        "Done! {}/{} images in {:.1}s ({:.1} img/s)",
                        summary.processed,
                        summary.total,
                        summary.elapsed_secs,
                        summary.processed as f64 / summary.elapsed_secs.max(0.001)
                    );
                    finished = true;
                }
                ProgressMsg::Error(e) => {
                    self.processing = false;
                    self.status_msg = format!("Error: {e}");
                    finished = true;
                }
            }
        }

        if finished {
            self.progress_rx = None;
        }

        if self.processing {
            ctx.request_repaint();
        }
    }

    fn update_preview_textures(&mut self, preview: &PreviewData, ctx: &egui::Context) {
        let size = [preview.thumb_w as usize, preview.thumb_h as usize];
        self.preview_original = Some(ctx.load_texture(
            "preview-orig",
            egui::ColorImage::from_rgb(size, &preview.original_thumb),
            egui::TextureOptions::LINEAR,
        ));
        self.preview_dewarped = Some(ctx.load_texture(
            "preview-dewarp",
            egui::ColorImage::from_rgb(size, &preview.dewarped_thumb),
            egui::TextureOptions::LINEAR,
        ));
    }

    fn load_calibration_file(&mut self, ctx: &egui::Context) {
        let file = rfd::FileDialog::new()
            .add_filter("Calibration files", &["cal", "txt"])
            .add_filter("All files", &["*"])
            .set_title("Load Calibration File")
            .pick_file();

        if let Some(path) = file {
            match DjiCalibration::load_from_file(&path) {
                Some(mut cal) => {
                    cal.dewarp_flag = 0;
                    self.calibration = Some(cal);
                    self.cal_from_file = true;
                    self.needs_cal_file = false;
                    self.reverse_mode = true;
                    self.status_msg = format!(
                        "{} images  |  Calibration loaded from file  |  Mode: REVERSE",
                        self.image_count
                    );
                    let path = std::path::Path::new(&self.input_folder);
                    let images = pipeline::collect_images(path, self.recursive);
                    if !images.is_empty() {
                        self.generate_preview(&images[0], ctx);
                    }
                }
                None => {
                    self.status_msg = "Failed to parse calibration file.".into();
                }
            }
        }
    }

    fn export_calibration_file(&self) {
        let Some(cal) = &self.calibration else { return };

        let file = rfd::FileDialog::new()
            .add_filter("Calibration files", &["cal"])
            .set_title("Export Calibration File")
            .set_file_name("lens_calibration.cal")
            .save_file();

        if let Some(path) = file {
            if let Err(e) = cal.save_to_file(&path) {
                log::error!("Failed to save calibration: {e}");
            }
        }
    }

    fn show_about_window(&mut self, ctx: &egui::Context) {
        egui::Window::new("About Camera Dewarp")
            .open(&mut self.show_about)
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                ui.vertical_centered(|ui| {
                    ui.add_space(8.0);
                    ui.heading("Camera Dewarp");
                    ui.label(format!("Version {VERSION}"));
                    ui.add_space(12.0);
                    ui.label("DJI drone image lens distortion correction tool.");
                    ui.label("Reads factory calibration from XMP metadata");
                    ui.label("and applies geometric undistortion at speed.");
                    ui.add_space(12.0);
                    ui.separator();
                    ui.add_space(8.0);
                    ui.label(egui::RichText::new("Created by Manuel Avitia").strong());
                    ui.add_space(8.0);
                    ui.hyperlink_to(
                        "github.com/MAvitia/Camera_Dewarp",
                        "https://github.com/MAvitia/Camera_Dewarp",
                    );
                    ui.add_space(8.0);
                    ui.separator();
                    ui.add_space(4.0);
                    ui.label(
                        egui::RichText::new("Licensed under the MIT License")
                            .small()
                            .weak(),
                    );
                    ui.add_space(4.0);
                });
            });
    }
}

impl eframe::App for DewarpGui {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.poll_progress(&ctx);

        // Menu bar
        egui::menu::bar(ui, |ui| {
            ui.menu_button("File", |ui| {
                if ui.button("Open Folder...").clicked() {
                    if let Some(path) = rfd::FileDialog::new().pick_folder() {
                        self.input_folder = path.to_string_lossy().to_string();
                        self.load_input_folder(&ctx);
                    }
                    ui.close_menu();
                }
                ui.separator();
                if ui.button("Load Calibration...").clicked() {
                    self.load_calibration_file(&ctx);
                    ui.close_menu();
                }
                let has_cal = self.calibration.is_some();
                if ui.add_enabled(has_cal, egui::Button::new("Export Calibration...")).clicked() {
                    self.export_calibration_file();
                    ui.close_menu();
                }
                ui.separator();
                if ui.button("Quit").clicked() {
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                }
            });
            ui.menu_button("Help", |ui| {
                if ui.button("About Camera Dewarp").clicked() {
                    self.show_about = true;
                    ui.close_menu();
                }
            });
        });

        // About window (floating)
        self.show_about_window(&ctx);

        ui.add_space(4.0);

        // Input / Output row
        ui.group(|ui| {
            egui::Grid::new("io-grid")
                .num_columns(3)
                .spacing([8.0, 4.0])
                .show(ui, |ui| {
                    ui.label("Input Folder:");
                    ui.add(
                        egui::TextEdit::singleline(&mut self.input_folder).desired_width(400.0),
                    );
                    if ui.button("Browse...").clicked() {
                        if let Some(path) = rfd::FileDialog::new().pick_folder() {
                            self.input_folder = path.to_string_lossy().to_string();
                            self.load_input_folder(&ctx);
                        }
                    }
                    ui.end_row();

                    ui.label("Output Folder:");
                    ui.add(
                        egui::TextEdit::singleline(&mut self.output_folder).desired_width(400.0),
                    );
                    if ui.button("Browse...").clicked() {
                        if let Some(path) = rfd::FileDialog::new().pick_folder() {
                            self.output_folder = path.to_string_lossy().to_string();
                        }
                    }
                    ui.end_row();
                });
        });

        ui.add_space(2.0);

        // Settings (always visible, compact horizontal layout)
        ui.group(|ui| {
            ui.label(egui::RichText::new("Settings").strong());
            ui.separator();
            ui.horizontal(|ui| {
                ui.checkbox(&mut self.dewarp_enabled, "Dewarp enabled");
                ui.add_space(12.0);
                ui.checkbox(&mut self.crop_edges, "Crop black edges");
                ui.add_space(12.0);
                ui.checkbox(&mut self.use_gpu, "Use GPU");
                ui.add_space(12.0);
                ui.checkbox(&mut self.recursive, "Recursive");
                ui.add_space(20.0);
                ui.label("Quality:");
                ui.add(egui::Slider::new(&mut self.quality, 1..=100).fixed_decimals(0));
            });
        });

        ui.add_space(2.0);

        // Preview area (takes remaining space)
        ui.group(|ui| {
            let preview_label = if self.reverse_mode {
                "Preview  (Dewarped | Re-distorted)"
            } else {
                "Preview  (Original | Dewarped)"
            };
            ui.label(egui::RichText::new(preview_label).strong());
            ui.separator();

            let avail = ui.available_size();
            let preview_h = (avail.y - 100.0).max(80.0);

            ui.horizontal(|ui| {
                let half_w = (avail.x - 20.0) / 2.0;

                if let Some(tex) = &self.preview_original {
                    let size = scale_to_fit(tex.size_vec2(), half_w, preview_h);
                    ui.image(egui::load::SizedTexture::new(tex.id(), size));
                } else {
                    ui.allocate_space(egui::vec2(half_w, preview_h));
                }

                ui.separator();

                if let Some(tex) = &self.preview_dewarped {
                    let size = scale_to_fit(tex.size_vec2(), half_w, preview_h);
                    ui.image(egui::load::SizedTexture::new(tex.id(), size));
                } else {
                    ui.allocate_space(egui::vec2(half_w, preview_h));
                }
            });
        });

        ui.add_space(2.0);

        // Lens calibration — collapsible, below preview
        let cal_header = if let Some(cal) = &self.calibration {
            let source = if self.cal_from_file { "file" } else { "XMP" };
            format!(
                "Lens Calibration   (fx={:.1}  k1={:.4}  Flag: {}  src: {source})",
                cal.fx, cal.k1, cal.dewarp_flag
            )
        } else if self.needs_cal_file {
            "Lens Calibration  (not found — load file)".to_string()
        } else {
            "Lens Calibration  (no image loaded)".to_string()
        };
        let cal_open = self.show_calibration || self.needs_cal_file;
        egui::CollapsingHeader::new(egui::RichText::new(cal_header).strong())
            .default_open(cal_open)
            .show(ui, |ui| {
                if let Some(cal) = &self.calibration {
                    ui.horizontal(|ui| {
                        ui.monospace(format!(
                            "Cal: {}   fx={:.2}  fy={:.2}   cx={:.2}  cy={:.2}",
                            cal.calibration_date, cal.fx, cal.fy, cal.cx, cal.cy
                        ));
                    });
                    ui.horizontal(|ui| {
                        ui.monospace(format!(
                            "k1={:.8}  k2={:.8}  k3={:.8}  p1={:.8}  p2={:.8}",
                            cal.k1, cal.k2, cal.k3, cal.p1, cal.p2
                        ));
                    });
                } else if self.needs_cal_file {
                    ui.label("No embedded DewarpData. Load a calibration file extracted from a raw capture.");
                    ui.add_space(4.0);
                    if ui.button("Load Calibration File...").clicked() {
                        self.load_calibration_file(&ctx);
                    }
                } else {
                    ui.label("Load a folder with DJI images to see calibration data.");
                }
            });

        ui.add_space(2.0);

        // Bottom bar: progress + button
        ui.horizontal(|ui| {
            if self.processing {
                let fraction = if self.progress_total > 0 {
                    self.progress_current as f32 / self.progress_total as f32
                } else {
                    0.0
                };
                let bar = egui::ProgressBar::new(fraction)
                    .text(format!(
                        "{} / {}   {:.1} img/s   {}",
                        self.progress_current, self.progress_total, self.images_per_sec,
                        self.progress_name,
                    ))
                    .animate(true);
                ui.add_sized([ui.available_width() - 140.0, 24.0], bar);
            } else {
                ui.label(&self.status_msg);
                ui.add_space(ui.available_width() - 140.0);
            }

            let btn_enabled = !self.processing
                && !self.input_folder.is_empty()
                && self.calibration.is_some()
                && self.dewarp_enabled;
            let btn_label = if self.reverse_mode {
                "  Re-distort Batch  "
            } else {
                "  Dewarp Batch  "
            };
            if ui
                .add_enabled(
                    btn_enabled,
                    egui::Button::new(
                        egui::RichText::new(btn_label).strong().size(16.0),
                    ),
                )
                .clicked()
            {
                self.start_processing();
            }
        });
    }
}

fn scale_to_fit(tex_size: egui::Vec2, max_w: f32, max_h: f32) -> egui::Vec2 {
    let scale = (max_w / tex_size.x).min(max_h / tex_size.y).min(1.0);
    tex_size * scale
}

pub fn run_gui() {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([850.0, 650.0])
            .with_min_inner_size([700.0, 500.0])
            .with_title("Camera Dewarp"),
        ..Default::default()
    };

    eframe::run_native(
        "Camera Dewarp",
        options,
        Box::new(|_cc| Ok(Box::new(DewarpGui::default()))),
    )
    .unwrap();
}
