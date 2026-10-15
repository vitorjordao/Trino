//! Fixture writers shared by the asset-pipeline integration tests: PNG and
//! WAV masters, and a small glTF 2.0 builder (embedded base64 buffers) so
//! every model feature can be generated in a temp dir instead of committed.

#![allow(dead_code)] // each test binary uses a different subset

use std::path::Path;

/// `w`x`h` RGBA PNG whose pixel (x, y) is `px(x, y)`.
pub fn write_png_with(path: &Path, w: u32, h: u32, px: impl Fn(u32, u32) -> [u8; 4]) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, png_bytes(w, h, px)).unwrap();
}

/// Solid-color `w`x`h` RGBA PNG.
pub fn write_png(path: &Path, w: u32, h: u32, color: [u8; 4]) {
    write_png_with(path, w, h, |_, _| color);
}

pub fn png_bytes(w: u32, h: u32, px: impl Fn(u32, u32) -> [u8; 4]) -> Vec<u8> {
    let mut out = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut out, w, h);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder.write_header().unwrap();
        let pixels: Vec<u8> = (0..h)
            .flat_map(|y| (0..w).map(move |x| (x, y)))
            .flat_map(|(x, y)| px(x, y))
            .collect();
        writer.write_image_data(&pixels).unwrap();
    }
    out
}

/// PNG in another color type (RGB, grayscale) with raw `data`.
pub fn write_png_typed(path: &Path, w: u32, h: u32, color: png::ColorType, data: &[u8]) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, png_bytes_typed(w, h, color, data)).unwrap();
}

pub fn png_bytes_typed(w: u32, h: u32, color: png::ColorType, data: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut out, w, h);
        encoder.set_color(color);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder.write_header().unwrap();
        writer.write_image_data(data).unwrap();
    }
    out
}

/// 16-bit integer WAV, interleaved `samples` over `channels`.
pub fn write_wav_i16(path: &Path, rate: u32, channels: u16, samples: &[i16]) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let spec = hound::WavSpec {
        channels,
        sample_rate: rate,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut writer = hound::WavWriter::create(path, spec).unwrap();
    for s in samples {
        writer.write_sample(*s).unwrap();
    }
    writer.finalize().unwrap();
}

/// 32-bit float WAV.
pub fn write_wav_f32(path: &Path, rate: u32, channels: u16, samples: &[f32]) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let spec = hound::WavSpec {
        channels,
        sample_rate: rate,
        bits_per_sample: 32,
        sample_format: hound::SampleFormat::Float,
    };
    let mut writer = hound::WavWriter::create(path, spec).unwrap();
    for s in samples {
        writer.write_sample(*s).unwrap();
    }
    writer.finalize().unwrap();
}

