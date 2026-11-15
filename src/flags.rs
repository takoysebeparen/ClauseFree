//! Национальные флаги из `gfx/flags/` игры (Zero Asset Policy: читаются на лету).
//!
//! Флаг страны — `<TAG>_<идеология>.tga` (иногда просто `<TAG>.tga`), в каталогах `medium/` (41x26)
//! и корневом (82x52). Идеология берётся из `ruling_party` в `history/countries/<TAG> - *.txt`.
//! Любая проблема (нет файла, битый TGA, странный размер) — не паника, а цветной прямоугольник
//! 32x20 базового цвета нации из `CountryColors`.
//!
//! Подключение: `app.add_plugins(FlagPlugin { root: PathBuf::from(HOI4_PATH) })` (EguiPlugin уже должен
//! быть добавлен), затем в UI-системе: `flags: ResMut<FlagTextures>`, `colors: Res<CountryColors>` и
//! `flags.show(ui, "GER", &colors)`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use bevy::prelude::*;
use bevy_egui::egui;

use crate::colors::CountryColors;

/// Размер запасной текстуры.
pub const FALLBACK_WIDTH: usize = 32;
pub const FALLBACK_HEIGHT: usize = 20;
/// Флаги больше этого размера считаются подозрительными (битый заголовок) и заменяются запасным.
const MAX_FLAG_SIDE: u32 = 512;
/// Размер миниатюры флага в интерфейсе, пиксели.
const THUMB_SIZE: [f32; 2] = [27.0, 17.0];
/// Идеологии, перебираемые, если флаг партии у власти не найден.
const IDEOLOGIES: [&str; 4] = ["neutrality", "democratic", "fascism", "communism"];

/// Декодированный флаг: RGBA8, построчно сверху вниз.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FlagPixels {
    pub width: usize,
    pub height: usize,
    pub rgba: Vec<u8>,
}

/// Запасной флаг: сплошная заливка цветом нации.
pub fn fallback_flag(rgb: [u8; 3]) -> FlagPixels {
    let mut rgba = Vec::with_capacity(FALLBACK_WIDTH * FALLBACK_HEIGHT * 4);
    for _ in 0..FALLBACK_WIDTH * FALLBACK_HEIGHT {
        rgba.extend_from_slice(&[rgb[0], rgb[1], rgb[2], 255]);
    }
    FlagPixels { width: FALLBACK_WIDTH, height: FALLBACK_HEIGHT, rgba }
}

/// Тег страны безопасен для имени файла: 2–4 символа ASCII, буквы и цифры.
fn valid_tag(tag: &str) -> bool {
    (2..=4).contains(&tag.len()) && tag.bytes().all(|b| b.is_ascii_alphanumeric())
}

/// Декодирует TGA (24/32 бита, RLE и без) в RGBA8. Ошибка — строка, не паника.
pub fn decode_flag(path: &Path) -> Result<FlagPixels, String> {
    let image = image::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let (width, height) = (image.width(), image.height());
    if width == 0 || height == 0 || width > MAX_FLAG_SIDE || height > MAX_FLAG_SIDE {
        return Err(format!("{}: недопустимый размер {width}x{height}", path.display()));
    }
    Ok(FlagPixels { width: width as usize, height: height as usize, rgba: image.to_rgba8().into_raw() })
}

/// `ruling_party` страны на старте — первое вхождение в `history/countries/<TAG> - *.txt`.
pub fn ruling_party(root: &Path, tag: &str) -> Option<String> {
    if !valid_tag(tag) {
        return None;
    }
    let dir = root.join("history").join("countries");
    let prefix = format!("{tag} - ");
    let file = std::fs::read_dir(dir)
        .ok()?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .find(|p| p.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.starts_with(&prefix) && n.ends_with(".txt")))?;
    let text = std::fs::read(file).ok()?;
    let text = String::from_utf8_lossy(&text);
    let mut tokens = text.split_whitespace();
    while let Some(token) = tokens.next() {
        if token == "ruling_party" && tokens.next() == Some("=") {
            return tokens.next().map(|p| p.trim_matches('"').to_string());
        }
    }
    None
}

