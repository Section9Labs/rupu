use crate::output::palette::{RgbColor, UiPaletteTheme};
use anyhow::{anyhow, Context};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

pub const DEFAULT_PALETTE_THEME: &str = "rupu-dark";
pub const DEFAULT_SYNTAX_THEME: &str = "base16-ocean.dark";

#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ThemeImportFormat {
    Auto,
    Rupu,
    Base16,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ThemeSpec {
    pub name: String,
    pub description: Option<String>,
    pub syntax_theme: Option<String>,
    pub source: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    pub palette: UiPaletteTheme,
}

#[derive(Debug, Deserialize, Serialize)]
struct ThemeFile {
    #[serde(default = "default_theme_version")]
    version: u8,
    name: String,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    base: Option<String>,
    #[serde(default)]
    syntax_theme: Option<String>,
    #[serde(default)]
    palette: ThemePaletteFile,
}

#[derive(Debug, Default, Deserialize, Serialize)]
struct ThemePaletteFile {
    running: Option<String>,
    complete: Option<String>,
    failed: Option<String>,
    awaiting: Option<String>,
    skipped: Option<String>,
    soft_failed: Option<String>,
    retrying: Option<String>,
    dim: Option<String>,
    brand: Option<String>,
    brand_subtle: Option<String>,
    tool_arrow: Option<String>,
    separator: Option<String>,
    sev_critical: Option<String>,
    sev_high: Option<String>,
    sev_medium: Option<String>,
    sev_low: Option<String>,
    sev_info: Option<String>,
    label_palette: Option<Vec<String>>,
}

#[allow(dead_code)]
#[derive(Debug, Deserialize)]
struct Base16Scheme {
    #[serde(default)]
    scheme: Option<String>,
    #[serde(default)]
    author: Option<String>,
    base00: String,
    base01: String,
    base02: String,
    base03: String,
    base04: String,
    base05: String,
    base06: String,
    base07: String,
    base08: String,
    base09: String,
    base0_a: String,
    base0_b: String,
    base0_c: String,
    base0_d: String,
    base0_e: String,
    base0_f: String,
}

fn default_theme_version() -> u8 {
    1
}

pub fn default_palette_theme() -> ThemeSpec {
    builtin_palette_themes()
        .into_iter()
        .find(|theme| theme.name == DEFAULT_PALETTE_THEME)
        .expect("default palette theme exists")
}

pub fn list_palette_themes(
    global: &Path,
    project_root: Option<&Path>,
) -> anyhow::Result<Vec<ThemeSpec>> {
    let mut out = BTreeMap::<String, ThemeSpec>::new();
    for theme in builtin_palette_themes() {
        out.insert(theme.name.clone(), theme);
    }
    for theme in load_theme_dir(
        &crate::paths::themes_dir(global),
        "global file",
        global,
        project_root,
    )? {
        out.insert(theme.name.clone(), theme);
    }
    if let Some(project_root) = project_root {
        for theme in load_theme_dir(
            &crate::paths::project_themes_dir(project_root),
            "project file",
            global,
            project_root.into(),
        )? {
            out.insert(theme.name.clone(), theme);
        }
    }
    Ok(out.into_values().collect())
}

pub fn resolve_palette_theme(
    requested: Option<&str>,
    global: &Path,
    project_root: Option<&Path>,
) -> ThemeSpec {
    let wanted = requested.unwrap_or(DEFAULT_PALETTE_THEME);
    list_palette_themes(global, project_root)
        .ok()
        .and_then(|themes| {
            themes
                .into_iter()
                .find(|theme| theme.name.eq_ignore_ascii_case(wanted))
        })
        .unwrap_or_else(default_palette_theme)
}

pub async fn import_theme(
    source: &str,
    format: ThemeImportFormat,
    name_override: Option<&str>,
    global: &Path,
    project_root: Option<&Path>,
    project_install: bool,
) -> anyhow::Result<ThemeSpec> {
    let body = read_theme_source(source).await?;
    let mut theme = parse_theme_source(&body, format, global, project_root)?;
    if let Some(name) = name_override {
        theme.name = slugify(name);
    }
    if theme.name.is_empty() {
        anyhow::bail!("theme must have a non-empty name");
    }

    let target_dir = if project_install {
        let Some(project_root) = project_root else {
            anyhow::bail!("project install requested outside a tracked project checkout");
        };
        crate::paths::project_themes_dir(project_root)
    } else {
        crate::paths::themes_dir(global)
    };
    crate::paths::ensure_dir(&target_dir)?;
    let file = serialize_theme_file(&theme);
    let target_path = target_dir.join(format!("{}.toml", slugify(&theme.name)));
    std::fs::write(&target_path, file)
        .with_context(|| format!("write theme {}", target_path.display()))?;
    theme.path = Some(target_path.display().to_string());
    theme.source = if project_install {
        "project file".into()
    } else {
        "global file".into()
    };
    Ok(theme)
}