pub fn b64(bytes: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for chunk in bytes.chunks(3) {
        let n = chunk.len();
        let v = (chunk[0] as u32) << 16
            | (*chunk.get(1).unwrap_or(&0) as u32) << 8
            | *chunk.get(2).unwrap_or(&0) as u32;
        for i in 0..4 {
            if i <= n {
                out.push(T[(v >> (18 - 6 * i) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

/// Accumulates one binary buffer plus its views/accessors; the caller
/// supplies the rest of the document (scenes, nodes, meshes, ...).
#[derive(Default)]
pub struct Gltf {
    buf: Vec<u8>,
    views: Vec<String>,
    accessors: Vec<String>,
}

fn components(ty: &str) -> usize {
    match ty {
        "SCALAR" => 1,
        "VEC2" => 2,
        "VEC3" => 3,
        "VEC4" => 4,
        "MAT4" => 16,
        other => panic!("accessor type {other}"),
    }
}

impl Gltf {
    pub fn new() -> Self {
        Self::default()
    }

    fn view(&mut self, bytes: &[u8]) -> usize {
        while !self.buf.len().is_multiple_of(4) {
            self.buf.push(0);
        }
        let at = self.buf.len();
        self.buf.extend_from_slice(bytes);
        self.views.push(format!(
            "{{\"buffer\": 0, \"byteOffset\": {at}, \"byteLength\": {}}}",
            bytes.len()
        ));
        self.views.len() - 1
    }

    /// Float accessor of `ty` (min/max always written: POSITION needs them).
    pub fn f32s(&mut self, ty: &str, vals: &[f32]) -> usize {
        let n = components(ty);
        assert_eq!(vals.len() % n, 0);
        let bytes: Vec<u8> = vals.iter().flat_map(|v| v.to_le_bytes()).collect();
        let view = self.view(&bytes);
        let count = vals.len() / n;
        let fmt = |v: Vec<f32>| {
            v.iter()
                .map(|x| format!("{x:?}"))
                .collect::<Vec<_>>()
                .join(",")
        };
        let min: Vec<f32> = (0..n)
            .map(|c| (0..count).map(|i| vals[i * n + c]).fold(f32::MAX, f32::min))
            .collect();
        let max: Vec<f32> = (0..n)
            .map(|c| (0..count).map(|i| vals[i * n + c]).fold(f32::MIN, f32::max))
            .collect();
        self.accessors.push(format!(
            "{{\"bufferView\": {view}, \"componentType\": 5126, \"count\": {count}, \"type\": \"{ty}\", \"min\": [{}], \"max\": [{}]}}",
            fmt(min),
            fmt(max)
        ));
        self.accessors.len() - 1
    }

    /// Unsigned-short accessor (indices, JOINTS_0).
    pub fn u16s(&mut self, ty: &str, vals: &[u16]) -> usize {
        let n = components(ty);
        let bytes: Vec<u8> = vals.iter().flat_map(|v| v.to_le_bytes()).collect();
        let view = self.view(&bytes);
        self.accessors.push(format!(
            "{{\"bufferView\": {view}, \"componentType\": 5123, \"count\": {}, \"type\": \"{ty}\"}}",
            vals.len() / n
        ));
        self.accessors.len() - 1
    }

    /// Unsigned-byte accessor (e.g. an integer custom attribute).
    pub fn u8s(&mut self, ty: &str, vals: &[u8]) -> usize {
        let n = components(ty);
        let view = self.view(vals);
        self.accessors.push(format!(
            "{{\"bufferView\": {view}, \"componentType\": 5121, \"count\": {}, \"type\": \"{ty}\"}}",
            vals.len() / n
        ));
        self.accessors.len() - 1
    }

    /// Complete document: `body` holds the remaining top-level members
    /// (no trailing comma), e.g. `"scene": 0, "scenes": [...], "nodes": [...]`.
    pub fn json(&self, body: &str) -> String {
        format!(
            "{{\n\"asset\": {{\"version\": \"2.0\"}},\n{body},\n\"buffers\": [{{\"byteLength\": {}, \"uri\": \"data:application/octet-stream;base64,{}\"}}],\n\"bufferViews\": [{}],\n\"accessors\": [{}]\n}}",
            self.buf.len(),
            b64(&self.buf),
            self.views.join(",\n"),
            self.accessors.join(",\n")
        )
    }

    pub fn write(&self, path: &Path, body: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, self.json(body)).unwrap();
    }
}

/// One +Z-facing triangle (x, y in [0, 1]) with normals: (POSITION, NORMAL)
/// accessor indices.
pub fn triangle(g: &mut Gltf) -> (usize, usize) {
    let pos = g.f32s("VEC3", &[0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0]);
    let nrm = g.f32s("VEC3", &[0.0, 0.0, 1.0, 0.0, 0.0, 1.0, 0.0, 0.0, 1.0]);
    (pos, nrm)
}

/// A static single-triangle glTF (indexed), one node, written to `path`.
pub fn write_triangle_gltf(path: &Path) {
    let mut g = Gltf::new();
    let (pos, nrm) = triangle(&mut g);
    let idx = g.u16s("SCALAR", &[0, 1, 2]);
    g.write(
        path,
        &format!(
            r#""scene": 0, "scenes": [{{"nodes": [0]}}],
"nodes": [{{"name": "tri", "mesh": 0}}],
"meshes": [{{"primitives": [{{"attributes": {{"POSITION": {pos}, "NORMAL": {nrm}}}, "indices": {idx}}}]}}]"#
        ),
    );
}
