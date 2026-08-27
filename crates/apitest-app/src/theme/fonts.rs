use std::{path::PathBuf, sync::Arc};

use egui::{FontData, FontDefinitions, FontFamily};
use iconflow::fonts;
use serde::{Deserialize, Serialize};

/// Simplified-Chinese capable families, most preferred first.
///
/// Sans and mono candidates share one list because only a single CJK face is
/// installed: egui resolves Latin glyphs in the monospace family from its own
/// bundled font, and CJK glyphs are full-width in both variants, so a second
/// ~19 MiB face would cost memory without changing what the user sees.
const CJK_FAMILIES: &[&str] = &[
    "Noto Sans CJK SC",
    "Noto Sans SC",
    "Source Han Sans SC",
    "Source Han Sans CN",
    "Microsoft YaHei",
    "PingFang SC",
    "Sarasa Gothic SC",
    "Sarasa Mono SC",
    "Noto Sans Mono CJK SC",
    "WenQuanYi Micro Hei",
    "WenQuanYi Zen Hei",
    "Droid Sans Fallback",
    "Heiti SC",
    "SimHei",
    "SimSun",
];

/// Substrings that mark a family as covering Han script, used only when none of
/// [`CJK_FAMILIES`] is installed.
const CJK_NAME_HINTS: &[&str] = &[
    "cjk", "hei", "song", "ming", "kai", "yuan", "gothic", "mincho", "hanazono", "黑", "宋", "楷",
    "圆",
];

/// Font files shipped by distributions that do not always register with
/// fontconfig, tried only when system discovery comes up empty.
const FALLBACK_PATHS: &[&str] = &[
    // Debian / Ubuntu
    "/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc",
    "/usr/share/fonts/opentype/noto/NotoSansCJKsc-Regular.otf",
    "/usr/share/fonts/truetype/wqy/wqy-microhei.ttc",
    "/usr/share/fonts/truetype/wqy/wqy-zenhei.ttc",
    // Fedora / RHEL
    "/usr/share/fonts/google-noto-cjk/NotoSansCJK-Regular.ttc",
    "/usr/share/fonts/google-noto-sans-cjk-fonts/NotoSansCJK-Regular.ttc",
    // Arch / Alpine
    "/usr/share/fonts/noto-cjk/NotoSansCJK-Regular.ttc",
    "/usr/share/fonts/adobe-source-han-sans-cn/SourceHanSansCN-Regular.otf",
    // openSUSE
    "/usr/share/fonts/truetype/NotoSansCJK-Regular.ttc",
    // macOS
    "/System/Library/Fonts/PingFang.ttc",
    "/System/Library/Fonts/STHeiti Light.ttc",
    // Windows
    "C:\\Windows\\Fonts\\msyh.ttc",
    "C:\\Windows\\Fonts\\msyh.ttf",
    "C:\\Windows\\Fonts\\simhei.ttf",
    "C:\\Windows\\Fonts\\simsun.ttc",
];

/// Which CJK family ended up installed, so startup can warn instead of silently
/// rendering every Chinese label as tofu.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FontReport {
    pub cjk_family: Option<String>,
    pub scanned_fallback_paths: bool,
}

impl FontReport {
    pub fn is_missing(&self) -> bool {
        self.cjk_family.is_none()
    }
}

/// Where a previous startup found the CJK face, so the next one can load that
/// file directly instead of enumerating every installed font.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FontCacheHint {
    pub path: PathBuf,
    pub family: String,
}

pub fn install_fonts_with_hint(
    ctx: &egui::Context,
    hint: Option<&FontCacheHint>,
) -> (FontReport, Option<FontCacheHint>) {
    // Fast path: the remembered file still exists and still carries the
    // remembered family. Loading one file re-validates it fully, at a
    // fraction of the system-wide scan.
    if let Some(hint) = hint {
        let mut database = fontdb::Database::new();
        if database.load_font_file(&hint.path).is_ok()
            && let Some((bytes, index)) = face_data(&database, &hint.family)
        {
            install_definitions(ctx, Some((bytes, index)));
            return (
                FontReport {
                    cjk_family: Some(hint.family.clone()),
                    scanned_fallback_paths: false,
                },
                Some(hint.clone()),
            );
        }
    }

    let mut database = fontdb::Database::new();
    database.load_system_fonts();

    let mut family = select_family(&family_names(&database));
    let mut scanned_fallback_paths = false;
    if family.is_none() {
        scanned_fallback_paths = true;
        for path in FALLBACK_PATHS {
            let _ = database.load_font_file(path);
        }
        family = select_family(&family_names(&database));
    }

    let mut face = None;
    if let Some(name) = family.as_deref() {
        face = face_data(&database, name);
        if face.is_none() {
            family = None;
        }
    }
    let next_hint = family.as_deref().and_then(|name| {
        face_source(&database, name).map(|path| FontCacheHint {
            path,
            family: name.to_owned(),
        })
    });
    install_definitions(ctx, face);

    (
        FontReport {
            cjk_family: family,
            scanned_fallback_paths,
        },
        next_hint,
    )
}

