//! Картографический шрифт игры (`gfx/fonts/hoi_mapfont4.fnt` + `.dds`) и размещение названий стран вдоль
//! «хребта» территории. Только слой отображения: симуляция не затрагивается.
//!
//! - `BmFont` — разбор текстового формата BMFont (бинарный не поддерживается: в игре карта использует текстовый).
//! - `MapFont` — шрифт и атлас покрытия глифов (несжатый 32-битный `.dds`; DXT не нужен: атлас карты несжатый).
//! - Геометрия: главная ось инерции (PCA) центров провинций, сглаженный хребет по сегментам кластера,
//!   подбор мирового размера глифа (слово не длиннее `0.75 L`, буквы не выше `0.65 W`) и раскладка букв по дуге.
//! - Прозрачность и видимость при зуме: `watermark_alpha`, `label_visible`.
//!
//! Размер надписи задан в мировых координатах карты, поэтому на экране буквы растут вместе с провинциями.

use std::collections::BTreeMap;
use std::path::Path;

use bevy::prelude::*;

use crate::ui::decode_dds;

/// Слово занимает не более этой доли длины территории.
pub const WORD_FILL: f32 = 0.75;
/// Высота заглавных букв не более этой доли средней толщины территории.
pub const HEIGHT_FILL: f32 = 0.65;
/// Доля длины хребта, которую слово не должно превосходить (запас на изгиб).
const RIDGE_FILL: f32 = 0.9;
/// Трёхфазная кривая прозрачности по масштабу камеры (мировых единиц на пиксель).
/// Тактический зум: водяной знак 0.16 (scale <= 0.25) -> 0.68 (scale = 0.55).
pub const TACTICAL_FULL: f32 = 0.25;
pub const TACTICAL_END: f32 = 0.55;
pub const ALPHA_TACTICAL_MIN: f32 = 0.16;
pub const ALPHA_TACTICAL_END: f32 = 0.68;
/// Региональный зум 0.55..=1.65: пик читаемости 0.75 (на 0.75..=1.2), к концу зоны 0.72.
pub const REGIONAL_PEAK_FROM: f32 = 0.75;
pub const REGIONAL_PEAK_TO: f32 = 1.2;
pub const REGIONAL_END: f32 = 1.65;
pub const ALPHA_PEAK: f32 = 0.75;
pub const ALPHA_REGIONAL_END: f32 = 0.72;
/// Планетарный зум: названия растворяются от 0.72 (scale = 1.65) до 0 (scale >= 2.6).
pub const PLANETARY_GONE: f32 = 2.6;
/// Буквы ниже этой экранной высоты (px) не рисуются: мелкая страна на сильном отдалении не превращается в шум.
pub const MIN_SCREEN_LETTER_PX: f32 = 7.0;
/// Добавка межбуквенного интервала (доля ширины глифа) для коротких названий: чем короче слово, тем шире разрядка.
pub fn tracking_for(chars: usize) -> f32 {
    match chars {
        0..=7 => 0.25,
        8..=10 => 0.20,
        11..=13 => 0.15,
        _ => 0.0,
    }
}

// ---------------------------------------------------------------------------
// BMFont
// ---------------------------------------------------------------------------

/// Глиф шрифта в пикселях атласа.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Glyph {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
    pub xoffset: i32,
    pub yoffset: i32,
    pub xadvance: i32,
}

/// Шрифт BMFont: метрики и глифы по коду символа.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct BmFont {
    pub line_height: u32,
    pub base: u32,
    pub scale_w: u32,
    pub scale_h: u32,
    pub glyphs: BTreeMap<u32, Glyph>,
}

/// `key=value` пары строки BMFont.
fn pairs(line: &str) -> BTreeMap<&str, &str> {
    line.split_whitespace().skip(1).filter_map(|t| t.split_once('=')).collect()
}

/// Разбирает текстовый `.fnt`. Нет `common` или глифов, бинарный файл и мусор — `None`.
pub fn parse_bmfont(text: &str) -> Option<BmFont> {
    let mut font = BmFont::default();
    let mut have_common = false;
    for line in text.trim_start_matches('\u{feff}').lines() {
        let line = line.trim();
        let kv = pairs(line);
        let num = |key: &str| kv.get(key).and_then(|v| v.parse::<i64>().ok());
        if line.starts_with("common ") {
            font.line_height = u32::try_from(num("lineHeight")?).ok().filter(|&h| h > 0)?;
            font.base = u32::try_from(num("base").unwrap_or(0)).unwrap_or(0);
            font.scale_w = u32::try_from(num("scaleW")?).ok().filter(|&w| w > 0)?;
            font.scale_h = u32::try_from(num("scaleH")?).ok().filter(|&h| h > 0)?;
            have_common = true;
        } else if line.starts_with("char ") {
            let (Some(id), Some(x), Some(y), Some(w), Some(h)) = (num("id"), num("x"), num("y"), num("width"), num("height")) else {
                continue;
            };
            let glyph = Glyph {
                x: u32::try_from(x).ok()?,
                y: u32::try_from(y).ok()?,
                width: u32::try_from(w).ok()?,
                height: u32::try_from(h).ok()?,
                xoffset: i32::try_from(num("xoffset").unwrap_or(0)).ok()?,
                yoffset: i32::try_from(num("yoffset").unwrap_or(0)).ok()?,
                xadvance: i32::try_from(num("xadvance").unwrap_or(0)).ok()?,
            };
            font.glyphs.insert(u32::try_from(id).ok()?, glyph);
        }
    }
    (have_common && !font.glyphs.is_empty()).then_some(font)
}

