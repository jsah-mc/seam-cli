use material_color_utilities_rs::{
  htc::Hct,
  palettes::{core::CorePalette, tonal::TonalPalette},
  quantize::quantizer_celebi::QuantizerCelebi,
  scheme::Scheme,
  score::score,
};
use std::{
  collections::BTreeMap,
  env, fs, io,
  path::{Path, PathBuf},
  process::Command,
};
use tera::Context;

pub struct TemplateJob {
  pub input: PathBuf,
  pub output: Option<PathBuf>,
}

pub fn generate(
  image: PathBuf,
  base16_file: Option<PathBuf>,
  bundled_scheme: Option<String>,
  templates: Vec<TemplateJob>,
  light: bool,
) -> Result<(), Box<dyn std::error::Error>> {
  let dynamic_type = bundled_scheme
    .as_deref()
    .map(dynamic_scheme_type)
    .transpose()?
    .flatten();
  let imported = match bundled_scheme.as_deref() {
    Some(_) if dynamic_type.is_some() => None,
    Some(name) => Some(load_bundled_base16(name, light)?),
    None => base16_file.as_deref().map(load_base16).transpose()?,
  };
  let dynamic_name = format!("dynamic {}", dynamic_type.unwrap_or("tonal-spot"));
  let (seed_argb, light_theme, dark_theme, theme_name) = if let Some(palette) = &imported {
    eprintln!("Loading theme: {}...", palette.theme);
    let scheme = scheme_from_base16(&palette.colors);
    (palette.colors[0x0d], scheme, scheme, palette.theme.as_str())
  } else {
    eprintln!("Processing wallpaper asset: {}...", image.display());
    let (seed, light_scheme, dark_scheme) =
      generate_dynamic_schemes(&image, dynamic_type.unwrap_or("tonal-spot"))?;
    (seed, light_scheme, dark_scheme, dynamic_name.as_str())
  };

  let context = template_context(
    &light_theme,
    &dark_theme,
    seed_argb,
    &image,
    light,
    imported.as_ref().map(|palette| &palette.colors),
    theme_name,
  );
  // QStandardPaths.StateLocation is application-specific for QuickShell.
  let quickshell_output = state_home()?.join("quickshell/user/generated/colors.json");
  render_template(
    include_str!("../templates/quickshell.json"),
    Some(&quickshell_output),
    &context,
  )?;
  reload_quickshell();
  let hyprland_output = config_home()?.join("hypr/modules/colors.lua");
  render_template(
    include_str!("../templates/hyprland.lua"),
    Some(&hyprland_output),
    &context,
  )?;
  let kitty_output = config_home()?.join("kitty/colors.conf");
  render_template(
    include_str!("../templates/kitty.conf"),
    Some(&kitty_output),
    &context,
  )?;
  let gtk3_output = config_home()?.join("gtk-3.0/gtk.css");
  render_template(
    include_str!("../templates/gtk.css"),
    Some(&gtk3_output),
    &context,
  )?;
  let gtk4_output = config_home()?.join("gtk-4.0/gtk.css");
  render_template(
    include_str!("../templates/gtk.css"),
    Some(&gtk4_output),
    &context,
  )?;
  let vscode_output = cache_home()?.join("matugen/vscode-colors");
  render_template(
    include_str!("../templates/vscode-colors"),
    Some(&vscode_output),
    &context,
  )?;
  let vscodejson_output = cache_home()?.join("matugen/vscode-colors.json");
  render_template(
    include_str!("../templates/vscode-colors.json"),
    Some(&vscodejson_output),
    &context,
  )?;
  let kde_output = data_home()?.join("color-schemes/Seam.colors");
  render_template(
    include_str!("../templates/kcolorscheme.colors"),
    Some(&kde_output),
    &context,
  )?;
  let discord_output = config_home()?.join("vesktop/themes/seam.theme.css");
  render_template(
    include_str!("../templates/discord.css"),
    Some(&discord_output),
    &context,
  )?;
  let wallpaper_output = state_home()?.join("quickshell/user/generated/wallpaper/path.txt");
  render_template(
    include_str!("../templates/wallpaper.txt"),
    Some(&wallpaper_output),
    &context,
  )?;
  for template in templates {
    generate_template(&template.input, template.output.as_deref(), &context)?;
  }

  write_state(light, theme_name)?;

  Ok(())
}

fn reload_quickshell() {
  let result = Command::new("qs")
    .args(["-c", "seam", "ipc", "call", "theme", "reload"])
    .status();

  if let Ok(status) = result
    && !status.success()
  {
    eprintln!("Warning: QuickShell did not accept the theme reload request");
  }
}