/// `ruling_party` из текста файла истории страны (первое вхождение).
pub fn ruling_party_in(text: &str) -> Option<String> {
    let mut tokens = text.split_whitespace();
    while let Some(token) = tokens.next() {
        if token == "ruling_party" && tokens.next() == Some("=") {
            return tokens.next().map(|p| p.trim_matches('"').to_string());
        }
    }
    None
}

/// Правящие партии всех стран на старте: тег -> `ruling_party` по файлам `history/countries/<TAG> - *.txt`.
/// Нет папки — пустая карта.
pub fn load_ruling_parties(root: &Path) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    let Ok(dir) = std::fs::read_dir(root.join("history").join("countries")) else { return out };
    for path in dir.filter_map(|e| e.ok().map(|e| e.path())) {
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else { continue };
        let Some(tag) = name.strip_suffix(".txt").and_then(|n| n.split_once(" - ")).map(|(tag, _)| tag) else { continue };
        if !valid_tag(tag) {
            continue;
        }
        let Ok(bytes) = std::fs::read(&path) else { continue };
        if let Some(party) = ruling_party_in(&String::from_utf8_lossy(&bytes)) {
            out.insert(tag.to_string(), party);
        }
    }
    out
}

/// Пути-кандидаты по убыванию приоритета: средний размер, затем крупный; сначала флаг партии у власти,
/// потом `<TAG>.tga`, потом остальные идеологии.
pub fn candidate_paths(root: &Path, tag: &str, party: Option<&str>) -> Vec<PathBuf> {
    if !valid_tag(tag) {
        return Vec::new();
    }
    let mut names: Vec<String> = Vec::new();
    if let Some(party) = party.filter(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')) {
        names.push(format!("{tag}_{party}.tga"));
    }
    names.push(format!("{tag}.tga"));
    names.extend(IDEOLOGIES.iter().map(|i| format!("{tag}_{i}.tga")));
    let flags = root.join("gfx").join("flags");
    let mut paths = Vec::new();
    for dir in [flags.join("medium"), flags] {
        paths.extend(names.iter().map(|n| dir.join(n)));
    }
    paths
}

/// Грузит флаг страны; при любой неудаче — запасной цвет нации. Второе значение — найден ли файл.
pub fn load_flag_pixels(root: &Path, tag: &str, colors: &CountryColors) -> (FlagPixels, bool) {
    let party = ruling_party(root, tag);
    for path in candidate_paths(root, tag, party.as_deref()) {
        if !path.is_file() {
            continue;
        }
        match decode_flag(&path) {
            Ok(pixels) => return (pixels, true),
            Err(reason) => println!("[ФЛАГИ] {tag}: не прочитан ({reason}), пробую следующий"),
        }
    }
    (fallback_flag(colors.ui_rgb(tag)), false)
}

/// Кэш текстур флагов в контексте egui. Текстура создаётся при первом показе тега и живёт всё время.
#[derive(Resource)]
pub struct FlagTextures {
    root: PathBuf,
    cache: BTreeMap<String, egui::TextureHandle>,
}

impl FlagTextures {
    pub fn new(root: PathBuf) -> Self {
        Self { root, cache: BTreeMap::new() }
    }

    /// Сколько флагов уже в кэше.
    #[cfg(test)]
    pub(crate) fn cached(&self) -> usize {
        self.cache.len()
    }

    /// Текстура флага страны (с загрузкой при первом запросе).
    pub fn texture(&mut self, ctx: &egui::Context, tag: &str, colors: &CountryColors) -> egui::TextureId {
        if let Some(handle) = self.cache.get(tag) {
            return handle.id();
        }
        let (pixels, found) = load_flag_pixels(&self.root, tag, colors);
        if !found {
            println!("[ФЛАГИ] {tag}: файл флага не найден, запасной цвет");
        }
        let image = egui::ColorImage::from_rgba_unmultiplied([pixels.width, pixels.height], &pixels.rgba);
        let handle = ctx.load_texture(format!("flag_{tag}"), image, egui::TextureOptions::LINEAR);
        let id = handle.id();
        self.cache.insert(tag.to_string(), handle);
        id
    }

