use crate::calibration::DjiCalibration;
use rayon::prelude::*;

pub struct RemapLut {
    pub map_x: Vec<f32>,
    pub map_y: Vec<f32>,
    pub width: u32,
    pub height: u32,
}

/// Build the undistortion remap LUT using the Brown-Conrady forward model.
/// `alpha`: 0 = no black edges (crop), 1 = keep all pixels (black borders).
pub fn build_undistort_lut(cal: &DjiCalibration, width: u32, height: u32, alpha: f64) -> RemapLut {
    let (new_fx, new_fy, new_cx, new_cy) = compute_new_camera_params(cal, width, height, alpha);

    let w = width as usize;
    let h = height as usize;
    let total = w * h;
    let mut map_x = vec![0.0f32; total];
    let mut map_y = vec![0.0f32; total];

    map_x
        .par_chunks_mut(w)
        .zip(map_y.par_chunks_mut(w))
        .enumerate()
        .for_each(|(row, (mx_row, my_row))| {
            for col in 0..w {
                let x = (col as f64 - new_cx) / new_fx;
                let y = (row as f64 - new_cy) / new_fy;

                let r2 = x * x + y * y;
                let r4 = r2 * r2;
                let r6 = r4 * r2;
                let radial = 1.0 + cal.k1 * r2 + cal.k2 * r4 + cal.k3 * r6;
                let x_d = x * radial + 2.0 * cal.p1 * x * y + cal.p2 * (r2 + 2.0 * x * x);
                let y_d = y * radial + cal.p1 * (r2 + 2.0 * y * y) + 2.0 * cal.p2 * x * y;

                mx_row[col] = (x_d * cal.fx + cal.cx) as f32;
                my_row[col] = (y_d * cal.fy + cal.cy) as f32;
            }
        });

    RemapLut {
        map_x,
        map_y,
        width,
        height,
    }
}

/// Build a re-distortion remap LUT: maps each output pixel (in distorted space)
/// back to where it came from in an already-dewarped input image.
/// Used to reverse DJI's on-device dewarping and restore barrel distortion.
pub fn build_redistort_lut(cal: &DjiCalibration, width: u32, height: u32) -> RemapLut {
    let (new_fx, new_fy, new_cx, new_cy) =
        compute_new_camera_params(cal, width, height, 0.0);

    let w = width as usize;
    let h = height as usize;
    let total = w * h;
    let mut map_x = vec![0.0f32; total];
    let mut map_y = vec![0.0f32; total];

    map_x
        .par_chunks_mut(w)
        .zip(map_y.par_chunks_mut(w))
        .enumerate()
        .for_each(|(row, (mx_row, my_row))| {
            for col in 0..w {
                // Output pixel is in distorted space — normalize with original K
                let x_d = (col as f64 - cal.cx) / cal.fx;
                let y_d = (row as f64 - cal.cy) / cal.fy;

                // Invert distortion to get undistorted normalized coords
                let (x_u, y_u) = undistort_point(x_d, y_d, cal);

                // Map to pixel coords in the undistorted (dewarped) input using new K
                mx_row[col] = (x_u * new_fx + new_cx) as f32;
                my_row[col] = (y_u * new_fy + new_cy) as f32;
            }
        });

    RemapLut {
        map_x,
        map_y,
        width,
        height,
    }
}

/// Iteratively invert the Brown-Conrady distortion model.
/// Given distorted normalized coords (x_d, y_d), find undistorted (x, y).
fn undistort_point(x_d: f64, y_d: f64, cal: &DjiCalibration) -> (f64, f64) {
    let mut x = x_d;
    let mut y = y_d;
    for _ in 0..50 {
        let r2 = x * x + y * y;
        let r4 = r2 * r2;
        let r6 = r4 * r2;
        let radial = 1.0 + cal.k1 * r2 + cal.k2 * r4 + cal.k3 * r6;
        let dx = 2.0 * cal.p1 * x * y + cal.p2 * (r2 + 2.0 * x * x);
        let dy = cal.p1 * (r2 + 2.0 * y * y) + 2.0 * cal.p2 * x * y;
        x = (x_d - dx) / radial;
        y = (y_d - dy) / radial;
    }
    (x, y)
}