fn install_definitions(ctx: &egui::Context, face: Option<(Vec<u8>, u32)>) {
    let mut definitions = FontDefinitions::default();
    if let Some((bytes, index)) = face {
        let mut data = FontData::from_owned(bytes);
        data.index = index;
        definitions
            .font_data
            .insert("apitest-cjk".into(), Arc::new(data));
        for target in [FontFamily::Proportional, FontFamily::Monospace] {
            if let Some(family_fonts) = definitions.families.get_mut(&target) {
                family_fonts.push("apitest-cjk".into());
            }
        }
    }

    let fallback_fonts = definitions.font_data.keys().cloned().collect::<Vec<_>>();
    for font in fonts() {
        definitions.font_data.insert(
            font.family.to_owned(),
            Arc::new(FontData::from_static(font.bytes)),
        );
        let family = definitions
            .families
            .entry(FontFamily::Name(font.family.into()))
            .or_default();
        family.push(font.family.to_owned());
        family.extend(
            fallback_fonts
                .iter()
                .filter(|fallback| fallback.as_str() != font.family)
                .cloned(),
        );
    }
    ctx.set_fonts(definitions);
}

fn family_names(database: &fontdb::Database) -> Vec<String> {
    let mut names = database
        .faces()
        .flat_map(|face| face.families.iter().map(|(name, _)| name.clone()))
        .collect::<Vec<_>>();
    names.sort_unstable();
    names.dedup();
    names
}

/// Preferred families first, then anything whose name marks it as Han-capable.
///
/// Kept free of `fontdb` so the priority rules can be tested without depending
/// on which fonts the build machine happens to have installed.
fn select_family(installed: &[String]) -> Option<String> {
    for wanted in CJK_FAMILIES {
        if let Some(found) = installed
            .iter()
            .find(|name| name.eq_ignore_ascii_case(wanted))
        {
            return Some(found.clone());
        }
    }
    installed
        .iter()
        .find(|name| {
            let lowered = name.to_lowercase();
            CJK_NAME_HINTS.iter().any(|hint| lowered.contains(hint))
        })
        .cloned()
}

/// The bytes and *face index* for a family.
///
/// The index matters: `NotoSansCJK-Regular.ttc` packs JP, KR, SC, TC and HK in
/// that order, so loading index 0 silently renders Simplified Chinese with
/// Japanese glyph variants.
fn face_data(database: &fontdb::Database, family: &str) -> Option<(Vec<u8>, u32)> {
    let id = database.query(&fontdb::Query {
        families: &[fontdb::Family::Name(family)],
        weight: fontdb::Weight::NORMAL,
        stretch: fontdb::Stretch::Normal,
        style: fontdb::Style::Normal,
    })?;
    database.with_face_data(id, |data, index| (data.to_vec(), index))
}

/// The on-disk path of the face `family` resolves to, when it has one.
fn face_source(database: &fontdb::Database, family: &str) -> Option<PathBuf> {
    let id = database.query(&fontdb::Query {
        families: &[fontdb::Family::Name(family)],
        weight: fontdb::Weight::NORMAL,
        stretch: fontdb::Stretch::Normal,
        style: fontdb::Style::Normal,
    })?;
    match &database.face(id)?.source {
        fontdb::Source::File(path) => Some(path.clone()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::select_family;

    fn installed(names: &[&str]) -> Vec<String> {
        names.iter().map(|name| (*name).to_owned()).collect()
    }

    #[test]
    fn preferred_families_win_over_installation_order() {
        let names = installed(&["WenQuanYi Micro Hei", "DejaVu Sans", "Noto Sans CJK SC"]);
        assert_eq!(select_family(&names).as_deref(), Some("Noto Sans CJK SC"));
    }

    #[test]
    fn family_matching_ignores_case() {
        let names = installed(&["noto sans cjk sc"]);
        assert_eq!(select_family(&names).as_deref(), Some("noto sans cjk sc"));
    }

    #[test]
    fn unknown_but_han_capable_families_are_a_last_resort() {
        let names = installed(&["DejaVu Sans", "Foobar Hei Regular"]);
        assert_eq!(select_family(&names).as_deref(), Some("Foobar Hei Regular"));
    }

    #[test]
    fn latin_only_installations_report_no_cjk_family() {
        let names = installed(&["DejaVu Sans", "Liberation Mono", "Ubuntu"]);
        assert_eq!(select_family(&names), None);
    }

    /// The report must not claim success unless Chinese actually renders, and
    /// must not claim failure while it does. egui ships no CJK glyphs of its
    /// own, so this holds on machines with and without a Chinese font.
    #[test]
    fn the_report_agrees_with_what_egui_can_render() {
        let ctx = egui::Context::default();
        let (report, _) = super::install_fonts_with_hint(&ctx, None);
        let _ = ctx.run_ui(egui::RawInput::default(), |_| {});
        let renders_chinese = ctx.fonts_mut(|fonts| {
            fonts.has_glyphs(&egui::FontId::proportional(13.0), "测试接口")
                && fonts.has_glyphs(&egui::FontId::monospace(13.0), "测试接口")
        });
        assert_eq!(renders_chinese, !report.is_missing(), "{report:?}");
    }
}