    /// Рисует миниатюру флага страны.
    pub fn show(&mut self, ui: &mut egui::Ui, tag: &str, colors: &CountryColors) {
        self.show_sized(ui, tag, colors, THUMB_SIZE);
    }

    /// Рисует флаг заданного размера (шапки окон — 41x26).
    pub fn show_sized(&mut self, ui: &mut egui::Ui, tag: &str, colors: &CountryColors, size: [f32; 2]) {
        let id = self.texture(ui.ctx(), tag, colors);
        ui.add(egui::Image::new(egui::load::SizedTexture::new(id, size)));
    }
}

/// Плагин: ресурс `FlagTextures` с корнем игры.
pub struct FlagPlugin {
    pub root: PathBuf,
}

impl Plugin for FlagPlugin {
    fn build(&self, app: &mut App) {
        app.insert_resource(FlagTextures::new(self.root.clone()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::colors::CountryColor;

    fn colors() -> CountryColors {
        CountryColors(BTreeMap::from([("GER".to_string(), CountryColor { map: [1, 2, 3], ui: [40, 50, 60] })]))
    }

    /// Временный каталог игры в системном temp: `<dir>/gfx/flags/medium`.
    fn game_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("clausefree_flags_{name}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("gfx").join("flags").join("medium")).expect("каталог");
        std::fs::create_dir_all(dir.join("history").join("countries")).expect("каталог");
        dir
    }

    fn write_tga(path: &Path, width: u32, height: u32, rgba: [u8; 4]) {
        let img = image::RgbaImage::from_pixel(width, height, image::Rgba(rgba));
        img.save_with_format(path, image::ImageFormat::Tga).expect("запись TGA");
    }

    #[test]
    fn fallback_is_a_solid_32x20_rectangle_of_the_nation_color() {
        let flag = fallback_flag([10, 20, 30]);
        assert_eq!((flag.width, flag.height, flag.rgba.len()), (32, 20, 32 * 20 * 4));
        assert!(flag.rgba.chunks(4).all(|p| p == [10, 20, 30, 255]));
    }

    #[test]
    fn tag_validation_blocks_path_tricks() {
        assert!(valid_tag("GER") && valid_tag("D01"));
        assert!(!valid_tag("") && !valid_tag("G") && !valid_tag("GERMANY") && !valid_tag("../x") && !valid_tag("G/R"));
        assert!(candidate_paths(Path::new("x"), "../..", None).is_empty());
        // подозрительная идеология в имени файла игнорируется
        let paths = candidate_paths(Path::new("x"), "GER", Some("../evil"));
        assert!(paths.iter().all(|p| !p.to_string_lossy().contains("evil")));
    }

    #[test]
    fn candidates_prefer_ruling_party_and_medium_size() {
        let paths = candidate_paths(Path::new("g"), "GER", Some("fascism"));
        let names: Vec<String> = paths.iter().map(|p| p.to_string_lossy().replace('\\', "/")).collect();
        assert_eq!(names[0], "g/gfx/flags/medium/GER_fascism.tga");
        assert_eq!(names[1], "g/gfx/flags/medium/GER.tga");
        assert!(names.contains(&"g/gfx/flags/GER_fascism.tga".to_string()));
        assert!(names.iter().position(|n| n.ends_with("medium/GER_neutrality.tga")) < names.iter().position(|n| n == "g/gfx/flags/GER_fascism.tga"));
    }

    #[test]
    fn tga_flag_is_decoded_with_its_size_and_pixels() {
        let dir = game_dir("decode");
        let path = dir.join("gfx").join("flags").join("medium").join("GER_fascism.tga");
        write_tga(&path, 41, 26, [200, 10, 20, 255]);
        let flag = decode_flag(&path).expect("флаг");
        assert_eq!((flag.width, flag.height), (41, 26));
        assert_eq!(&flag.rgba[..4], &[200, 10, 20, 255]);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn ruling_party_selects_the_flag_variant() {
        let dir = game_dir("party");
        std::fs::write(dir.join("history/countries/GER - Germany.txt"), "capital = 64\nset_politics = {\n\truling_party = fascism\n}\n").unwrap();
        let medium = dir.join("gfx/flags/medium");
        write_tga(&medium.join("GER_fascism.tga"), 41, 26, [255, 0, 0, 255]);
        write_tga(&medium.join("GER_neutrality.tga"), 41, 26, [0, 255, 0, 255]);
        assert_eq!(ruling_party(&dir, "GER").as_deref(), Some("fascism"));
        let (flag, found) = load_flag_pixels(&dir, "GER", &colors());
        assert!(found);
        assert_eq!(&flag.rgba[..3], &[255, 0, 0], "флаг фашистской партии, а не нейтральной");
        // без файла страны перебираются идеологии
        let (other, found) = load_flag_pixels(&dir, "GER", &CountryColors::default());
        assert!(found && other.width == 41);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn missing_or_corrupt_files_fall_back_to_nation_color() {
        let dir = game_dir("broken");
        // нет файла
        let (flag, found) = load_flag_pixels(&dir, "GER", &colors());
        assert!(!found);
        assert_eq!((flag.width, flag.height), (32, 20));
        assert_eq!(&flag.rgba[..4], &[40, 50, 60, 255], "цвет интерфейса нации");
        // битый файл: мусор и обрезанный заголовок; приоритетный битый, следующий корректный
        let medium = dir.join("gfx/flags/medium");
        std::fs::write(medium.join("GER_neutrality.tga"), b"not a tga at all").unwrap();
        std::fs::write(dir.join("gfx/flags/GER_fascism.tga"), [0u8, 0, 2]).unwrap();
        assert!(decode_flag(&medium.join("GER_neutrality.tga")).is_err());
        let (flag, found) = load_flag_pixels(&dir, "GER", &colors());
        assert!(!found, "оба кандидата битые");
        assert_eq!(flag.width, 32);
        write_tga(&dir.join("gfx/flags/GER_communism.tga"), 82, 52, [9, 9, 9, 255]);
        let (flag, found) = load_flag_pixels(&dir, "GER", &colors());
        assert!(found && flag.width == 82, "после битых берётся первый рабочий файл");
        // неизвестная страна и мусорный тег — запасной цвет по умолчанию, без паники
        let (unknown, found) = load_flag_pixels(&dir, "ZZZ", &CountryColors::default());
        assert!(!found);
        assert_eq!(&unknown.rgba[..3], &crate::colors::FALLBACK_COLOR);
        assert!(!load_flag_pixels(&dir, "../..", &colors()).1);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn textures_are_cached_per_tag() {
        let ctx = egui::Context::default();
        let mut flags = FlagTextures::new(PathBuf::from("Z:\\нет\\такой\\папки"));
        let a = flags.texture(&ctx, "GER", &colors());
        let b = flags.texture(&ctx, "GER", &colors());
        assert_eq!(a, b);
        let c = flags.texture(&ctx, "FRA", &colors());
        assert_ne!(a, c);
        assert_eq!(flags.cached(), 2);
    }

    /// Реальные данные игры (пропускается без неё): флаги ключевых стран находятся и имеют размер medium.
    #[test]
    fn real_data_flags_are_found() {
        let root = Path::new("D:\\SteamLibrary\\steamapps\\common\\Hearts of Iron IV");
        if !root.exists() {
            return;
        }
        for tag in ["GER", "FRA", "SOV", "SWE", "ENG", "USA"] {
            let (flag, found) = load_flag_pixels(root, tag, &CountryColors::default());
            assert!(found, "{tag}: флаг найден");
            assert_eq!((flag.width, flag.height), (41, 26), "{tag}: medium");
            assert!(flag.rgba.chunks(4).any(|p| p[3] > 0));
        }
        assert_eq!(ruling_party(root, "GER").as_deref(), Some("fascism"));
    }
}
