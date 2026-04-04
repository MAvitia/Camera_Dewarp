mod calibration;
mod gpu;
mod gui;
mod pipeline;
mod remap;

use clap::Parser;
use std::path::PathBuf;

#[derive(Parser)]
#[command(name = "dewarp-rs", about = "DJI drone image dewarp tool with GPU acceleration")]
struct Cli {
    /// Input image or folder
    input: Option<PathBuf>,

    /// Output folder
    #[arg(short, long)]
    output: Option<PathBuf>,

    /// JPEG quality 1-100
    #[arg(short, long, default_value_t = 95)]
    quality: u8,

    /// Process subfolders recursively
    #[arg(short, long)]
    recursive: bool,

    /// Crop control: 0 = no black edges, 1 = keep all pixels
    #[arg(long, default_value_t = 0.0)]
    alpha: f64,

    /// Print calibration info and exit
    #[arg(long)]
    info: bool,

    /// Launch GUI (default if no input given)
    #[arg(long)]
    gui: bool,

    /// Use GPU for remap
    #[arg(long)]
    gpu: bool,

    /// Path to external calibration file (.cal)
    #[arg(long)]
    cal: Option<PathBuf>,
}

fn main() {
    env_logger::init();
    let cli = Cli::parse();

    if cli.gui || cli.input.is_none() {
        gui::run_gui();
        return;
    }

    let input = cli.input.unwrap();
    if !input.exists() {
        eprintln!("Error: {} does not exist", input.display());
        std::process::exit(1);
    }

    if cli.info {
        let images = pipeline::collect_images(&input, cli.recursive);
        if images.is_empty() {
            eprintln!("No images found.");
            std::process::exit(1);
        }
        match calibration::read_dji_calibration(&images[0]) {
            Some(cal) => {
                println!("Image:       {}", images[0].file_name().unwrap().to_string_lossy());
                println!(
                    "DewarpFlag:  {}  ({})",
                    cal.dewarp_flag,
                    if cal.dewarp_flag == 0 { "needs dewarping" } else { "already dewarped" }
                );
                println!("Calibration: {}", cal.calibration_date);
                println!("  fx = {:.6}", cal.fx);
                println!("  fy = {:.6}", cal.fy);
                println!("  cx = {:.6}", cal.cx);
                println!("  cy = {:.6}", cal.cy);
                println!("  k1 = {:.12}", cal.k1);
                println!("  k2 = {:.12}", cal.k2);
                println!("  k3 = {:.12}", cal.k3);
                println!("  p1 = {:.12}", cal.p1);
                println!("  p2 = {:.12}", cal.p2);
            }
            None => {
                eprintln!("No DJI DewarpData found.");
                std::process::exit(1);
            }
        }
        return;
    }

    let output = cli.output.unwrap_or_else(|| {
        if input.is_file() {
            input.parent().unwrap().join("dewarped")
        } else {
            let name = input.file_name().unwrap().to_string_lossy();
            input.parent().unwrap().join(format!("{name}_dewarped"))
        }
    });

    println!("DJI Dewarp Tool (Rust)\n");

    let external_cal = cli.cal.as_ref().and_then(|p| {
        let cal = calibration::DjiCalibration::load_from_file(p);
        if cal.is_none() {
            eprintln!("Error: failed to parse calibration file {}", p.display());
            std::process::exit(1);
        }
        println!("  Loaded external calibration from {}\n", p.display());
        cal
    });

    let settings = pipeline::BatchSettings {
        quality: cli.quality,
        alpha: cli.alpha,
        recursive: cli.recursive,
        use_gpu: cli.gpu,
        reverse: None,
        external_cal,
    };

    let (tx_progress, rx_progress) = crossbeam_channel::unbounded();

    let handle = std::thread::spawn({
        let input = input.clone();
        let output = output.clone();
        move || pipeline::batch_dewarp(&input, &output, &settings, Some(tx_progress))
    });

    for msg in &rx_progress {
        match msg {
            pipeline::ProgressMsg::CalibrationLoaded(cal) => {
                if cal.dewarp_flag != 0 {
                    println!("  DewarpFlag={} -> REVERSE mode (re-applying barrel distortion)\n", cal.dewarp_flag);
                }
            }
            pipeline::ProgressMsg::Processing { current, total, name } => {
                println!("  [{current}/{total}] {name}");
            }
            pipeline::ProgressMsg::Done(summary) => {
                println!(
                    "\nProcessed {}/{} images in {:.1}s ({:.2}s/image)",
                    summary.processed,
                    summary.total,
                    summary.elapsed_secs,
                    summary.elapsed_secs / summary.processed.max(1) as f64
                );
                if summary.skipped > 0 {
                    println!("Skipped: {}", summary.skipped);
                }
                println!("Output: {}", output.display());
            }
            pipeline::ProgressMsg::Error(e) => {
                eprintln!("Error: {e}");
                std::process::exit(1);
            }
            _ => {}
        }
    }

    handle.join().unwrap();
}
