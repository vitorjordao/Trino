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
| `Renderer::draw_model` | `crates/core/tests/tessellate.rs::shading_modes_tint_and_emissive`, `crates/platform-pc/tests/render_e2e.rs::baked_models_light_fog_depth_textures_and_billboards`, `crates/platform-pc/tests/golden.rs::cube3d_scene_matches_golden` |
| `Renderer::draw_model_matrix` | `crates/core/tests/renderer_defaults.rs::draw_model_matrix_default_decomposes_into_draw_model`, `crates/platform-pc/tests/render_e2e.rs::baked_models_light_fog_depth_textures_and_billboards` |
| `Renderer::build_batch` | `crates/platform-pc/tests/batch_pc.rs::a_batch_draws_like_its_parts_and_drops_cleanly`, `crates/core/tests/batch.rs::a_batch_draws_exactly_like_its_parts` |
| `Renderer::drop_batch` | `crates/platform-pc/tests/batch_pc.rs::a_batch_draws_like_its_parts_and_drops_cleanly` |
| Static batching (`batch::merge_tmdl`, `merge_parts`) | `crates/core/tests/batch.rs::a_batch_draws_exactly_like_its_parts`, `crates/core/tests/batch.rs::the_batch_bounds_cover_every_part_and_cull_as_one`, `crates/core/tests/batch.rs::what_a_batch_cannot_hold_is_refused` |
| `Renderer::draw_billboard` | `crates/core/tests/tessellate.rs::billboards_face_the_camera_and_respect_fog_and_regions`, `crates/platform-pc/tests/render_e2e.rs::baked_models_light_fog_depth_textures_and_billboards` |
| `Renderer::stats` | `crates/core/tests/renderer_defaults.rs::recorder_stats_cover_the_last_finished_frame`, `crates/platform-pc/tests/render_e2e.rs::baked_sprites_rects_and_text_draw_on_the_pc_renderer` |
| `Renderer::second_screen` | `crates/core/tests/renderer_defaults.rs::provided_renderer_methods_are_harmless_defaults`, `crates/platform-pc/tests/render_e2e.rs::n3ds_second_screen_renders_reads_back_and_presents_stacked` |
| `Renderer::set_screen` | `crates/core/tests/renderer_defaults.rs::provided_renderer_methods_are_harmless_defaults`, `crates/platform-pc/tests/render_e2e.rs::n3ds_second_screen_renders_reads_back_and_presents_stacked` |
| `Renderer::read_pixel` | `crates/core/tests/renderer_defaults.rs::provided_renderer_methods_are_harmless_defaults`, `crates/platform-pc/tests/render_e2e.rs::baked_sprites_rects_and_text_draw_on_the_pc_renderer` |
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
| `Distance LOD` | `crates/core/tests/scene_nodes.rs::lod_levels_switch_models_with_distance`, `examples/showcase/src/lib.rs::far_trees_switch_to_cheaper_lods` |
| `Collision: Aabb, colliders, move_aabb, raycast` | `crates/core/tests/physics.rs::aabb_operations`, `crates/core/tests/physics.rs::collider_shapes_become_world_boxes`, `crates/core/tests/physics.rs::move_aabb_lands_slides_and_bonks`, `crates/core/tests/physics.rs::raycast_finds_the_nearest_face_and_its_normal`, `crates/core/tests/physics.rs::raycasts_along_z_and_single_row_tilemaps` |
| `Tilemap and move_and_collide` | `crates/core/tests/physics.rs::tilemap_parses_ascii_levels`, `crates/core/tests/physics.rs::move_and_collide_runs_a_platformer_step_loop` |
| `Runtime entities: EntityStore` | `crates/core/tests/entities.rs::handles_are_generational_and_capacity_is_fixed`, `crates/core/tests/entities.rs::entities_move_query_and_collide`, `crates/core/tests/entities.rs::draw_applies_lod_animation_visibility_and_cells`, `crates/core/tests/entities.rs::a_baked_scene_spawns_into_the_store` |
| `Rigid animation: Mesh::pose` | `crates/core/tests/tmdl_mesh.rs::skel_and_anim_sections_pose_a_rigid_skeleton`, `crates/core/tests/tmdl_mesh.rs::hierarchy_composes_parent_before_child_with_inverse_bind`, `crates/core/tests/tessellate.rs::skinned_meshes_follow_their_pose` |
| `Clip crossfade: AnimRef::blend_to` | `crates/core/tests/tmdl_mesh.rs::blend_to_crossfades_two_clips_per_joint` |
| `Billboards` | `crates/core/tests/tessellate.rs::billboards_face_the_camera_and_respect_fog_and_regions`, `crates/platform-pc/tests/render_e2e.rs::baked_models_light_fog_depth_textures_and_billboards` |
| `Particles: ParticleSystem` | `crates/core/tests/particles.rs::same_seed_same_particles_different_seed_different_ones`, `crates/core/tests/particles.rs::rate_lifetime_and_motion`, `crates/core/tests/particles.rs::draw_emits_one_billboard_per_particle_with_fading_and_flipbook`, `crates/core/tests/particles.rs::sideways_jets_spawn_inside_their_cone` |
| `Positional audio: Listener and spatialize` | `crates/core/tests/audio.rs::spatialize_falls_off_quadratically_and_pans_by_side`, `crates/core/tests/audio.rs::gains_split_volume_by_pan` |
| `Positional audio: Ambience and scene sounds` | `crates/core/tests/audio.rs::ambience_keeps_the_nearest_sources_voiced_as_the_listener_walks`, `crates/core/tests/audio.rs::ambience_survives_a_mixer_without_free_voices`, `crates/core/tests/entities.rs::sound_sources_drive_ambience`, `examples/showcase/src/lib.rs::audio_is_positional_and_the_last_coin_plays_the_fanfare` |
| `Portal/cell visibility` | `crates/core/tests/vis.rs::cells_and_portals_come_from_scene_nodes`, `crates/core/tests/vis.rs::looking_down_the_doors_draws_every_room_looking_away_only_ours`, `crates/core/tests/vis.rs::hidden_rooms_never_reach_the_renderer`, `crates/core/tests/vis.rs::baked_cells_match_exported_ones`, `crates/core/tests/vis.rs::capacity_limits_and_conservative_fallbacks` |
| `Software T&L: culling, back faces, depth` | `crates/core/tests/tessellate.rs::a_cube_face_on_emits_its_front_face_only`, `crates/core/tests/tessellate.rs::meshes_outside_the_frustum_are_culled_whole`, `crates/core/tests/tessellate.rs::double_sided_draws_keep_back_faces`, `crates/core/tests/tessellate.rs::painter_keys_order_far_first_then_submission` |
| `Software T&L: shading, tint, emissive, colored light, fog` | `crates/core/tests/tessellate.rs::shading_modes_tint_and_emissive`, `crates/core/tests/tessellate.rs::colored_light_tints_lit_surfaces`, `crates/core/tests/tessellate.rs::fog_blends_toward_the_fog_color_with_view_depth` |
| `Software T&L: near and guard-band clipping, depth splits` | `crates/core/tests/tessellate.rs::geometry_crossing_the_near_plane_is_clipped_into_the_guard_band` |
| `Software T&L: textured draws` | `crates/core/tests/tessellate.rs::textured_draws_carry_uvs_and_view_depth`, `crates/platform-pc/tests/render_e2e.rs::baked_models_light_fog_depth_textures_and_billboards` |
| `Software T&L: TnlContext and TnlScratch reuse` | `crates/core/tests/tessellate.rs::tessellate_with_reuses_context_and_scratch_bit_identically` |
| `RSP eligibility and split policy` | `crates/core/tests/tessellate.rs::split_policy_and_rsp_eligibility_follow_the_draw`, `crates/core/tests/tessellate.rs::degenerate_flat_skinned_and_multi_segment_meshes` |
| `Z-buffered occlusion` | `crates/platform-pc/tests/render_e2e.rs::baked_models_light_fog_depth_textures_and_billboards`, `xtask/tests/engine_checks.rs::engine_checks_pass_on_pc` |
| `Caps and strict mode` | `crates/core/tests/perf_caps.rs::caps_validate_textures_per_console`, `crates/platform-pc/tests/render_e2e.rs::n64_look_and_strict_mode_budgets`, `crates/platform-pc/tests/golden.rs::strict_mode_rejects_texture_over_budget`, `crates/platform-pc/tests/golden.rs::strict_mode_rejects_3d_over_the_n64_time_ceiling` |
| `Perf: N64 cost model and frame profile` | `crates/core/tests/perf_caps.rs::n64_cost_model_prices_real_tnl_work`, `crates/core/tests/perf_caps.rs::render_profile_accumulates_and_reports_per_frame`, `crates/core/tests/perf_caps.rs::models_past_the_tracked_limit_pool_into_other`, `crates/core/tests/perf_caps.rs::report_write_errors_propagate`, `xtask/tests/perf_budget.rs::castle64_tnl_work_matches_the_baseline` |
| `Game trait and update_with` | `crates/core/tests/game_loop.rs::default_update_with_forwards_to_update_and_resize_is_a_no_op`, `crates/core/tests/game_loop.rs::games_reach_blobs_and_saves_through_update_with` |
| `InputState (buttons, keys, pointer)` | `crates/core/tests/game_loop.rs::input_state_edges_for_buttons_keys_and_pointer` |
| `Math: vectors, colors, Mat34, Quat, software trig` | `crates/core/tests/math.rs::software_trig_and_sqrt_stay_within_their_documented_error`, `crates/core/tests/math.rs::vectors_rects_and_colors`, `crates/core/tests/math.rs::matrices_compose_transform_and_look`, `crates/core/tests/math.rs::quaternions_interpolate_and_build_trs_matrices` |
| `Hot-reload ABI: export_game!` | `crates/game-api/tests/export_game.rs::exports_handshake_and_forward_to_the_game`, `xtask/tests/hot_reload.rs::state_survives_dylib_swap_and_behavior_changes` |
| `Console self-test steps on PC` | `xtask/tests/engine_checks.rs::engine_checks_pass_on_pc`, `xtask/tests/engine_checks.rs::audio_checks_pass_on_pc`, `xtask/tests/engine_checks.rs::baked_showcase_matches_code_and_plays` |

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
| `asset scenes` | `crates/asset-pipeline/tests/scene_runtime.rs::the_showcase_scene_loads_from_its_blob_and_draws_every_model`, `crates/asset-pipeline/tests/asset_kinds.rs::scene_bakes_fail_on_library_errors_and_scene_warnings`, `xtask/tests/engine_checks.rs::baked_showcase_matches_code_and_plays` |
| `Asset materials (assets/materials/*.material.ron)` | `crates/scene/tests/authoring.rs::materials_parse_save_and_become_model_params`, `crates/scene/tests/library_resolve.rs::resolve_flattens_every_component` |

