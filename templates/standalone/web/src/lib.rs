//! Web host: the game in a browser tab, on WebGPU or WebGL2
//! (`cargo trino run web`). `cargo trino build web --release --out dist/web`
//! writes a folder to upload anywhere static files are served.
//!
//! The page's URL can pick the look (`?sim=n64`), the API
//! (`?backend=webgl2`) and the pixel scale (`?scale=2`).

#[cfg(target_arch = "wasm32")]
#[wasm_bindgen::prelude::wasm_bindgen(start)]
pub fn start() {
    let query = trino_platform_web::Query::from_location();
    let config = trino_platform_web::WebConfig::new("{{name}}", &query);
    trino_platform_web::run_game(config, "assets/trino.pak".into(), {{game_new}});
}