fn set_wall(wallpaper: &Path) -> Result<(), Box<dyn std::error::Error>> {
  let wallpaper = wallpaper.canonicalize().map_err(|error| {
    io::Error::new(
      error.kind(),
      format!("cannot set wallpaper '{}': {error}", wallpaper.display()),
    )
  })?;
  let status = Command::new("qs")
    .args(["-c", "seam", "ipc", "call", "wallpaper", "set"])
    .arg(&wallpaper)
    .status()
    .map_err(|error| {
      io::Error::new(
        error.kind(),
        format!("could not run QuickShell wallpaper command: {error}"),
      )
    })?;

  if !status.success() {
    return Err(
      io::Error::other(format!(
        "QuickShell rejected wallpaper '{}' with status {status}",
        wallpaper.display()
      ))
      .into(),
    );
  }

  Ok(())
}

fn generate_dynamic_schemes(
  image: &Path,
  scheme_type: &str,
) -> Result<([u8; 4], Scheme, Scheme), Box<dyn std::error::Error>> {
  let downsampled = image::open(image)?
    .resize(32, 32, image::imageops::FilterType::Nearest)
    .into_rgba8();
  let pixels: Vec<[u8; 4]> = downsampled
    .as_raw()
    .chunks_exact(4)
    .filter(|pixel| pixel[3] != 0)
    .map(|pixel| [pixel[3], pixel[0], pixel[1], pixel[2]])
    .collect();
  let fallback = [0xFF, 0x67, 0x50, 0xA4];
  let seed = if pixels.is_empty() {
    fallback
  } else {
    let quantized = QuantizerCelebi.quantize(&pixels, 16);
    score(&quantized).first().copied().unwrap_or(fallback)
  };
  let mut palette = dynamic_core_palette(seed, scheme_type);
  let light = Scheme::light_from_core_palette(&mut palette);
  let dark = Scheme::dark_from_core_palette(&mut palette);
  Ok((seed, light, dark))
}

fn dynamic_core_palette(seed: [u8; 4], scheme_type: &str) -> CorePalette {
  let source = Hct::from_int(seed);
  let hue = source.hue();
  let tonal = |hue, chroma| TonalPalette::from_hue_and_chroma(hue, chroma);
  let error = tonal(25.0, 84.0);

  match scheme_type {
    "auto" => CorePalette::new(seed, false),
    "content" | "fidelity" => CorePalette::new(seed, true),
    "expressive" => CorePalette {
      a1: tonal(hue + 240.0, 40.0),
      a2: tonal(hue + 45.0, 24.0),
      a3: tonal(hue + 120.0, 32.0),
      n1: tonal(hue + 15.0, 8.0),
      n2: tonal(hue + 15.0, 12.0),
      error,
    },
    "fruit-salad" => CorePalette {
      a1: tonal(hue - 50.0, 48.0),
      a2: tonal(hue - 50.0, 36.0),
      a3: tonal(hue, 36.0),
      n1: tonal(hue, 10.0),
      n2: tonal(hue, 16.0),
      error,
    },
    "monochrome" => CorePalette {
      a1: tonal(hue, 0.0),
      a2: tonal(hue, 0.0),
      a3: tonal(hue, 0.0),
      n1: tonal(hue, 0.0),
      n2: tonal(hue, 0.0),
      error,
    },
    "neutral" => CorePalette {
      a1: tonal(hue, 12.0),
      a2: tonal(hue, 8.0),
      a3: tonal(hue, 16.0),
      n1: tonal(hue, 2.0),
      n2: tonal(hue, 4.0),
      error,
    },
    "rainbow" => CorePalette {
      a1: tonal(hue, 48.0),
      a2: tonal(hue, 16.0),
      a3: tonal(hue + 60.0, 24.0),
      n1: tonal(hue, 0.0),
      n2: tonal(hue, 0.0),
      error,
    },
    "vibrant" => CorePalette {
      a1: tonal(hue, 200.0),
      a2: tonal(hue + 18.0, 24.0),
      a3: tonal(hue + 35.0, 32.0),
      n1: tonal(hue, 10.0),
      n2: tonal(hue, 12.0),
      error,
    },
    _ => CorePalette::new(seed, false),
  }
}

fn generate_template(
  input: &Path,
  output: Option<&Path>,
  context: &Context,
) -> Result<(), Box<dyn std::error::Error>> {
  let source = fs::read_to_string(input)?;
  render_template(&source, output, context)
}

fn render_template(
  source: &str,
  output: Option<&Path>,
  context: &Context,
) -> Result<(), Box<dyn std::error::Error>> {
  let rendered = tera::Tera::one_off(source, context, false)?;

  if let Some(output) = output {
    if let Some(parent) = output
      .parent()
      .filter(|parent| !parent.as_os_str().is_empty())
    {
      fs::create_dir_all(parent)?;
    }
    fs::write(output, rendered)?;
    eprintln!("Wrote theme to {}", output.display());
  } else {
    print!("{rendered}");
  }

  Ok(())
}

