# Camera Dewarp

A fast, GPU-accelerated lens distortion correction tool for DJI drone images. Built in Rust with a native GUI.

Camera Dewarp reads the factory lens calibration embedded in DJI image XMP metadata (`DewarpData`) and applies geometric undistortion to remove barrel distortion — preparing images for photogrammetry, mapping, and surveying workflows.

## Features

- **Automatic calibration** — reads DJI's factory `DewarpData` directly from image XMP metadata, no manual calibration needed
- **Fast parallel processing** — uses all CPU cores via [rayon](https://github.com/rayon-rs/rayon) work-stealing parallelism
- **GPU acceleration** — optional [wgpu](https://github.com/gfx-rs/wgpu) compute shader for the remap step (Vulkan/DX12)
- **Native GUI** — [egui](https://github.com/emilk/egui) immediate-mode interface with live before/after preview during batch processing
- **CLI mode** — headless batch processing for scripting and automation
- **Single binary** — no Python, no dependencies, just one portable `.exe`

## Screenshot

```
+-----------------------------------------------------+
|  File  Help                                          |
+-----------------------------------------------------+
|  Input Folder:  [________________________] [Browse]  |
|  Output Folder: [________________________] [Browse]  |
+-----------------------------------------------------+
|  Settings                                            |
|  [x] Dewarp  [x] Crop black edges  [ ] GPU  Quality |
+-----------------------------------------------------+
|  Preview  (Original | Dewarped)                      |
|  +-------------------+  +-------------------+        |
|  |                   |  |                   |        |
|  +-------------------+  +-------------------+        |
+-----------------------------------------------------+
|  [============-------] 1234 / 3556   21.1 img/s      |
+-----------------------------------------------------+
```

## Installation

### Pre-built binary

Download the latest release from [Releases](https://github.com/MAvitia/Camera_Dewarp/releases).

### Build from source

Requires [Rust](https://rustup.rs/) 1.80+.

```bash
git clone https://github.com/MAvitia/Camera_Dewarp.git
cd Camera_Dewarp
cargo build --release
```

The binary will be at `target/release/dewarp-rs.exe` (Windows) or `target/release/dewarp-rs` (Linux/macOS).

## Usage

### GUI (default)

```bash
# Launch the graphical interface
dewarp-rs

# Or explicitly
dewarp-rs --gui
```

1. Click **Browse** to select your mission image folder
2. Optionally set an output folder (defaults to `<input>_dewarped`)
3. Adjust settings (crop, quality, GPU toggle)
4. Click **Dewarp Batch**

### CLI

```bash
# Show calibration info from images
dewarp-rs --info -r ./mission_photos

# Batch dewarp a folder
dewarp-rs -r ./mission_photos -o ./dewarped

# Single image
dewarp-rs photo.jpg -o ./out

# With GPU acceleration
dewarp-rs -r ./photos -o ./out --gpu

# Keep all pixels (black borders instead of cropping)
dewarp-rs -r ./photos -o ./out --alpha 1.0
```

### CLI Options

| Flag | Description |
|------|-------------|
| `-o, --output` | Output folder (default: `<input>_dewarped`) |
| `-q, --quality` | JPEG quality 1-100 (default: 95) |
| `-r, --recursive` | Process subfolders |
| `--alpha` | Crop control: 0 = no black edges, 1 = keep all pixels |
| `--gpu` | Use GPU for the remap step |
| `--info` | Print calibration data and exit |
| `--gui` | Launch GUI |

## How It Works

1. **XMP Parsing** — Reads the first 64KB of each JPEG to find DJI's `DewarpData` tag containing factory-calibrated Brown-Conrady distortion coefficients (k1, k2, k3, p1, p2) and camera intrinsics (fx, fy, cx, cy)

2. **LUT Construction** — Builds a remap lookup table once for the entire batch. For each output pixel, the forward distortion model computes where to sample in the distorted input image

3. **Parallel Remap** — Each image is decoded, remapped (bilinear interpolation), and re-encoded in parallel across all CPU cores. The GPU path dispatches the remap as a wgpu compute shader

4. **Crop Optimization** — Implements the equivalent of OpenCV's `getOptimalNewCameraMatrix` to find the maximal valid region, eliminating black borders from the undistortion

## Supported Cameras

Any DJI drone that embeds `DewarpData` in XMP metadata, including:

- DJI Mini series
- DJI Air series
- DJI Mavic series
- DJI Phantom series
- DJI Matrice series (with Zenmuse cameras)

Images must have `DewarpFlag: 0` (not already dewarped by DJI's processing).

## Performance

Benchmarked on AMD Ryzen 9 7950X (16C/32T) with 3,940 images (5280x3956 JPEG):

| Mode | Speed | Total Time |
|------|-------|------------|
| Python (single-thread) | 2.3 img/s | ~28 min |
| **Rust (rayon, 32 threads)** | **21 img/s** | **3 min 7 sec** |

## Project Structure

```
src/
  main.rs          — CLI (clap) + GUI launcher
  calibration.rs   — XMP byte scan, DewarpData parsing
  remap.rs         — Undistort LUT, bilinear remap with rayon
  gpu.rs           — wgpu compute shader pipeline
  pipeline.rs      — Batch processor, parallel workers, progress channel
  gui.rs           — egui application
shaders/
  remap.wgsl       — GPU remap compute shader
```

## License

This project is licensed under the [MIT License](LICENSE).

## Author

**Manuel Avitia** — [github.com/MAvitia](https://github.com/MAvitia)

## Acknowledgments

- [egui](https://github.com/emilk/egui) — Immediate-mode GUI library by Emil Ernerfeldt (MIT)
- [rayon](https://github.com/rayon-rs/rayon) — Data parallelism library (MIT/Apache-2.0)
- [wgpu](https://github.com/gfx-rs/wgpu) — Cross-platform GPU compute/graphics (MIT/Apache-2.0)
- [image](https://github.com/image-rs/image) — Image encoding/decoding (MIT/Apache-2.0)
- [clap](https://github.com/clap-rs/clap) — Command-line argument parsing (MIT/Apache-2.0)
- [rfd](https://github.com/PolyMeilex/rfd) — Native file dialogs (MIT)
- [crossbeam](https://github.com/crossbeam-rs/crossbeam) — Concurrent programming tools (MIT/Apache-2.0)