pub fn validate_theme_file(
    path: &Path,
    global: &Path,
    project_root: Option<&Path>,
) -> anyhow::Result<ThemeSpec> {
    let body = std::fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
    let mut theme = parse_theme_source(&body, ThemeImportFormat::Auto, global, project_root)?;
    theme.path = Some(path.display().to_string());
    theme.source = "file".into();
    Ok(theme)
}

pub fn builtin_syntax_themes() -> Vec<&'static str> {
    vec![
        "base16-ocean.dark",
        "base16-eighties.dark",
        "base16-mocha.dark",
        "base16-ocean.light",
        "InspiredGitHub",
        "Solarized (dark)",
        "Solarized (light)",
    ]
}

fn builtin_palette_themes() -> Vec<ThemeSpec> {
    vec![
        theme(
            "rupu-dark",
            "Default dark CLI palette aligned with current rupu chrome.",
            Some("base16-ocean.dark"),
            UiPaletteTheme::default(),
        ),
        theme(
            "rupu-light",
            "Native light palette tuned for bright terminal backgrounds.",
            Some("base16-ocean.light"),
            UiPaletteTheme {
                running: rgb(37, 99, 235),
                complete: rgb(22, 163, 74),
                failed: rgb(220, 38, 38),
                awaiting: rgb(217, 119, 6),
                skipped: rgb(148, 163, 184),
                soft_failed: rgb(180, 83, 9),
                retrying: rgb(124, 58, 237),
                dim: rgb(100, 116, 139),
                brand: rgb(109, 40, 217),
                brand_subtle: rgb(196, 181, 253),
                tool_arrow: rgb(100, 116, 139),
                separator: rgb(148, 163, 184),
                sev_critical: rgb(147, 51, 234),
                sev_high: rgb(220, 38, 38),
                sev_medium: rgb(234, 88, 12),
                sev_low: rgb(202, 138, 4),
                sev_info: rgb(100, 116, 139),
                label_palette: label_palette(&[
                    0xdc3d43, 0xf59e0b, 0x84cc16, 0x10b981, 0x06b6d4, 0x3b82f6, 0x8b5cf6, 0xec4899,
                ]),
            },
        ),
        theme(
            "rupu-midnight",
            "Higher-contrast dark palette.",
            Some("base16-mocha.dark"),
            UiPaletteTheme {
                running: rgb(56, 189, 248),
                complete: rgb(74, 222, 128),
                failed: rgb(248, 113, 113),
                awaiting: rgb(250, 204, 21),
                skipped: rgb(148, 163, 184),
                soft_failed: rgb(245, 158, 11),
                retrying: rgb(192, 132, 252),
                dim: rgb(148, 163, 184),
                brand: rgb(129, 140, 248),
                brand_subtle: rgb(165, 180, 252),
                tool_arrow: rgb(148, 163, 184),
                separator: rgb(71, 85, 105),
                sev_critical: rgb(216, 180, 254),
                sev_high: rgb(248, 113, 113),
                sev_medium: rgb(251, 146, 60),
                sev_low: rgb(250, 204, 21),
                sev_info: rgb(148, 163, 184),
                label_palette: label_palette(&[
                    0xfb7185, 0xf59e0b, 0xa3e635, 0x2dd4bf, 0x38bdf8, 0x818cf8, 0xc084fc, 0xf472b6,
                ]),
            },
        ),
        theme(
            "tokyo-night",
            "Tokyo Night-inspired dark palette.",
            Some("base16-ocean.dark"),
            UiPaletteTheme {
                running: rgb(122, 162, 247),
                complete: rgb(158, 206, 106),
                failed: rgb(247, 118, 142),
                awaiting: rgb(224, 175, 104),
                skipped: rgb(86, 95, 137),
                soft_failed: rgb(255, 158, 100),
                retrying: rgb(187, 154, 247),
                dim: rgb(86, 95, 137),
                brand: rgb(122, 162, 247),
                brand_subtle: rgb(125, 207, 255),
                tool_arrow: rgb(86, 95, 137),
                separator: rgb(59, 66, 97),
                sev_critical: rgb(187, 154, 247),
                sev_high: rgb(247, 118, 142),
                sev_medium: rgb(255, 158, 100),
                sev_low: rgb(224, 175, 104),
                sev_info: rgb(125, 207, 255),
                label_palette: label_palette(&[
                    0xf7768e, 0xff9e64, 0xe0af68, 0x9ece6a, 0x73daca, 0x7dcfff, 0x7aa2f7, 0xbb9af7,
                ]),
            },
        ),
        theme(
            "dracula",
            "Dracula-inspired palette.",
            Some("base16-eighties.dark"),
            UiPaletteTheme {
                running: rgb(139, 233, 253),
                complete: rgb(80, 250, 123),
                failed: rgb(255, 85, 85),
                awaiting: rgb(241, 250, 140),
                skipped: rgb(98, 114, 164),
                soft_failed: rgb(255, 184, 108),
                retrying: rgb(189, 147, 249),
                dim: rgb(98, 114, 164),
                brand: rgb(255, 121, 198),
                brand_subtle: rgb(189, 147, 249),
                tool_arrow: rgb(98, 114, 164),
                separator: rgb(68, 71, 90),
                sev_critical: rgb(189, 147, 249),
                sev_high: rgb(255, 85, 85),
                sev_medium: rgb(255, 184, 108),
                sev_low: rgb(241, 250, 140),
                sev_info: rgb(139, 233, 253),
                label_palette: label_palette(&[
                    0xff5555, 0xffb86c, 0xf1fa8c, 0x50fa7b, 0x8be9fd, 0xbd93f9, 0xff79c6, 0x6272a4,
                ]),
            },
        ),
        theme(
            "gruvbox-dark",
            "Gruvbox dark palette.",
            Some("base16-mocha.dark"),
            UiPaletteTheme {
                running: rgb(131, 165, 152),
                complete: rgb(184, 187, 38),
                failed: rgb(251, 73, 52),
                awaiting: rgb(250, 189, 47),
                skipped: rgb(146, 131, 116),
                soft_failed: rgb(254, 128, 25),
                retrying: rgb(211, 134, 155),
                dim: rgb(146, 131, 116),
                brand: rgb(211, 134, 155),
                brand_subtle: rgb(177, 98, 134),
                tool_arrow: rgb(146, 131, 116),
                separator: rgb(80, 73, 69),
                sev_critical: rgb(211, 134, 155),
                sev_high: rgb(251, 73, 52),
                sev_medium: rgb(254, 128, 25),
                sev_low: rgb(250, 189, 47),
                sev_info: rgb(131, 165, 152),
                label_palette: label_palette(&[
                    0xfb4934, 0xfe8019, 0xfabd2f, 0xb8bb26, 0x8ec07c, 0x83a598, 0xd3869b, 0xd65d0e,
                ]),
            },
        ),
        theme(
            "github-dark",
            "GitHub dark-inspired palette.",
            Some("InspiredGitHub"),
            UiPaletteTheme {
                running: rgb(47, 129, 247),
                complete: rgb(63, 185, 80),
                failed: rgb(248, 81, 73),
                awaiting: rgb(210, 153, 34),
                skipped: rgb(110, 118, 129),
                soft_failed: rgb(219, 109, 40),
                retrying: rgb(163, 113, 247),
                dim: rgb(110, 118, 129),
                brand: rgb(88, 166, 255),
                brand_subtle: rgb(121, 192, 255),
                tool_arrow: rgb(110, 118, 129),
                separator: rgb(48, 54, 61),
                sev_critical: rgb(163, 113, 247),
                sev_high: rgb(248, 81, 73),
                sev_medium: rgb(219, 109, 40),
                sev_low: rgb(210, 153, 34),
                sev_info: rgb(88, 166, 255),
                label_palette: label_palette(&[
                    0xf85149, 0xdb6d28, 0xd29922, 0x3fb950, 0x39c5cf, 0x58a6ff, 0xa371f7, 0xdb61a2,
                ]),
            },
        ),
        theme(
            "github-light",
            "GitHub light-inspired palette.",
            Some("InspiredGitHub"),
            UiPaletteTheme {
                running: rgb(9, 105, 218),
                complete: rgb(26, 127, 55),
                failed: rgb(207, 34, 46),
                awaiting: rgb(154, 103, 0),
                skipped: rgb(87, 96, 106),
                soft_failed: rgb(188, 76, 0),
                retrying: rgb(130, 80, 223),
                dim: rgb(87, 96, 106),
                brand: rgb(130, 80, 223),
                brand_subtle: rgb(191, 160, 255),
                tool_arrow: rgb(87, 96, 106),
                separator: rgb(208, 215, 222),
                sev_critical: rgb(130, 80, 223),
                sev_high: rgb(207, 34, 46),
                sev_medium: rgb(188, 76, 0),
                sev_low: rgb(154, 103, 0),
                sev_info: rgb(9, 105, 218),
                label_palette: label_palette(&[
                    0xcf222e, 0xbc4c00, 0x9a6700, 0x1a7f37, 0x0969da, 0x8250df, 0xbf3989, 0x57606a,
                ]),
            },
        ),
        theme(
            "solarized-dark",
            "Solarized dark palette.",
            Some("Solarized (dark)"),
            palette_from_base16(
                "solarized-dark",
                Some("builtin"),
                &base16(
                    "002b36", "073642", "586e75", "657b83", "839496", "93a1a1", "eee8d5", "fdf6e3",
                    "dc322f", "cb4b16", "b58900", "859900", "2aa198", "268bd2", "6c71c4", "d33682",
                ),
            )
            .palette,
        ),
        theme(
            "solarized-light",
            "Solarized light palette.",
            Some("Solarized (light)"),
            palette_from_base16(
                "solarized-light",
                Some("builtin"),
                &base16(
                    "fdf6e3", "eee8d5", "93a1a1", "839496", "657b83", "586e75", "073642", "002b36",
                    "dc322f", "cb4b16", "b58900", "859900", "2aa198", "268bd2", "6c71c4", "d33682",
                ),
            )
            .palette,
        ),
        theme(
            "catppuccin-mocha",
            "Catppuccin Mocha-inspired palette.",
            Some("base16-mocha.dark"),
            UiPaletteTheme {
                running: rgb(137, 180, 250),
                complete: rgb(166, 227, 161),
                failed: rgb(243, 139, 168),
                awaiting: rgb(249, 226, 175),
                skipped: rgb(127, 132, 156),
                soft_failed: rgb(250, 179, 135),
                retrying: rgb(203, 166, 247),
                dim: rgb(127, 132, 156),
                brand: rgb(203, 166, 247),
                brand_subtle: rgb(180, 190, 254),
                tool_arrow: rgb(127, 132, 156),
                separator: rgb(88, 91, 112),
                sev_critical: rgb(203, 166, 247),
                sev_high: rgb(243, 139, 168),
                sev_medium: rgb(250, 179, 135),
                sev_low: rgb(249, 226, 175),
                sev_info: rgb(137, 180, 250),
                label_palette: label_palette(&[
                    0xf38ba8, 0xfab387, 0xf9e2af, 0xa6e3a1, 0x94e2d5, 0x89b4fa, 0xcba6f7, 0xf5c2e7,
                ]),
            },
        ),
    ]
}