### Asset pipeline

| Feature | Tests |
|---|---|
| `Bake index snapshot and incremental rebakes` | `crates/asset-pipeline/tests/pipeline.rs::bake_produces_deterministic_index_snapshot`, `crates/asset-pipeline/tests/watch.rs::saving_a_master_or_a_scene_rebakes_only_what_changed` |
| `Console texture formats and platform overrides` | `crates/asset-pipeline/tests/asset_kinds.rs::console_bakes_record_and_validate_each_platforms_format`, `crates/asset-pipeline/tests/pipeline.rs::n64_bake_fails_without_explicit_format`, `crates/asset-pipeline/tests/pipeline.rs::platform_override_wins_and_reload_keeps_handle` |
| `Bake errors: every error at once, missing sources, collisions, output dir` | `crates/asset-pipeline/tests/asset_kinds.rs::a_broken_project_reports_every_error_at_once`, `crates/asset-pipeline/tests/pipeline.rs::missing_source_is_a_bake_error`, `crates/asset-pipeline/tests/asset_kinds.rs::colliding_handles_fail_the_bake_for_every_kind`, `crates/asset-pipeline/tests/asset_kinds.rs::an_unwritable_output_directory_is_a_bake_error` |
| `Manifest validation` | `crates/asset-pipeline/tests/asset_kinds.rs::manifests_reject_unknown_fields_versions_and_missing_files` |
| `Manifest edits (add, retarget, same handle)` | `crates/asset-pipeline/tests/asset_kinds.rs::manifest_edits_keep_comments_and_retarget_a_model_under_one_handle` |
| `Loader: corrupt baked output is an error` | `crates/asset-pipeline/tests/asset_kinds.rs::the_loader_rejects_corrupt_baked_output` |
| `glTF → TMDL → Mesh → tessellate` | `crates/asset-pipeline/tests/model_pipeline.rs::the_cube_master_bakes_and_tessellates_its_visible_faces`, `crates/asset-pipeline/tests/model_pipeline.rs::mesh_data_encodes_every_tmdl_section_and_rejects_bad_layouts` |
| `glTF vertex colors and texture sampling` | `crates/asset-pipeline/tests/model_pipeline.rs::vertex_colors_come_from_color0_then_texture_then_factor`, `crates/asset-pipeline/tests/model_pipeline.rs::texture_color_types_are_sampled_into_vertex_colors` |
| `glTF errors and unsupported primitives` | `crates/asset-pipeline/tests/model_pipeline.rs::line_primitives_are_skipped_and_bad_files_fail_with_a_reason` |
| `glTF animation → pose (channels, interpolation, once/loop)` | `crates/asset-pipeline/tests/model_pipeline.rs::every_channel_and_interpolation_poses_its_joint`, `crates/asset-pipeline/tests/model_pipeline.rs::rest_rotations_round_trip_for_half_turns_on_every_axis`, `crates/asset-pipeline/tests/animated_model.rs::node_animation_bakes_to_a_rigid_skeleton`, `crates/asset-pipeline/tests/animated_model.rs::static_gltf_stays_skeleton_free` |
| `glTF skins (rigid and smooth)` | `crates/asset-pipeline/tests/model_pipeline.rs::skinned_files_give_unskinned_parts_a_static_joint`, `crates/asset-pipeline/tests/animated_model.rs::smooth_skinning_keeps_two_weights_only_when_asked` |
| `Model bounds for colliders` | `crates/asset-pipeline/tests/model_pipeline.rs::manifest_model_bounds_cover_every_bakeable_model` |
| `Fake objects (.prim.ron)` | `crates/asset-pipeline/tests/fake_objects.rs::every_shape_bakes_from_its_ron_file_at_its_declared_size`, `crates/asset-pipeline/tests/fake_objects.rs::segments_smoothing_and_colors_shape_the_mesh`, `crates/asset-pipeline/tests/fake_objects.rs::invalid_dimensions_are_rejected_with_the_field_name` |
| `Font bake: TTF and PNG grid` | `crates/asset-pipeline/tests/fonts.rs::ttf_masters_rasterize_to_an_atlas_and_a_glyph_table`, `crates/asset-pipeline/tests/fonts.rs::ttf_errors_name_the_problem`, `crates/asset-pipeline/tests/fonts.rs::png_grids_trim_proportional_glyphs_and_keep_monospaced_cells` |
| `Baked scene → BakedScene → draw_nodes with real meshes` | `crates/asset-pipeline/tests/scene_runtime.rs::the_showcase_scene_loads_from_its_blob_and_draws_every_model`, `crates/asset-pipeline/tests/scene_runtime.rs::the_interior_scene_culls_rooms_through_its_portals` |
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

