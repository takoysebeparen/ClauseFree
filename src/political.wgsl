// Политическая карта: цвет провинции берётся из палитры по id контроллера,
// границы (провинций внутри страны и государств / побережья) строятся по соседним текселям.
#import bevy_sprite::mesh2d_vertex_output::VertexOutput

// id провинции на пиксель: R = младший байт, G = старший байт (текстура статична)
@group(2) @binding(0) var ids: texture_2d<f32>;
// палитра: тексель (id % 256, id / 256) = цвет контроллера; a = 1 суша, a = 0 вода / без стейта
@group(2) @binding(1) var palette: texture_2d<f32>;

const PALETTE_WIDTH: i32 = 256;
// [25, 25, 25] в sRGB, переведённый в линейное пространство
const COUNTRY_BORDER: vec3<f32> = vec3<f32>(0.0097, 0.0097, 0.0097);
// 0.75 в sRGB-пространстве = 0.53 в линейном
const PROVINCE_SHADE: f32 = 0.53;

fn province_at(p: vec2<i32>, dims: vec2<i32>) -> i32 {
    let q = clamp(p, vec2<i32>(0, 0), dims - vec2<i32>(1, 1));
    let t = textureLoad(ids, q, 0);
    return i32(round(t.r * 255.0)) + i32(round(t.g * 255.0)) * 256;
}

fn color_of(id: i32) -> vec4<f32> {
    return textureLoad(palette, vec2<i32>(id % PALETTE_WIDTH, id / PALETTE_WIDTH), 0);
}

@fragment
fn fragment(in: VertexOutput) -> @location(0) vec4<f32> {
    let dims = vec2<i32>(textureDimensions(ids));
    let p = clamp(vec2<i32>(in.uv * vec2<f32>(dims)), vec2<i32>(0, 0), dims - vec2<i32>(1, 1));
    let id = province_at(p, dims);
    let base = color_of(id);

    // Вода и провинции без стейта: без границ
    if (base.a < 0.5) {
        return vec4<f32>(base.rgb, 1.0);
    }

    var offsets = array<vec2<i32>, 4>(
        vec2<i32>(-1, 0),
        vec2<i32>(1, 0),
        vec2<i32>(0, -1),
        vec2<i32>(0, 1),
    );
    var country_border = false;
    var province_border = false;
    for (var k = 0; k < 4; k = k + 1) {
        let nid = province_at(p + offsets[k], dims);
        if (nid == id) {
            continue;
        }
        let nc = color_of(nid);
        let diff = abs(nc.rgb - base.rgb);
        if (nc.a < 0.5 || max(diff.x, max(diff.y, diff.z)) > 0.001) {
            country_border = true;   // другой контроллер или вода / побережье
        } else {
            province_border = true;  // граница провинций внутри одной страны
        }
    }

    if (country_border) {
        return vec4<f32>(COUNTRY_BORDER, 1.0);
    }
    if (province_border) {
        return vec4<f32>(base.rgb * PROVINCE_SHADE, 1.0);
    }
    return vec4<f32>(base.rgb, 1.0);
}
