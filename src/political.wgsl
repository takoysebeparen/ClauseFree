// Политическая карта: цвет провинции берётся из палитры по id контроллера,
// границы (провинций внутри страны и государств / побережья) строятся по соседним текселям.
#import bevy_sprite::mesh2d_vertex_output::VertexOutput

// id провинции на пиксель: R = младший байт, G = старший байт (текстура статична)
@group(2) @binding(0) var ids: texture_2d<f32>;
// палитра: тексель (id % 256, id / 256) = цвет контроллера; a = 1 суша, a = 0 вода / без стейта
@group(2) @binding(1) var palette: texture_2d<f32>;
// карта высот (R8Unorm); текстура 1x1 — рельефа нет (файла не было), карта остаётся плоской
@group(2) @binding(2) var heights: texture_2d<f32>;
@group(2) @binding(3) var heights_sampler: sampler;

// маска рек (R8Unorm, >0 = река); текстура 1x1 — рек нет (файла не было)
@group(2) @binding(4) var rivers: texture_2d<f32>;
@group(2) @binding(5) var rivers_sampler: sampler;

const PALETTE_WIDTH: i32 = 256;
// Глубокий сине-голубой цвет русла и максимальная доля подмешивания (русла в маске тонкие — 1-3 пикселя).
const RIVER_COLOR: vec3<f32> = vec3<f32>(0.16, 0.34, 0.56);
const RIVER_STRENGTH: f32 = 0.9;
// Река чуть темнеет на тенях рельефа, но не светлеет на склонах так же сильно, как суша.
const RIVER_SHADE_FOLLOW: f32 = 0.5;
// [25, 25, 25] в sRGB, переведённый в линейное пространство
const COUNTRY_BORDER: vec3<f32> = vec3<f32>(0.0097, 0.0097, 0.0097);
// 0.75 в sRGB-пространстве = 0.53 в линейном
const PROVINCE_SHADE: f32 = 0.53;
// Затухание сетки провинций при отдалении: масштаб = текселей карты на пиксель экрана (по fwidth(uv)).
// До FADE_START (приближение) сетка в полную силу, к FADE_END (стратегический зум) почти исчезает.
const FADE_START: f32 = 1.2;
const FADE_END: f32 = 4.0;
// Яркость внутренней границы при полном затухании (1.0 = границы нет).
const PROVINCE_SHADE_FAR: f32 = 0.97;

// Рельеф. Уровень моря в heightmap.bmp ~ 95 из 255: всё ниже считается берегом, чтобы дно моря
// не давало ложных склонов на побережье.
const SEA_LEVEL: f32 = 95.0 / 255.0;
// Крутизна рельефа: 1 уровень яркости карты высот на пиксель даёт этот наклон нормали.
const RELIEF_SCALE: f32 = 0.2;
// Сила оттенения и допустимый разброс яркости (горы не уходят в чёрное и не выгорают).
const SHADE_GAIN: f32 = 0.9;
const SHADE_MIN: f32 = 0.65;
const SHADE_MAX: f32 = 1.3;
// Направление на источник света в координатах изображения (x на восток, y на юг, z вверх): северо-запад, 45 над горизонтом.
const LIGHT_DIR: vec3<f32> = vec3<f32>(-0.5, -0.5, 0.7071);

fn height_at(uv: vec2<f32>) -> f32 {
    // билинейная выборка сглаживает 8-битные ступени высот
    return max(textureSampleLevel(heights, heights_sampler, uv, 0.0).r, SEA_LEVEL) * 255.0;
}

// Множитель яркости по hillshading: нормаль из градиента высот соседних текселей, свет с северо-запада.
// Плоская местность даёт ровно 1.0.
fn relief_shade(uv: vec2<f32>) -> f32 {
    let hd = vec2<f32>(textureDimensions(heights));
    if (hd.x <= 1.0) {
        return 1.0;
    }
    let tx = vec2<f32>(1.0 / hd.x, 0.0);
    let ty = vec2<f32>(0.0, 1.0 / hd.y);
    let gx = (height_at(uv + tx) - height_at(uv - tx)) * 0.5;
    let gy = (height_at(uv + ty) - height_at(uv - ty)) * 0.5;
    let n = normalize(vec3<f32>(-gx * RELIEF_SCALE, -gy * RELIEF_SCALE, 1.0));
    let l = normalize(LIGHT_DIR);
    return clamp(1.0 + (dot(n, l) - l.z) * SHADE_GAIN, SHADE_MIN, SHADE_MAX);
}

// Доля реки в точке: билинейная выборка маски с лёгким сглаживанием по соседним текселям (усиливает тонкие русла).
fn river_amount(uv: vec2<f32>) -> f32 {
    let rd = vec2<f32>(textureDimensions(rivers));
    if (rd.x <= 1.0) {
        return 0.0;
    }
    let px = vec2<f32>(1.0 / rd.x, 1.0 / rd.y);
    let center = textureSampleLevel(rivers, rivers_sampler, uv, 0.0).r;
    let around = (textureSampleLevel(rivers, rivers_sampler, uv + vec2<f32>(px.x, 0.0), 0.0).r
        + textureSampleLevel(rivers, rivers_sampler, uv - vec2<f32>(px.x, 0.0), 0.0).r
        + textureSampleLevel(rivers, rivers_sampler, uv + vec2<f32>(0.0, px.y), 0.0).r
        + textureSampleLevel(rivers, rivers_sampler, uv - vec2<f32>(0.0, px.y), 0.0).r) * 0.25;
    return clamp(center * 0.6 + around * 0.8, 0.0, 1.0);
}

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
    // производные UV берутся до любых ранних возвратов (однородный поток управления)
    let texels_per_pixel = max(fwidth(in.uv.x) * f32(dims.x), fwidth(in.uv.y) * f32(dims.y));
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
    // Рельеф только на суше (вода вернулась выше); границы государств остаются чёткими
    let shade = relief_shade(in.uv);
    // Реки на суше: подмешиваем сине-голубой цвет, приглушённый рельефом (hillshading)
    let river = river_amount(in.uv) * RIVER_STRENGTH;
    let land = base.rgb * shade;
    let water = RIVER_COLOR * mix(1.0, shade, RIVER_SHADE_FOLLOW);
    let color = mix(land, water, river);
    if (province_border) {
        // внутренние границы гаснут при отдалении; границы государств (выше) остаются чёткими всегда
        let fade = smoothstep(FADE_START, FADE_END, texels_per_pixel);
        return vec4<f32>(color * mix(PROVINCE_SHADE, PROVINCE_SHADE_FAR, fade), 1.0);
    }
    return vec4<f32>(color, 1.0);
}