fn theme(
    name: &str,
    description: &str,
    syntax_theme: Option<&str>,
    palette: UiPaletteTheme,
) -> ThemeSpec {
    ThemeSpec {
        name: name.to_string(),
        description: Some(description.to_string()),
        syntax_theme: syntax_theme.map(str::to_string),
        source: "builtin".into(),
        path: None,
        palette,
    }
}

fn rgb(r: u8, g: u8, b: u8) -> RgbColor {
    RgbColor::new(r, g, b)
}

fn label_palette(values: &[u32]) -> Vec<RgbColor> {
    values
        .iter()
        .map(|hex| {
            RgbColor::new(
                ((hex >> 16) & 0xff) as u8,
                ((hex >> 8) & 0xff) as u8,
                (hex & 0xff) as u8,
            )
        })
        .collect()
}

fn load_theme_dir(
    dir: &Path,
    source: &str,
    global: &Path,
    project_root: Option<&Path>,
) -> anyhow::Result<Vec<ThemeSpec>> {
    if !dir.is_dir() {
        return Ok(Vec::new());
    }
    let mut out = Vec::new();
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.extension().and_then(|value| value.to_str()) != Some("toml") {
            continue;
        }
        let mut visited = BTreeSet::new();
        let mut theme = load_theme_file(&path, source, global, project_root, &mut visited)?;
        theme.path = Some(path.display().to_string());
        out.push(theme);
    }
    Ok(out)
}