fn write_state(light: bool, scheme: &str) -> io::Result<()> {
  write_state_to(&state_home()?.join("seam"), light, scheme)
}

fn state_home() -> io::Result<PathBuf> {
  Ok(match env::var_os("XDG_STATE_HOME") {
    Some(path) if !path.is_empty() => PathBuf::from(path),
    _ => {
      let home = env::var_os("HOME").ok_or_else(|| {
        io::Error::new(
          io::ErrorKind::NotFound,
          "cannot determine state directory: HOME is not set",
        )
      })?;
      PathBuf::from(home).join(".local/state")
    }
  })
}

fn config_home() -> io::Result<PathBuf> {
  Ok(match env::var_os("XDG_CONFIG_HOME") {
    Some(path) if !path.is_empty() => PathBuf::from(path),
    _ => {
      let home = env::var_os("HOME").ok_or_else(|| {
        io::Error::new(
          io::ErrorKind::NotFound,
          "cannot determine config directory: HOME is not set",
        )
      })?;
      PathBuf::from(home).join(".config")
    }
  })
}

fn data_home() -> io::Result<PathBuf> {
  Ok(match env::var_os("XDG_DATA_HOME") {
    Some(path) if !path.is_empty() => PathBuf::from(path),
    _ => {
      let home = env::var_os("HOME")
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "HOME is not set"))?;
      PathBuf::from(home).join(".local/share")
    }
  })
}

fn cache_home() -> io::Result<PathBuf> {
  Ok(match env::var_os("XDG_CACHE_HOME") {
    Some(path) if !path.is_empty() => PathBuf::from(path),
    _ => {
      let home = env::var_os("HOME").ok_or_else(|| {
        io::Error::new(
          io::ErrorKind::NotFound,
          "cannot determine cache directory: HOME is not set",
        )
      })?;
      PathBuf::from(home).join(".cache")
    }
  })
}

fn write_state_to(directory: &Path, light: bool, scheme: &str) -> io::Result<()> {
  fs::create_dir_all(directory)?;
  fs::write(
    directory.join("mode.txt"),
    if light { "light\n" } else { "dark\n" },
  )?;
  fs::write(directory.join("scheme.txt"), format!("{scheme}\n"))?;
  Ok(())
}

// #[warn(unused)]
// fn format_hex(argb: [u8; 4]) -> String {
//   format!("#{:02X}{:02X}{:02X}", argb[1], argb[2], argb[3])
// }