/// Compute new camera intrinsics for the undistorted output.
/// Mimics OpenCV's getOptimalNewCameraMatrix.
fn compute_new_camera_params(
    cal: &DjiCalibration,
    width: u32,
    height: u32,
    alpha: f64,
) -> (f64, f64, f64, f64) {
    let w = width as f64;
    let h = height as f64;
    let alpha = alpha.clamp(0.0, 1.0);
    let n = 200usize;

    // Sample border points of the INPUT (distorted) image and undistort them
    // to find the valid region in normalized undistorted coordinates.
    let mut top_edge = Vec::with_capacity(n);
    let mut bottom_edge = Vec::with_capacity(n);
    let mut left_edge = Vec::with_capacity(n);
    let mut right_edge = Vec::with_capacity(n);

    for i in 0..n {
        let t = i as f64 / (n - 1) as f64;

        // Top edge: y_d = 0, x_d varies
        let xd = (t * (w - 1.0) - cal.cx) / cal.fx;
        let yd = (0.0 - cal.cy) / cal.fy;
        top_edge.push(undistort_point(xd, yd, cal));

        // Bottom edge: y_d = h-1
        let yd = ((h - 1.0) - cal.cy) / cal.fy;
        bottom_edge.push(undistort_point(xd, yd, cal));

        // Left edge: x_d = 0, y_d varies
        let xd = (0.0 - cal.cx) / cal.fx;
        let yd = (t * (h - 1.0) - cal.cy) / cal.fy;
        left_edge.push(undistort_point(xd, yd, cal));

        // Right edge: x_d = w-1
        let xd = ((w - 1.0) - cal.cx) / cal.fx;
        right_edge.push(undistort_point(xd, yd, cal));
    }

    // Inner rectangle (alpha=0): most restrictive bounds
    let inner_left = left_edge.iter().map(|p| p.0).fold(f64::NEG_INFINITY, f64::max);
    let inner_right = right_edge.iter().map(|p| p.0).fold(f64::INFINITY, f64::min);
    let inner_top = top_edge.iter().map(|p| p.1).fold(f64::NEG_INFINITY, f64::max);
    let inner_bottom = bottom_edge.iter().map(|p| p.1).fold(f64::INFINITY, f64::min);

    // Outer rectangle (alpha=1): widest bounds
    let all_pts: Vec<(f64, f64)> = top_edge
        .iter()
        .chain(bottom_edge.iter())
        .chain(left_edge.iter())
        .chain(right_edge.iter())
        .copied()
        .collect();
    let outer_left = all_pts.iter().map(|p| p.0).fold(f64::INFINITY, f64::min);
    let outer_right = all_pts
        .iter()
        .map(|p| p.0)
        .fold(f64::NEG_INFINITY, f64::max);
    let outer_top = all_pts.iter().map(|p| p.1).fold(f64::INFINITY, f64::min);
    let outer_bottom = all_pts
        .iter()
        .map(|p| p.1)
        .fold(f64::NEG_INFINITY, f64::max);

    // Blend
    let x_left = inner_left + alpha * (outer_left - inner_left);
    let x_right = inner_right + alpha * (outer_right - inner_right);
    let y_top = inner_top + alpha * (outer_top - inner_top);
    let y_bottom = inner_bottom + alpha * (outer_bottom - inner_bottom);

    // new_K maps output pixels [0..w-1, 0..h-1] to normalized undistorted coords [x_left..x_right, y_top..y_bottom]
    let new_fx = (w - 1.0) / (x_right - x_left);
    let new_fy = (h - 1.0) / (y_bottom - y_top);
    let new_cx = -x_left * new_fx;
    let new_cy = -y_top * new_fy;

    (new_fx, new_fy, new_cx, new_cy)
}

/// Apply the remap LUT to an image using bilinear interpolation (CPU, parallelized).
pub fn cpu_remap(src: &[u8], src_width: u32, src_height: u32, lut: &RemapLut) -> Vec<u8> {
    let sw = src_width as usize;
    let sh = src_height as usize;
    let dw = lut.width as usize;
    let dh = lut.height as usize;
    let channels = 3usize;
    let mut dst = vec![0u8; dw * dh * channels];

    dst.par_chunks_mut(dw * channels)
        .enumerate()
        .for_each(|(row, dst_row)| {
            for col in 0..dw {
                let idx = row * dw + col;
                let sx = lut.map_x[idx] as f64;
                let sy = lut.map_y[idx] as f64;

                let x0 = sx.floor() as i64;
                let y0 = sy.floor() as i64;
                let x1 = x0 + 1;
                let y1 = y0 + 1;

                if x0 < 0 || y0 < 0 || x1 >= sw as i64 || y1 >= sh as i64 {
                    continue;
                }

                let fx = sx - x0 as f64;
                let fy = sy - y0 as f64;
                let w00 = (1.0 - fx) * (1.0 - fy);
                let w10 = fx * (1.0 - fy);
                let w01 = (1.0 - fx) * fy;
                let w11 = fx * fy;

                let x0u = x0 as usize;
                let y0u = y0 as usize;
                let x1u = x1 as usize;
                let y1u = y1 as usize;

                for c in 0..channels {
                    let v = w00 * src[(y0u * sw + x0u) * channels + c] as f64
                        + w10 * src[(y0u * sw + x1u) * channels + c] as f64
                        + w01 * src[(y1u * sw + x0u) * channels + c] as f64
                        + w11 * src[(y1u * sw + x1u) * channels + c] as f64;
                    dst_row[col * channels + c] = v.round().clamp(0.0, 255.0) as u8;
                }
            }
        });

    dst
}

/// Nearest-neighbor downscale for thumbnails.
pub fn make_thumbnail(src: &[u8], src_w: u32, src_h: u32, max_dim: u32) -> (Vec<u8>, u32, u32) {
    let scale = (max_dim as f64 / src_w as f64)
        .min(max_dim as f64 / src_h as f64)
        .min(1.0);
    let tw = (src_w as f64 * scale).round().max(1.0) as u32;
    let th = (src_h as f64 * scale).round().max(1.0) as u32;
    let mut out = vec![0u8; (tw * th * 3) as usize];

    for y in 0..th {
        for x in 0..tw {
            let sx = ((x as f64 + 0.5) / scale).min(src_w as f64 - 1.0) as u32;
            let sy = ((y as f64 + 0.5) / scale).min(src_h as f64 - 1.0) as u32;
            let si = (sy * src_w + sx) as usize * 3;
            let di = (y * tw + x) as usize * 3;
            out[di..di + 3].copy_from_slice(&src[si..si + 3]);
        }
    }
    (out, tw, th)
}
