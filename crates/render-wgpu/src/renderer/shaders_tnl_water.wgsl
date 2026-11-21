// Appended to shaders_tnl.wgsl and water.wgsl (gpu_tnl.rs `MESH_WATER_WGSL`):
// water surfaces on the GPU T&L path (ADR 0016 §13). The vertex stage does
// what the CPU path's water T&L does (`tessellate_surface` with a water
// setup): the mesh into the world, lifted by the swell and the ripples
// (`water_height`, the core's own sine), then viewed and projected like any
// GPU draw; the texture's UVs scrolled by the flow; the unlit color
// truncated like the CPU path's surface. The fragment is the CPU path's
// per-pixel water (`water_shade`), so both paths shade the same pixels.
// Group 3 is the water sets and the scene's depth copy (mod.rs
// `water_bgl`), as for the screen-space water.

struct MeshWaterOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) world: vec3<f32>,
    @location(2) base: vec4<f32>,
    // x: the water set, y: the detail layer's UV scale.
    @location(3) @interpolate(flat) params: vec2<f32>,
};

@vertex
fn vs_mesh_water(v: MeshIn, inst: InstIn) -> MeshWaterOut {
    let m = mesh_vertex(v, inst);
    // The set rides in the flags' light-set bits (a water draw is never
    // shaded per pixel by the light sets).
    let si = (inst.misc.z >> 16u) & 7u;
    let ws = &water_sets.sets[si];
    let h = water_height(si, m.world.xz);
    let p = m.view + h * ctx.view_up.xyz;
    var out: MeshWaterOut;
    out.clip = vec4<f32>(p.x * ctx.proj.x, p.y * ctx.proj.y, p.z - ctx.proj.z, p.z);
    out.uv = v.uv + (*ws).scroll.xy;
    out.world = vec3<f32>(m.world.x, m.world.y + h, m.world.z);
    // The CPU path's surface color is bytes (`albedo`: truncated).
    out.base = floor(m.base * 255.0 + vec4<f32>(1e-3)) / 255.0;
    out.params = vec2<f32>(f32(si), m.out.params.y);
    return out;
}

@fragment
fn fs_mesh_water(in: MeshWaterOut) -> @location(0) vec4<f32> {
    return water_shade(u32(in.params.x + 0.5), in.world, in.uv, in.base, in.params.y, in.clip.xy);
}
