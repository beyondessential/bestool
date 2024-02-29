use std::path::PathBuf;

use clap::{Parser, ValueEnum};
use fontdue::{
	layout::{HorizontalAlign, Layout, LayoutSettings, TextStyle, VerticalAlign},
	Font, FontSettings,
};
use miette::{miette, IntoDiagnostic, Result, WrapErr};
use rust_fontconfig::{FcFontCache, FcFontPath, FcPattern, PatternMatch};
use tracing::{debug, instrument};

use crate::actions::Context;

use super::EinkArgs;

/// Print some text.
///
/// By default, this tries to load a few well-known system fonts, then falls back to the first font
/// it can find. To change this, either use a specific with `--font-name` or `--font-file`, or use
/// the `--family`, `--monospace`, `--bold`, and `--italic` selectors to filter the system fonts.
/// If it can't find a matching font, it will fall back; and if it can't find any font it will fail.
#[derive(Debug, Clone, Parser)]
pub struct TextArgs {
	/// Text to display.
	#[arg(
		name = "TEXT",
		default_value = "",
		required_unless_present = "list_fonts"
	)]
	pub text: String,

	/// Load a TTF or OTF font file directly.
	#[arg(long, conflicts_with_all = &["font_name", "family", "monospace", "bold", "italic"])]
	pub font_file: Option<PathBuf>,

	/// Load a system font by name.
	#[arg(long, conflicts_with_all = &["font_file", "family", "monospace", "bold", "italic"])]
	pub font_name: Option<String>,

	/// Select a system font by family name.
	#[arg(long)]
	pub family: Option<String>,

	/// Select a monospace font.
	///
	/// The default is to use any font.
	///
	/// The underlying library has a known issue where on some systems it fails to find any
	/// monospace fonts, so this may not work as expected. See <https://github.com/fschutt/rust-fontconfig/issues/6>.
	#[arg(long)]
	pub monospace: bool,

	/// Select a bold font.
	#[arg(long)]
	pub bold: bool,

	/// Select an italic font.
	#[arg(long)]
	pub italic: bool,

	/// Font size in pixels.
	#[arg(long, default_value = "12.0")]
	pub size: f32,

	/// Start from this line offset, in pixels.
	#[arg(long, conflicts_with = "start_line")]
	pub start_pixel: Option<u16>,

	/// Start from this line offset, in lines of the current font size.
	#[arg(long)]
	pub start_line: Option<u16>,

	/// Don't wrap the text.
	///
	/// This will cause the text to be cut off if it's too long for the display.
	#[arg(long)]
	pub nowrap: bool,

	/// Horizontal alignment.
	#[arg(long, default_value = "left", alias = "align")]
	pub align_h: HAlign,

	/// Vertical alignment.
	#[arg(long, default_value = "top")]
	pub align_v: VAlign,

	/// Line height as a multiplier of the font default.
	#[arg(long, default_value = "1.0")]
	pub line_height: f32,

	/// Don't print to the screen, only list matched fonts.
	///
	/// This is useful for debugging font selection. For stable use it's better to use `--font-name`
	/// or a font file, you can use this flag to find a font to use or to discover what's available.
	///
	/// Also prints the font that would be used if this flag were not present.
	#[arg(long, conflicts_with_all = &["font_file", "font_name"])]
	pub list_fonts: bool,
}

pub async fn run(ctx: Context<EinkArgs, TextArgs>) -> Result<()> {
	if ctx.args_sub.list_fonts {
		println!("Fonts matched by selectors:");
		list_fonts(&ctx.args_sub);

		let font = select_font(&ctx.args_sub);
		println!("\nWould use: {font:?}");
		return Ok(());
	}

	let font = load_font(&ctx.args_sub)?;
	let fonts = &[font][..];

	let mut layout = Layout::new(fontdue::layout::CoordinateSystem::PositiveYUp);
	layout.reset(&LayoutSettings {
		max_width: Some(ctx.args_top.width as f32),
		max_height: Some(ctx.args_top.height as f32),
		horizontal_align: ctx.args_sub.align_h.into(),
		vertical_align: ctx.args_sub.align_v.into(),
		line_height: ctx.args_sub.line_height,
		..LayoutSettings::default()
	});
	layout.append(
		fonts,
		&TextStyle::new(&ctx.args_sub.text, ctx.args_sub.size, 0),
	);

	for glyph in layout.glyphs() {
		println!("{:?}", glyph);
		let (metrics, bitmap) = fonts[glyph.font_index].rasterize(glyph.parent, glyph.key.px);
		dbg!(metrics);
		println!("{bitmap:?}");
		break;
	}

	Ok(())
}

#[instrument(level = "debug")]
fn load_font(args: &TextArgs) -> Result<Font> {
	if let Some(path) = &args.font_file {
		debug!(?path, "loading font from file");
		let file = std::fs::read(path)
			.into_diagnostic()
			.wrap_err(format!("reading file {path:?}"))?;

		return Font::from_bytes(file, FontSettings::default())
			.map_err(|err| miette!("loading font: {}", err));
	}

	let FcFontPath { path, font_index } = select_font(args);

	debug!(?path, ?font_index, "loading font from system");
	let file = std::fs::read(&path)
		.into_diagnostic()
		.wrap_err(format!("reading file {path:?}"))?;
	Font::from_bytes(
		file,
		FontSettings {
			collection_index: u32::try_from(font_index).expect("font index too large"),
			..Default::default()
		},
	)
	.map_err(|err| miette!("loading font: {}", err))
}

