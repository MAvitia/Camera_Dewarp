struct Params {
    width: u32,
    height: u32,
    src_width: u32,
    src_height: u32,
}

@group(0) @binding(0) var<uniform> params: Params;
@group(0) @binding(1) var<storage, read> src: array<u32>;    // RGBA packed per pixel
@group(0) @binding(2) var<storage, read> lut_x: array<f32>;
@group(0) @binding(3) var<storage, read> lut_y: array<f32>;
@group(0) @binding(4) var<storage, read_write> dst: array<u32>; // RGBA packed per pixel

fn unpack_rgba(packed: u32) -> vec4<f32> {
    return vec4<f32>(
        f32(packed & 0xFFu),
        f32((packed >> 8u) & 0xFFu),
        f32((packed >> 16u) & 0xFFu),
        f32((packed >> 24u) & 0xFFu),
    );
}

fn pack_rgba(c: vec4<f32>) -> u32 {
    let r = u32(clamp(c.x, 0.0, 255.0));
    let g = u32(clamp(c.y, 0.0, 255.0));
    let b = u32(clamp(c.z, 0.0, 255.0));
    let a = u32(clamp(c.w, 0.0, 255.0));
    return r | (g << 8u) | (b << 16u) | (a << 24u);
}

@compute @workgroup_size(16, 16)
fn main(@builtin(global_invocation_id) gid: vec3<u32>) {
    let x = gid.x;
    let y = gid.y;
    if (x >= params.width || y >= params.height) {
        return;
    }

    let idx = y * params.width + x;
    let sx = lut_x[idx];
    let sy = lut_y[idx];

    let x0 = i32(floor(sx));
    let y0 = i32(floor(sy));
    let x1 = x0 + 1;
    let y1 = y0 + 1;

    if (x0 < 0 || y0 < 0 || x1 >= i32(params.src_width) || y1 >= i32(params.src_height)) {
        dst[idx] = 0u;
        return;
    }

    let fx = sx - f32(x0);
    let fy = sy - f32(y0);

    let sw = params.src_width;
    let p00 = unpack_rgba(src[u32(y0) * sw + u32(x0)]);
    let p10 = unpack_rgba(src[u32(y0) * sw + u32(x1)]);
    let p01 = unpack_rgba(src[u32(y1) * sw + u32(x0)]);
    let p11 = unpack_rgba(src[u32(y1) * sw + u32(x1)]);

    let color = p00 * (1.0 - fx) * (1.0 - fy)
              + p10 * fx * (1.0 - fy)
              + p01 * (1.0 - fx) * fy
              + p11 * fx * fy;

    dst[idx] = pack_rgba(color);
}
