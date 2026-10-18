//! The GPU T&L path against its oracle (issue #51, ADR 0016): one frame
//! drawn twice by the same renderer, once with the draws `pc_gpu_check`
//! accepts on the GPU and once with every draw on the CPU T&L
//! (`tessellate_with`, the reference on every target), and the difference.
//! The PC goldens of the native profile run it next to their golden check.

use crate::PcRenderer;
use trino_core::Renderer;

/// Mean channel difference (of 255) the GPU path may have from the CPU
/// path over a frame.
pub const MAX_MEAN: f64 = 1.0;
/// Pixels whose channels differ by more than [`FAR`] (of 255), as a share
/// of the frame.
pub const MAX_FAR_SHARE: f64 = 0.005;
/// What counts as a far pixel.
pub const FAR: u8 = 8;

/// One frame on both paths.
pub struct TnlDiff {
    pub width: u32,
    pub height: u32,
    /// RGBA8 rows, the GPU path's frame.
    pub gpu: Vec<u8>,
    /// RGBA8 rows, the CPU path's frame.
    pub cpu: Vec<u8>,
    /// Mean absolute difference of the R, G and B channels.
    pub mean: f64,
    /// Share of pixels with a channel more than [`FAR`] off.
    pub far_share: f64,
    /// Draws the GPU path took in its frame (`TnlStats::pc_gpu_draws`).
    pub gpu_draws: u32,
}

impl TnlDiff {
    /// Within the oracle's tolerance. A frame whose draws all stay on the
    /// CPU (`PcGpuReject`) passes trivially: check [`TnlDiff::gpu_draws`]
    /// where the GPU should draw.
    pub fn passes(&self) -> bool {
        self.mean <= MAX_MEAN && self.far_share <= MAX_FAR_SHARE
    }

    /// Panics with the figures unless [`TnlDiff::passes`].
    pub fn assert_within(&self, scene: &str) {
        assert!(self.passes(), "{scene}: {self}");
    }

    /// GPU | CPU | difference ×8, side by side (RGBA8, `3 × width` wide):
    /// the evidence picture.
    pub fn side_by_side(&self) -> (u32, u32, Vec<u8>) {
        let (w, h) = (self.width as usize, self.height as usize);
        let mut out = vec![0u8; w * 3 * h * 4];
        for y in 0..h {
            for x in 0..w {
                let i = (y * w + x) * 4;
                let row = y * w * 3;
                let g = &self.gpu[i..i + 4];
                let c = &self.cpu[i..i + 4];
                out[(row + x) * 4..][..4].copy_from_slice(g);
                out[(row + w + x) * 4..][..4].copy_from_slice(c);
                let d = &mut out[(row + 2 * w + x) * 4..][..4];
                for k in 0..3 {
                    d[k] = g[k].abs_diff(c[k]).saturating_mul(8);
                }
                d[3] = 255;
            }
        }
        (self.width * 3, self.height, out)
    }
}

impl std::fmt::Display for TnlDiff {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "GPU vs CPU T&L: mean {:.3}/255 (max {MAX_MEAN}), {:.3}% of pixels over {FAR}/255 \
             (max {}%), {} GPU draws",
            self.mean,
            self.far_share * 100.0,
            MAX_FAR_SHARE * 100.0,
            self.gpu_draws
        )
    }
}

/// Compare two RGBA8 frames of the same size.
pub fn diff(width: u32, height: u32, gpu: Vec<u8>, cpu: Vec<u8>, gpu_draws: u32) -> TnlDiff {
    assert_eq!(gpu.len(), cpu.len(), "frames of different sizes");
    let (mut sum, mut far) = (0u64, 0usize);
    for (g, c) in gpu.as_chunks::<4>().0.iter().zip(cpu.as_chunks::<4>().0) {
        let d = [0, 1, 2].map(|k| g[k].abs_diff(c[k]));
        sum += d.iter().map(|&v| v as u64).sum::<u64>();
        far += (d.iter().any(|&v| v > FAR)) as usize;
    }
    let px = (gpu.len() / 4).max(1);
    TnlDiff {
        width,
        height,
        gpu,
        cpu,
        mean: sum as f64 / (px * 3) as f64,
        far_share: far as f64 / px as f64,
        gpu_draws,
    }
}

/// Draw `frame` (a whole frame, returning its pixels) on the GPU path and
/// on the CPU path, and compare. `None` when the renderer has no GPU path
/// now (a console profile, the N64 look). The renderer's setting is
/// restored.
pub fn compare(
    r: &mut PcRenderer,
    mut frame: impl FnMut(&mut PcRenderer) -> Vec<u8>,
) -> Option<TnlDiff> {
    let was = r.hw_tnl_setting();
    r.set_hw_tnl(true);
    if !r.hw_tnl() {
        r.set_hw_tnl(was);
        return None;
    }
    let gpu = frame(r);
    let gpu_draws = r.stats().tnl.pc_gpu_draws;
    r.set_hw_tnl(false);
    let cpu = frame(r);
    r.set_hw_tnl(was);
    let (w, h) = r.internal_size();
    Some(diff(w, h, gpu, cpu, gpu_draws))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_figures_count_channels_and_far_pixels() {
        let a = vec![10, 10, 10, 255, 0, 0, 0, 255];
        let b = vec![10, 13, 10, 255, 0, 0, 20, 0];
        let d = diff(2, 1, a, b, 3);
        assert!((d.mean - 23.0 / 6.0).abs() < 1e-9);
        assert_eq!(d.far_share, 0.5);
        assert!(!d.passes());
        let (w, h, img) = d.side_by_side();
        assert_eq!((w, h, img.len()), (6, 1, 24));
        assert_eq!(&img[16..24], &[0, 24, 0, 255, 0, 0, 160, 255]);
    }

    #[test]
    fn the_tolerance_is_the_issues() {
        let a = vec![100u8; 4 * 1000];
        assert!(diff(1000, 1, a.clone(), a.clone(), 1).passes());
        // Every channel one off: the mean limit, still in.
        let one: Vec<u8> = a.iter().map(|v| v + 1).collect();
        assert!(diff(1000, 1, one, a.clone(), 1).passes());
        // Five far pixels of 1000 pass, six don't.
        let mut far = a.clone();
        far[..5 * 4].fill(0);
        assert!(diff(1000, 1, far.clone(), a.clone(), 1).passes());
        far[..6 * 4].fill(0);
        assert!(!diff(1000, 1, far, a, 1).passes());
    }
}