### PC backend and runner

| Feature | Tests |
|---|---|
| `manifest → bake → load → draw on the PC renderer` | `crates/platform-pc/tests/render_e2e.rs::baked_sprites_rects_and_text_draw_on_the_pc_renderer`, `crates/platform-pc/tests/render_e2e.rs::baked_models_light_fog_depth_textures_and_billboards`, `crates/runner-pc/tests/runner.rs::bake_upload_and_draw_like_the_runner` |
| `Golden frames (sprites, N64 look, 3DS look, platformer, 3D)` | `crates/platform-pc/tests/golden.rs::basic_scene_matches_golden`, `crates/platform-pc/tests/golden.rs::n64_look_scene_matches_golden`, `crates/platform-pc/tests/golden.rs::n3ds_scene_matches_golden`, `crates/platform-pc/tests/golden.rs::platformer_scene_matches_golden`, `crates/platform-pc/tests/golden.rs::cube3d_scene_matches_golden` |
| `N64 look and VI dedither` | `crates/platform-pc/tests/render_e2e.rs::n64_look_and_strict_mode_budgets`, `crates/platform-pc/tests/golden.rs::vi_dedither_smooths_the_dither_and_keeps_flat_areas`, `xtask/tests/n64_parity.rs::pc_n64_look_tracks_the_ares_frame` |
| `3DS look and two screens` | `crates/platform-pc/tests/render_e2e.rs::n3ds_second_screen_renders_reads_back_and_presents_stacked`, `xtask/tests/n3ds_parity.rs::pc_3ds_look_tracks_the_azahar_frame` |
| `Internal size, shared device, present_preview, window mapping` | `crates/platform-pc/tests/render_e2e.rs::internal_size_shared_device_and_tooling_accessors`, `crates/platform-pc/tests/render_e2e.rs::large_frames_grow_buffers_and_batches_mix_textures` |
| `SimProfile, pixel scale, native picture` | `crates/platform-pc/tests/platform_services.rs::sim_profiles_and_native_picture_sizes`, `crates/runner-pc/tests/runner.rs::window_and_picture_sizes_follow_settings_and_profiles` |
| `Display settings file` | `crates/platform-pc/tests/platform_services.rs::display_settings_text_round_trip_and_repair` |
| `RendererError` | `crates/platform-pc/tests/platform_services.rs::renderer_errors_are_readable` |
| `PC input: buttons, keys, stick, pointer` | `crates/platform-pc/tests/input_audio.rs::every_console_button_maps_from_its_key_with_edges`, `crates/platform-pc/tests/input_audio.rs::every_pc_key_sets_its_bit`, `crates/platform-pc/tests/input_audio.rs::wasd_stick_and_pointer_state_across_polls` |
| `runner-pc: RunConfig, resample, TRINO_SIM` | `crates/runner-pc/tests/runner.rs::run_config_lays_out_the_project`, `crates/runner-pc/tests/runner.rs::resampler_preserves_level_and_duration`, `crates/runner-pc/tests/runner.rs::profile_knob_reads_trino_sim` |

