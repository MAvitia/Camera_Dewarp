use regex::Regex;
use std::fs::File;
use std::io::Read;
use std::path::Path;

#[derive(Debug, Clone)]
pub struct DjiCalibration {
    pub fx: f64,
    pub fy: f64,
    pub cx: f64,
    pub cy: f64,
    pub k1: f64,
    pub k2: f64,
    pub k3: f64,
    pub p1: f64,
    pub p2: f64,
    pub calibration_date: String,
    pub dewarp_flag: i32,
}

/// Read DJI DewarpData from XMP metadata in the first 64KB of a JPEG file.
pub fn read_dji_calibration(path: &Path) -> Option<DjiCalibration> {
    let mut file = File::open(path).ok()?;
    let mut buf = vec![0u8; 65536];
    let n = file.read(&mut buf).ok()?;
    buf.truncate(n);

    let xmp_start = find_subsequence(&buf, b"<x:xmpmeta")?;
    let xmp_end = find_subsequence(&buf[xmp_start..], b"</x:xmpmeta>")?;
    let xmp_slice = &buf[xmp_start..xmp_start + xmp_end + b"</x:xmpmeta>".len()];
    let xmp = String::from_utf8_lossy(xmp_slice);

    let dewarp_re = Regex::new(r#"drone-dji:DewarpData="([^"]*)""#).ok()?;
    let dewarp_str = dewarp_re.captures(&xmp)?.get(1)?.as_str().to_string();

    let flag_re = Regex::new(r#"drone-dji:DewarpFlag="([^"]*)""#).ok()?;
    let dewarp_flag: i32 = flag_re
        .captures(&xmp)
        .and_then(|c| c.get(1)?.as_str().parse().ok())
        .unwrap_or(-1);

    let cx_re = Regex::new(r#"drone-dji:CalibratedOpticalCenterX="([^"]*)""#).ok()?;
    let cy_re = Regex::new(r#"drone-dji:CalibratedOpticalCenterY="([^"]*)""#).ok()?;

    let base_cx: f64 = cx_re.captures(&xmp)?.get(1)?.as_str().parse().ok()?;
    let base_cy: f64 = cy_re.captures(&xmp)?.get(1)?.as_str().parse().ok()?;

    // DewarpData format: "date;fx,fy,cx_offset,cy_offset,k1,k2,p1,p2,k3"
    let parts: Vec<&str> = dewarp_str.splitn(2, ';').collect();
    if parts.len() != 2 {
        return None;
    }
    let cal_date = parts[0].to_string();
    let values: Vec<f64> = parts[1].split(',').filter_map(|s| s.parse().ok()).collect();
    if values.len() != 9 {
        return None;
    }

    let fx = values[0];
    let fy = values[1];
    let cx = base_cx + values[2];
    let cy = base_cy + values[3];
    let k1 = values[4];
    let k2 = values[5];
    let p1 = values[6];
    let p2 = values[7];
    let k3 = values[8];

    Some(DjiCalibration {
        fx,
        fy,
        cx,
        cy,
        k1,
        k2,
        k3,
        p1,
        p2,
        calibration_date: cal_date,
        dewarp_flag,
    })
}

fn find_subsequence(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|w| w == needle)
}