impl BmFont {
    /// Высота заглавных в долях высоты строки (по глифу `H`); без него — типичные 0.65.
    pub fn cap_ratio(&self) -> f32 {
        self.glyphs.get(&u32::from('H')).map_or(0.65, |g| g.height as f32 / self.line_height as f32)
    }

    /// Ширина слова в высотах строки (сумма продвижений глифов; нет глифа — треть строки).
    #[cfg(test)]
    pub fn width_em(&self, text: &str) -> f32 {
        let units: f32 = text
            .chars()
            .map(|c| self.glyphs.get(&u32::from(c)).map_or(self.line_height as f32 / 3.0, |g| g.xadvance as f32))
            .sum();
        units / self.line_height as f32
    }
}

impl BmFont {
    /// Ширина слова с межбуквенным интервалом (`tracking_for` от числа букв) в высотах строки.
    pub fn text_width_em(&self, text: &str) -> f32 {
        let chars = text.chars().count();
        let units: f32 = text
            .chars()
            .map(|c| self.glyphs.get(&u32::from(c)).map_or(self.line_height as f32 / 3.0, |g| g.xadvance as f32))
            .sum();
        units * (1.0 + tracking_for(chars)) / self.line_height as f32
    }
}

/// Картографический шрифт: метрики и покрытие атласа (один байт на пиксель).
#[derive(Debug, Clone)]
pub struct MapFont {
    pub font: BmFont,
    pub width: usize,
    pub height: usize,
    /// Покрытие тела глифа 0..=255 для пикселей атласа.
    pub coverage: Vec<u8>,
    /// Покрытие светлого ореола вокруг букв 0..=255 (из того же атласа).
    pub halo: Vec<u8>,
}

/// Ресурс шрифта карты; `None` — шрифта нет, надписи рисуются шрифтом egui.
#[derive(Resource, Default, Clone)]
pub struct MapFontRes(pub Option<MapFont>);

/// Покрытие из пикселя атласа игры: тёмная заливка глифа (R≈0) на полупрозрачной подложке (A≈128), светлый
/// ореол (R≈255) покрытием не считается — получается чистый глиф без обводки.
pub fn glyph_coverage(r: u8, a: u8) -> u8 {
    let ink = (255 - u32::from(r)) * u32::from(a) * 2 / 255;
    ink.min(255) as u8
}

/// Покрытие светлого ореола: в атласе игры он лежит в тех же пикселях светлым цветом (R≈255) с альфой,
/// убывающей от тела буквы наружу. Тело и ореол в сумме дают полное покрытие пикселя.
pub fn glyph_halo(r: u8, a: u8) -> u8 {
    // сам ореол в атласе мягкий (альфа до ~0.5), поэтому усилен в 1.6 раза до читаемой подложки
    let light = u32::from(r) * u32::from(a) * 2 * 8 / (255 * 5);
    light.min(255) as u8
}

/// Читает `gfx/fonts/hoi_mapfont4.{fnt,dds}`. Нет файлов, сжатый или битый атлас — `None`.
pub fn load_map_font(root: &Path) -> Option<MapFont> {
    let dir = root.join("gfx").join("fonts");
    let font = parse_bmfont(&String::from_utf8_lossy(&std::fs::read(dir.join("hoi_mapfont4.fnt")).ok()?))?;
    let atlas = decode_dds(&std::fs::read(dir.join("hoi_mapfont4.dds")).ok()?)?;
    if atlas.width != font.scale_w as usize || atlas.height != font.scale_h as usize {
        return None;
    }
    let coverage = atlas.rgba.chunks_exact(4).map(|p| glyph_coverage(p[0], p[3])).collect();
    let halo = atlas.rgba.chunks_exact(4).map(|p| glyph_halo(p[0], p[3])).collect();
    Some(MapFont { font, width: atlas.width, height: atlas.height, coverage, halo })
}

// ---------------------------------------------------------------------------
// Геометрия территории
// ---------------------------------------------------------------------------

