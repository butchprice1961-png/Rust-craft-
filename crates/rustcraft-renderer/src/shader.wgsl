// RustCraft world shader: texture-array blocks, vanilla-style light curve,
// day/night sky light, distance fog.

struct Globals {
    view_proj: mat4x4<f32>,
    fog_color: vec4<f32>,
    // x: fog start, y: fog end, z: sky light brightness (0-1), w: output gamma.
    params: vec4<f32>,
};

@group(0) @binding(0) var<uniform> globals: Globals;
// Camera-relative origin of each drawn section, indexed by instance.
@group(0) @binding(1) var<storage, read> offsets: array<vec4<f32>>;
@group(0) @binding(2) var atlas: texture_2d_array<f32>;
@group(0) @binding(3) var atlas_sampler: sampler;

struct VertexIn {
    @location(0) pos: vec3<f32>,
    @location(1) uv: vec2<f32>,
    @location(2) layer: u32,
    @location(3) color: vec4<f32>,
    @location(4) light: vec4<f32>,
};

struct VertexOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) @interpolate(flat) layer: u32,
    @location(2) color: vec3<f32>,
    @location(3) light: vec2<f32>,
    @location(4) dist: f32,
};

@vertex
fn vs_main(v: VertexIn, @builtin(instance_index) instance: u32) -> VertexOut {
    let world = v.pos + offsets[instance].xyz;
    var out: VertexOut;
    out.clip = globals.view_proj * vec4<f32>(world, 1.0);
    out.uv = v.uv;
    out.layer = v.layer;
    out.color = v.color.rgb;
    out.light = v.light.xy * 15.0;
    out.dist = length(world);
    return out;
}

// The 1.8 brightness curve: f = 1 - level/15, (1 - f) / (3f + 1).
fn curve(level: f32) -> f32 {
    let f = 1.0 - level / 15.0;
    return (1.0 - f) / (f * 3.0 + 1.0);
}

fn lighting(light: vec2<f32>) -> vec3<f32> {
    let sky = curve(light.x) * globals.params.z;
    let block = curve(light.y);
    let torch = vec3<f32>(block, block * 0.93, block * 0.82);
    return clamp(max(vec3<f32>(sky), torch) * 0.94 + 0.06, vec3<f32>(0.0), vec3<f32>(1.0));
}

fn finish(rgb: vec3<f32>, dist: f32) -> vec3<f32> {
    let fog = clamp((dist - globals.params.x) / max(globals.params.y - globals.params.x, 0.001), 0.0, 1.0);
    // The colour maths matches vanilla's gamma-space blending; on an sRGB
    // target the result is pre-linearised so it displays the same.
    return pow(mix(rgb, globals.fog_color.rgb, fog), vec3<f32>(globals.params.w));
}

@fragment
fn fs_solid(in: VertexOut) -> @location(0) vec4<f32> {
    let tex = textureSample(atlas, atlas_sampler, in.uv, in.layer);
    if tex.a < 0.5 {
        discard;
    }
    return vec4<f32>(finish(tex.rgb * in.color * lighting(in.light), in.dist), 1.0);
}

@fragment
fn fs_translucent(in: VertexOut) -> @location(0) vec4<f32> {
    let tex = textureSample(atlas, atlas_sampler, in.uv, in.layer);
    if tex.a < 0.02 {
        discard;
    }
    return vec4<f32>(finish(tex.rgb * in.color * lighting(in.light), in.dist), tex.a);
}

struct LineIn {
    @location(0) pos: vec3<f32>,
    @location(1) color: vec4<f32>,
};

struct LineOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) color: vec4<f32>,
};

@vertex
fn vs_line(v: LineIn) -> LineOut {
    var out: LineOut;
    out.clip = globals.view_proj * vec4<f32>(v.pos, 1.0);
    out.color = v.color;
    return out;
}

@fragment
fn fs_line(in: LineOut) -> @location(0) vec4<f32> {
    return vec4<f32>(pow(in.color.rgb, vec3<f32>(globals.params.w)), in.color.a);
}
