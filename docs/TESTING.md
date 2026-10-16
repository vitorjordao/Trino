# Testing

Every engine feature has at least one test that goes through the public API.
This file maps each feature to those tests. It is the place to look for "what
proves X works", and the place to add a row when you add a feature.

`xtask/tests/feature_matrix.rs` keeps the matrix honest, and it runs in
`cargo test --workspace`. It fails when:

- a `` `path/to/file.rs::fn_name` `` reference below is not a `#[test]`
  function in that file (a renamed or deleted test);
- a feature the code exposes has no row. The guard reads these lists from
  the source: the methods of `Renderer`, `Platform` and `Audio`, the TMDL
  sections `Mesh::from_tmdl` reads, the asset kinds of the manifest, and the
  top-level xtask commands.

A row's test cell lists test references. Only when no host test can exist
(the command needs Docker, an emulator, Blender or a window) does the cell
start with `n/a:` and give the reason.

## Kinds of tests

- **Integration tests** live in `crates/<crate>/tests/*.rs` and
  `xtask/tests/*.rs`. They use only the crate's public API, and they work end
  to end: write a manifest and masters into a temp dir, bake, load, then
  draw, play or parse what came out.
- **Unit tests** live next to the code (`#[cfg(test)]`), for internals no
  public path reaches cheaply. The editor is a binary crate, so all its tests
  are unit tests.
- **GPU tests** create a headless `PcRenderer`. Without an adapter they skip,
  and with `TRINO_REQUIRE_GPU=1` they fail instead, which is what CI sets on
  Linux (Mesa lavapipe). They probe flat areas within a tolerance, so the same
  assertions hold on a real GPU and on lavapipe. Window, emulator and
  slow tests are `#[ignore]` and run in their own CI jobs.
- **Console self-test steps** (`castle64::selftest`) run the same checks on
  PC (`xtask/tests/engine_checks.rs`), in ares and in Azahar.

Shared fixtures: `crates/core/tests/common/` (a recording renderer that runs
the real T&L, a mock mixer, an EEPROM-like save medium, TMDL/TFNT/TSCN
builders), `crates/asset-pipeline/tests/common/` and
`crates/platform-pc/tests/common/` (synthetic projects baked for real).

## Coverage

```sh
cargo install cargo-llvm-cov --locked && rustup component add llvm-tools-preview
TRINO_REQUIRE_GPU=1 cargo llvm-cov --workspace --summary-only \
  --ignore-filename-regex '(crates/platform-(n64|3ds)|apps/(n64|3ds))/|crates/editor/src/main\.rs'
```

The regex leaves out what cannot run on a host: the N64/3DS backends and
apps, and the editor's event loop. cargo-llvm-cov also drops `tests/` and
`examples/` by default, so the example games don't count toward the engine's
number. The editor's App-level flows are `#[ignore]` (they change the working
directory). Add them with
`cargo llvm-cov --no-report -p trino-editor -- --ignored --test-threads=1`
before `cargo llvm-cov report`.

Keep a test module out of `#[path = "../src/x.rs"]` includes when coverage
matters: lines compiled that way are reported under `tests/../src/x.rs`, and
the default filter drops them.

## Matrix

### Renderer trait

Core rows prove the provided defaults and the backend-independent part (the
recording renderer runs the real software T&L). PC rows draw baked assets on
the wgpu backend and read pixels back.

| Feature | Tests |
|---|---|
| `Renderer::caps` | `crates/platform-pc/tests/render_e2e.rs::baked_sprites_rects_and_text_draw_on_the_pc_renderer`, `crates/platform-pc/tests/golden.rs::strict_mode_rejects_texture_over_budget` |
| `Renderer::begin_frame` | `crates/platform-pc/tests/render_e2e.rs::baked_sprites_rects_and_text_draw_on_the_pc_renderer`, `crates/platform-pc/tests/golden.rs::basic_scene_matches_golden` |
| `Renderer::end_frame` | `crates/platform-pc/tests/render_e2e.rs::baked_sprites_rects_and_text_draw_on_the_pc_renderer`, `crates/core/tests/renderer_defaults.rs::recorder_stats_cover_the_last_finished_frame` |
| `Renderer::draw_sprite` | `crates/platform-pc/tests/render_e2e.rs::baked_sprites_rects_and_text_draw_on_the_pc_renderer`, `crates/platform-pc/tests/golden.rs::basic_scene_matches_golden`, `crates/platform-pc/tests/golden.rs::platformer_scene_matches_golden` |
| `Renderer::fill_rect` | `crates/core/tests/renderer_defaults.rs::provided_renderer_methods_are_harmless_defaults`, `crates/platform-pc/tests/render_e2e.rs::baked_sprites_rects_and_text_draw_on_the_pc_renderer`, `crates/platform-pc/tests/render_e2e.rs::large_frames_grow_buffers_and_batches_mix_textures` |
| `Renderer::draw_text` | `crates/core/tests/text.rs::draw_text_lays_out_lines_glyphs_and_alignment`, `crates/core/tests/renderer_defaults.rs::provided_renderer_methods_are_harmless_defaults`, `crates/platform-pc/tests/render_e2e.rs::baked_sprites_rects_and_text_draw_on_the_pc_renderer` |
| `Renderer::measure_text` | `crates/core/tests/text.rs::draw_text_scales_tints_offsets_and_falls_back`, `crates/platform-pc/tests/render_e2e.rs::baked_sprites_rects_and_text_draw_on_the_pc_renderer` |
| `Renderer::set_camera` | `crates/core/tests/tessellate.rs::a_cube_face_on_emits_its_front_face_only`, `crates/platform-pc/tests/render_e2e.rs::baked_models_light_fog_depth_textures_and_billboards` |
| `Renderer::set_light` | `crates/core/tests/tessellate.rs::colored_light_tints_lit_surfaces`, `crates/core/tests/tessellate.rs::fog_blends_toward_the_fog_color_with_view_depth`, `crates/platform-pc/tests/render_e2e.rs::baked_models_light_fog_depth_textures_and_billboards` |
| `Renderer::set_local_lights` | `crates/core/tests/renderer_defaults.rs::provided_renderer_methods_are_harmless_defaults`, `crates/platform-pc/tests/local_lights.rs::lights_add_their_color_where_they_reach_on_every_tier`, `crates/platform-pc/tests/local_lights.rs::no_lights_draw_exactly_what_the_renderer_drew_before`, `crates/platform-pc/tests/local_lights.rs::per_vertex_tiers_use_at_most_their_cap`, `xtask/tests/engine_checks.rs::engine_checks_pass_on_pc` |
| `Renderer::set_wetness` | `crates/core/tests/renderer_defaults.rs::provided_renderer_methods_are_harmless_defaults`, `crates/platform-pc/tests/local_lights.rs::gloss_and_wetness_add_highlights_per_pixel_only`, `crates/platform-pc/tests/local_lights.rs::each_tier_matches_its_golden` |
| `Renderer::set_texture_filter` | `crates/platform-pc/tests/texture_filter.rs::the_default_filter_is_the_profiles_and_set_texture_filter_changes_it` |
| `Renderer::draw_model` | `crates/core/tests/tessellate.rs::shading_modes_tint_and_emissive`, `crates/platform-pc/tests/render_e2e.rs::baked_models_light_fog_depth_textures_and_billboards`, `crates/platform-pc/tests/golden.rs::cube3d_scene_matches_golden` |
| `Renderer::draw_model_matrix` | `crates/core/tests/renderer_defaults.rs::draw_model_matrix_default_decomposes_into_draw_model`, `crates/platform-pc/tests/render_e2e.rs::baked_models_light_fog_depth_textures_and_billboards` |
| `Renderer::draw_billboard` | `crates/core/tests/tessellate.rs::billboards_face_the_camera_and_respect_fog_and_regions`, `crates/platform-pc/tests/render_e2e.rs::baked_models_light_fog_depth_textures_and_billboards` |
| `Renderer::stats` | `crates/core/tests/renderer_defaults.rs::recorder_stats_cover_the_last_finished_frame`, `crates/platform-pc/tests/render_e2e.rs::baked_sprites_rects_and_text_draw_on_the_pc_renderer` |
| `Renderer::second_screen` | `crates/core/tests/renderer_defaults.rs::provided_renderer_methods_are_harmless_defaults`, `crates/platform-pc/tests/render_e2e.rs::n3ds_second_screen_renders_reads_back_and_presents_stacked` |
| `Renderer::set_screen` | `crates/core/tests/renderer_defaults.rs::provided_renderer_methods_are_harmless_defaults`, `crates/platform-pc/tests/render_e2e.rs::n3ds_second_screen_renders_reads_back_and_presents_stacked` |
| `Renderer::screen` | `crates/core/tests/renderer_defaults.rs::screen_api_collapses_on_a_single_screen`, `crates/core/tests/renderer_defaults.rs::screen_api_goes_top_then_bottom`, `crates/platform-pc/tests/render_e2e.rs::n3ds_second_screen_renders_reads_back_and_presents_stacked` |
| `Renderer::on_screen` | `crates/core/tests/renderer_defaults.rs::screen_api_collapses_on_a_single_screen`, `crates/core/tests/renderer_defaults.rs::screen_api_goes_top_then_bottom`, `crates/platform-pc/tests/render_e2e.rs::n3ds_second_screen_renders_reads_back_and_presents_stacked`, `xtask/tests/engine_checks.rs::engine_checks_pass_on_the_pc_3ds_profile`, `xtask/tests/engine_checks.rs::engine_checks_pass_on_pc` |
| `Renderer::screen_size` | `crates/core/tests/renderer_defaults.rs::screen_api_collapses_on_a_single_screen`, `crates/core/tests/renderer_defaults.rs::screen_api_goes_top_then_bottom`, `crates/platform-pc/tests/render_e2e.rs::n3ds_second_screen_renders_reads_back_and_presents_stacked` |
| `Renderer::read_pixel` | `crates/core/tests/renderer_defaults.rs::provided_renderer_methods_are_harmless_defaults`, `crates/platform-pc/tests/render_e2e.rs::baked_sprites_rects_and_text_draw_on_the_pc_renderer` |
| `Renderer::read_screen_pixel` | `crates/core/tests/renderer_defaults.rs::screen_api_collapses_on_a_single_screen`, `crates/platform-pc/tests/render_e2e.rs::n3ds_second_screen_renders_reads_back_and_presents_stacked`, `xtask/tests/engine_checks.rs::engine_checks_pass_on_the_pc_3ds_profile` |
| `Render params and handles (SpriteParams, TextParams, BillboardParams, ModelParams, *Id::from_path)` | `crates/core/tests/renderer_defaults.rs::param_defaults_and_constructors` |

