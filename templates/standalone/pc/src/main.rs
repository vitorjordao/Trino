//! Desktop host: bakes `assets/`, opens a window and runs the game.
//! `TRINO_SIM=native|n64|3ds` picks the console look (default n64).

fn main() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    let mut config = trino_runner_pc::RunConfig::new("{{name}}", root);
    config.save_name = "{{name}}".into();
    trino_runner_pc::run(config, {{game_new}});
}
