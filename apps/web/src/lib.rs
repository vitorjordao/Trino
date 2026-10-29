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
//! - `?net=<room>&lobby=ws://<host>:7401` (`?players=`, `?stun=`): online
//!   play over WebRTC data channels, met through `cargo xtask net lobby`
//!   (issue #67, `net.rs`);
//! - `?replay=<url>` / `?record`: play a `TREC` recording back (the page
//!   finishes with its verdict) or record the session into
//!   `__trino.info.recording` (issue #67, `trino_platform_web::replay`);
//! - the backend and look: `?backend=webgpu|webgl2`, `?sim=web|n64|3ds`,
//!   `?scale=N`, `?size=WxH`, `?msaa=0`, `?quality=low|medium|high|ultra|auto`
//!   (`trino_platform_web::WebConfig`).
//!
//! Off wasm32 the crate is empty (the workspace's host builds skip it).

#[cfg(target_arch = "wasm32")]
mod net;
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

        /// `saves`: castle64 keeps the player's progress (issue #121).
        fn make(self, screen: Vec2, saves: bool) -> Box<dyn Game> {
            match self {
                Which::Castle64 => {
                    let g = castle64::Castle64Game::new(screen);
                    Box::new(if saves { g.with_saves() } else { g })
                }
                Which::Showcase => Box::new(showcase::ShowcaseGame::with_screen(screen)),
                Which::SolidEleven => Box::new(solid_eleven::SolidElevenGame::new(screen)),
            }
        }
    }

    /// The page's allocator, counted for `Platform::memory` (issue #56).
    #[global_allocator]
    static HEAP: trino_core::mem::Counting<std::alloc::System> =
        trino_core::mem::Counting::new(std::alloc::System);

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
        host.platform.set_heap(Some(&HEAP));
        host.load_pack(which.pack()).await?;
        if selftest {
            crate::selftest::start(host);
            return Ok(());
        }
        // The progress in `localStorage` (issue #121), but never in a smoke
        // run (`?frames`), a replay or a recording.
        let saves = !query.has("frames") && !query.has("replay") && !query.has("record");
        let mut game = which.make(host.screen(), saves);
        let label = match which {
            Which::Castle64 => "castle64",
            Which::Showcase => "showcase",
            Which::SolidEleven => "solid-eleven",
        };
        // `?replay=<url>` / `?record` (issue #67).
        let screen = host.screen();
        let mut session = trino_platform_web::replay::WebSession::from_query(
            &query,
            label,
            (screen.x as u16, screen.y as u16),
        )
        .await?;
        session.start(game.as_ref());
        // `?net=<room>&lobby=ws://…` (issue #67): online from the first frame.
        let mut net = crate::net::WebNet::from_query(&query, &host).await?;
        let smoke = query.get("frames").and_then(|f| f.parse::<u64>().ok());
        let mut frames = 0u64;
        run(host, move |host, dt| {
            if let Some(screen) = host.sync_size() {
                game.resize(screen);
            }
            let live = host.poll_players();
            if let Some(n) = &mut net {
                host.pump_groups();
                n.frame(
                    game.as_mut(),
                    &mut host.platform,
                    &mut host.audio,
                    &live.players[0],
                );
                host.renderer
                    .apply_access(&trino_core::Platform::access(&host.platform));
                game.render(&mut host.renderer);
                host.end_frame();
                // `?frames=N` counts the frames played online.
                if smoke.is_some_and(|f| n.frames() >= f) {
                    trino_platform_web::js::log(&n.line());
                    trino_platform_web::js::trino_finish(true, None);
                    return Flow::Stop;
                }
                return Flow::Continue;
            }
            let Some((players, dt)) = session.input(&live, dt) else {
                // The replay ended: its verdict once, then its last frame
                // stays on the canvas (a screenshot can take it).
                if let Some(pass) = session.finish() {
                    trino_platform_web::js::trino_finish(
                        pass,
                        (!pass).then(|| "replay diverged".to_string()),
                    );
                }
                game.render(&mut host.renderer);
                host.end_frame();
                return Flow::Continue;
            };
            if session.is_replay() {
                host.platform.players.publish_players(&players);
            }
            host.pump_groups();
            game.update_with(&mut host.platform, &players.players[0], &mut host.audio, dt);
            session.after(game.as_ref(), &players, dt);
            host.renderer
                .apply_access(&trino_core::Platform::access(&host.platform));
            game.render(&mut host.renderer);
            host.end_frame();
            frames += 1;
            // `?frames=N`: a smoke run, like the PC's TRINO_SMOKE_FRAMES.
            if smoke.is_some_and(|n| frames >= n) {
                session.finish();
                trino_platform_web::js::log(&host.frame_line(label));
                trino_platform_web::js::trino_finish(true, None);
                return Flow::Stop;
            }
            Flow::Continue
        });
        Ok(())
    }
}