#[instrument(level = "debug")]
fn select_font(args: &TextArgs) -> FcFontPath {
	debug!("building font cache");
	let cache = FcFontCache::build();

	if let Some(name) = &args.font_name {
		debug!(?name, "querying for a specific name");
		if let Some(font) = cache.query(&FcPattern {
			name: Some(name.clone()),
			..Default::default()
		}) {
			debug!(?font, "found a font");
			return font.clone();
		}
	}

	let attributes = FcPattern {
		italic: if args.italic {
			PatternMatch::True
		} else {
			PatternMatch::False
		},
		bold: if args.bold {
			PatternMatch::True
		} else {
			PatternMatch::False
		},
		monospace: if args.monospace {
			PatternMatch::True
		} else {
			// use dontcare instead of false to workaround https://github.com/fschutt/rust-fontconfig/issues/6
			PatternMatch::DontCare
		},
		..Default::default()
	};

	if let Some(family) = &args.family {
		debug!(?family, "querying for family");
		if let Some(font) = cache.query(&FcPattern {
			family: Some(family.clone()),
			..attributes.clone()
		}) {
			debug!(?font, "found a font");
			return font.clone();
		}
	}

	for well_known in if args.monospace {
		&WELL_KNOWN_MONOSPACE[..]
	} else {
		&WELL_KNOWN_SANS[..]
	} {
		debug!(?well_known, "querying for well-known font by family");
		if let Some(font) = cache.query(&FcPattern {
			family: Some(well_known.to_string()),
			monospace: PatternMatch::DontCare, // as already set by WELL_KNOWN_*
			..attributes.clone()
		}) {
			debug!(?font, "found a font");
			return font.clone();
		}

		debug!(?well_known, "querying for well-known font by name");
		if let Some(font) = cache.query(&FcPattern {
			name: Some(well_known.to_string()),
			monospace: PatternMatch::DontCare, // as already set by WELL_KNOWN_*
			..attributes.clone()
		}) {
			debug!(?font, "found a font");
			return font.clone();
		}

		debug!(
			?well_known,
			"querying for well-known font by family without attributes"
		);
		if let Some(font) = cache.query(&FcPattern {
			family: Some(well_known.to_string()),
			..Default::default()
		}) {
			debug!(?font, "found a font");
			return font.clone();
		}

		debug!(
			?well_known,
			"querying for well-known font by name without attributes"
		);
		if let Some(font) = cache.query(&FcPattern {
			name: Some(well_known.to_string()),
			..Default::default()
		}) {
			debug!(?font, "found a font");
			return font.clone();
		}
	}

	debug!("querying for any font matching attributes");
	if let Some(font) = cache.query(&attributes) {
		debug!(?font, "found a font");
		return font.clone();
	}

	if args.monospace {
		debug!("querying for any font matching attributes (without monospace)");
		if let Some(font) = cache.query(&FcPattern {
			monospace: PatternMatch::DontCare,
			..attributes
		}) {
			debug!(?font, "found a font");
			return font.clone();
		}
	}

	debug!("querying for any font at all");
	cache
		.query(&FcPattern::default())
		.expect("there are no fonts on this system")
		.clone()
}

const WELL_KNOWN_MONOSPACE: [&str; 5] = [
	// fonts commonly installed on linux
	"DejaVu Sans Mono",
	"Noto Sans Mono",
	// fonts commonly installed on windows
	"Consolas",
	// fonts commonly installed on macOS
	"Monaco",
	"Menlo",
];

const WELL_KNOWN_SANS: [&str; 6] = [
	// fonts commonly installed on linux
	"DejaVu Sans",
	"Noto Sans",
	// fonts commonly installed on windows
	"Segoe UI",
	"Arial",
	// fonts commonly installed on macOS
	"Helvetica",
	"Arial",
];

#[instrument(level = "debug")]
fn list_fonts(args: &TextArgs) {
	let cache = FcFontCache::build();

	let attributes = FcPattern {
		family: args.family.clone(),
		italic: if args.italic {
			PatternMatch::True
		} else {
			PatternMatch::False
		},
		bold: if args.bold {
			PatternMatch::True
		} else {
			PatternMatch::False
		},
		monospace: if args.monospace {
			PatternMatch::True
		} else {
			// use dontcare instead of false to workaround https://github.com/fschutt/rust-fontconfig/issues/6
			PatternMatch::DontCare
		},
		..Default::default()
	};

	for font in cache.query_all(&attributes) {
		println!("{:?}", font);
	}
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, ValueEnum)]
#[clap(rename_all = "lowercase")]
pub enum HAlign {
	#[default]
	Left,
	Center,
	Right,
}

impl From<HAlign> for HorizontalAlign {
	fn from(align: HAlign) -> Self {
		match align {
			HAlign::Left => HorizontalAlign::Left,
			HAlign::Center => HorizontalAlign::Center,
			HAlign::Right => HorizontalAlign::Right,
		}
	}
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, ValueEnum)]
#[clap(rename_all = "lowercase")]
pub enum VAlign {
	#[default]
	Top,
	Middle,
	Bottom,
}

impl From<VAlign> for VerticalAlign {
	fn from(align: VAlign) -> Self {
		match align {
			VAlign::Top => VerticalAlign::Top,
			VAlign::Middle => VerticalAlign::Middle,
			VAlign::Bottom => VerticalAlign::Bottom,
		}
	}
}