fn load_theme_file(
    path: &Path,
    source: &str,
    global: &Path,
    project_root: Option<&Path>,
    visited: &mut BTreeSet<String>,
) -> anyhow::Result<ThemeSpec> {
    let key = path.display().to_string();
    if !visited.insert(key.clone()) {
        anyhow::bail!("cyclic theme inheritance detected at {}", path.display());
    }
    let body = std::fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
    let file: ThemeFile =
        toml::from_str(&body).with_context(|| format!("parse {}", path.display()))?;
    let mut base = if let Some(base_name) = file.base.as_deref() {
        Some(resolve_named_theme(
            base_name,
            global,
            project_root,
            visited,
        )?)
    } else {
        None
    };
    let mut palette = base
        .as_ref()
        .map(|theme| theme.palette.clone())
        .unwrap_or_default();
    apply_palette_overrides(&mut palette, &file.palette)?;
    Ok(ThemeSpec {
        name: file.name,
        description: file
            .description
            .or_else(|| base.take().and_then(|value| value.description)),
        syntax_theme: file
            .syntax_theme
            .or_else(|| base.and_then(|value| value.syntax_theme)),
        source: source.to_string(),
        path: Some(path.display().to_string()),
        palette,
    })
}

fn resolve_named_theme(
    name: &str,
    global: &Path,
    project_root: Option<&Path>,
    visited: &mut BTreeSet<String>,
) -> anyhow::Result<ThemeSpec> {
    if let Some(theme) = builtin_palette_themes()
        .into_iter()
        .find(|theme| theme.name.eq_ignore_ascii_case(name))
    {
        return Ok(theme);
    }
    let mut candidates = Vec::new();
    if let Some(project_root) = project_root {
        candidates
            .push(crate::paths::project_themes_dir(project_root).join(format!("{name}.toml")));
    }
    candidates.push(crate::paths::themes_dir(global).join(format!("{name}.toml")));
    for candidate in candidates {
        if candidate.is_file() {
            return load_theme_file(&candidate, "file", global, project_root, visited);
        }
    }
    Err(anyhow!("unknown base theme `{name}`"))
}

