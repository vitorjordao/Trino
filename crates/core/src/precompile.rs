//! Shader and pipeline precompilation (issue #101, ADR 0031): what a game
//! asks for at a loading screen, and the progress it draws.
//!
//! Where shaders compile, per target:
//!
//! - **N64:** nowhere at run time. The RSP overlays are assembled by the
//!   build and registered once at boot; the RDP combiner is register state.
//! - **3DS:** nowhere at run time. The PICA200 vertex shaders are assembled
//!   by picasso at build time and parsed at boot; TexEnv is register state.
//! - **PC and Web:** the GPU driver (or the browser) compiles each render
//!   pipeline when it is created. The renderer creates most of them up front
//!   and the rest (MSAA twins, the shadow map, ambient occlusion, the
//!   upscale, color-vision views, the GPU T&L) the first time a frame needs
//!   one: a hitch in that frame. [`Renderer::precompile`] builds those ahead.
//!
//! Trino has no user shaders (a surface is `ModelParams` data), so the set
//! of pipelines is finite and known: a complete warm-up is possible.
//!
//! ```ignore
//! // A loading screen (any target; consoles report done at once).
//! if !self.warm_started {
//!     r.precompile(&PrecompileRequest::EVERYTHING);
//!     self.warm_started = true;
//! }
//! let p = r.precompile_status();
//! draw_bar(r, p.fraction());
//! if p.is_done() && level_loaded { self.state = State::Play; }
//! ```
//!
//! [`Renderer::precompile`]: crate::Renderer::precompile

/// When the work runs.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum PrecompileMode {
    /// On worker threads (one job per frame on the Web, which has none)
    /// while the game keeps drawing: poll
    /// [`Renderer::precompile_status`](crate::Renderer::precompile_status).
    #[default]
    Background,
    /// Before `precompile` returns, on the thread budget's workers.
    Blocking,
}

/// What to build. [`PrecompileRequest::EVERYTHING`] covers every pipeline
/// the active profile can reach; [`PrecompileRequest::CURRENT`] only what
/// the current settings (quality preset, MSAA) draw with.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct PrecompileRequest {
    pub mode: PrecompileMode,
    /// Every pipeline the profile can reach (any quality preset, MSAA on
    /// and off, every color-vision view), not only the current settings':
    /// the options page then never hitches.
    pub all_settings: bool,
}

impl PrecompileRequest {
    pub const EVERYTHING: PrecompileRequest = PrecompileRequest {
        mode: PrecompileMode::Background,
        all_settings: true,
    };
    pub const CURRENT: PrecompileRequest = PrecompileRequest {
        mode: PrecompileMode::Background,
        all_settings: false,
    };

    pub const fn blocking(self) -> PrecompileRequest {
        PrecompileRequest {
            mode: PrecompileMode::Blocking,
            ..self
        }
    }
}

impl Default for PrecompileRequest {
    fn default() -> Self {
        PrecompileRequest::EVERYTHING
    }
}

/// Progress of the last [`Renderer::precompile`](crate::Renderer::precompile).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct PrecompileStatus {
    /// Jobs finished (each builds one pass: a few pipelines).
    pub done: u32,
    /// Jobs planned; 0 when nothing compiles at run time (consoles).
    pub total: u32,
    /// Wall time since the request, in microseconds (0 on consoles).
    pub elapsed_us: u32,
    /// Threads the work runs on (0: none needed).
    pub threads: u32,
}

impl PrecompileStatus {
    /// Nothing to do: what every console answers.
    pub const DONE: PrecompileStatus = PrecompileStatus {
        done: 0,
        total: 0,
        elapsed_us: 0,
        threads: 0,
    };

    pub fn is_done(&self) -> bool {
        self.done >= self.total
    }

    /// 0.0..=1.0 for a progress bar (1.0 when there is nothing to do).
    pub fn fraction(&self) -> f32 {
        if self.total == 0 {
            1.0
        } else {
            self.done.min(self.total) as f32 / self.total as f32
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn consoles_are_done_at_once() {
        assert!(PrecompileStatus::DONE.is_done());
        assert_eq!(PrecompileStatus::DONE.fraction(), 1.0);
        let half = PrecompileStatus {
            done: 3,
            total: 6,
            ..PrecompileStatus::DONE
        };
        assert!(!half.is_done());
        assert_eq!(half.fraction(), 0.5);
        assert_eq!(
            PrecompileRequest::CURRENT.blocking().mode,
            PrecompileMode::Blocking
        );
    }
}