/// Главная ось территории.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Axis {
    /// Среднее центров провинций.
    pub center: Vec2,
    /// Угол оси к оси X, радианы в (-π/2, π/2]: текст читается слева направо.
    pub angle: f32,
    /// Длина вдоль оси `L` (с запасом на размер крайних провинций).
    pub length: f32,
    /// Средняя толщина поперёк оси `W`.
    pub thickness: f32,
}

impl Axis {
    pub fn direction(&self) -> Vec2 {
        Vec2::new(self.angle.cos(), self.angle.sin())
    }
}

/// PCA центров провинций: ковариационная матрица -> угол главной оси; длина — размах проекций,
/// толщина — учетверённое среднее поперечное отклонение (для равномерной полосы ширины w оно равно w/4);
/// к обеим добавлена оценка размера провинции (шаг между центрами), чтобы крайние провинции были учтены.
pub fn principal_axis(points: &[Vec2]) -> Option<Axis> {
    if points.is_empty() {
        return None;
    }
    let n = points.len() as f32;
    let center = points.iter().fold(Vec2::ZERO, |a, &p| a + p) / n;
    let (mut sxx, mut sxy, mut syy) = (0.0_f32, 0.0_f32, 0.0_f32);
    for &p in points {
        let d = p - center;
        sxx += d.x * d.x;
        sxy += d.x * d.y;
        syy += d.y * d.y;
    }
    let angle = 0.5 * (2.0 * sxy).atan2(sxx - syy);
    // atan2 даёт угол в (-π/2, π/2]
    let u = Vec2::new(angle.cos(), angle.sin());
    let v = Vec2::new(-u.y, u.x);
    let (mut tmin, mut tmax, mut across) = (f32::MAX, f32::MIN, 0.0_f32);
    for &p in points {
        let d = p - center;
        let (t, s) = (d.dot(u), d.dot(v));
        tmin = tmin.min(t);
        tmax = tmax.max(t);
        across += s.abs();
    }
    let raw_length = tmax - tmin;
    let raw_thickness = 4.0 * across / n;
    let spacing = (raw_length.max(1.0) * raw_thickness.max(1.0) / n).sqrt();
    Some(Axis { center, angle, length: raw_length + spacing, thickness: raw_thickness + spacing })
}

/// Сглаженная кривая вдоль территории с длиной по дуге.
#[derive(Debug, Clone, PartialEq)]
pub struct Ridge {
    pub points: Vec<Vec2>,
    cumulative: Vec<f32>,
}

impl Ridge {
    pub fn new(points: Vec<Vec2>) -> Self {
        let mut cumulative = Vec::with_capacity(points.len());
        let mut total = 0.0;
        for (i, p) in points.iter().enumerate() {
            if i > 0 {
                total += p.distance(points[i - 1]);
            }
            cumulative.push(total);
        }
        Self { points, cumulative }
    }

    pub fn length(&self) -> f32 {
        self.cumulative.last().copied().unwrap_or(0.0)
    }

    /// Точка и единичная касательная на расстоянии `s` по дуге (за пределами — на концах).
    pub fn at(&self, s: f32) -> (Vec2, Vec2) {
        let (Some(&first), Some(&last)) = (self.points.first(), self.points.last()) else { return (Vec2::ZERO, Vec2::X) };
        if self.points.len() < 2 {
            return (first, Vec2::X);
        }
        let s = s.clamp(0.0, self.length());
        let i = self.cumulative.partition_point(|&c| c < s).clamp(1, self.points.len() - 1);
        let (a, b) = (self.points[i - 1], self.points[i]);
        let seg = (self.cumulative[i] - self.cumulative[i - 1]).max(1e-6);
        let t = ((s - self.cumulative[i - 1]) / seg).clamp(0.0, 1.0);
        let dir = (b - a).try_normalize().unwrap_or_else(|| (last - first).try_normalize().unwrap_or(Vec2::X));
        (a.lerp(b, t), dir)
    }
}

/// Сглаживание углов (Чайкин): каждая итерация срезает углы ломаной, концы остаются на месте.
fn chaikin(points: &[Vec2], iterations: u32) -> Vec<Vec2> {
    let mut current = points.to_vec();
    for _ in 0..iterations {
        if current.len() < 3 {
            break;
        }
        let mut next = vec![current[0]];
        for w in current.windows(2) {
            next.push(w[0] * 0.75 + w[1] * 0.25);
            next.push(w[0] * 0.25 + w[1] * 0.75);
        }
        next.push(current[current.len() - 1]);
        current = next;
    }
    current
}