fn apply_palette_overrides(
    palette: &mut UiPaletteTheme,
    overrides: &ThemePaletteFile,
) -> anyhow::Result<()> {
    apply_color(&mut palette.running, overrides.running.as_deref())?;
    apply_color(&mut palette.complete, overrides.complete.as_deref())?;
    apply_color(&mut palette.failed, overrides.failed.as_deref())?;
    apply_color(&mut palette.awaiting, overrides.awaiting.as_deref())?;
    apply_color(&mut palette.skipped, overrides.skipped.as_deref())?;
    apply_color(&mut palette.soft_failed, overrides.soft_failed.as_deref())?;
    apply_color(&mut palette.retrying, overrides.retrying.as_deref())?;
    apply_color(&mut palette.dim, overrides.dim.as_deref())?;
    apply_color(&mut palette.brand, overrides.brand.as_deref())?;
    apply_color(&mut palette.brand_subtle, overrides.brand_subtle.as_deref())?;
    apply_color(&mut palette.tool_arrow, overrides.tool_arrow.as_deref())?;
    apply_color(&mut palette.separator, overrides.separator.as_deref())?;
    apply_color(&mut palette.sev_critical, overrides.sev_critical.as_deref())?;
    apply_color(&mut palette.sev_high, overrides.sev_high.as_deref())?;
    apply_color(&mut palette.sev_medium, overrides.sev_medium.as_deref())?;
    apply_color(&mut palette.sev_low, overrides.sev_low.as_deref())?;
    apply_color(&mut palette.sev_info, overrides.sev_info.as_deref())?;
    if let Some(label_palette) = overrides.label_palette.as_ref() {
        palette.label_palette = label_palette
            .iter()
            .map(|value| parse_hex_color(value))
            .collect::<anyhow::Result<Vec<_>>>()?;
    }
    Ok(())
}

fn apply_color(slot: &mut RgbColor, value: Option<&str>) -> anyhow::Result<()> {
    if let Some(value) = value {
        *slot = parse_hex_color(value)?;
    }
    Ok(())
}