### Platform trait

| Feature | Tests |
|---|---|
| `Platform::now_micros` | `crates/platform-pc/tests/platform_services.rs::clock_log_second_screen_and_display_requests` |
| `Platform::log` | `crates/platform-pc/tests/platform_services.rs::clock_log_second_screen_and_display_requests`, `crates/core/tests/game_loop.rs::games_reach_blobs_and_saves_through_update_with` |
| `Platform::blob` | `crates/core/tests/platform_save.rs::blobs_are_served_by_logical_handle`, `crates/platform-pc/tests/platform_services.rs::baked_blobs_are_served_by_handle` |
| `Platform::save_capacity` | `crates/core/tests/platform_save.rs::platform_defaults_report_no_services`, `crates/core/tests/platform_save.rs::saves_past_capacity_or_without_medium_fail`, `crates/platform-pc/tests/platform_services.rs::save_file_round_trip_framing_and_errors` |
| `Platform::save_read` | `crates/core/tests/platform_save.rs::save_round_trips_through_the_framed_medium`, `crates/core/tests/platform_save.rs::torn_and_foreign_media_read_back_as_no_save`, `crates/platform-pc/tests/platform_services.rs::save_file_round_trip_framing_and_errors` |
| `Platform::save_write` | `crates/core/tests/platform_save.rs::save_round_trips_through_the_framed_medium`, `crates/core/tests/platform_save.rs::saves_past_capacity_or_without_medium_fail`, `crates/platform-pc/tests/platform_services.rs::save_file_round_trip_framing_and_errors` |
| `Platform::save_begin` | `crates/platform-pc/tests/save_file.rs::write_read_and_limits`, `crates/core/tests/save_slots.rs::the_write_can_be_split_into_steps` |
| `Platform::save_step` | `crates/platform-pc/tests/save_file.rs::write_read_and_limits`, `crates/core/tests/save_slots.rs::the_write_can_be_split_into_steps` |
| Slotted saves (`save::plan_write`, `read_newest`, power cuts) | `crates/core/tests/save_slots.rs::a_power_cut_anywhere_keeps_the_previous_save`, `crates/core/tests/save_slots.rs::saves_alternate_slots_and_the_newest_wins`, `crates/core/tests/save_slots.rs::single_frame_saves_still_read_and_migrate_safely` |
| Save compression (`save::compress`/`decompress`) | `crates/core/tests/save_slots.rs::compression_makes_big_saves_fit_a_slot`, `crates/core/tests/save_slots.rs::incompressible_saves_go_wide_and_still_read_back` |
| PC save crash recovery (temp file) | `crates/platform-pc/tests/save_file.rs::a_crash_before_the_rename_keeps_the_new_save` |
| `Platform::second_screen` | `crates/core/tests/platform_save.rs::platform_defaults_report_no_services`, `crates/platform-pc/tests/platform_services.rs::clock_log_second_screen_and_display_requests` |
| `Platform::display` | `crates/core/tests/platform_save.rs::display_settings_default_is_auto_fullscreen`, `crates/platform-pc/tests/platform_services.rs::clock_log_second_screen_and_display_requests` |
| `Platform::set_display` | `crates/core/tests/platform_save.rs::platform_defaults_report_no_services`, `crates/platform-pc/tests/platform_services.rs::clock_log_second_screen_and_display_requests`, `crates/platform-pc/tests/platform_services.rs::display_settings_text_round_trip_and_repair` |
| `Platform::input_info` | `crates/core/tests/platform_save.rs::platform_defaults_report_no_services`, `crates/platform-pc/tests/platform_services.rs::rebinding_keys_through_the_platform` |
| `Platform::gamepad_name` | `crates/core/tests/platform_save.rs::platform_defaults_report_no_services`, `crates/platform-pc/tests/platform_services.rs::rebinding_keys_through_the_platform` |
| `Platform::binding_label` | `crates/core/tests/platform_save.rs::platform_defaults_report_no_services`, `crates/platform-pc/tests/platform_services.rs::rebinding_keys_through_the_platform`, `crates/platform-pc/tests/platform_services.rs::rebinding_a_pad_button` |
| `Platform::rebind` | `crates/core/tests/platform_save.rs::platform_defaults_report_no_services`, `crates/platform-pc/tests/platform_services.rs::rebinding_keys_through_the_platform`, `crates/platform-pc/tests/platform_services.rs::rebinding_a_pad_button` |
| `Platform::cancel_rebind` | `crates/core/tests/platform_save.rs::platform_defaults_report_no_services`, `crates/platform-pc/tests/platform_services.rs::rebinding_keys_through_the_platform` |
| `Platform::reset_bindings` | `crates/core/tests/platform_save.rs::platform_defaults_report_no_services`, `crates/platform-pc/tests/platform_services.rs::rebinding_keys_through_the_platform` |

### Audio trait

| Feature | Tests |
|---|---|
| `Audio::play_sound` | `crates/core/tests/audio.rs::provided_audio_methods_degrade_to_the_basics`, `crates/platform-pc/tests/input_audio.rs::audio_voices_music_and_stingers_bookkeeping`, `xtask/tests/engine_checks.rs::audio_checks_pass_on_pc` |
| `Audio::play_music` | `crates/platform-pc/tests/input_audio.rs::audio_voices_music_and_stingers_bookkeeping`, `crates/core/tests/audio.rs::voices_and_stingers_on_a_mixing_backend` |
| `Audio::stop_music` | `crates/platform-pc/tests/input_audio.rs::audio_voices_music_and_stingers_bookkeeping`, `crates/core/tests/audio.rs::voices_and_stingers_on_a_mixing_backend` |
| `Audio::set_master_volume` | `crates/platform-pc/tests/input_audio.rs::mixer_gains_ducking_looping_and_clamping`, `crates/platform-pc/tests/input_audio.rs::audio_voices_music_and_stingers_bookkeeping` |
| `Audio::play_sound_with` | `crates/core/tests/audio.rs::provided_audio_methods_degrade_to_the_basics`, `crates/platform-pc/tests/input_audio.rs::audio_voices_music_and_stingers_bookkeeping` |
| `Audio::play_voice` | `crates/core/tests/audio.rs::voices_and_stingers_on_a_mixing_backend`, `crates/platform-pc/tests/input_audio.rs::audio_voices_music_and_stingers_bookkeeping` |
| `Audio::set_voice` | `crates/core/tests/audio.rs::voices_and_stingers_on_a_mixing_backend`, `crates/platform-pc/tests/input_audio.rs::audio_voices_music_and_stingers_bookkeeping` |
| `Audio::stop_voice` | `crates/core/tests/audio.rs::voices_and_stingers_on_a_mixing_backend`, `crates/platform-pc/tests/input_audio.rs::audio_voices_music_and_stingers_bookkeeping` |
| `Audio::voice_playing` | `crates/core/tests/audio.rs::provided_audio_methods_degrade_to_the_basics`, `crates/platform-pc/tests/input_audio.rs::audio_voices_music_and_stingers_bookkeeping` |
| `Audio::play_stinger` | `crates/core/tests/audio.rs::voices_and_stingers_on_a_mixing_backend`, `crates/platform-pc/tests/input_audio.rs::audio_voices_music_and_stingers_bookkeeping` |
| `Audio::stinger_playing` | `crates/core/tests/audio.rs::provided_audio_methods_degrade_to_the_basics`, `crates/platform-pc/tests/input_audio.rs::audio_voices_music_and_stingers_bookkeeping` |
| `Audio::set_music_volume` | `crates/core/tests/audio.rs::provided_audio_methods_degrade_to_the_basics`, `crates/platform-pc/tests/input_audio.rs::mixer_gains_ducking_looping_and_clamping` |
| `PC mixer (gains, pan, ducking, looping, clamping)` | `crates/platform-pc/tests/input_audio.rs::mixer_gains_ducking_looping_and_clamping` |

### Engine services