### Editor

The editor is a binary, so these are unit tests. The two App-level flows are
`#[ignore]` (they change the working directory) and run in CI's editor step:
`cargo test -p trino-editor ui_tests -- --ignored --test-threads=1`.

| Feature | Tests |
|---|---|
| `Editor: commands on a temp project (entities, undo, prefabs, materials, fake objects, scenes, export, play here)` | `crates/editor/src/ui_tests.rs::commands_edit_save_and_export_in_a_temp_project` |
| `Editor: hierarchy click, command palette, duplicate, undo` | `crates/editor/src/ui_tests.rs::hierarchy_select_palette_duplicate_and_undo` |
| `Editor: gizmo hits and drags (move, plane, scale, rotate, snapping)` | `crates/editor/src/gizmo.rs::gizmo_handles_are_hit_where_they_are_drawn`, `crates/editor/src/gizmo.rs::drags_move_scale_and_rotate_the_local_transform`, `crates/editor/src/gizmo.rs::rotation_under_a_rotated_parent_stays_in_world_axes`, `crates/editor/src/gizmo.rs::snapped_y_spin_stays_exact_and_continuous` |
| `Editor: orbit camera, projection, picking` | `crates/editor/src/gizmo.rs::orbit_camera_moves_clamps_and_settles`, `crates/editor/src/gizmo.rs::orbit_camera_views_take_the_short_way_and_match_a_game_camera`, `crates/editor/src/gizmo.rs::projection_helpers_reject_points_behind_and_hit_the_ground`, `crates/editor/src/gizmo.rs::projection_and_ray_agree`, `crates/editor/src/gizmo.rs::ray_hits_box_in_front`, `crates/editor/src/gizmo.rs::affine_inverse_and_box_wireframe` |
| `Editor: document undo/redo, selection, save` | `crates/editor/src/doc.rs::edits_coalesce_until_settled_and_undo_restores`, `crates/editor/src/doc.rs::multi_selection_toggles_and_prunes`, `crates/editor/src/doc.rs::save_reload_and_history_bounds`, `crates/editor/src/doc.rs::prefab_docs_serialize_as_prefabs` |
| `Editor: project bake, load and fake objects` | `crates/editor/src/project.rs::bake_and_load_reads_every_asset_with_its_stats`, `crates/editor/src/project.rs::saving_a_fake_object_rebakes_it_in_place`, `crates/editor/src/project.rs::create_proxy_writes_the_prim_and_the_manifest_entry_then_retargets` |
| `Editor: inspector material and fake-object editors` | `crates/editor/src/widget_tests.rs::material_editor_sets_shading_sides_and_texture`, `crates/editor/src/widget_tests.rs::primitive_editor_switches_shape_pivot_and_keeps_sizes_valid` |
| `Editor: console and toasts` | `crates/editor/src/widget_tests.rs::console_counts_filters_and_clears` |
| `Editor: arrange (align, distribute, array, drop to surface)` | `crates/editor/src/ops.rs::align_and_distribute`, `crates/editor/src/ops.rs::array_makes_offset_named_copies`, `crates/editor/src/ops.rs::drop_lands_on_the_highest_surface_below`, `crates/editor/src/ops.rs::remove_many_keeps_paths_valid` |
| `Editor: level painter file sync` | `crates/editor/src/level_editor.rs::paint_writes_the_file_immediately`, `crates/editor/src/level_editor.rs::external_change_reloads_but_own_save_does_not` |
| `Editor: budget HUD` | `crates/editor/src/budget.rs::levels_follow_the_n64_caps`, `crates/editor/src/budget.rs::n64_profile_estimates_3d_time`, `crates/editor/src/budget.rs::pc_profile_never_warns` |
| `Editor: command palette matching` | `crates/editor/src/palette.rs::subsequence_word_starts_win`, `crates/editor/src/palette.rs::order_matters_and_case_does_not` |
| `Editor: play here` | `crates/editor/src/play.rs::plays_the_showcase_scene_in_memory`, `crates/editor/src/play.rs::keys_map_like_the_pc_app` |