fn scheme_colors(scheme: &Scheme) -> [(&'static str, [u8; 4]); 49] {
  [
    ("primary", scheme.primary),
    ("on_primary", scheme.on_primary),
    ("primary_container", scheme.primary_container),
    ("on_primary_container", scheme.on_primary_container),
    ("secondary", scheme.secondary),
    ("on_secondary", scheme.on_secondary),
    ("secondary_container", scheme.secondary_container),
    ("on_secondary_container", scheme.on_secondary_container),
    ("tertiary", scheme.tertiary),
    ("on_tertiary", scheme.on_tertiary),
    ("tertiary_container", scheme.tertiary_container),
    ("on_tertiary_container", scheme.on_tertiary_container),
    ("error", scheme.error),
    ("on_error", scheme.on_error),
    ("error_container", scheme.error_container),
    ("on_error_container", scheme.on_error_container),
    ("background", scheme.background),
    ("on_background", scheme.on_background),
    ("surface", scheme.surface),
    ("on_surface", scheme.on_surface),
    ("surface_variant", scheme.surface_variant),
    ("on_surface_variant", scheme.on_surface_variant),
    ("outline", scheme.outline),
    ("outline_variant", scheme.outline_variant),
    ("shadow", scheme.shadow),
    ("scrim", scheme.scrim),
    ("inverse_surface", scheme.inverse_surface),
    ("inverse_on_surface", scheme.inverse_on_surface),
    ("inverse_primary", scheme.inverse_primary),
    ("primary_fixed", scheme.primary_container),
    ("primary_fixed_dim", scheme.primary),
    ("on_primary_fixed", scheme.on_primary_container),
    ("on_primary_fixed_variant", scheme.on_primary),
    ("secondary_fixed", scheme.secondary_container),
    ("secondary_fixed_dim", scheme.secondary),
    ("on_secondary_fixed", scheme.on_secondary_container),
    ("on_secondary_fixed_variant", scheme.on_secondary),
    ("tertiary_fixed", scheme.tertiary_container),
    ("tertiary_fixed_dim", scheme.tertiary),
    ("on_tertiary_fixed", scheme.on_tertiary_container),
    ("on_tertiary_fixed_variant", scheme.on_tertiary),
    ("surface_dim", scheme.surface),
    ("surface_bright", scheme.inverse_surface),
    ("surface_container_lowest", scheme.background),
    ("surface_container_low", scheme.surface),
    ("surface_container", scheme.surface_variant),
    ("surface_container_high", scheme.outline_variant),
    ("surface_container_highest", scheme.outline),
    ("surface_tint", scheme.primary),
  ]
}

type Formats = BTreeMap<&'static str, String>;
type Schemes = BTreeMap<&'static str, Formats>;

fn template_context(
  light: &Scheme,
  dark: &Scheme,
  seed: [u8; 4],
  image: &Path,
  default_light: bool,
  imported_base16: Option<&[[u8; 4]; 16]>,
  theme: &str,
) -> Context {
  let mut context = Context::new();
  let mut colors: BTreeMap<&str, Schemes> = BTreeMap::new();

  let mut light_colors: BTreeMap<_, _> = scheme_colors(light).into_iter().collect();
  let mut dark_colors: BTreeMap<_, _> = scheme_colors(dark).into_iter().collect();
  if imported_base16.is_none() {
    apply_dynamic_roles(&mut light_colors, light, seed, true);
    apply_dynamic_roles(&mut dark_colors, dark, seed, false);
  }

  for role in light_colors.keys() {
    let light_formats = matugen_formats(light_colors[role]);
    let dark_formats = matugen_formats(dark_colors[role]);
    let default_formats = if default_light {
      light_formats.clone()
    } else {
      dark_formats.clone()
    };
    let schemes = BTreeMap::from([
      ("light", light_formats),
      ("dark", dark_formats),
      ("default", default_formats.clone()),
    ]);
    colors.insert(role, schemes);

    // Keep the concise {{ primary.hex }} syntax supported as well.
    context.insert(*role, &default_formats);
  }

  let source_formats = matugen_formats(seed);
  colors.insert(
    "source_color",
    BTreeMap::from([
      ("light", source_formats.clone()),
      ("dark", source_formats.clone()),
      ("default", source_formats.clone()),
    ]),
  );
  context.insert("seed", &source_formats);
  insert_base16_context(
    &mut context,
    &mut colors,
    light,
    dark,
    default_light,
    imported_base16,
  );
  context.insert("colors", &colors);
  context.insert("image", &image.to_string_lossy());
  context.insert("wallpaper", &wallpaper_path(image));
  context.insert("theme", theme);
  context.insert("scheme", theme);

  context
}

fn wallpaper_path(image: &Path) -> String {
  if !image.as_os_str().is_empty() {
    return image
      .canonicalize()
      .unwrap_or_else(|_| image.to_path_buf())
      .to_string_lossy()
      .into_owned();
  }

  state_home()
    .ok()
    .and_then(|state| fs::read_to_string(state.join("seam/wallpaper.txt")).ok())
    .map(|path| path.trim().to_owned())
    .unwrap_or_default()
}

fn apply_dynamic_roles(
  colors: &mut BTreeMap<&'static str, [u8; 4]>,
  scheme: &Scheme,
  seed: [u8; 4],
  light: bool,
) {
  let source = Hct::from_int(seed);
  let hue = source.hue();
  let mut primary = TonalPalette::from_int(scheme.primary);
  let mut secondary = TonalPalette::from_int(scheme.secondary);
  let mut tertiary = TonalPalette::from_int(scheme.tertiary);
  let mut neutral = TonalPalette::from_hue_and_chroma(hue, 4.0);
  let mut neutral_variant = TonalPalette::from_hue_and_chroma(hue, 8.0);

  let surface_tones = if light {
    [87, 98, 98, 100, 96, 94, 92, 90]
  } else {
    [6, 6, 24, 4, 10, 12, 17, 22]
  };
  for (role, tone) in [
    ("surface_dim", surface_tones[0]),
    ("surface", surface_tones[1]),
    ("surface_bright", surface_tones[2]),
    ("surface_container_lowest", surface_tones[3]),
    ("surface_container_low", surface_tones[4]),
    ("surface_container", surface_tones[5]),
    ("surface_container_high", surface_tones[6]),
    ("surface_container_highest", surface_tones[7]),
  ] {
    colors.insert(role, neutral.tone(tone));
  }

  colors.insert("background", neutral.tone(if light { 98 } else { 6 }));
  colors.insert("on_background", neutral.tone(if light { 10 } else { 90 }));
  colors.insert(
    "surface_variant",
    neutral_variant.tone(if light { 90 } else { 30 }),
  );
  colors.insert(
    "on_surface_variant",
    neutral_variant.tone(if light { 30 } else { 80 }),
  );
  colors.insert("surface_tint", primary.tone(if light { 40 } else { 80 }));

  for (palette, roles) in [
    (
      &mut primary,
      [
        "primary_fixed",
        "primary_fixed_dim",
        "on_primary_fixed",
        "on_primary_fixed_variant",
      ],
    ),
    (
      &mut secondary,
      [
        "secondary_fixed",
        "secondary_fixed_dim",
        "on_secondary_fixed",
        "on_secondary_fixed_variant",
      ],
    ),
    (
      &mut tertiary,
      [
        "tertiary_fixed",
        "tertiary_fixed_dim",
        "on_tertiary_fixed",
        "on_tertiary_fixed_variant",
      ],
    ),
  ] {
    for (role, tone) in roles.into_iter().zip([90, 80, 10, 30]) {
      colors.insert(role, palette.tone(tone));
    }
  }
}

fn insert_base16_context(
  context: &mut Context,
  colors: &mut BTreeMap<&str, Schemes>,
  light_scheme: &Scheme,
  dark_scheme: &Scheme,
  default_light: bool,
  imported: Option<&[[u8; 4]; 16]>,
) {
  const COLOR_NAMES: [&str; 16] = [
    "color0", "color1", "color2", "color3", "color4", "color5", "color6", "color7", "color8",
    "color9", "color10", "color11", "color12", "color13", "color14", "color15",
  ];
  const BASE_NAMES: [&str; 16] = [
    "base00", "base01", "base02", "base03", "base04", "base05", "base06", "base07", "base08",
    "base09", "base0a", "base0b", "base0c", "base0d", "base0e", "base0f",
  ];
  let generated_light;
  let generated_dark;
  let (light, dark) = if let Some(imported) = imported {
    (imported, imported)
  } else {
    generated_light = base16_colors(light_scheme, true);
    generated_dark = base16_colors(dark_scheme, false);
    (&generated_light, &generated_dark)
  };
  let mut base16 = BTreeMap::new();

  for index in 0..16 {
    let light_formats = matugen_formats(light[index]);
    let dark_formats = matugen_formats(dark[index]);
    let default_formats = if default_light {
      light_formats.clone()
    } else {
      dark_formats.clone()
    };
    let schemes = BTreeMap::from([
      ("light", light_formats),
      ("dark", dark_formats),
      ("default", default_formats.clone()),
    ]);

    let color_name = COLOR_NAMES[index];
    let base_name = BASE_NAMES[index];
    colors.insert(color_name, schemes.clone());
    base16.insert(base_name, schemes);
    let default_hex = default_formats
      .get("hex")
      .expect("every generated color has a hex format");
    context.insert(color_name, default_hex);
    context.insert(base_name, default_hex);
  }

  context.insert("base16", &base16);
}

fn base16_colors(scheme: &Scheme, light: bool) -> [[u8; 4]; 16] {
  let normal_tone = if light { 40.0 } else { 70.0 };
  let bright_tone = if light { 30.0 } else { 82.0 };
  let accent = |hue, tone| Hct::from(hue, 48.0, tone).to_int();

  [
    scheme.background,
    accent(25.0, normal_tone),
    accent(145.0, normal_tone),
    accent(90.0, normal_tone),
    accent(260.0, normal_tone),
    accent(320.0, normal_tone),
    accent(200.0, normal_tone),
    scheme.on_surface,
    scheme.outline,
    accent(25.0, bright_tone),
    accent(145.0, bright_tone),
    accent(90.0, bright_tone),
    accent(260.0, bright_tone),
    accent(320.0, bright_tone),
    accent(200.0, bright_tone),
    if light {
      scheme.shadow
    } else {
      scheme.inverse_on_surface
    },
  ]
}

fn matugen_formats(color: [u8; 4]) -> Formats {
  let (hue, saturation, lightness) = rgb_to_hsl(color[1], color[2], color[3]);
  let hex_stripped = format!("{:02x}{:02x}{:02x}", color[1], color[2], color[3]);
  let hex_alpha_stripped = format!("{hex_stripped}{:02x}", color[0]);

  BTreeMap::from([
    ("hex", format!("#{hex_stripped}")),
    ("hex_stripped", hex_stripped.clone()),
    ("hex_plain", hex_stripped),
    ("hex_alpha", format!("#{hex_alpha_stripped}")),
    ("hex_alpha_stripped", hex_alpha_stripped),
    (
      "rgb",
      format!("rgb({}, {}, {})", color[1], color[2], color[3]),
    ),
    (
      "rgba",
      format!(
        "rgba({}, {}, {}, {})",
        color[1], color[2], color[3], color[0]
      ),
    ),
    (
      "hsl",
      format!("hsl({hue:.2}, {saturation:.2}%, {lightness:.2}%)"),
    ),
    (
      "hsla",
      format!(
        "hsla({hue:.2}, {saturation:.2}%, {lightness:.2}%, {:.3})",
        f64::from(color[0]) / 255.0
      ),
    ),
    ("red", color[1].to_string()),
    ("green", color[2].to_string()),
    ("blue", color[3].to_string()),
    ("alpha", color[0].to_string()),
    ("hue", format!("{hue:.2}")),
    ("saturation", format!("{saturation:.2}")),
    ("lightness", format!("{lightness:.2}")),
    (
      "argb",
      format!(
        "0x{:02X}{:02X}{:02X}{:02X}",
        color[0], color[1], color[2], color[3]
      ),
    ),
  ])
}

fn rgb_to_hsl(red: u8, green: u8, blue: u8) -> (f64, f64, f64) {
  let red = f64::from(red) / 255.0;
  let green = f64::from(green) / 255.0;
  let blue = f64::from(blue) / 255.0;
  let max = red.max(green).max(blue);
  let min = red.min(green).min(blue);
  let delta = max - min;
  let lightness = (max + min) / 2.0;

  if delta == 0.0 {
    return (0.0, 0.0, lightness * 100.0);
  }

  let saturation = delta / (1.0 - (2.0 * lightness - 1.0).abs());
  let hue = if max == red {
    60.0 * ((green - blue) / delta).rem_euclid(6.0)
  } else if max == green {
    60.0 * ((blue - red) / delta + 2.0)
  } else {
    60.0 * ((red - green) / delta + 4.0)
  };

  (hue, saturation * 100.0, lightness * 100.0)
}

struct ImportedBase16 {
  name: String,
  theme: String,
  colors: [[u8; 4]; 16],
}

fn dynamic_scheme_type(name: &str) -> Result<Option<&str>, Box<dyn std::error::Error>> {
  let Some(scheme_type) = name.strip_prefix("dynamic-") else {
    return Ok(None);
  };
  match scheme_type {
    "auto" | "tonal-spot" | "content" | "expressive" | "fidelity" | "fruit-salad"
    | "monochrome" | "neutral" | "rainbow" | "vibrant" => Ok(Some(scheme_type)),
    _ => Err(io::Error::new(
      io::ErrorKind::InvalidInput,
      format!(
        "unknown dynamic scheme '{scheme_type}'; available: auto, tonal-spot, content, expressive, fidelity, fruit-salad, monochrome, neutral, rainbow, vibrant"
      ),
    )
    .into()),
  }
}

fn load_base16(path: &Path) -> Result<ImportedBase16, Box<dyn std::error::Error>> {
  let source = fs::read_to_string(path)?;
  let fallback_name = path
    .file_stem()
    .and_then(|value| value.to_str())
    .unwrap_or("base16")
    .to_owned();
  let mut palette = parse_base16(&source, fallback_name)?;
  palette.theme = format!("base16 {}", palette.name.to_ascii_lowercase());
  Ok(palette)
}

fn load_bundled_base16(
  requested: &str,
  light: bool,
) -> Result<ImportedBase16, Box<dyn std::error::Error>> {
  let normalized = requested.to_ascii_lowercase().replace(['_', ' '], "-");
  let variant = if light { "light" } else { "dark" };
  let resolved = match normalized.as_str() {
    "catppuccin" => format!("catppuccin-{variant}"),
    "gruvbox" => format!("gruvbox-{variant}"),
    "rosepine" | "rose-pine" => format!("rosepine-{variant}"),
    "tokyonight" | "tokyo-night" => format!("tokyonight-{variant}"),
    explicit => explicit.to_owned(),
  };
  let source = match resolved.as_str() {
    "catppuccin-dark" => include_str!("../schemes/catppuccin-dark.yaml"),
    "catppuccin-light" => include_str!("../schemes/catppuccin-light.yaml"),
    "gruvbox-dark" => include_str!("../schemes/gruvbox-dark.yaml"),
    "gruvbox-light" => include_str!("../schemes/gruvbox-light.yaml"),
    "rosepine-dark" | "rose-pine-dark" => include_str!("../schemes/rosepine-dark.yaml"),
    "rosepine-light" | "rose-pine-light" => include_str!("../schemes/rosepine-light.yaml"),
    "tokyonight-dark" | "tokyo-night-dark" => include_str!("../schemes/tokyonight-dark.yaml"),
    "tokyonight-light" | "tokyo-night-light" => include_str!("../schemes/tokyonight-light.yaml"),
    _ => {
      return Err(io::Error::new(
        io::ErrorKind::InvalidInput,
        format!(
          "unknown scheme '{requested}'; available: dynamic-tonal-spot, catppuccin, gruvbox, rosepine, tokyonight"
        ),
      )
      .into());
    }
  };
  let mut palette = parse_base16(source, resolved.clone())?;
  palette.theme = resolved.replace('-', " ");
  Ok(palette)
}

fn parse_base16(
  source: &str,
  mut name: String,
) -> Result<ImportedBase16, Box<dyn std::error::Error>> {
  let mut colors = [[0_u8; 4]; 16];
  let mut found = [false; 16];

  for raw_line in source.lines() {
    let line = raw_line.trim();
    if line.is_empty() || line.starts_with('#') {
      continue;
    }
    let Some((raw_key, raw_value)) = line.split_once(':') else {
      continue;
    };
    let key = raw_key
      .trim()
      .trim_matches(['\'', '"'])
      .to_ascii_lowercase();
    let value = yaml_scalar(raw_value);

    if matches!(key.as_str(), "scheme" | "name") && !value.is_empty() {
      name = value.to_owned();
      continue;
    }

    let Some(suffix) = key.strip_prefix("base") else {
      continue;
    };
    if suffix.len() != 2 {
      continue;
    }
    let Ok(index) = usize::from_str_radix(suffix, 16) else {
      continue;
    };
    if index >= 16 {
      continue;
    }
    colors[index] = parse_hex_color(value).ok_or_else(|| {
      io::Error::new(
        io::ErrorKind::InvalidData,
        format!("invalid Base16 color for {key}: {value:?}"),
      )
    })?;
    found[index] = true;
  }

  if let Some(index) = found.iter().position(|present| !present) {
    return Err(
      io::Error::new(
        io::ErrorKind::InvalidData,
        format!("Base16 YAML is missing base{index:02x}"),
      )
      .into(),
    );
  }

  Ok(ImportedBase16 {
    theme: name.to_ascii_lowercase(),
    name,
    colors,
  })
}

fn yaml_scalar(raw: &str) -> &str {
  let value = raw.trim();
  if let Some(quote @ ('\'' | '"')) = value.chars().next() {
    let quoted = &value[quote.len_utf8()..];
    return quoted
      .find(quote)
      .map_or(quoted, |closing_quote| &quoted[..closing_quote]);
  }

  value
    .split_once(" #")
    .map_or(value, |(scalar, _comment)| scalar)
    .trim()
}

fn parse_hex_color(value: &str) -> Option<[u8; 4]> {
  let value = value.trim().trim_start_matches('#');
  if value.len() != 6 {
    return None;
  }
  Some([
    0xff,
    u8::from_str_radix(&value[0..2], 16).ok()?,
    u8::from_str_radix(&value[2..4], 16).ok()?,
    u8::from_str_radix(&value[4..6], 16).ok()?,
  ])
}

fn scheme_from_base16(base: &[[u8; 4]; 16]) -> Scheme {
  let background = base[0x00];
  let foreground = base[0x05];
  let primary = base[0x0d];
  let secondary = base[0x0c];
  let tertiary = base[0x0e];
  let error = base[0x08];
  let primary_container = readable_container(background, primary);
  let secondary_container = readable_container(background, secondary);
  let tertiary_container = readable_container(background, tertiary);
  let error_container = readable_container(background, error);

  Scheme {
    primary,
    on_primary: best_contrast(primary, background, foreground),
    primary_container,
    on_primary_container: best_contrast(primary_container, background, foreground),
    secondary,
    on_secondary: best_contrast(secondary, background, foreground),
    secondary_container,
    on_secondary_container: best_contrast(secondary_container, background, foreground),
    tertiary,
    on_tertiary: best_contrast(tertiary, background, foreground),
    tertiary_container,
    on_tertiary_container: best_contrast(tertiary_container, background, foreground),
    error,
    on_error: best_contrast(error, background, foreground),
    error_container,
    on_error_container: best_contrast(error_container, background, foreground),
    background,
    on_background: foreground,
    surface: base[0x01],
    on_surface: foreground,
    surface_variant: base[0x02],
    on_surface_variant: foreground,
    outline: base[0x04],
    outline_variant: base[0x03],
    shadow: background,
    scrim: background,
    inverse_surface: base[0x06],
    inverse_on_surface: base[0x01],
    inverse_primary: base[0x0c],
  }
}

fn readable_container(background: [u8; 4], accent: [u8; 4]) -> [u8; 4] {
  let amount = if luminance(background) < 0.5 {
    0.22
  } else {
    0.16
  };
  mix(background, accent, amount)
}

fn mix(background: [u8; 4], foreground: [u8; 4], amount: f64) -> [u8; 4] {
  let channel = |index| {
    (f64::from(background[index]) * (1.0 - amount) + f64::from(foreground[index]) * amount).round()
      as u8
  };
  [0xff, channel(1), channel(2), channel(3)]
}

fn best_contrast(color: [u8; 4], first: [u8; 4], second: [u8; 4]) -> [u8; 4] {
  if contrast_ratio(color, first) >= contrast_ratio(color, second) {
    first
  } else {
    second
  }
}

fn contrast_ratio(first: [u8; 4], second: [u8; 4]) -> f64 {
  let (lighter, darker) = if luminance(first) >= luminance(second) {
    (luminance(first), luminance(second))
  } else {
    (luminance(second), luminance(first))
  };
  (lighter + 0.05) / (darker + 0.05)
}

fn luminance(color: [u8; 4]) -> f64 {
  let linear = |channel: u8| {
    let value = f64::from(channel) / 255.0;
    if value <= 0.04045 {
      value / 12.92
    } else {
      ((value + 0.055) / 1.055).powf(2.4)
    }
  };
  0.2126 * linear(color[1]) + 0.7152 * linear(color[2]) + 0.0722 * linear(color[3])
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn writes_dark_mode_and_scheme_state() {
    let directory = std::env::temp_dir().join(format!("seam-state-{}", std::process::id()));
    write_state_to(&directory, false, "dynamic tonal-spot").unwrap();

    assert_eq!(
      fs::read_to_string(directory.join("mode.txt")).unwrap(),
      "dark\n"
    );
    assert_eq!(
      fs::read_to_string(directory.join("scheme.txt")).unwrap(),
      "dynamic tonal-spot\n"
    );

    fs::remove_dir_all(directory).unwrap();
  }

  #[test]
  fn parses_every_bundled_scheme() {
    for name in ["catppuccin", "gruvbox", "rosepine", "tokyonight"] {
      load_bundled_base16(name, false).unwrap();
      load_bundled_base16(name, true).unwrap();
    }
  }

  #[test]
  fn recognizes_dynamic_scheme_types() {
    assert_eq!(
      dynamic_scheme_type("dynamic-tonal-spot").unwrap(),
      Some("tonal-spot")
    );
    assert_eq!(
      dynamic_scheme_type("dynamic-fruit-salad").unwrap(),
      Some("fruit-salad")
    );
    assert_eq!(dynamic_scheme_type("dynamic-auto").unwrap(), Some("auto"));
    assert_eq!(
      dynamic_scheme_type("dynamic-vibrant").unwrap(),
      Some("vibrant")
    );
    assert_eq!(dynamic_scheme_type("catppuccin").unwrap(), None);
    assert!(dynamic_scheme_type("dynamic-unknown").is_err());
  }

  #[test]
  fn renders_valid_hyprland_color_scalars() {
    let seed = [0xff, 0x67, 0x50, 0xa4];
    let mut palette = CorePalette::new(seed, false);
    let light = Scheme::light_from_core_palette(&mut palette);
    let dark = Scheme::dark_from_core_palette(&mut palette);
    let context = template_context(
      &light,
      &dark,
      seed,
      Path::new("wallpaper.png"),
      false,
      None,
      "dynamic tonal-spot",
    );
    let rendered =
      tera::Tera::one_off(include_str!("../templates/hyprland.lua"), &context, false).unwrap();

    assert!(rendered.contains("focused = \"#"));
    assert!(rendered.contains("focused2 = \"#"));
    assert!(rendered.contains("unfocused = \"#"));
    assert!(!rendered.contains("{\"alpha\""));
  }

  #[test]
  fn renders_kitty_base16_colors() {
    let seed = [0xff, 0x67, 0x50, 0xa4];
    let mut palette = CorePalette::new(seed, false);
    let light = Scheme::light_from_core_palette(&mut palette);
    let dark = Scheme::dark_from_core_palette(&mut palette);
    let context = template_context(
      &light,
      &dark,
      seed,
      Path::new("wallpaper.png"),
      false,
      None,
      "dynamic tonal-spot",
    );
    let rendered =
      tera::Tera::one_off(include_str!("../templates/kitty.conf"), &context, false).unwrap();

    for index in 0..16 {
      assert!(rendered.contains(&format!("color{index}")));
    }
    assert!(!rendered.contains("{{"));
  }

  #[test]
  fn renders_kde_color_scheme() {
    let seed = [0xff, 0x67, 0x50, 0xa4];
    let mut palette = CorePalette::new(seed, false);
    let light = Scheme::light_from_core_palette(&mut palette);
    let dark = Scheme::dark_from_core_palette(&mut palette);
    let context = template_context(
      &light,
      &dark,
      seed,
      Path::new("wallpaper.png"),
      false,
      None,
      "dynamic tonal-spot",
    );
    let rendered = tera::Tera::one_off(
      include_str!("../templates/kcolorscheme.colors"),
      &context,
      false,
    )
    .unwrap();

    assert!(rendered.contains("[Colors:Window]"));
    assert!(rendered.contains("ColorScheme=Seam"));
    assert!(!rendered.contains("{{"));
  }

  #[test]
  fn exposes_wallpaper_template_variable() {
    let seed = [0xff, 0x67, 0x50, 0xa4];
    let mut palette = CorePalette::new(seed, false);
    let light = Scheme::light_from_core_palette(&mut palette);
    let dark = Scheme::dark_from_core_palette(&mut palette);
    let context = template_context(
      &light,
      &dark,
      seed,
      Path::new("wallpaper.png"),
      false,
      None,
      "dynamic tonal-spot",
    );
    let rendered = tera::Tera::one_off("{{ wallpaper }}", &context, false).unwrap();

    assert!(rendered.ends_with("wallpaper.png"));
  }
}
