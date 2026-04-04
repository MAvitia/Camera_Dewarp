<div align="center">

# Camera Dewarp

**Fast, GPU-accelerated lens distortion correction for DJI drone images**

[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)
[![Rust](https://img.shields.io/badge/Built_with-Rust-orange.svg)](https://www.rust-lang.org/)

<br>

[<img src="https://img.shields.io/badge/Download-Portable_EXE-green?style=for-the-badge&logo=windows" alt="Download">](https://github.com/MAvitia/Camera_Dewarp/releases/latest)

</div>

---

> **Warning — Use at your own risk.**
> This tool relies on lens calibration data to geometrically transform images. If the calibration parameters are incorrect, missing, or applied to images from a different lens/sensor, the output will be **geometrically inaccurate**. Always verify results against known straight-line references before using dewarped images in photogrammetry, surveying, or measurement workflows. The author assumes no liability for inaccurate outputs.

---

<br>

## Before & After

<table>
<tr>
<td align="center"><strong>Original (barrel distortion)</strong></td>
<td align="center"><strong>Dewarped</strong></td>
</tr>
<tr>
<td><img src="samples/before.jpg" alt="Before — barrel distortion visible at edges" width="400"></td>
<td><img src="samples/after.jpg" alt="After — straight lines corrected" width="400"></td>
</tr>
</table>

> Notice the curved edges and bent rooflines in the original. After dewarping, all straight lines are geometrically correct — critical for photogrammetry and mapping accuracy.

<br>

## What It Does

Camera Dewarp reads the **factory lens calibration** embedded in every DJI drone image's XMP metadata (`DewarpData`) and applies geometric undistortion to remove barrel distortion. No manual calibration, no checkerboard patterns — just point it at your mission folder and go.

It also supports **reverse mode**: if images were already dewarped (by DJI on-device processing or a previous run), Camera Dewarp can re-apply barrel distortion to restore the original camera perspective.

<br>

## Features

<table>
<tr>
<td width="50%">

- **Zero-config** — calibration is read automatically from each image's XMP metadata
- **Batch processing** — process entire mission folders with thousands of images
- **Native GUI** — folder pickers, live before/after preview, progress bar
- **Metadata preserved** — EXIF, GPS geotags, XMP, and ICC profiles are retained in output

</td>
<td width="50%">

- **Reverse mode** — re-apply barrel distortion to already-dewarped images
- **Calibration files** — export/import `.cal` files for images without embedded data
- **21 images/sec** — parallel processing across all CPU cores via rayon
- **Single portable binary** — no Python, no runtime, no installer

</td>
</tr>
</table>

<br>

## Quick Start

### GUI

Download [`camera-dewarp.exe`](https://github.com/MAvitia/Camera_Dewarp/releases/latest) and double-click — no installation needed.

1. Click **Browse** to select your mission image folder
2. Set an output folder (or leave blank for auto)
3. Click **Dewarp Batch** (or **Re-distort Batch** if reverse mode is detected)

#### Exporting & Loading Calibration Files

If your images were dewarped on-device by DJI and no longer contain embedded calibration data:

1. Take a **raw capture** from the same drone with dewarp disabled — this image will contain the factory `DewarpData`
2. Open the raw image folder in Camera Dewarp
3. Go to **File → Export Calibration...** and save the `.cal` file
4. Open the folder with the on-device dewarped images (the tool will warn "no embedded calibration")
5. Go to **File → Load Calibration...** and select the `.cal` file
6. The tool switches to **Reverse mode** automatically — click **Re-distort Batch**

### CLI

```bash
# Batch dewarp a mission folder
camera-dewarp -r ./mission_photos -o ./dewarped

# Show embedded calibration data
camera-dewarp --info -r ./mission_photos

# Single image
camera-dewarp photo.jpg -o ./out

# Use an external calibration file (auto-detects reverse mode)
camera-dewarp ./dewarped_photos -o ./rewarped --cal lens_calibration.cal

# GPU-accelerated
camera-dewarp -r ./photos -o ./out --gpu
```

<br>

## CLI Reference

| Flag | Description |
|------|-------------|
| `-o, --output` | Output folder (default: `<input>_dewarped` or `<input>_warped`) |
| `-q, --quality` | JPEG quality 1–100 (default: 95) |
| `-r, --recursive` | Process subfolders |
| `--alpha` | Crop control: `0` = no black edges, `1` = keep all pixels |
| `--gpu` | Use GPU compute shader for remap |
| `--cal <file>` | Load external calibration file (`.cal`) |
| `--info` | Print calibration info and exit |
| `--gui` | Launch GUI (default if no input given) |

<br>

## How It Works

1. **XMP Parsing** — reads the first 64 KB of each JPEG to extract DJI's `DewarpData`: Brown-Conrady distortion coefficients (k1, k2, k3, p1, p2) and camera intrinsics (fx, fy, cx, cy)

2. **LUT Construction** — builds a remap lookup table once per batch; for each output pixel, the forward distortion model computes where to sample in the distorted source

3. **Parallel Remap** — images are decoded, remapped with bilinear interpolation, and re-encoded in parallel across all CPU cores; the GPU path dispatches the remap as a wgpu compute shader

4. **Crop Optimization** — equivalent of OpenCV's `getOptimalNewCameraMatrix` finds the maximal valid region, eliminating black borders

5. **Metadata Preservation** — EXIF (GPS, camera info), XMP (DJI flight data), and ICC color profiles are extracted from source images and spliced into outputs; the `DewarpFlag` is updated to reflect the processing state

<br>

## Reverse Mode

Camera Dewarp auto-detects when images have already been dewarped:

| Condition | Mode | Output suffix |
|-----------|------|---------------|
| `DewarpFlag = 0` (raw image) | **Dewarp** — removes barrel distortion | `_dewarped` |
| `DewarpFlag != 0` (already dewarped) | **Reverse** — re-applies barrel distortion | `_warped` |
| No embedded data + external `.cal` loaded | **Reverse** — assumes on-device dewarped | `_warped` |

The `DewarpFlag` in the output metadata is updated accordingly (`0` → `1` after dewarp, `1` → `0` after reverse), enabling clean round-trip processing.

<br>

## Calibration File Format

The `.cal` file is a plain-text key-value format:

```
# Camera Dewarp — Lens Calibration File
calibration_date=2025-10-29
fx=3707.311686000000
fy=3707.311686000000
cx=2660.730558000000
cy=1943.102160000000
k1=-0.107120805743000
k2=-0.001098361050000
k3=-0.014380999076000
p1=0.000310881741000
p2=-0.000553323386000
```

> **Warning:** Only use calibration files extracted from the **same drone and lens** as the target images. Applying calibration from a different device will produce geometrically inaccurate results. Calibration parameters are unique to each camera/lens assembly.

<br>

## Supported Cameras

Any DJI drone that embeds `DewarpData` in XMP metadata:

| Series | Examples |
|--------|----------|
| Mini | Mini 2, Mini 3, Mini 3 Pro, Mini 4 Pro |
| Air | Air 2, Air 2S, Air 3 |
| Mavic | Mavic 3, Mavic 3 Pro, Mavic 3 Enterprise |
| Matrice | M300 RTK, M350 RTK (with Zenmuse cameras) |
| Phantom | Phantom 4 RTK, Phantom 4 Pro V2 |

For forward dewarp, images must have `DewarpFlag: 0`. For reverse mode, images can have any `DewarpFlag` value or use an external calibration file.

<br>

## Performance

Benchmarked on AMD Ryzen 9 7950X (16C/32T), 3,940 images at 5280×3956 px:

| | Speed | 3,940 images |
|---|---|---|
| Python + OpenCV (1 thread) | 2.3 img/s | ~28 min |
| **Camera Dewarp (Rust, 32 threads)** | **21 img/s** | **3 min 7 sec** |

<br>

## Build from Source

Requires [Rust](https://rustup.rs/) 1.80+.

```bash
git clone https://github.com/MAvitia/Camera_Dewarp.git
cd Camera_Dewarp
cargo build --release
```

Binary output: `target/release/camera-dewarp.exe`

<br>

## Project Structure

```
src/
  main.rs          — CLI (clap) + GUI launcher
  calibration.rs   — XMP byte scan, DewarpData parsing, .cal file I/O
  remap.rs         — Undistort/redistort LUT, bilinear remap with rayon
  gpu.rs           — wgpu compute shader pipeline
  pipeline.rs      — Batch processor, parallel workers, progress channel
  gui.rs           — egui application with calibration import/export
shaders/
  remap.wgsl       — GPU remap compute shader (WGSL)
samples/
  before.jpg       — Sample: original with barrel distortion
  after.jpg        — Sample: corrected output
```

<br>

## License

This project is licensed under the [MIT License](LICENSE).

<br>

## Author

**Manuel Avitia** — [@MAvitia](https://github.com/MAvitia)

<br>

## Acknowledgments

| Crate | Purpose | License |
|-------|---------|---------|
| [egui](https://github.com/emilk/egui) | Immediate-mode GUI | MIT |
| [rayon](https://github.com/rayon-rs/rayon) | Data parallelism | MIT / Apache-2.0 |
| [wgpu](https://github.com/gfx-rs/wgpu) | GPU compute (Vulkan/DX12) | MIT / Apache-2.0 |
| [image](https://github.com/image-rs/image) | JPEG/PNG encode/decode | MIT / Apache-2.0 |
| [clap](https://github.com/clap-rs/clap) | CLI argument parsing | MIT / Apache-2.0 |
| [rfd](https://github.com/PolyMeilex/rfd) | Native file dialogs | MIT |
| [crossbeam](https://github.com/crossbeam-rs/crossbeam) | Concurrent channels | MIT / Apache-2.0 |