## Tools: xtask

Every top-level command has a row. Commands that need Docker, devkitPro, an
emulator, Blender or a window, or that rewrite committed files, are `n/a` on
the host; CI's console jobs and smoke runs cover them.

| Feature | Tests |
|---|---|
| `xtask help` | `xtask/tests/cli.rs::help_and_no_arguments_list_every_command`, `xtask/tests/cli.rs::unknown_invocations_fail_with_the_usage` |
| `xtask scene` | `xtask/tests/scene_cli.rs::scene_check_accepts_every_repo_scene`, `xtask/tests/scene_cli.rs::scene_check_reports_dangling_references_and_parse_errors`, `xtask/tests/scene_cli.rs::scene_without_a_subcommand_prints_the_usage`, `xtask/tests/scene_cli.rs::scene_export_writes_the_committed_showcase_code`, `xtask/tests/scene_cli.rs::scene_export_warns_on_dangling_references_and_fails_on_io_errors`, `xtask/tests/scene_codegen.rs::showcase_codegen_matches_the_scene` |
| `xtask model` | `xtask/tests/model_inspect.rs::static_glb_reports_its_mesh_and_n64_cost`, `xtask/tests/model_inspect.rs::animated_glb_reports_its_skeleton_and_clips`, `xtask/tests/model_inspect.rs::smooth_flag_keeps_two_joint_weights`, `xtask/tests/model_inspect.rs::fake_objects_bake_and_heavy_or_centered_ones_warn`, `xtask/tests/model_inspect.rs::bad_invocations_and_files_fail` |
| `xtask agents` | `xtask/tests/cli.rs::agents_check_passes_on_the_clean_tree_and_rejects_unknown_subcommands`, `xtask/tests/agents_sync.rs::skill_mirrors_match_canonical_tree`, `xtask/tests/agents_sync.rs::every_skill_is_listed_in_agents_md` |
| `xtask assets` | `xtask/tests/cli.rs::assets_pc_bakes_the_repo_manifest` |
| `xtask new` | `xtask/tests/cli.rs::new_rejects_bad_names_and_bad_options`, `xtask/tests/cli.rs::new_standalone_scaffolds_a_complete_project`, `xtask/tests/standalone.rs::standalone_project_builds_tests_and_runs` |
| `xtask --project` (external projects) | `xtask/tests/cli.rs::project_selection_fails_fast_on_bad_projects` |
| `xtask emulators` | n/a: needs Docker or the network; only its usage errors run on the host (`xtask/tests/cli.rs::emulator_and_blender_commands_reject_unknown_subcommands`) |
| `xtask blender` | n/a: needs Blender; only its usage errors run on the host (`xtask/tests/cli.rs::emulator_and_blender_commands_reject_unknown_subcommands`) |
| `xtask gen-assets` | n/a: rewrites the committed masters. Its generators are checked into a temp dir instead (`xtask/src/main.rs::gen_assets_reproduces_the_committed_masters`) |
| `xtask build pc` | n/a: recursive cargo build; CI builds the PC app |
| `xtask run pc` | n/a: opens a window; CI's smoke runs cover it |
| `xtask test pc` | n/a: runs this whole suite |
| `xtask watch pc` | n/a: a live-reload session; the reload path is `xtask/tests/hot_reload.rs::state_survives_dylib_swap_and_behavior_changes` (ignored, CI job) |
| `xtask editor` | n/a: opens a window; the editor's own tests are listed below |
| `xtask assets n64` | n/a: needs the libdragon Docker image |
| `xtask build n64` | n/a: needs the libdragon Docker image (CI `n64` job) |
| `xtask run n64` | n/a: needs ares |
| `xtask test n64` | n/a: needs Docker + ares (CI `n64` job); its pure gates are unit-tested in `xtask/src/perf.rs` and `xtask/src/fbgolden.rs` |
| `xtask watch n64` | n/a: needs Docker + ares |
| `xtask assets 3ds` | n/a: needs devkitPro (tex3ds) |
| `xtask build 3ds` | n/a: needs devkitPro (CI `n3ds` job) |
| `xtask run 3ds` | n/a: needs Azahar |
| `xtask test 3ds` | n/a: needs devkitPro + Azahar (CI `n3ds-emulator` job) |
| `xtask watch 3ds` | n/a: needs devkitPro + Azahar |
