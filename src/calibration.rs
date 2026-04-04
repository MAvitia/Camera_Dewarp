use regex::Regex;
use std::fs::File;
use std::io::{BufRead, BufReader, Read, Write};
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

impl DjiCalibration {
    /// Save calibration parameters to a human-readable text file.
    pub fn save_to_file(&self, path: &Path) -> std::io::Result<()> {
        let mut f = File::create(path)?;
        writeln!(f, "# Camera Dewarp — Lens Calibration File")?;
        writeln!(f, "# Extracted from DJI XMP DewarpData")?;
        writeln!(f, "calibration_date={}", self.calibration_date)?;
        writeln!(f, "fx={:.12}", self.fx)?;
        writeln!(f, "fy={:.12}", self.fy)?;
        writeln!(f, "cx={:.12}", self.cx)?;
        writeln!(f, "cy={:.12}", self.cy)?;
        writeln!(f, "k1={:.15}", self.k1)?;
        writeln!(f, "k2={:.15}", self.k2)?;
        writeln!(f, "k3={:.15}", self.k3)?;
        writeln!(f, "p1={:.15}", self.p1)?;
        writeln!(f, "p2={:.15}", self.p2)?;
        Ok(())
    }

    /// Load calibration parameters from a previously saved text file.
    pub fn load_from_file(path: &Path) -> Option<Self> {
        let file = File::open(path).ok()?;
        let reader = BufReader::new(file);

        let mut cal = DjiCalibration {
            fx: 0.0, fy: 0.0, cx: 0.0, cy: 0.0,
            k1: 0.0, k2: 0.0, k3: 0.0, p1: 0.0, p2: 0.0,
            calibration_date: String::new(),
            dewarp_flag: 0,
        };
        let mut fields_found = 0u32;

        for line in reader.lines().map_while(Result::ok) {
            let line = line.trim().to_string();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let Some((key, val)) = line.split_once('=') else { continue };
            let key = key.trim();
            let val = val.trim();
            match key {
                "fx" => { cal.fx = val.parse().ok()?; fields_found += 1; }
                "fy" => { cal.fy = val.parse().ok()?; fields_found += 1; }
                "cx" => { cal.cx = val.parse().ok()?; fields_found += 1; }
                "cy" => { cal.cy = val.parse().ok()?; fields_found += 1; }
                "k1" => { cal.k1 = val.parse().ok()?; fields_found += 1; }
                "k2" => { cal.k2 = val.parse().ok()?; fields_found += 1; }
                "k3" => { cal.k3 = val.parse().ok()?; fields_found += 1; }
                "p1" => { cal.p1 = val.parse().ok()?; fields_found += 1; }
                "p2" => { cal.p2 = val.parse().ok()?; fields_found += 1; }
                "calibration_date" => { cal.calibration_date = val.to_string(); }
                _ => {}
            }
        }

        if fields_found >= 9 { Some(cal) } else { None }
    }
}