/// Хребет: кластер режется вдоль главной оси на K = 3..=5 сегментов, центроиды сегментов взвешены по площади
/// провинций (`weights`, пиксели; пустой или неподходящий по длине список — равные веса) и сглаживаются кривой.
/// Концы кривой доведены до крайних проекций, поэтому длина хребта не меньше размаха.
pub fn build_ridge(points: &[Vec2], weights: &[f32], axis: &Axis) -> Ridge {
    let u = axis.direction();
    let v = Vec2::new(-u.y, u.x);
    if points.is_empty() {
        return Ridge::new(vec![axis.center - u * axis.length * 0.5, axis.center + u * axis.length * 0.5]);
    }
    let weight_of = |i: usize| if weights.len() == points.len() { weights[i].max(1e-3) } else { 1.0 };
    let projected: Vec<(f32, f32)> = points.iter().map(|&p| ((p - axis.center).dot(u), (p - axis.center).dot(v))).collect();
    let tmin = projected.iter().map(|p| p.0).fold(f32::MAX, f32::min);
    let tmax = projected.iter().map(|p| p.0).fold(f32::MIN, f32::max);
    let total_weight: f32 = (0..projected.len()).map(weight_of).sum();
    let mean_s = projected.iter().enumerate().map(|(i, p)| p.1 * weight_of(i)).sum::<f32>() / total_weight;
    let k = (points.len() / 4).clamp(3, 5);
    let step = ((tmax - tmin) / k as f32).max(1e-3);
    let mut centroids: Vec<(f32, f32)> = Vec::with_capacity(k);
    for bin in 0..k {
        let (lo, hi) = (tmin + step * bin as f32, tmin + step * (bin + 1) as f32);
        let members: Vec<(usize, &(f32, f32))> =
            projected.iter().enumerate().filter(|(_, p)| p.0 >= lo && (p.0 < hi || (bin == k - 1 && p.0 <= hi))).collect();
        if members.is_empty() {
            centroids.push(((lo + hi) * 0.5, mean_s));
        } else {
            let w: f32 = members.iter().map(|&(i, _)| weight_of(i)).sum();
            centroids.push((
                members.iter().map(|&(i, p)| p.0 * weight_of(i)).sum::<f32>() / w,
                members.iter().map(|&(i, p)| p.1 * weight_of(i)).sum::<f32>() / w,
            ));
        }
    }
    // концы — на крайних проекциях с поперечным смещением крайних сегментов
    let mut knots: Vec<Vec2> = Vec::with_capacity(k + 2);
    let to_world = |t: f32, s: f32| axis.center + u * t + v * s;
    knots.push(to_world(tmin, centroids[0].1));
    knots.extend(centroids.iter().map(|&(t, s)| to_world(t, s)));
    knots.push(to_world(tmax, centroids[k - 1].1));
    Ridge::new(chaikin(&knots, 3))
}

// ---------------------------------------------------------------------------
// Размер глифа и раскладка букв
// ---------------------------------------------------------------------------

/// Метрики слова для подбора размера: ширина в высотах строки и высота заглавных в долях строки.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TextMetrics {
    pub width_em: f32,
    pub cap_ratio: f32,
}

impl TextMetrics {
    /// Метрики шрифта игры; без шрифта — приблизительные метрики шрифта egui.
    pub fn of(font: Option<&MapFont>, text: &str) -> Self {
        match font {
            Some(f) => Self { width_em: f.font.text_width_em(text), cap_ratio: f.font.cap_ratio() },
            None => {
                let chars = text.chars().count();
                Self { width_em: chars as f32 * 0.62 * (1.0 + tracking_for(chars)), cap_ratio: 0.7 }
            }
        }
    }
}

/// Мировой размер глифа (высота строки): слово не длиннее `0.75 L`, заглавные не выше `0.65 W`,
/// и слово помещается на хребте. Нулевые и нечисловые входы дают 0 (надпись не рисуется).
pub fn fit_glyph_size(axis: &Axis, ridge_length: f32, m: &TextMetrics) -> f32 {
    if !(axis.length > 0.0 && axis.thickness > 0.0 && m.width_em > 0.0 && m.cap_ratio > 0.0 && ridge_length.is_finite()) {
        return 0.0;
    }
    let by_length = WORD_FILL * axis.length / m.width_em;
    let by_height = HEIGHT_FILL * axis.thickness / m.cap_ratio;
    let by_ridge = RIDGE_FILL * ridge_length / m.width_em;
    by_length.min(by_height).min(by_ridge)
}

/// Квадрат буквы: углы в мировых координатах и UV в атласе (порядок: верх-лево, верх-право, низ-право, низ-лево).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GlyphQuad {
    pub corners: [Vec2; 4],
    pub uv: [Vec2; 4],
}