fn parse_hex_color(value: &str) -> anyhow::Result<RgbColor> {
    let trimmed = value.trim().trim_start_matches('#');
    if trimmed.len() != 6 {
        anyhow::bail!("invalid hex color `{value}`");
    }
    let parsed =
        u32::from_str_radix(trimmed, 16).with_context(|| format!("invalid hex color `{value}`"))?;
    Ok(RgbColor::new(
        ((parsed >> 16) & 0xff) as u8,
        ((parsed >> 8) & 0xff) as u8,
        (parsed & 0xff) as u8,
    ))
}

fn serialize_theme_file(theme: &ThemeSpec) -> String {
    let file = ThemeFile {
        version: 1,
        name: theme.name.clone(),
        description: theme.description.clone(),
        base: None,
        syntax_theme: theme.syntax_theme.clone(),
        palette: ThemePaletteFile {
            running: Some(format_hex(theme.palette.running)),
            complete: Some(format_hex(theme.palette.complete)),
            failed: Some(format_hex(theme.palette.failed)),
            awaiting: Some(format_hex(theme.palette.awaiting)),
            skipped: Some(format_hex(theme.palette.skipped)),
            soft_failed: Some(format_hex(theme.palette.soft_failed)),
            retrying: Some(format_hex(theme.palette.retrying)),
            dim: Some(format_hex(theme.palette.dim)),
            brand: Some(format_hex(theme.palette.brand)),
            brand_subtle: Some(format_hex(theme.palette.brand_subtle)),
            tool_arrow: Some(format_hex(theme.palette.tool_arrow)),
            separator: Some(format_hex(theme.palette.separator)),
            sev_critical: Some(format_hex(theme.palette.sev_critical)),
            sev_high: Some(format_hex(theme.palette.sev_high)),
            sev_medium: Some(format_hex(theme.palette.sev_medium)),
            sev_low: Some(format_hex(theme.palette.sev_low)),
            sev_info: Some(format_hex(theme.palette.sev_info)),
            label_palette: Some(
                theme
                    .palette
                    .label_palette
                    .iter()
                    .copied()
                    .map(format_hex)
                    .collect(),
            ),
        },
    };
    toml::to_string_pretty(&file).expect("serialize theme")
}

async fn read_theme_source(source: &str) -> anyhow::Result<String> {
    if source.starts_with("http://") || source.starts_with("https://") {
        let response = reqwest::get(source).await?;
        if !response.status().is_success() {
            anyhow::bail!("failed to fetch theme: HTTP {}", response.status());
        }
        return response.text().await.map_err(Into::into);
    }
    std::fs::read_to_string(source).with_context(|| format!("read {source}"))
}

fn parse_theme_source(
    body: &str,
    format: ThemeImportFormat,
    global: &Path,
    project_root: Option<&Path>,
) -> anyhow::Result<ThemeSpec> {
    match format {
        ThemeImportFormat::Rupu => parse_native_theme(body, global, project_root),
        ThemeImportFormat::Base16 => parse_base16_theme(body),
        ThemeImportFormat::Auto => {
            parse_native_theme(body, global, project_root).or_else(|_| parse_base16_theme(body))
        }
    }
}

fn parse_native_theme(
    body: &str,
    global: &Path,
    project_root: Option<&Path>,
) -> anyhow::Result<ThemeSpec> {
    let temp_dir = tempfile::tempdir()?;
    let path = temp_dir.path().join("theme.toml");
    std::fs::write(&path, body)?;
    let mut visited = BTreeSet::new();
    let mut spec = load_theme_file(&path, "imported", global, project_root, &mut visited)?;
    spec.path = None;
    Ok(spec)
}

fn parse_base16_theme(body: &str) -> anyhow::Result<ThemeSpec> {
    let scheme = parse_base16(body)?;
    Ok(palette_from_base16(
        &scheme
            .scheme
            .clone()
            .unwrap_or_else(|| "base16-import".to_string()),
        scheme.author.as_deref(),
        &scheme,
    ))
}

