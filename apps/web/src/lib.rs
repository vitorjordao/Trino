//! Web glue (issue #47): `trino-platform-web` and the engine's games in one
//! wasm module, built by `cargo xtask build web` into
//! `target/web/{index.html, trino.js, trino.wasm, assets/}`.
//!
//! The page's URL picks what runs:
//! - `?game=castle64` (default), `?game=showcase` (the editor's scene
//!   exported to code), `?game=solid-eleven` (the football demo, its own
//!   asset pack);
//! - `?selftest`: the browser self-test (`selftest.rs`) — the engine
//!   checks, the demos' bots and the golden frames, reported on the console
//!   as `TRINO_STEP:` / `TRINO_TEST_PASS` lines and to the harness
//!   (`globalThis.__trino`, `tools/web-test/run.mjs`);
//! - the backend and look: `?backend=webgpu|webgl2`, `?sim=web|n64|3ds`,
//!   `?scale=N`, `?size=WxH`, `?msaa=0` (`trino_platform_web::WebConfig`).
//!
//! Off wasm32 the crate is empty (the workspace's host builds skip it).

#[cfg(target_arch = "wasm32")]
mod selftest;

#[cfg(target_arch = "wasm32")]
mod web {
    use trino_core::{Game, Vec2};
    use trino_platform_web::{Flow, Query, WebConfig, WebHost, fail, install_panic_hook, run};
    use wasm_bindgen::prelude::*;

    /// The games this page can run.
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub enum Which {
        Castle64,
        Showcase,
        SolidEleven,
    }

    impl Which {
        pub fn parse(s: &str) -> Option<Which> {
            match s {
                "castle64" | "" => Some(Which::Castle64),
                "showcase" => Some(Which::Showcase),
                "solid-eleven" => Some(Which::SolidEleven),
                _ => None,
            }
        }

        /// Its asset pack: the engine's (castle64, the showcase) or Solid
        /// Eleven's own (`examples/solid-eleven/assets`).
        pub fn pack(self) -> &'static str {
            match self {
                Which::Castle64 | Which::Showcase => "assets/trino.pak",
                Which::SolidEleven => "assets/solid-eleven.pak",
            }
        }

        fn make(self, screen: Vec2) -> Box<dyn Game> {
            match self {
                Which::Castle64 => Box::new(castle64::Castle64Game::new(screen)),
                Which::Showcase => Box::new(showcase::ShowcaseGame::with_screen(screen)),
                Which::SolidEleven => Box::new(solid_eleven::SolidElevenGame::new(screen)),
            }
        }
    }

    #[wasm_bindgen(start)]
    pub fn start() {
        install_panic_hook();
        wasm_bindgen_futures::spawn_local(async {
            if let Err(e) = main().await {
                fail(&e);
            }
        });
    }

    async fn main() -> Result<(), String> {
        let query = Query::from_location();
        let name = query.get("game").unwrap_or_default();
        let which = Which::parse(&name)
            .ok_or_else(|| format!("unknown game `{name}` (castle64, showcase, solid-eleven)"))?;
        let selftest = query.has("selftest");
        let save = if selftest { "selftest" } else { &name };
        let save = if save.is_empty() { "castle64" } else { save };
        let mut host = WebHost::new(WebConfig::new(save, &query)).await?;
        host.load_pack(which.pack()).await?;
        if selftest {
            crate::selftest::start(host);
            return Ok(());
        }
        let mut game = which.make(host.screen());
        let smoke = query.get("frames").and_then(|f| f.parse::<u64>().ok());
        let mut frames = 0u64;
        run(host, move |host, dt| {
            if let Some(screen) = host.sync_size() {
                game.resize(screen);
            }
            let input = host.poll_input();
            game.update_with(&mut host.platform, &input, &mut host.audio, dt);
            game.render(&mut host.renderer);
            host.end_frame();
            frames += 1;
            // `?frames=N`: a smoke run, like the PC's TRINO_SMOKE_FRAMES.
            if smoke.is_some_and(|n| frames >= n) {
                let label = match which {
                    Which::Castle64 => "castle64",
                    Which::Showcase => "showcase",
                    Which::SolidEleven => "solid-eleven",
                };
                trino_platform_web::js::log(&host.meter.line(label));
                trino_platform_web::js::trino_finish(true, None);
                return Flow::Stop;
            }
            Flow::Continue
        });
        Ok(())
    }
}