/// Раскладывает слово по хребту: слово по центру хребта, каждая буква — по касательной в своей точке.
/// Размер `size` — высота строки в мировых единицах. Символы без глифа пропускаются (занимают треть строки).
pub fn layout_on_ridge(font: &BmFont, text: &str, ridge: &Ridge, size: f32) -> Vec<GlyphQuad> {
    if size <= 0.0 || font.line_height == 0 || ridge.points.len() < 2 {
        return Vec::new();
    }
    let k = size / font.line_height as f32;
    let tracking = 1.0 + tracking_for(text.chars().count());
    let total: f32 = font.text_width_em(text) * size;
    let start = (ridge.length() - total) * 0.5;
    // вертикальный центр заглавных буквы относительно верха строки
    let cap_h = font.glyphs.get(&u32::from('H')).map_or(font.line_height as f32 * 0.65, |g| g.height as f32);
    let cap_top = font.glyphs.get(&u32::from('H')).map_or(font.line_height as f32 * 0.2, |g| g.yoffset as f32);
    let ref_y = cap_top + cap_h * 0.5;
    let mut pen = 0.0_f32;
    let mut quads = Vec::new();
    for c in text.chars() {
        let Some(g) = font.glyphs.get(&u32::from(c)) else {
            pen += font.line_height as f32 / 3.0 * tracking;
            continue;
        };
        let advance = g.xadvance as f32;
        let cell = advance * tracking;
        if g.width > 0 && g.height > 0 {
            let s = start + (pen + cell * 0.5) * k;
            let (pos, tangent) = ridge.at(s);
            let right = tangent;
            let up = Vec2::new(-tangent.y, tangent.x);
            // локальные координаты глифа относительно центра продвижения; y шрифта направлен вниз
            let x0 = g.xoffset as f32 - advance * 0.5;
            let x1 = x0 + g.width as f32;
            let y0 = g.yoffset as f32 - ref_y;
            let y1 = y0 + g.height as f32;
            let world = |x: f32, y: f32| pos + (right * x - up * y) * k;
            let (sw, sh) = (font.scale_w as f32, font.scale_h as f32);
            let (u0, u1) = (g.x as f32 / sw, (g.x + g.width) as f32 / sw);
            let (v0, v1) = (g.y as f32 / sh, (g.y + g.height) as f32 / sh);
            quads.push(GlyphQuad {
                corners: [world(x0, y0), world(x1, y0), world(x1, y1), world(x0, y1)],
                uv: [Vec2::new(u0, v0), Vec2::new(u1, v0), Vec2::new(u1, v1), Vec2::new(u0, v1)],
            });
        }
        pen += cell;
    }
    quads
}

// ---------------------------------------------------------------------------
// Прозрачность и видимость при зуме
// ---------------------------------------------------------------------------

fn lerp_between(x: f32, x0: f32, x1: f32, y0: f32, y1: f32) -> f32 {
    y0 + (y1 - y0) * ((x - x0) / (x1 - x0)).clamp(0.0, 1.0)
}

/// Трёхфазная прозрачность надписи по масштабу камеры.
/// - тактический (`scale < 0.55`): водяной знак 0.16 (scale <= 0.25) -> 0.68 (scale = 0.55), надпись не исчезает;
/// - региональный (0.55..=1.65): максимум читаемости 0.75, к концу зоны 0.72;
/// - планетарный (`scale > 1.65`): плавное растворение от 0.72 до 0 (scale >= 2.6).
pub fn watermark_alpha(camera_scale: f32) -> f32 {
    let x = camera_scale;
    if !x.is_finite() {
        return ALPHA_PEAK;
    }
    if x <= TACTICAL_END {
        lerp_between(x, TACTICAL_FULL, TACTICAL_END, ALPHA_TACTICAL_MIN, ALPHA_TACTICAL_END)
    } else if x < REGIONAL_PEAK_FROM {
        lerp_between(x, TACTICAL_END, REGIONAL_PEAK_FROM, ALPHA_TACTICAL_END, ALPHA_PEAK)
    } else if x <= REGIONAL_PEAK_TO {
        ALPHA_PEAK
    } else if x <= REGIONAL_END {
        lerp_between(x, REGIONAL_PEAK_TO, REGIONAL_END, ALPHA_PEAK, ALPHA_REGIONAL_END)
    } else {
        lerp_between(x, REGIONAL_END, PLANETARY_GONE, ALPHA_REGIONAL_END, 0.0)
    }
}

/// Экранная высота заглавных букв, пиксели: размер глифа в мире / масштаб камеры × доля заглавных.
pub fn screen_letter_height(world_glyph_size: f32, cap_ratio: f32, camera_scale: f32) -> f32 {
    if camera_scale <= 0.0 || !camera_scale.is_finite() {
        return 0.0;
    }
    world_glyph_size * cap_ratio / camera_scale
}