fn parse_base16(body: &str) -> anyhow::Result<Base16Scheme> {
    fn normalize_keys(mut value: serde_json::Value) -> serde_json::Value {
        if let Some(obj) = value.as_object_mut() {
            for suffix in ['a', 'b', 'c', 'd', 'e', 'f'] {
                let upper = format!("base0{}", suffix.to_ascii_uppercase());
                let lower = format!("base0_{suffix}");
                if let Some(value) = obj.remove(&upper) {
                    obj.insert(lower, value);
                }
            }
        }
        value
    }

    if let Ok(value) = serde_yaml::from_str::<serde_json::Value>(body) {
        let value = normalize_keys(value);
        if let Ok(parsed) = serde_json::from_value::<Base16Scheme>(value) {
            return Ok(parsed);
        }
    }
    if let Ok(value) = toml::from_str::<toml::Value>(body) {
        let value = normalize_keys(serde_json::to_value(value)?);
        if let Ok(parsed) = serde_json::from_value::<Base16Scheme>(value) {
            return Ok(parsed);
        }
    }
    let value: serde_json::Value = serde_json::from_str(body)?;
    let value = normalize_keys(value);
    serde_json::from_value(value).map_err(Into::into)
}

fn palette_from_base16(name: &str, author: Option<&str>, scheme: &Base16Scheme) -> ThemeSpec {
    let base02 = parse_hex_color(&scheme.base02).expect("base16 base02");
    let base03 = parse_hex_color(&scheme.base03).expect("base16 base03");
    let base05 = parse_hex_color(&scheme.base05).expect("base16 base05");
    let base08 = parse_hex_color(&scheme.base08).expect("base16 base08");
    let base09 = parse_hex_color(&scheme.base09).expect("base16 base09");
    let base0a = parse_hex_color(&scheme.base0_a).expect("base16 base0A");
    let base0b = parse_hex_color(&scheme.base0_b).expect("base16 base0B");
    let base0c = parse_hex_color(&scheme.base0_c).expect("base16 base0C");
    let base0d = parse_hex_color(&scheme.base0_d).expect("base16 base0D");
    let base0e = parse_hex_color(&scheme.base0_e).expect("base16 base0E");
    let base0f = parse_hex_color(&scheme.base0_f).expect("base16 base0F");
    ThemeSpec {
        name: slugify(name),
        description: Some(match author {
            Some(author) if !author.is_empty() => format!("Imported Base16 theme by {author}"),
            _ => "Imported Base16 theme".into(),
        }),
        syntax_theme: Some(DEFAULT_SYNTAX_THEME.to_string()),
        source: "imported".into(),
        path: None,
        palette: UiPaletteTheme {
            running: base0d,
            complete: base0b,
            failed: base08,
            awaiting: base0a,
            skipped: base03,
            soft_failed: base09,
            retrying: base0e,
            dim: base03,
            brand: base0d,
            brand_subtle: base0e,
            tool_arrow: base03,
            separator: base02,
            sev_critical: base0e,
            sev_high: base08,
            sev_medium: base09,
            sev_low: base0a,
            sev_info: base05,
            label_palette: vec![
                base08, base09, base0a, base0b, base0c, base0d, base0e, base0f,
            ],
        },
    }
}

fn base16(
    base00: &str,
    base01: &str,
    base02: &str,
    base03: &str,
    base04: &str,
    base05: &str,
    base06: &str,
    base07: &str,
    base08: &str,
    base09: &str,
    base0_a: &str,
    base0_b: &str,
    base0_c: &str,
    base0_d: &str,
    base0_e: &str,
    base0_f: &str,
) -> Base16Scheme {
    Base16Scheme {
        scheme: None,
        author: None,
        base00: base00.into(),
        base01: base01.into(),
        base02: base02.into(),
        base03: base03.into(),
        base04: base04.into(),
        base05: base05.into(),
        base06: base06.into(),
        base07: base07.into(),
        base08: base08.into(),
        base09: base09.into(),
        base0_a: base0_a.into(),
        base0_b: base0_b.into(),
        base0_c: base0_c.into(),
        base0_d: base0_d.into(),
        base0_e: base0_e.into(),
        base0_f: base0_f.into(),
    }
}

fn format_hex(color: RgbColor) -> String {
    format!("#{:02x}{:02x}{:02x}", color.r, color.g, color.b)
}

pub fn slugify(value: &str) -> String {
    let mut out = String::new();
    let mut dash = false;
    for ch in value.chars() {
        if ch.is_ascii_alphanumeric() {
            if dash && !out.is_empty() {
                out.push('-');
            }
            dash = false;
            out.push(ch.to_ascii_lowercase());
        } else if !out.is_empty() {
            dash = true;
        }
    }
    out.trim_matches('-').to_string()
}