| Feature | Tests |
|---|---|
| `Bitmap text: TFNT parse and errors` | `crates/core/tests/text.rs::tfnt_parses_and_looks_up_glyphs`, `crates/core/tests/text.rs::tfnt_rejects_bad_tables` |
| `Bitmap text: layout, alignment, scale, fallback` | `crates/core/tests/text.rs::draw_text_lays_out_lines_glyphs_and_alignment`, `crates/core/tests/text.rs::draw_text_scales_tints_offsets_and_falls_back`, `crates/core/tests/text.rs::fonts_without_a_fallback_skip_unknown_chars` |
| `TextBuf` | `crates/core/tests/text.rs::textbuf_formats_without_an_allocator_and_truncates_on_char_boundaries` |
| `Save framing (header, checksum, torn and foreign media)` | `crates/core/tests/platform_save.rs::save_round_trips_through_the_framed_medium`, `crates/core/tests/platform_save.rs::torn_and_foreign_media_read_back_as_no_save`, `crates/core/tests/platform_save.rs::checksum_is_fnv1a_like_asset_handles`, `crates/platform-pc/tests/platform_services.rs::save_file_round_trip_framing_and_errors` |
| `Baked scenes: BakedScene (TSCN) parse` | `crates/core/tests/scene_nodes.rs::baked_scene_blob_reads_every_field`, `crates/core/tests/scene_nodes.rs::baked_scene_rejects_bad_blobs`, `crates/core/tests/scene_nodes.rs::baked_scene_plays_like_exported_nodes` |
| `Scene nodes: find, with_tag, solids` | `crates/core/tests/scene_nodes.rs::exported_nodes_query_by_name_tag_and_collider` |
| `Scene nodes: draw_nodes and draw_nodes_in` | `crates/core/tests/scene_nodes.rs::draw_nodes_draws_model_nodes_with_lod_and_animation`, `crates/core/tests/scene_nodes.rs::draw_nodes_in_honors_the_cell_mask` |
| `Distance LOD` | `crates/core/tests/scene_nodes.rs::lod_levels_switch_models_with_distance`, `examples/showcase/src/lib.rs::far_trees_switch_to_cheaper_lods`, `crates/core/tests/impostors.rs::a_lod_chain_switches_at_the_same_distances_every_time`, `crates/core/tests/impostors.rs::packed_lod_records_carry_the_impostor` |
| `Impostors: view by yaw, 2 triangles, TIMP tables` | `crates/core/tests/impostors.rs::the_view_follows_the_camera_yaw_around_the_object`, `crates/core/tests/impostors.rs::an_impostor_costs_one_billboard_of_two_triangles`, `crates/core/tests/impostors.rs::timp_tables_read_back_and_bad_ones_are_refused`, `xtask/tests/engine_checks.rs::engine_checks_pass_on_pc` |
| `Collision: Aabb, colliders, move_aabb, raycast` | `crates/core/tests/physics.rs::aabb_operations`, `crates/core/tests/physics.rs::collider_shapes_become_world_boxes`, `crates/core/tests/physics.rs::move_aabb_lands_slides_and_bonks`, `crates/core/tests/physics.rs::raycast_finds_the_nearest_face_and_its_normal`, `crates/core/tests/physics.rs::raycasts_along_z_and_single_row_tilemaps` |
| `Tilemap and move_and_collide` | `crates/core/tests/physics.rs::tilemap_parses_ascii_levels`, `crates/core/tests/physics.rs::move_and_collide_runs_a_platformer_step_loop` |
| `Runtime entities: EntityStore` | `crates/core/tests/entities.rs::handles_are_generational_and_capacity_is_fixed`, `crates/core/tests/entities.rs::entities_move_query_and_collide`, `crates/core/tests/entities.rs::draw_applies_lod_animation_visibility_and_cells`, `crates/core/tests/entities.rs::a_baked_scene_spawns_into_the_store` |
| `Rigid animation: Mesh::pose` | `crates/core/tests/tmdl_mesh.rs::skel_and_anim_sections_pose_a_rigid_skeleton`, `crates/core/tests/tmdl_mesh.rs::hierarchy_composes_parent_before_child_with_inverse_bind`, `crates/core/tests/tessellate.rs::skinned_meshes_follow_their_pose` |
| `Clip crossfade: AnimRef::blend_to` | `crates/core/tests/tmdl_mesh.rs::blend_to_crossfades_two_clips_per_joint`, `crates/asset-pipeline/tests/animation_metrics.rs::a_crossfade_half_way_is_the_interpolated_pose` |
| `Posed vertices (Mesh::posed_position, posed_normal)` | `crates/core/tests/tmdl_mesh.rs::posed_vertices_follow_their_joint` |
| `Billboards` | `crates/core/tests/tessellate.rs::billboards_face_the_camera_and_respect_fog_and_regions`, `crates/platform-pc/tests/render_e2e.rs::baked_models_light_fog_depth_textures_and_billboards` |
| `Particles: ParticleSystem` | `crates/core/tests/particles.rs::same_seed_same_particles_different_seed_different_ones`, `crates/core/tests/particles.rs::rate_lifetime_and_motion`, `crates/core/tests/particles.rs::draw_emits_one_billboard_per_particle_with_fading_and_flipbook`, `crates/core/tests/particles.rs::sideways_jets_spawn_inside_their_cone` |
| `Particle effects: TPFX, platform caps, off per platform, determinism` | `crates/core/tests/particle_effects.rs::a_platform_cap_holds_against_bursts_and_emission`, `crates/core/tests/particle_effects.rs::an_effect_off_on_a_platform_spawns_nothing`, `crates/core/tests/particle_effects.rs::the_same_seed_plays_the_same_particles`, `crates/core/tests/particle_effects.rs::tpfx_blobs_round_trip`, `xtask/tests/engine_checks.rs::engine_checks_pass_on_pc` |
| `Positional audio: Listener and spatialize` | `crates/core/tests/audio.rs::spatialize_falls_off_quadratically_and_pans_by_side`, `crates/core/tests/audio.rs::gains_split_volume_by_pan` |
| `Positional audio: Ambience and scene sounds` | `crates/core/tests/audio.rs::ambience_keeps_the_nearest_sources_voiced_as_the_listener_walks`, `crates/core/tests/audio.rs::ambience_survives_a_mixer_without_free_voices`, `crates/core/tests/entities.rs::sound_sources_drive_ambience`, `examples/showcase/src/lib.rs::audio_is_positional_and_the_last_coin_plays_the_fanfare` |
| `Portal/cell visibility` | `crates/core/tests/vis.rs::cells_and_portals_come_from_scene_nodes`, `crates/core/tests/vis.rs::looking_down_the_doors_draws_every_room_looking_away_only_ours`, `crates/core/tests/vis.rs::hidden_rooms_never_reach_the_renderer`, `crates/core/tests/vis.rs::baked_cells_match_exported_ones`, `crates/core/tests/vis.rs::capacity_limits_and_conservative_fallbacks` |
| `Software T&L: culling, back faces, depth` | `crates/core/tests/tessellate.rs::a_cube_face_on_emits_its_front_face_only`, `crates/core/tests/tessellate.rs::meshes_outside_the_frustum_are_culled_whole`, `crates/core/tests/tessellate.rs::double_sided_draws_keep_back_faces`, `crates/core/tests/tessellate.rs::painter_keys_order_far_first_then_submission` |
| `Software T&L: shading, tint, emissive, colored light, fog` | `crates/core/tests/tessellate.rs::shading_modes_tint_and_emissive`, `crates/core/tests/tessellate.rs::colored_light_tints_lit_surfaces`, `crates/core/tests/tessellate.rs::fog_blends_toward_the_fog_color_with_view_depth` |
| `Software T&L: near and guard-band clipping, depth splits` | `crates/core/tests/tessellate.rs::geometry_crossing_the_near_plane_is_clipped_into_the_guard_band` |
| `Software T&L: textured draws` | `crates/core/tests/tessellate.rs::textured_draws_carry_uvs_and_view_depth`, `crates/platform-pc/tests/render_e2e.rs::baked_models_light_fog_depth_textures_and_billboards` |
| `Software T&L: color slots and palette (ModelParams::palette)` | `crates/core/tests/color_slots.rs::a_palette_multiplies_slotted_vertices_before_tint_and_light`, `crates/core/tests/color_slots.rs::without_a_palette_or_slots_draws_are_unchanged`, `crates/core/tests/color_slots.rs::unlit_vertices_show_their_palette_entry_exactly`, `crates/core/tests/color_slots.rs::palette_draws_stay_off_the_rsp_and_out_of_the_memo`, `crates/platform-pc/tests/render_e2e.rs::a_palette_recolors_slot_regions_per_draw`, `xtask/tests/engine_checks.rs::engine_checks_pass_on_pc` |
| `Local lights: point and spot, per-vertex tier (TnlContext::with_local_lights)` | `crates/core/tests/local_lights.rs::no_lights_and_lights_out_of_reach_change_nothing`, `crates/core/tests/local_lights.rs::a_point_light_adds_its_color_to_the_faces_it_sees`, `crates/core/tests/local_lights.rs::faces_turned_away_from_the_light_stay_dark`, `crates/core/tests/local_lights.rs::a_spot_lights_only_inside_its_cone`, `crates/core/tests/local_lights.rs::unlit_draws_ignore_local_lights_and_flat_draws_get_them`, `crates/core/src/light.rs::the_list_keeps_the_first_lights_up_to_the_cap`, `crates/core/src/light.rs::falloff_is_smooth_and_zero_past_the_radius`, `crates/core/src/light.rs::spot_cone_fades_between_the_angles` |
| `Local lights on the N64: lit draws leave the RSP (RspReject::LocalLight)` | `crates/core/tests/local_lights.rs::the_rsp_refuses_lit_draws_and_keeps_the_rest`, `xtask/tests/perf_budget.rs::castle64_tnl_work_matches_the_baseline`, `xtask/tests/engine_checks.rs::engine_checks_pass_on_pc` |
| `Local lights: per-pixel tier, gloss and wetness (tessellate_surface, PC lit pipeline)` | `crates/core/tests/local_lights.rs::the_surface_path_emits_the_same_triangles_plus_each_vertex_surface`, `crates/platform-pc/tests/local_lights.rs::per_pixel_lights_are_round_on_a_coarse_mesh`, `crates/platform-pc/tests/local_lights.rs::gloss_and_wetness_add_highlights_per_pixel_only`, `crates/platform-pc/tests/local_lights.rs::each_tier_matches_its_golden`, `crates/platform-pc/src/renderer/lights.rs::layout_matches_the_shader`, `crates/platform-pc/src/renderer/lights.rs::same_state_shares_a_set_and_the_table_caps`, `crates/platform-pc/src/renderer/batch.rs::tri_vertex_layout_matches_the_pipelines` |
| `Light pools on the floor (ground_pool)` | `crates/core/src/light.rs::a_point_light_pools_under_itself`, `crates/core/src/light.rs::a_spot_pools_where_its_axis_meets_the_floor` |
| `Material gloss (material files, codegen)` | `crates/scene/src/material.rs::gloss_round_trips_and_defaults_to_matte` |
| `Texture filter per draw (ModelParams::filter: point, bilinear)` | `crates/platform-pc/tests/texture_filter.rs::point_keeps_texels_and_bilinear_blends_them_in_every_look`, `crates/platform-pc/tests/texture_filter.rs::three_cards_match_the_golden_per_profile`, `xtask/tests/engine_checks.rs::engine_checks_pass_on_pc` |
| `Detail texture layer (ModelParams::detail, N64 TILE1 2-cycle, 3DS TexEnv stage 1)` | `crates/platform-pc/tests/texture_filter.rs::the_detail_layer_multiplies_the_texture_on_every_path`, `crates/platform-pc/tests/texture_filter.rs::strict_mode_rejects_a_detail_pair_over_the_n64_tmem`, `crates/platform-pc/tests/texture_filter.rs::three_cards_match_the_golden_per_profile`, `crates/scene/src/material.rs::filter_and_detail_round_trip_and_map_onto_the_core`, `crates/editor/src/widget_tests.rs::material_editor_sets_shading_sides_and_texture`, `xtask/tests/engine_checks.rs::engine_checks_pass_on_pc`, `xtask/tests/engine_checks.rs::baked_showcase_matches_code_and_plays` |
| `TSCN v3: surface (gloss, filter, detail) per node` | `crates/core/src/scene.rs::baked_scene_reads_every_field`, `crates/core/tests/scene_nodes.rs::baked_scene_rejects_bad_blobs`, `xtask/tests/engine_checks.rs::baked_showcase_matches_code_and_plays` |
| `Software T&L: TnlContext and TnlScratch reuse` | `crates/core/tests/tessellate.rs::tessellate_with_reuses_context_and_scratch_bit_identically` |
| `Draw command memo (trino_core::memo)` | `crates/core/tests/memo.rs::a_repeated_draw_hits_and_anything_it_depends_on_misses`, `crates/core/tests/memo.rs::only_plain_draws_are_memoized`, `crates/core/tests/memo.rs::a_full_memo_replaces_and_an_empty_one_never_hits` |
| `RSP eligibility from a given view·model (rsp_eligible_mv)` | `crates/core/tests/tessellate.rs::rsp_eligibility_with_a_given_view_model_product_matches` |
| `RSP rule per draw (rsp_check, RspReject: which rule keeps a draw off the fast path)` | `crates/core/tests/tessellate.rs::rsp_check_names_the_rule_each_draw_fails` |
| `Saturating float→int (math3d::to_i32)` | `crates/core/tests/math.rs::to_i32_matches_the_saturating_cast_everywhere` |
| `RSP eligibility and split policy` | `crates/core/tests/tessellate.rs::split_policy_and_rsp_eligibility_follow_the_draw`, `crates/core/tests/tessellate.rs::degenerate_flat_skinned_and_multi_segment_meshes` |
| `3DS GPU vertex T&L (render3d::gpu_check, tnl3d.v.pica)` | `crates/core/tests/gpu_tnl.rs::the_shader_constants_reproduce_the_cpu_path`, `crates/core/tests/gpu_tnl.rs::the_gpu_path_refuses_what_needs_the_cpu`, `xtask/tests/perf_budget.rs::castle64_3ds_gpu_split_matches_the_baseline` |
| `3DS GPU T&L fog (tnl3d.v.pica fog row, GpuDraw::fog)` | `crates/core/tests/gpu_tnl.rs::the_shader_constants_reproduce_the_cpu_path` (fog 5-9 m and a saturated fog, colored light), `crates/core/tests/gpu_tnl.rs::the_gpu_path_refuses_what_needs_the_cpu`; Azahar: the `gpu tnl showcase-fog` self-test step (the showcase's fogged first frame, GPU vs CPU) |
| `3DS off-GPU reasons (GpuReject histogram, TRINO_FRAME off_gpu=, scene budget 3DS split)` | `crates/core/src/render3d/hw.rs::every_reason_has_its_own_histogram_slot`, `xtask/tests/scene_budget.rs::the_rsp_split_names_each_rule_and_follows_the_scene`, `crates/editor/src/budget.rs::platform_lines_name_the_rsp_reason_and_unseen_draws` |
| `PC frame breakdown (PcRenderer::frame_times)` | n/a: wall-clock times on the host's GPU; printed by external projects' frame-time tests (the counters are covered by their frame runs) |
| `Text glyph index (text::GlyphIndex)` | `crates/core/tests/text.rs::the_glyph_index_lays_out_exactly_like_the_table_search` |
| `Z-buffered occlusion` | `crates/platform-pc/tests/render_e2e.rs::baked_models_light_fog_depth_textures_and_billboards`, `xtask/tests/engine_checks.rs::engine_checks_pass_on_pc` |
| `Caps and strict mode` | `crates/core/tests/perf_caps.rs::caps_validate_textures_per_console`, `crates/platform-pc/tests/render_e2e.rs::n64_look_and_strict_mode_budgets`, `crates/platform-pc/tests/golden.rs::strict_mode_rejects_texture_over_budget`, `crates/platform-pc/tests/golden.rs::strict_mode_rejects_3d_over_the_n64_time_ceiling` |
| `Perf: N64 cost model and frame profile` | `crates/core/tests/perf_caps.rs::n64_cost_model_prices_real_tnl_work`, `crates/core/tests/perf_caps.rs::render_profile_accumulates_and_reports_per_frame`, `crates/core/tests/perf_caps.rs::models_past_the_tracked_limit_pool_into_other`, `crates/core/tests/perf_caps.rs::report_write_errors_propagate`, `xtask/tests/perf_budget.rs::castle64_tnl_work_matches_the_baseline` |
| `Game trait and update_with` | `crates/core/tests/game_loop.rs::default_update_with_forwards_to_update_and_resize_is_a_no_op`, `crates/core/tests/game_loop.rs::games_reach_blobs_and_saves_through_update_with` |
| `InputState (buttons, keys, pointer, pointer_on touch routing)` | `crates/core/tests/game_loop.rs::input_state_edges_for_buttons_keys_and_pointer`, `crates/core/src/input.rs::pointer_on_reports_the_pointer_only_on_its_screen`, `examples/castle64/src/lib.rs::bottom_screen_buttons_turn_the_camera_and_go_to_the_hub`, `examples/showcase/src/lib.rs::panel_taps_and_pad_buttons_toggle_the_look` |
| `Key / KeySet (128-bit key set, every key its own bit, u32-era compatibility)` | `crates/core/src/input.rs::every_key_is_its_own_bit`, `crates/core/src/input.rs::the_first_keys_keep_their_old_bits`, `crates/core/src/input.rs::key_set_operators_keep_u32_era_code_working` |
| `Buttons L2/R2/L3/R3, right_stick, triggers` | `crates/core/src/input.rs::new_buttons_sticks_and_triggers`, `crates/platform-pc/tests/input_audio.rs::gamepad_readings_map_to_buttons_sticks_and_triggers`, `crates/platform-n64/src/pad.rs::c_buttons_are_x_y_and_the_right_stick`, `crates/platform-3ds/src/pad.rs::zl_zr_and_the_c_stick` |
| `Console input shims at idle (N64 C bits, 3DS ZL/ZR + C-stick)` | `xtask/tests/engine_checks.rs::idle_input_checks_pass_on_pc` (the `input` step of `cargo xtask test n64/3ds`) |
| `Math: vectors, colors, Mat34, Quat, software trig` | `crates/core/tests/math.rs::software_trig_and_sqrt_stay_within_their_documented_error`, `crates/core/tests/math.rs::vectors_rects_and_colors`, `crates/core/tests/math.rs::matrices_compose_transform_and_look`, `crates/core/tests/math.rs::quaternions_interpolate_and_build_trs_matrices` |
| `Hot-reload ABI: export_game!` | `crates/game-api/tests/export_game.rs::exports_handshake_and_forward_to_the_game`, `xtask/tests/hot_reload.rs::state_survives_dylib_swap_and_behavior_changes` |
| `Console self-test steps on PC` | `xtask/tests/engine_checks.rs::engine_checks_pass_on_pc`, `xtask/tests/engine_checks.rs::engine_checks_pass_on_the_pc_3ds_profile`, `xtask/tests/engine_checks.rs::audio_checks_pass_on_pc`, `xtask/tests/engine_checks.rs::baked_showcase_matches_code_and_plays` |
| `Two screens: Screen, 3D and 2D per screen, touch routing (dual screen step)` | `xtask/tests/engine_checks.rs::engine_checks_pass_on_the_pc_3ds_profile`, `xtask/tests/engine_checks.rs::engine_checks_pass_on_pc`, `examples/castle64/src/lib.rs::bottom_screen_panel_draws_only_with_a_second_screen`, `xtask/tests/n3ds_parity.rs::pc_3ds_bottom_screen_tracks_the_azahar_frame` |
| `Showcase touch panel: metrics and look toggles on the bottom screen (showcase panel step)` | `examples/showcase/src/lib.rs::panel_buttons_hit_inside_and_miss_just_outside`, `examples/showcase/src/lib.rs::panel_toggles_cycle_back_to_the_default`, `examples/showcase/src/lib.rs::panel_taps_and_pad_buttons_toggle_the_look`, `examples/showcase/src/lib.rs::panel_options_change_the_clock_and_the_camera`, `examples/showcase/src/lib.rs::panel_draws_only_with_a_second_screen`, `examples/showcase/src/lib.rs::panel_options_reach_the_renderer`, `examples/showcase/src/lib.rs::metric_lines_have_a_fixed_width`, `xtask/tests/showcase_panel.rs::showcase_panel_is_probed_on_the_pc_3ds_profile`, `xtask/tests/showcase_panel.rs::showcase_panel_collapses_on_the_n64_profile` |

### TMDL sections

| Feature | Tests |
|---|---|
| `TMDL base` | `crates/core/tests/tmdl_mesh.rs::base_layout_vertices_normals_colors_indices`, `crates/core/tests/tmdl_mesh.rs::every_mesh_error`, `crates/core/tests/tmdl_mesh.rs::layout_rebuilds_an_identical_mesh_without_reparsing` |
| `TMDL BNDS` | `crates/core/tests/tmdl_mesh.rs::bnds_section_carries_the_bounding_sphere`, `crates/core/tests/tessellate.rs::meshes_outside_the_frustum_are_culled_whole` |
| `TMDL SKEL` | `crates/core/tests/tmdl_mesh.rs::skel_and_anim_sections_pose_a_rigid_skeleton`, `crates/asset-pipeline/tests/animated_model.rs::node_animation_bakes_to_a_rigid_skeleton` |
| `TMDL ANIM` | `crates/core/tests/tmdl_mesh.rs::skel_and_anim_sections_pose_a_rigid_skeleton`, `crates/core/tests/tmdl_mesh.rs::single_frame_clips_and_empty_meshes`, `crates/asset-pipeline/tests/animated_model.rs::clips_start_at_their_first_keyframe` |
| `TMDL TXUV` | `crates/core/tests/tmdl_mesh.rs::txuv_and_txid_sections_make_a_textured_mesh`, `crates/core/tests/tessellate.rs::textured_draws_carry_uvs_and_view_depth` |
| `TMDL TXID` | `crates/core/tests/tmdl_mesh.rs::txuv_and_txid_sections_make_a_textured_mesh`, `crates/platform-pc/tests/render_e2e.rs::baked_models_light_fog_depth_textures_and_billboards` |
| `TMDL SKW2` | `crates/core/tests/tmdl_mesh.rs::skw2_section_adds_a_second_joint_weight`, `crates/asset-pipeline/tests/animated_model.rs::smooth_skinning_keeps_two_weights_only_when_asked` |
| `TMDL SLOT` | `crates/core/tests/color_slots.rs::slot_sections_parse_validate_and_survive_the_layout`, `crates/asset-pipeline/tests/color_slots.rs::slot_attribute_bakes_per_vertex_through_reordering`, `crates/asset-pipeline/tests/model_pipeline.rs::mesh_data_encodes_every_tmdl_section_and_rejects_bad_layouts` |
| `TMDL unknown sections are skipped` | `crates/core/tests/tmdl_mesh.rs::unknown_sections_are_skipped_and_later_ones_still_read` |

### Asset kinds (manifest → bake → load)

| Feature | Tests |
|---|---|
| `asset sprites` | `crates/asset-pipeline/tests/asset_kinds.rs::every_asset_kind_bakes_indexes_and_loads_on_pc`, `crates/asset-pipeline/tests/asset_kinds.rs::console_bakes_record_and_validate_each_platforms_format`, `crates/asset-pipeline/tests/pipeline.rs::platform_override_wins_and_reload_keeps_handle`, `crates/platform-pc/tests/render_e2e.rs::baked_sprites_rects_and_text_draw_on_the_pc_renderer` |
| `asset sounds` | `crates/asset-pipeline/tests/asset_kinds.rs::every_asset_kind_bakes_indexes_and_loads_on_pc`, `crates/asset-pipeline/tests/pipeline.rs::loader_round_trips_baked_data`, `crates/runner-pc/tests/runner.rs::bake_upload_and_draw_like_the_runner` |
| `asset music` | `crates/asset-pipeline/tests/asset_kinds.rs::every_asset_kind_bakes_indexes_and_loads_on_pc`, `crates/runner-pc/tests/runner.rs::bake_upload_and_draw_like_the_runner` |
| `asset models` | `crates/asset-pipeline/tests/asset_kinds.rs::every_asset_kind_bakes_indexes_and_loads_on_pc`, `crates/asset-pipeline/tests/model_pipeline.rs::the_cube_master_bakes_and_tessellates_its_visible_faces`, `crates/platform-pc/tests/render_e2e.rs::baked_models_light_fog_depth_textures_and_billboards` |
| `asset fonts` | `crates/asset-pipeline/tests/fonts.rs::ttf_masters_rasterize_to_an_atlas_and_a_glyph_table`, `crates/asset-pipeline/tests/fonts.rs::png_grids_trim_proportional_glyphs_and_keep_monospaced_cells`, `crates/platform-pc/tests/render_e2e.rs::baked_sprites_rects_and_text_draw_on_the_pc_renderer` |
| `asset blobs` | `crates/asset-pipeline/tests/asset_kinds.rs::every_asset_kind_bakes_indexes_and_loads_on_pc`, `crates/platform-pc/tests/platform_services.rs::baked_blobs_are_served_by_handle` |
| `asset particles` | `crates/asset-pipeline/tests/particles.rs::each_platform_bakes_its_own_cap`, `crates/asset-pipeline/tests/particles.rs::effects_round_trip_through_ron_both_ways`, `crates/asset-pipeline/tests/particles.rs::bad_effects_are_refused_with_the_field`, `crates/asset-pipeline/tests/particles.rs::the_simulation_is_deterministic_and_keeps_each_cap` |
| `asset scenes` | `crates/asset-pipeline/tests/scene_runtime.rs::the_showcase_scene_loads_from_its_blob_and_draws_every_model`, `crates/asset-pipeline/tests/asset_kinds.rs::scene_bakes_fail_on_library_errors_and_scene_warnings`, `xtask/tests/engine_checks.rs::baked_showcase_matches_code_and_plays` |
| `Asset materials (assets/materials/*.material.ron)` | `crates/scene/tests/authoring.rs::materials_parse_save_and_become_model_params`, `crates/scene/tests/library_resolve.rs::resolve_flattens_every_component` |

### Asset pipeline

| Feature | Tests |
|---|---|
| `Bake index snapshot and incremental rebakes` | `crates/asset-pipeline/tests/pipeline.rs::bake_produces_deterministic_index_snapshot`, `crates/asset-pipeline/tests/watch.rs::saving_a_master_or_a_scene_rebakes_only_what_changed` |
| `Console texture formats and platform overrides` | `crates/asset-pipeline/tests/asset_kinds.rs::console_bakes_record_and_validate_each_platforms_format`, `crates/asset-pipeline/tests/pipeline.rs::n64_bake_fails_without_explicit_format`, `crates/asset-pipeline/tests/pipeline.rs::platform_override_wins_and_reload_keeps_handle` |
| `Bake errors: every error at once, missing sources, collisions, output dir` | `crates/asset-pipeline/tests/asset_kinds.rs::a_broken_project_reports_every_error_at_once`, `crates/asset-pipeline/tests/pipeline.rs::missing_source_is_a_bake_error`, `crates/asset-pipeline/tests/asset_kinds.rs::colliding_handles_fail_the_bake_for_every_kind`, `crates/asset-pipeline/tests/asset_kinds.rs::an_unwritable_output_directory_is_a_bake_error` |
| `Per-platform models (platforms = [...])` | `crates/asset-pipeline/tests/asset_kinds.rs::a_model_bakes_only_for_the_platforms_it_names` |
| `Manifest validation` | `crates/asset-pipeline/tests/asset_kinds.rs::manifests_reject_unknown_fields_versions_and_missing_files` |
| `Manifest edits (add, retarget, same handle)` | `crates/asset-pipeline/tests/asset_kinds.rs::manifest_edits_keep_comments_and_retarget_a_model_under_one_handle` |
| `Loader: corrupt baked output is an error` | `crates/asset-pipeline/tests/asset_kinds.rs::the_loader_rejects_corrupt_baked_output` |
| `glTF → TMDL → Mesh → tessellate` | `crates/asset-pipeline/tests/model_pipeline.rs::the_cube_master_bakes_and_tessellates_its_visible_faces`, `crates/asset-pipeline/tests/model_pipeline.rs::mesh_data_encodes_every_tmdl_section_and_rejects_bad_layouts` |
| `glTF vertex colors and texture sampling` | `crates/asset-pipeline/tests/model_pipeline.rs::vertex_colors_come_from_color0_then_texture_then_factor`, `crates/asset-pipeline/tests/model_pipeline.rs::texture_color_types_are_sampled_into_vertex_colors` |
| `glTF errors and unsupported primitives` | `crates/asset-pipeline/tests/model_pipeline.rs::line_primitives_are_skipped_and_bad_files_fail_with_a_reason` |
| `glTF animation → pose (channels, interpolation, once/loop)` | `crates/asset-pipeline/tests/model_pipeline.rs::every_channel_and_interpolation_poses_its_joint`, `crates/asset-pipeline/tests/model_pipeline.rs::rest_rotations_round_trip_for_half_turns_on_every_axis`, `crates/asset-pipeline/tests/animated_model.rs::node_animation_bakes_to_a_rigid_skeleton`, `crates/asset-pipeline/tests/animated_model.rs::static_gltf_stays_skeleton_free` |
| `glTF skins (rigid and smooth)` | `crates/asset-pipeline/tests/model_pipeline.rs::skinned_files_give_unskinned_parts_a_static_joint`, `crates/asset-pipeline/tests/animated_model.rs::smooth_skinning_keeps_two_weights_only_when_asked` |
| `glTF color slots (_SLOT)` | `crates/asset-pipeline/tests/color_slots.rs::slot_attribute_bakes_per_vertex_through_reordering`, `crates/asset-pipeline/tests/color_slots.rs::files_without_slots_bake_as_before`, `crates/asset-pipeline/tests/color_slots.rs::bad_slot_attributes_fail_with_a_reason`, `crates/asset-pipeline/tests/color_slots.rs::a_palette_recolors_the_baked_slots`, `xtask/tests/model_inspect.rs::color_slots_are_counted_per_slot` |
| `Model bounds for colliders` | `crates/asset-pipeline/tests/model_pipeline.rs::manifest_model_bounds_cover_every_bakeable_model` |
| `Fake objects (.prim.ron)` | `crates/asset-pipeline/tests/fake_objects.rs::every_shape_bakes_from_its_ron_file_at_its_declared_size`, `crates/asset-pipeline/tests/fake_objects.rs::segments_smoothing_and_colors_shape_the_mesh`, `crates/asset-pipeline/tests/fake_objects.rs::invalid_dimensions_are_rejected_with_the_field_name` |
| `Font bake: TTF and PNG grid` | `crates/asset-pipeline/tests/fonts.rs::ttf_masters_rasterize_to_an_atlas_and_a_glyph_table`, `crates/asset-pipeline/tests/fonts.rs::ttf_errors_name_the_problem`, `crates/asset-pipeline/tests/fonts.rs::png_grids_trim_proportional_glyphs_and_keep_monospaced_cells` |
| `Baked scene → BakedScene → draw_nodes with real meshes` | `crates/asset-pipeline/tests/scene_runtime.rs::the_showcase_scene_loads_from_its_blob_and_draws_every_model`, `crates/asset-pipeline/tests/scene_runtime.rs::the_interior_scene_culls_rooms_through_its_portals` |
| `Impostor bake (atlas, N64 CI4 within TMEM, TIMP, manifest, staleness)` | `crates/asset-pipeline/tests/impostors.rs::bake_writes_atlases_tables_and_manifest_entries`, `crates/asset-pipeline/tests/impostors.rs::baked_impostors_reach_the_bake_and_scene_lod_levels`, `crates/asset-pipeline/tests/impostors.rs::bad_settings_and_missing_models_fail_with_a_reason` |
| `Animation measurements (stride, distance per cycle, foot slide, below the floor, bounds)` | `crates/asset-pipeline/tests/animation_metrics.rs::a_walk_cycle_reports_its_stride_speed_and_distance`, `crates/asset-pipeline/tests/animation_metrics.rs::a_sliding_foot_and_a_foot_through_the_floor_warn` |
| `Console pose: a clip's pose baked into a static model` | `crates/asset-pipeline/tests/animation_metrics.rs::a_pose_bakes_into_a_static_mesh_with_the_same_triangles`, `crates/asset-pipeline/tests/animation_metrics.rs::export_pose_writes_a_model_and_its_manifest_entry` |
| `Live reload: watch` | `crates/asset-pipeline/tests/watch.rs::saving_a_master_or_a_scene_rebakes_only_what_changed`, `xtask/tests/hot_reload.rs::state_survives_dylib_swap_and_behavior_changes` |

### Authored content (scenes, prefabs, materials)

| Feature | Tests |
|---|---|
| `Scenes: RON files, defaults, every component` | `crates/scene/tests/authoring.rs::scenes_round_trip_through_files_with_every_component` |
| `Scenes: v1 migration and version errors` | `crates/scene/tests/authoring.rs::old_new_and_broken_scene_files_get_clear_answers` |
| `Scenes: entity tree edits` | `crates/scene/tests/authoring.rs::entity_tree_edits_keep_paths_consistent` |
| `Scenes: environment and transforms` | `crates/scene/tests/authoring.rs::transforms_and_environment_map_onto_the_core_types` |
| `Materials: parse, save, ModelParams` | `crates/scene/tests/authoring.rs::materials_parse_save_and_become_model_params` |
| `Prefabs: files and overrides` | `crates/scene/tests/authoring.rs::prefab_files_and_overrides` |
| `Prefabs: make, apply overrides, unpack, nesting` | `crates/scene/tests/library_resolve.rs::prefabs_can_be_made_applied_unpacked_and_nested` |
| `Prefabs: cycle detection and missing references` | `crates/scene/tests/library_resolve.rs::broken_scenes_resolve_with_warnings_instead_of_failing` |
| `Library: load and hot reload` | `crates/scene/tests/library_resolve.rs::the_library_loads_and_hot_reloads_project_files` |
| `Resolve: draw list with every component` | `crates/scene/tests/library_resolve.rs::resolve_flattens_every_component`, `crates/scene/tests/library_resolve.rs::sprites_accumulate_their_parents_2d_transform` |
| `Scene codegen (no_std Rust)` | `crates/scene/tests/export.rs::generated_code_lists_every_exported_node_and_component`, `xtask/tests/scene_codegen.rs::showcase_codegen_matches_the_scene`, `xtask/tests/showcase_render.rs::runtime_scene_and_exported_code_render_identically` |
| `TSCN bake ↔ BakedScene` | `crates/scene/tests/export.rs::tscn_blobs_carry_everything_resolve_computed`, `crates/scene/tests/export.rs::corrupt_tscn_blobs_are_rejected_up_front` |
| `LOD chain: impostor levels, distances per platform, TSCN and codegen` | `crates/scene/tests/lod_chain.rs::the_chain_round_trips_through_ron`, `crates/scene/tests/lod_chain.rs::each_platform_gets_its_own_chain`, `crates/scene/tests/lod_chain.rs::exports_carry_the_impostor_and_a_missing_one_warns` |
| `Platform variants: RON both ways, v2 → v3 scene and v1 → v2 prefab migration` | `crates/scene/tests/platform_variants.rs::variants_round_trip_through_ron_both_ways`, `crates/scene/tests/platform_variants.rs::v2_scenes_and_v1_prefabs_migrate` |
| `Platform variants: resolve_for (model, material, scale, hidden, left out; colliders stay)` | `crates/scene/tests/platform_variants.rs::each_platform_resolves_its_own_variant`, `crates/scene/tests/platform_variants.rs::prefab_parts_and_instances_take_variants` |
| `Platform variants: TSCN bake and codegen per platform` | `crates/scene/tests/platform_variants.rs::exports_pick_the_variant_of_their_platform`, `crates/asset-pipeline/tests/scene_runtime.rs::each_platform_bakes_its_own_scene_variant`, `xtask/tests/scene_cli.rs::scene_export_applies_the_platform_variants`, `xtask/src/scene.rs::check_flags_models_a_platform_does_not_bake` |
| `Per-platform frame budget (trino_scene::budget: T&L work, N64 CPU/RSP split and reasons, variants, LOD, impostors, unseen draws)` | `xtask/tests/scene_budget.rs::the_estimate_matches_the_pc_renderers_counters_on_the_n64_profile`, `xtask/tests/scene_budget.rs::a_mesh_behind_the_camera_is_sent_for_nothing`, `xtask/tests/scene_budget.rs::the_rsp_split_names_each_rule_and_follows_the_scene`, `xtask/tests/scene_budget.rs::variants_lod_and_impostors_change_each_platforms_frame` |

### PC backend and runner

| Feature | Tests |
|---|---|
| `manifest → bake → load → draw on the PC renderer` | `crates/platform-pc/tests/render_e2e.rs::baked_sprites_rects_and_text_draw_on_the_pc_renderer`, `crates/platform-pc/tests/render_e2e.rs::baked_models_light_fog_depth_textures_and_billboards`, `crates/runner-pc/tests/runner.rs::bake_upload_and_draw_like_the_runner` |
| `Golden frames (sprites, N64 look, 3DS look, platformer, 3D)` | `crates/platform-pc/tests/golden.rs::basic_scene_matches_golden`, `crates/platform-pc/tests/golden.rs::n64_look_scene_matches_golden`, `crates/platform-pc/tests/golden.rs::n3ds_scene_matches_golden`, `crates/platform-pc/tests/golden.rs::platformer_scene_matches_golden`, `crates/platform-pc/tests/golden.rs::cube3d_scene_matches_golden` |
| `N64 look and VI dedither` | `crates/platform-pc/tests/render_e2e.rs::n64_look_and_strict_mode_budgets`, `crates/platform-pc/tests/golden.rs::vi_dedither_smooths_the_dither_and_keeps_flat_areas`, `xtask/tests/n64_parity.rs::pc_n64_look_tracks_the_ares_frame` |
| `3DS look and two screens` | `crates/platform-pc/tests/render_e2e.rs::n3ds_second_screen_renders_reads_back_and_presents_stacked`, `xtask/tests/n3ds_parity.rs::pc_3ds_look_tracks_the_azahar_frame`, `xtask/tests/n3ds_parity.rs::pc_3ds_bottom_screen_tracks_the_azahar_frame` |
| `Internal size, shared device, present_preview, window mapping` | `crates/platform-pc/tests/render_e2e.rs::internal_size_shared_device_and_tooling_accessors`, `crates/platform-pc/tests/render_e2e.rs::large_frames_grow_buffers_and_batches_mix_textures` |
| `SimProfile, pixel scale, native picture` | `crates/platform-pc/tests/platform_services.rs::sim_profiles_and_native_picture_sizes`, `crates/runner-pc/tests/runner.rs::window_and_picture_sizes_follow_settings_and_profiles` |
| `Display settings file` | `crates/platform-pc/tests/platform_services.rs::display_settings_text_round_trip_and_repair` |
| `RendererError` | `crates/platform-pc/tests/platform_services.rs::renderer_errors_are_readable` |
| `PC input: buttons, keys, stick, pointer` | `crates/platform-pc/tests/input_audio.rs::every_console_button_maps_from_its_key_with_edges`, `crates/platform-pc/tests/input_audio.rs::every_pc_key_sets_its_bit`, `crates/platform-pc/tests/input_audio.rs::wasd_stick_and_pointer_state_across_polls`, `crates/platform-pc/tests/input_audio.rs::keys_bound_to_l2_r2_are_full_triggers` |
| `PC gamepad mapping (gilrs → PadReading → InputState)` | `crates/platform-pc/tests/input_audio.rs::gamepad_readings_map_to_buttons_sticks_and_triggers`, `crates/platform-pc/src/input.rs::pad_buttons_are_distinct_bits` (with `--features gamepad`: `crates/platform-pc/src/input.rs::gilrs_buttons_map_one_to_one`) |
| `Bindings file (.input): old 12-line files, new buttons and keys` | `crates/platform-pc/tests/platform_services.rs::old_bindings_files_still_parse`, `crates/platform-pc/tests/platform_services.rs::rebinding_keys_through_the_platform` |
| `runner-pc: RunConfig, resample, TRINO_SIM` | `crates/runner-pc/tests/runner.rs::run_config_lays_out_the_project`, `crates/runner-pc/tests/runner.rs::resampler_preserves_level_and_duration`, `crates/runner-pc/tests/runner.rs::profile_knob_reads_trino_sim` |

### Editor

The editor is a binary, so these are unit tests. The App-level flows are
`#[ignore]` (they change the working directory) and run in CI's editor step:
`cargo test -p trino-editor ui_tests -- --ignored --test-threads=1`.

| Feature | Tests |
|---|---|
| `Editor: commands on a temp project (entities, undo, prefabs, materials, fake objects, scenes, export, play here)` | `crates/editor/src/ui_tests.rs::commands_edit_save_and_export_in_a_temp_project` |
| `Editor: hierarchy click, command palette, duplicate, undo` | `crates/editor/src/ui_tests.rs::hierarchy_select_palette_duplicate_and_undo` |
| `Editor: gizmo hits and drags (move, plane, scale, rotate, snapping)` | `crates/editor/src/gizmo.rs::gizmo_handles_are_hit_where_they_are_drawn`, `crates/editor/src/gizmo.rs::planar_handles_move_on_their_plane_and_the_view_ring_turns_the_view_axis`, `crates/editor/src/ui_tests.rs::gizmo_drags_grab_the_handle_under_the_press`, `crates/editor/src/gizmo.rs::drags_move_scale_and_rotate_the_local_transform`, `crates/editor/src/gizmo.rs::rotation_under_a_rotated_parent_stays_in_world_axes`, `crates/editor/src/gizmo.rs::snapped_y_spin_stays_exact_and_continuous` |
| `Editor: orbit camera, projection, picking` | `crates/editor/src/gizmo.rs::orbit_camera_moves_clamps_and_settles`, `crates/editor/src/gizmo.rs::orbit_camera_views_take_the_short_way_and_match_a_game_camera`, `crates/editor/src/gizmo.rs::projection_helpers_reject_points_behind_and_hit_the_ground`, `crates/editor/src/gizmo.rs::projection_and_ray_agree`, `crates/editor/src/gizmo.rs::ray_hits_box_in_front`, `crates/editor/src/gizmo.rs::affine_inverse_and_box_wireframe` |
| `Editor: scene navigation (axis widget drag/flip/projection, MMB pan, fly mode, dolly, near-orthographic view, focus pivot)` | `crates/editor/src/gizmo.rs::axis_widget_clicks_look_from_an_axis_then_flip_to_the_other_side`, `crates/editor/src/gizmo.rs::dragging_the_axis_widget_orbits_like_dragging_the_view`, `crates/editor/src/gizmo.rs::axis_widget_hit_test_prefers_the_front_tip_then_the_centre`, `crates/editor/src/gizmo.rs::orthographic_keeps_the_target_plane_and_renders_what_it_projects`, `crates/editor/src/gizmo.rs::fly_mode_looks_around_the_eye_and_moves_along_the_view`, `crates/editor/src/gizmo.rs::dolly_and_focus_pivot`, `crates/editor/src/gizmo.rs::drops_land_near_what_an_orthographic_view_looks_at`, `crates/editor/src/widget_tests.rs::axis_widget_snaps_orbits_and_toggles_the_projection`, `crates/editor/src/ui_tests.rs::scene_view_pans_flies_dollies_and_flips_axes` |
| `Editor: document undo/redo, selection, save` | `crates/editor/src/doc.rs::edits_coalesce_until_settled_and_undo_restores`, `crates/editor/src/doc.rs::multi_selection_toggles_and_prunes`, `crates/editor/src/doc.rs::save_reload_and_history_bounds`, `crates/editor/src/doc.rs::prefab_docs_serialize_as_prefabs` |
| `Editor: project bake, load and fake objects` | `crates/editor/src/project.rs::bake_and_load_reads_every_asset_with_its_stats`, `crates/editor/src/project.rs::saving_a_fake_object_rebakes_it_in_place`, `crates/editor/src/project.rs::create_proxy_writes_the_prim_and_the_manifest_entry_then_retargets` |
| `Editor: inspector material and fake-object editors` | `crates/editor/src/widget_tests.rs::material_editor_sets_shading_sides_and_texture`, `crates/editor/src/widget_tests.rs::primitive_editor_switches_shape_pivot_and_keeps_sizes_valid` |
| `Editor: console and toasts` | `crates/editor/src/widget_tests.rs::console_counts_filters_and_clears` |
| `Editor: arrange (align, distribute, array, drop to surface)` | `crates/editor/src/ops.rs::align_and_distribute`, `crates/editor/src/ops.rs::array_makes_offset_named_copies`, `crates/editor/src/ops.rs::drop_lands_on_the_highest_surface_below`, `crates/editor/src/ops.rs::remove_many_keeps_paths_valid` |
| `Editor: level painter file sync` | `crates/editor/src/level_editor.rs::paint_writes_the_file_immediately`, `crates/editor/src/level_editor.rs::external_change_reloads_but_own_save_does_not` |
| `Editor: budget HUD` | `crates/editor/src/budget.rs::levels_follow_the_n64_caps`, `crates/editor/src/budget.rs::n64_profile_estimates_3d_time`, `crates/editor/src/budget.rs::pc_profile_never_warns` |
| `Editor: command palette matching` | `crates/editor/src/palette.rs::subsequence_word_starts_win`, `crates/editor/src/palette.rs::order_matters_and_case_does_not` |
| `Editor: play here` | `crates/editor/src/play.rs::plays_the_showcase_scene_in_memory`, `crates/editor/src/play.rs::keys_map_like_the_pc_app` |
| `Editor: Platforms tab (variants per platform, cost, wide and narrow layouts)` | `crates/editor/src/widget_tests.rs::platforms_tab_edits_variants_per_platform`, `crates/editor/src/widget_tests.rs::platforms_tab_fits_wide_and_narrow_inspectors` |
| `Editor: LOD panel (chain, per-platform starts, cost, preview) and impostor settings` | `crates/editor/src/widget_tests.rs::lod_panel_builds_a_chain_with_per_platform_distances`, `crates/editor/src/widget_tests.rs::lod_and_impostor_panels_fit_narrow_inspectors` |
| `Editor: impostor bake from the Inspector (same files as the CLI), LOD level, preview, external settings` | `crates/editor/src/ui_tests.rs::impostors_bake_from_the_inspector_and_drive_a_lod_level` |
| `Editor: Animation tab (clips, measurements, frame steps, crossfade, pose export == CLI, clip into the entity, layout)` | `crates/editor/src/ui_tests.rs::animation_tab_previews_measures_and_exports`, `crates/editor/src/anim_panel.rs::playback_wraps_loops_clamps_one_shots_and_steps_frames`, `crates/editor/src/anim_panel.rs::the_pose_drawn_carries_the_crossfade`, `crates/editor/src/anim_panel.rs::the_hook_picks_a_model_clip_and_time` |
| `Editor: particle inspector (fields, caps per platform, preview bursts to each cap, files both ways)` | `crates/editor/src/widget_tests.rs::particle_editor_sets_fields_and_per_platform_caps`, `crates/editor/src/ui_tests.rs::particle_effects_preview_caps_and_files` |
| `Editor: budget HUD with impostors` | `crates/editor/src/budget.rs::impostors_add_their_billboard_cost_to_the_n64_time` |
| `Editor: platform budget (each console's own model bakes, HUD lines and N64 time with the RSP split, Inspector section with every reason, unseen draws select their entity, cache, layout)` | `crates/editor/src/budget.rs::platform_lines_name_the_rsp_reason_and_unseen_draws`, `crates/editor/src/app.rs::console_models_follow_overrides_and_platform_lists`, `crates/editor/src/ui_tests.rs::platform_budget_shows_rsp_reasons_and_unseen_draws` |
| `Editor: variants in the Game view, saved as text, external edits reload` | `crates/editor/src/ui_tests.rs::platform_variants_reach_the_game_view_and_the_file` |
| `Editor: project launcher (recent projects, forget, open with validation, create from a template into the editor, layout at 960x600 and 1600x940)` | `crates/editor/src/ui_tests.rs::launcher_lists_recent_projects_opens_and_creates`, `crates/editor/src/launcher.rs::projects_need_a_trino_toml_or_an_asset_manifest`, `crates/editor/src/launcher.rs::create_validates_then_scaffolds_into_folder_slash_name`, `crates/editor/src/main.rs::start_picks_the_launcher_unless_a_project_or_a_scene_hook_says_otherwise` |
| `Editor: recent-projects file (parse, newest first, cap, round trip, ages)` | `crates/editor/src/recent.rs::parse_keeps_good_lines_newest_first_once`, `crates/editor/src/recent.rs::touch_moves_to_the_top_and_caps_the_list`, `crates/editor/src/recent.rs::load_and_save_round_trip_through_the_file`, `crates/editor/src/recent.rs::ages_read_like_a_person_says_them` |
| `Editor: layout audit (nothing off the window or overlapping, 960x600 and 1600x940)` | `crates/editor/src/ui_tests.rs::editor_layout_is_clean_at_minimum_and_default_sizes`, `crates/editor/src/layout_audit.rs::flags_overflow_and_overlap_only`, `crates/editor/src/app.rs::long_paths_keep_their_end` |

## Tools: xtask

Every top-level command has a row. Commands that need Docker, devkitPro, an
emulator, Blender or a window, or that rewrite committed files, are `n/a` on
the host; CI's console jobs and smoke runs cover them.

| Feature | Tests |
|---|---|
| `xtask help` | `xtask/tests/cli.rs::help_and_no_arguments_list_every_command`, `xtask/tests/cli.rs::unknown_invocations_fail_with_the_usage` |
| `xtask scene` | `xtask/tests/scene_cli.rs::scene_check_accepts_every_repo_scene`, `xtask/tests/scene_cli.rs::scene_check_reports_dangling_references_and_parse_errors`, `xtask/tests/scene_cli.rs::scene_without_a_subcommand_prints_the_usage`, `xtask/tests/scene_cli.rs::scene_export_writes_the_committed_showcase_code`, `xtask/tests/scene_cli.rs::scene_export_warns_on_dangling_references_and_fails_on_io_errors`, `xtask/tests/scene_cli.rs::scene_export_applies_the_platform_variants`, `xtask/tests/scene_budget.rs::scene_budget_cli_prints_the_same_numbers`, `xtask/tests/scene_codegen.rs::showcase_codegen_matches_the_scene` |
| `xtask particles` | `xtask/tests/particles_cli.rs::inspect_simulates_each_platform_cap_deterministically`, `xtask/tests/particles_cli.rs::new_declares_an_effect_and_bad_invocations_say_why` |
| `xtask impostor` | `xtask/tests/impostor_cli.rs::new_bake_check_and_list_an_impostor_in_a_project`, `xtask/tests/impostor_cli.rs::bad_impostor_invocations_fail_with_the_usage_or_a_reason`, `xtask/tests/impostor_cli.rs::the_engines_impostors_are_fresh` |
| `xtask model` | `xtask/tests/model_inspect.rs::static_glb_reports_its_mesh_and_n64_cost`, `xtask/tests/model_inspect.rs::animated_glb_reports_its_skeleton_and_clips`, `xtask/tests/model_inspect.rs::smooth_flag_keeps_two_joint_weights`, `xtask/tests/model_inspect.rs::fake_objects_bake_and_heavy_or_centered_ones_warn`, `xtask/tests/model_inspect.rs::bad_invocations_and_files_fail`, `xtask/tests/model_inspect.rs::color_slots_are_counted_per_slot`, `xtask/tests/model_inspect.rs::anim_flag_measures_every_clip`, `xtask/tests/model_inspect.rs::model_pose_exports_a_static_model` |
| `xtask agents` | `xtask/tests/cli.rs::agents_check_passes_on_the_clean_tree_and_rejects_unknown_subcommands`, `xtask/tests/agents_sync.rs::skill_mirrors_match_canonical_tree`, `xtask/tests/agents_sync.rs::every_skill_is_listed_in_agents_md` |
| `xtask assets` | `xtask/tests/cli.rs::assets_pc_bakes_the_repo_manifest` |
| `xtask new` | `xtask/tests/cli.rs::new_rejects_bad_names_and_bad_options`, `xtask/tests/cli.rs::new_standalone_scaffolds_a_complete_project`, `xtask/tests/cli.rs::new_from_a_demo_template_and_from_the_prompts`, `xtask/tests/standalone.rs::standalone_project_builds_tests_and_runs`, `xtask/src/standalone.rs::parses_sources`, `xtask/src/standalone.rs::parses_templates`, `xtask/src/standalone.rs::prompts_ask_again_and_default`, `xtask/src/standalone.rs::scaffolded_trino_toml_loads_as_a_project` |
| `Project templates (blank, castle64, showcase): renames, copies, out-of-the-box compile` | `crates/scaffold/src/lib.rs::names_take_every_form`, `crates/scaffold/src/lib.rs::templates_parse_their_keys`, `crates/scaffold/src/lib.rs::demo_renames_touch_only_the_crate_and_its_type`, `crates/scaffold/src/lib.rs::scaffolds_blank_and_demo_projects`, `xtask/tests/standalone.rs::every_template_compiles_out_of_the_box` (ignored: `cargo check` of each template) |
| `xtask --project` (external projects) | `xtask/tests/cli.rs::project_selection_fails_fast_on_bad_projects` |
| `trino.toml [project] platforms (default all, gate with the field in the error, console sections only when listed)` | `xtask/src/project.rs::a_manifest_without_platforms_targets_everything`, `xtask/src/project.rs::platforms_parse_dedupe_and_sort`, `xtask/src/project.rs::console_sections_are_only_required_for_listed_platforms`, `xtask/src/project.rs::bad_platform_lists_are_rejected`, `xtask/src/project.rs::requiring_a_missing_platform_names_the_field_and_the_file`, `xtask/src/main.rs::the_platform_gate_only_stops_left_out_platforms`, `xtask/tests/cli.rs::platform_commands_follow_the_project_platforms` |
| `xtask assets all` | `xtask/tests/cli.rs::platform_commands_follow_the_project_platforms` (a PC-only project: the PC bake runs, n64 and 3ds are skipped) |
| `xtask build all` | n/a: builds every targeted platform (Docker, devkitPro); its plan and skips are `xtask/src/project.rs::platforms_parse_dedupe_and_sort` |
| `xtask test all` | n/a: the host suite plus the emulators of every targeted console; its plan is `xtask/src/project.rs::platforms_parse_dedupe_and_sort` |
| `xtask run n64 or 3ds --device (real hardware: SummerCart64, EverDrive, 3dslink, Luma3DS gdb script)` | `xtask/tests/hardware.rs::sc64_uploads_the_rom_then_forwards_the_debug_log`, `xtask/tests/hardware.rs::a_failed_upload_stops_the_deploy`, `xtask/tests/hardware.rs::everdrive_uploads_and_debugs_in_one_unfloader_call`, `xtask/tests/hardware.rs::network_deploy_runs_3dslink_with_the_ip_and_writes_the_gdb_script`, `xtask/tests/hardware.rs::device_flags_and_missing_tools_fail_with_a_reason`, `xtask/tests/hardware.rs::no_build_needs_an_existing_rom`, `xtask/tests/hardware.rs::the_n3ds_alias_deploys_like_3ds`, `xtask/src/hardware.rs::arguments_after_a_double_dash_are_left_alone`, `xtask/src/hardware.rs::loader_command_lines`, `xtask/src/hardware.rs::device_flags_parse_and_reject_bad_combinations`, `xtask/src/hardware.rs::devices_only_run_on_their_own_console`, `xtask/src/hardware.rs::target_flag_becomes_the_positional_platform`, `xtask/src/hardware.rs::gdb_scripts_follow_lumas_ports` |
| `xtask emulators` | n/a: needs Docker or the network; only its usage errors run on the host (`xtask/tests/cli.rs::emulator_and_blender_commands_reject_unknown_subcommands`) |
| `xtask blender` | n/a: needs Blender; only its usage errors run on the host (`xtask/tests/cli.rs::emulator_and_blender_commands_reject_unknown_subcommands`) |
| `xtask gen-assets` | n/a: rewrites the committed masters. Its generators are checked into a temp dir instead (`xtask/src/main.rs::gen_assets_reproduces_the_committed_masters`) |
| `xtask build pc` | n/a: recursive cargo build; CI builds the PC app |
| `xtask run pc` | n/a: opens a window; CI's smoke runs cover it |
| `xtask test pc` | n/a: runs this whole suite |
| `xtask watch pc` | n/a: a live-reload session; the reload path is `xtask/tests/hot_reload.rs::state_survives_dylib_swap_and_behavior_changes` (ignored, CI job) |
| `xtask editor` | n/a: opens a window (the launcher, or `<dir>` / the selected project); the editor's own tests are listed below, the start rule is `crates/editor/src/main.rs::start_picks_the_launcher_unless_a_project_or_a_scene_hook_says_otherwise` |
| `xtask assets n64` | n/a: needs the libdragon Docker image |
| `xtask build n64` | n/a: needs the libdragon Docker image (CI `n64` job) |
| `xtask run n64` | n/a: needs ares (`--device`: the real-hardware row above, with stub loaders) |
| `xtask test n64` | n/a: needs Docker + ares (CI `n64` job); its pure gates are unit-tested in `xtask/src/perf.rs` and `xtask/src/fbgolden.rs` |
| `xtask watch n64` | n/a: needs Docker + ares |
| `xtask assets 3ds` | n/a: needs devkitPro (tex3ds) |
| `xtask build 3ds` | n/a: needs devkitPro (CI `n3ds` job) |
| `xtask run 3ds` | n/a: needs Azahar (`--device network`: the real-hardware row above, with a stub 3dslink) |
| `xtask test 3ds` | n/a: needs devkitPro + Azahar (CI `n3ds-emulator` job) |
| `xtask watch 3ds` | n/a: needs devkitPro + Azahar |