/// Рисовать ли надпись: скрывается только при буквах ниже 7 px (мелкая страна на сильном отдалении).
pub fn label_visible(screen_letter_px: f32) -> bool {
    screen_letter_px >= MIN_SCREEN_LETTER_PX
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE_FNT: &str = "info face=\"Tahoma\" size=84 bold=1 italic=0 charset=\"\" unicode=1\ncommon lineHeight=84 base=70 scaleW=2048 scaleH=1024 pages=1\npage id=0 file=\"a.dds\"\nchars count=4\nchar id=72   x=10  y=20  width=40 height=55 xoffset=2 yoffset=17 xadvance=50 page=0 chnl=15\nchar id=73   x=60  y=20  width=10 height=55 xoffset=1 yoffset=17 xadvance=14 page=0 chnl=15\nchar id=32   x=0   y=0   width=0  height=0  xoffset=0 yoffset=0 xadvance=20 page=0 chnl=15\nchar id=bad x=1\n";

    fn font() -> BmFont {
        parse_bmfont(SAMPLE_FNT).expect("шрифт")
    }

    #[test]
    fn bmfont_text_is_parsed() {
        let f = font();
        assert_eq!((f.line_height, f.base, f.scale_w, f.scale_h), (84, 70, 2048, 1024));
        assert_eq!(f.glyphs.len(), 3, "глиф с нечисловым id пропущен");
        let h = f.glyphs[&72];
        assert_eq!((h.x, h.y, h.width, h.height, h.xoffset, h.yoffset, h.xadvance), (10, 20, 40, 55, 2, 17, 50));
        assert!((f.cap_ratio() - 55.0 / 84.0).abs() < 1e-4);
        // ширина слова: H 50 + I 14 = 64 / 84; неизвестный символ — треть строки
        assert!((f.width_em("HI") - 64.0 / 84.0).abs() < 1e-4);
        // короткое слово (2 буквы) растянуто на 25%
        assert!((f.text_width_em("HI") - 64.0 / 84.0 * 1.25).abs() < 1e-4);
        assert!((f.width_em("?") - 1.0 / 3.0).abs() < 1e-4);
    }

    #[test]
    fn broken_font_files_never_panic() {
        assert!(parse_bmfont("").is_none());
        assert!(parse_bmfont("common lineHeight=84 base=70 scaleW=2048 scaleH=1024").is_none(), "нет глифов");
        assert!(parse_bmfont("char id=65 x=0 y=0 width=1 height=1").is_none(), "нет common");
        assert!(parse_bmfont("common lineHeight=0 scaleW=1 scaleH=1\nchar id=65 x=0 y=0 width=1 height=1").is_none());
        assert!(parse_bmfont("BMF\u{3}\u{0}\u{0}garbage").is_none(), "бинарный формат не поддерживается");
        assert!(load_map_font(Path::new("Z:\\нет\\игры")).is_none(), "нет папки игры -> запасной шрифт");
        assert!(MapFontRes::default().0.is_none());
    }

    #[test]
    fn atlas_coverage_keeps_the_dark_fill_and_drops_the_halo() {
        assert_eq!(glyph_halo(255, 128), 255, "светлый ореол — отдельное покрытие");
        assert_eq!(glyph_halo(0, 128), 0, "в теле буквы ореола нет");
        assert!(glyph_halo(111, 122) > 90 && glyph_coverage(111, 122) > 120, "край: тело и ореол делят пиксель");
        assert_eq!(glyph_coverage(0, 128), 255, "заливка глифа");
        assert_eq!(glyph_coverage(255, 128), 0, "светлый ореол не считается");
        assert_eq!(glyph_coverage(0, 0), 0, "прозрачный фон");
        assert!(glyph_coverage(128, 128) > 100 && glyph_coverage(128, 128) < 140, "сглаженный край");
    }

    /// Реальный шрифт карты (пропускается без игры): кириллица, латиница и размер атласа.
    #[test]
    fn real_map_font_has_cyrillic() {
        let root = Path::new("D:\\SteamLibrary\\steamapps\\common\\Hearts of Iron IV");
        if !root.exists() {
            return;
        }
        let font = load_map_font(root).expect("hoi_mapfont4");
        assert!(font.font.glyphs.len() >= 500);
        for c in "ГЕРМАНИЯСОВЕТСКИЙСОЮЗ ABC".chars() {
            assert!(font.font.glyphs.contains_key(&u32::from(c)), "нет глифа {c}");
        }
        assert_eq!((font.width, font.height), (2048, 1024));
        assert_eq!(font.coverage.len(), font.width * font.height);
        assert_eq!(font.halo.len(), font.coverage.len());
        assert!(font.halo.iter().any(|&c| c > 180), "в атласе есть светлый ореол");
        // кириллица читается тем же весом, что и латиница: ширина штриха «Н» равна «H»
        for (latin, cyrillic) in [('H', 'Н')] {
            let (a, b) = (font.font.glyphs[&u32::from(latin)], font.font.glyphs[&u32::from(cyrillic)]);
            assert_eq!((a.width, a.height), (b.width, b.height));
        }
        assert!((0x0410..=0x044F).filter(|c| font.font.glyphs.contains_key(c)).count() >= 60, "кириллица 0x0410..0x044F в шрифте");
        assert!(font.coverage.iter().any(|&c| c > 200), "в атласе есть заливка глифов");
    }

    /// Вытянутый кластер: полоса длиной ~400 и шириной ~60 под углом 30°.
    fn strip(angle_deg: f32, length: f32, width: f32) -> Vec<Vec2> {
        let a = angle_deg.to_radians();
        let (u, v) = (Vec2::new(a.cos(), a.sin()), Vec2::new(-a.sin(), a.cos()));
        let mut pts = Vec::new();
        for i in 0..=40 {
            for j in 0..3 {
                let t = (i as f32 / 40.0 - 0.5) * length;
                let s = (j as f32 - 1.0) * width * 0.5;
                pts.push(Vec2::new(1500.0, 800.0) + u * t + v * s);
            }
        }
        pts
    }

    #[test]
    fn principal_axis_of_a_tilted_cluster() {
        let pts = strip(30.0, 400.0, 60.0);
        let axis = principal_axis(&pts).expect("ось");
        assert!((axis.angle.to_degrees() - 30.0).abs() < 1.0, "угол {}", axis.angle.to_degrees());
        assert!(axis.length > 390.0 && axis.length < 480.0, "L = {}", axis.length);
        assert!(axis.thickness > 40.0 && axis.thickness < 130.0, "W = {}", axis.thickness);
        assert!((axis.center - Vec2::new(1500.0, 800.0)).length() < 1.0);
        // угол всегда в (-90°, 90°]: текст читается слева направо
        for deg in [-80.0_f32, -45.0, 0.0, 45.0, 89.0, 120.0, -120.0] {
            let a = principal_axis(&strip(deg, 300.0, 40.0)).expect("ось").angle.to_degrees();
            assert!(a > -90.0 - 1e-3 && a <= 90.0 + 1e-3, "{deg} -> {a}");
        }
        assert!(principal_axis(&[]).is_none());
        let single = principal_axis(&[Vec2::new(5.0, 5.0)]).expect("одна точка");
        assert!(single.length.is_finite() && single.thickness.is_finite());
    }

    #[test]
    fn word_fits_into_three_quarters_of_the_length_and_below_the_height_limit() {
        let pts = strip(30.0, 400.0, 60.0);
        let axis = principal_axis(&pts).expect("ось");
        let ridge = build_ridge(&pts, &[], &axis);
        assert!(ridge.length() >= 390.0, "хребет не короче размаха: {}", ridge.length());
        let f = font();
        for text in ["HI", "HIHIHIHIHIHIHI", "H"] {
            let metrics = TextMetrics { width_em: f.text_width_em(text), cap_ratio: f.cap_ratio() };
            let size = fit_glyph_size(&axis, ridge.length(), &metrics);
            assert!(size > 0.0);
            let word = metrics.width_em * size;
            assert!(word <= WORD_FILL * axis.length + 1e-3, "слово {word} > 0.75 L {}", WORD_FILL * axis.length);
            assert!(metrics.cap_ratio * size <= HEIGHT_FILL * axis.thickness + 1e-3, "высота букв выше 0.65 W");
            // все буквы лежат на хребте внутри полосы: расстояние до оси не больше толщины
            let u = axis.direction();
            let v = Vec2::new(-u.y, u.x);
            for q in layout_on_ridge(&f, text, &ridge, size) {
                for c in q.corners {
                    assert!(((c - axis.center).dot(v)).abs() <= axis.thickness, "буква вылезла за территорию");
                    assert!(((c - axis.center).dot(u)).abs() <= axis.length * 0.5 + 1e-3, "буква вылезла за конец");
                }
            }
        }
        // вырожденные входы не дают размера
        let m = TextMetrics { width_em: 0.0, cap_ratio: 0.6 };
        assert_eq!(fit_glyph_size(&axis, 100.0, &m), 0.0);
        assert_eq!(fit_glyph_size(&axis, f32::NAN, &TextMetrics { width_em: 1.0, cap_ratio: 0.6 }), 0.0);
    }

    #[test]
    fn letters_follow_the_tangent_of_a_bent_ridge() {
        // дуга: подъём и спад — касательная меняет знак
        let pts: Vec<Vec2> = (0..=30).map(|i| {
            let x = i as f32 * 20.0;
            Vec2::new(x, -(x - 300.0).powi(2) / 600.0)
        }).collect();
        let axis = principal_axis(&pts).expect("ось");
        let ridge = build_ridge(&pts, &[], &axis);
        let f = font();
        let size = fit_glyph_size(&axis, ridge.length(), &TextMetrics { width_em: f.text_width_em("HIHIHI"), cap_ratio: f.cap_ratio() });
        let quads = layout_on_ridge(&f, "HIHIHI", &ridge, size);
        assert_eq!(quads.len(), 6);
        let slope = |q: &GlyphQuad| {
            let top = q.corners[1] - q.corners[0];
            top.y.atan2(top.x).to_degrees()
        };
        assert!(slope(&quads[0]) > slope(&quads[5]) + 1.0, "первая буква выше по наклону, чем последняя: {} / {}", slope(&quads[0]), slope(&quads[5]));
        // горизонтальный хребет: прямые прямоугольники, UV из атласа
        let flat = Ridge::new(vec![Vec2::new(0.0, 0.0), Vec2::new(1000.0, 0.0)]);
        let one = layout_on_ridge(&f, "H", &flat, 84.0);
        let q = one[0];
        assert!((q.corners[0].y - q.corners[1].y).abs() < 1e-3 && (q.corners[0].y - q.corners[3].y) > 1.0, "верх вверху (мир: Y вверх — низ меньше по Y)");
        assert!((q.corners[1].x - q.corners[0].x - 40.0).abs() < 1e-3, "ширина глифа 40 при размере = высоте строки");
        assert_eq!(q.uv[0], Vec2::new(10.0 / 2048.0, 20.0 / 1024.0));
        assert!(layout_on_ridge(&f, "H", &flat, 0.0).is_empty());
    }

    #[test]
    fn three_phase_alpha_curve() {
        // тактический зум: водяной знак ~0.16 у самой земли, 0.68 на границе зоны, не исчезает
        assert!((watermark_alpha(0.25) - 0.16).abs() < 1e-4);
        assert!((watermark_alpha(0.05) - 0.16).abs() < 1e-4, "ниже 0.25 остаётся 0.16, а не ноль");
        assert!((watermark_alpha(0.55) - 0.68).abs() < 1e-4);
        let mid = watermark_alpha(0.4);
        assert!(mid > 0.16 && mid < 0.68, "{mid}");
        assert!(watermark_alpha(0.5) > watermark_alpha(0.3), "растёт с отдалением");
        // региональный зум: пик ~0.75, вся зона 0.72..0.78
        for scale in [0.75, 0.9, 1.0, 1.2] {
            assert!((watermark_alpha(scale) - 0.75).abs() < 1e-4, "{scale}");
        }
        for scale in [0.6, 0.7, 1.4, 1.65] {
            let a = watermark_alpha(scale);
            assert!(a >= 0.68 && a <= 0.78, "{scale}: {a}");
        }
        assert!((watermark_alpha(1.65) - 0.72).abs() < 1e-4);
        // планетарный зум: плавное растворение до нуля
        let fading = watermark_alpha(2.1);
        assert!(fading > 0.0 && fading < 0.72, "{fading}");
        assert!(watermark_alpha(1.8) > watermark_alpha(2.3));
        assert_eq!(watermark_alpha(2.6), 0.0);
        assert_eq!(watermark_alpha(5.0), 0.0);
        assert!(watermark_alpha(f32::NAN) > 0.0, "мусор не обнуляет надписи");
    }

    #[test]
    fn short_names_get_wider_tracking() {
        assert_eq!(tracking_for(6), 0.25, "ИТАЛИЯ");
        assert_eq!(tracking_for(7), 0.25, "ФРАНЦИЯ");
        assert_eq!(tracking_for(9), 0.20);
        assert_eq!(tracking_for(12), 0.15);
        assert_eq!(tracking_for(30), 0.0, "длинное название не растягивается");
        let f = font();
        let flat = Ridge::new(vec![Vec2::new(0.0, 0.0), Vec2::new(2000.0, 0.0)]);
        let q = layout_on_ridge(&f, "HIH", &flat, 84.0);
        // расстояние между центрами двух H через «I»: (50 + 14) * 1.25 = 80 вместо 64
        let c = |g: &GlyphQuad| (g.corners[0].x + g.corners[1].x) * 0.5;
        assert!((c(&q[2]) - c(&q[0]) - 80.0).abs() < 1e-2, "{}", c(&q[2]) - c(&q[0]));
    }

    #[test]
    fn screen_size_grows_with_the_zoom_and_small_countries_hide_only_when_zoomed_out() {
        // мировой размер 100 при заглавных 0.65: на масштабе 1 — 65 px, при приближении x4 — 260 px (растёт)
        assert!((screen_letter_height(100.0, 0.65, 1.0) - 65.0).abs() < 1e-3);
        assert!((screen_letter_height(100.0, 0.65, 0.25) - 260.0).abs() < 1e-3);
        assert!(label_visible(screen_letter_height(100.0, 0.65, 0.25)), "при приближении надпись видна");
        // мелкая страна (мировой размер 12) пропадает только на сильном отдалении
        assert!(label_visible(screen_letter_height(12.0, 0.65, 1.0)));
        assert!(!label_visible(screen_letter_height(12.0, 0.65, 2.0)), "высота 3.9 px < 7 px");
        assert_eq!(screen_letter_height(10.0, 0.65, 0.0), 0.0);
        assert!(!label_visible(0.0));
    }
}
