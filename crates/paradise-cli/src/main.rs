use std::collections::HashSet;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand, ValueEnum};
use paradise_core::charset::Charset;
use paradise_core::convert::{Converter, Variant};
use paradise_core::glyphpack::FontChoice;
use paradise_core::mapping::{describe, export_template, Mapping};
use paradise_core::patch::{changed_glyphs, patch, verify_with, PatchOptions, RuntimeLanguage};
use paradise_core::preview::{affected_ids, render_comparison};
use paradise_core::rom::Rom;
#[derive(Parser)]
#[command(
    name = "paradise-cli",
    version,
    about = "Tamagotchi Paradise Simplified-to-Traditional Chinese ROM patcher"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Clone, Copy, ValueEnum)]
enum VariantArg {
    Hk,
    Tw,
    Standard,
}

impl From<VariantArg> for Variant {
    fn from(v: VariantArg) -> Variant {
        match v {
            VariantArg::Hk => Variant::HongKong,
            VariantArg::Tw => Variant::Taiwan,
            VariantArg::Standard => Variant::Standard,
        }
    }
}

#[derive(Clone, Copy, ValueEnum)]
enum FontArg {
    /// Fusion Pixel Font 12px (https://github.com/TakWolf/fusion-pixel-font)
    Fusion,
    /// Ark Pixel Font 12px (https://github.com/TakWolf/ark-pixel-font)
    Ark,
    /// Cubic 11 (https://github.com/ACh-K/Cubic-11)
    Cubic11,
    /// Mona12 (https://github.com/MonadABXY/mona-font)
    Mona12,
    /// Mona12 Bold
    Mona12Bold,
    /// Zpix (https://github.com/SolidZORO/zpix-pixel-font)
    Zpix,
    /// Z Labs RoundPix 12px (https://github.com/Astro-2539/ZLabs-RoundPix-12px)
    Roundpix,
    /// GNU Unifont, 16x16 (https://unifoundry.com/unifont/)
    Unifont,
    /// Galmuri11 (https://github.com/quiple/galmuri)
    Galmuri11,
    /// Fusion Bold Pixel Font 12px (https://github.com/pixel-font-studio/fusion-bold-pixel-font)
    FusionBold,
    /// BoutiqueBitmap9x9 Bold (https://github.com/scott0107000/BoutiqueBitmap9x9)
    Boutique9Bold,
}

impl From<FontArg> for FontChoice {
    fn from(f: FontArg) -> FontChoice {
        match f {
            FontArg::Fusion => FontChoice::FusionPixel,
            FontArg::Ark => FontChoice::ArkPixel,
            FontArg::Cubic11 => FontChoice::Cubic11,
            FontArg::Mona12 => FontChoice::Mona12,
            FontArg::Mona12Bold => FontChoice::Mona12Bold,
            FontArg::Zpix => FontChoice::Zpix,
            FontArg::Roundpix => FontChoice::RoundPix12,
            FontArg::Unifont => FontChoice::Unifont,
            FontArg::Galmuri11 => FontChoice::Galmuri11,
            FontArg::FusionBold => FontChoice::FusionBold,
            FontArg::Boutique9Bold => FontChoice::Boutique9Bold,
        }
    }
}

#[derive(Subcommand)]
enum Command {
    /// Show what was detected in a ROM
    Inspect { rom: PathBuf },
    /// List the Chinese strings (id and text), to find the text a mapping file should match
    Strings {
        rom: PathBuf,
        /// Only strings containing this text
        #[arg(long)]
        grep: Option<String>,
        /// Write a mapping JSON listing these strings with their converted text, to edit and
        /// pass to patch --mapping
        #[arg(long)]
        export: Option<PathBuf>,
        /// Conversion used for the exported values
        #[arg(long, value_enum, default_value = "hk")]
        variant: VariantArg,
    },
    /// Convert the Chinese text and font, writing a new ROM
    Patch {
        rom: PathBuf,
        /// Output ROM path (defaults to <name>_zh-hk.bin, <name>_zh-tw.bin, or <name>_zht.bin)
        #[arg(short, long)]
        output: Option<PathBuf>,
        #[arg(long, value_enum, default_value = "hk")]
        variant: VariantArg,
        /// Pixel font used for redrawn glyphs
        #[arg(long, value_enum, default_value = "cubic11")]
        font: FontArg,
        /// JSON file of text replacements, e.g. {"拓麻": "Tama"}
        #[arg(long)]
        mapping: Option<PathBuf>,
        /// Do not add the Language entry to the Settings menu
        #[arg(long)]
        no_runtime_language: bool,
        /// Keep the game's own glyphs for characters shared by Simplified and Traditional
        #[arg(long)]
        keep_shared_glyphs: bool,
        /// Keep the game's own Latin letters and digits instead of redrawing them in the font
        #[arg(long)]
        keep_latin_glyphs: bool,
        /// Run the conversion and report without writing the ROM
        #[arg(long)]
        dry_run: bool,
        /// Overwrite the output file if it exists
        #[arg(long)]
        force: bool,
    },
    /// Check a patched ROM against the original
    Verify {
        original: PathBuf,
        patched: PathBuf,
        /// The mapping file used when patching
        #[arg(long)]
        mapping: Option<PathBuf>,
    },
    /// Render before/after PNG of changed strings
    Preview {
        original: PathBuf,
        patched: PathBuf,
        #[arg(short, long, default_value = "preview.png")]
        output: PathBuf,
        /// Maximum number of strings to show
        #[arg(long, default_value_t = 40)]
        limit: usize,
    },
}

fn read_rom(path: &Path) -> Result<Rom> {
    let bytes = std::fs::read(path).with_context(|| format!("reading {}", path.display()))?;
    Ok(Rom::load(bytes)?)
}

fn default_output(input: &Path, variant: Variant) -> PathBuf {
    let stem = input.file_stem().and_then(|s| s.to_str()).unwrap_or("rom");
    input.with_file_name(format!("{stem}_{}.bin", variant.output_suffix()))
}

fn inspect(path: &Path) -> Result<()> {
    let rom = read_rom(path)?;
    let i = rom.info();
    println!("ROM size:             {} bytes", i.rom_len);
    if let Some((name, id)) = i.edition_name().zip(i.edition) {
        println!("Edition:              {id} ({name})");
    }
    if let Some((count, translated)) = rom.neon_items()? {
        let names = if translated {
            "Traditional Chinese names"
        } else {
            "original names"
        };
        println!("Neon items:           {count} ({names})");
    }
    println!(
        "Sonix signature:      {}",
        if i.signature_ok { "ok" } else { "missing" }
    );
    println!("Asset archive:        {} bytes at {:#x}", i.asset_len, i.asset_base);
    println!(
        "Archive checksums:    {}",
        if i.top_checksum_ok && i.strings_checksum_ok {
            "ok"
        } else {
            "MISMATCH"
        }
    );
    println!("Sprites:              {}", i.sprite_count);
    println!(
        "CJK font sprite:      #{} ({} glyphs)",
        i.font_sprite, i.font_glyphs
    );
    println!(
        "Chinese string table: #{} ({} strings, {} distinct hanzi)",
        i.zh_table, i.zh_string_count, i.zh_hanzi_distinct
    );
    if let Some(startup) = rom.startup()? {
        println!("Startup Chinese table: #{} ({} strings)", startup.info().zh_table,
            startup.info().zh_string_count);
    }
    Ok(())
}

struct PatchArgs {
    output: Option<PathBuf>,
    variant: VariantArg,
    font: FontArg,
    mapping: Option<PathBuf>,
    runtime_language: bool,
    redraw_shared: bool,
    redraw_latin: bool,
    dry_run: bool,
    force: bool,
}

fn do_patch(rom_path: &Path, args: PatchArgs) -> Result<()> {
    let PatchArgs {
        output,
        variant,
        font,
        mapping,
        runtime_language,
        redraw_shared,
        redraw_latin,
        dry_run,
        force,
    } = args;
    let rom = read_rom(rom_path)?;
    let variant = Variant::from(variant);
    let out_path = output.unwrap_or_else(|| default_output(rom_path, variant));
    if !dry_run {
        if out_path == rom_path {
            bail!("output path must differ from the input ROM");
        }
        if out_path.exists() && !force {
            bail!(
                "{} already exists (use --force to overwrite)",
                out_path.display()
            );
        }
    }
    let opts = PatchOptions {
        variant,
        font: font.into(),
        runtime_language,
        redraw_shared,
        redraw_latin,
        mapping: mapping.as_deref().map(read_mapping).transpose()?,
    };
    let result = patch(&rom, &opts, &mut |m| eprintln!("- {m}"))?;
    let r = &result.report;
    println!(
        "Strings changed:      {} of {}",
        r.stats.strings_changed, r.stats.strings_total
    );
    if let Some(startup) = &r.startup {
        println!("Startup strings changed: {} of {}", startup.stats.strings_changed,
            startup.stats.strings_total);
        for unresolved in &startup.unresolved {
            println!("Startup unresolved: {} -> {} x{} ({})", unresolved.source,
                unresolved.target, unresolved.count, unresolved.reason);
        }
        for &(ch, count) in &startup.remaining_simplified {
            println!("Startup Simplified character remaining: {ch} x{count}");
        }
    }
    println!("Characters converted: {}", r.stats.chars_converted);
    println!("Existing glyphs reused: {}", r.stats.reused_codes);
    println!("Glyph slots reskinned:  {}", r.stats.reskinned);
    println!("Shared glyphs restyled: {}", r.stats.restyled);
    println!("Overflow glyph slots:   {}", r.stats.overflow);
    if r.stats.restyle_skipped > 0 {
        println!(
            "Shared glyphs kept in the game's style (no spare slot): {}",
            r.stats.restyle_skipped
        );
    }
    if r.stats.unidentified_glyphs > 0 {
        println!(
            "Existing glyphs kept (identity uncertain): {}",
            r.stats.unidentified_glyphs
        );
    }
    println!("Glyph font:             {}", opts.font.label());
    println!("Latin glyphs redrawn:   {}", r.latin_glyphs);
    if r.fallback_glyphs > 0 {
        println!(
            "Glyphs taken from Fusion Pixel (missing in the chosen font): {}",
            r.fallback_glyphs
        );
    }
    println!(
        "Asset size:           {} -> {} bytes",
        r.asset_len_before, r.asset_len_after
    );
    if r.font_grew {
        println!("CJK font exceeded its original slot; later sprites were relocated.");
    }
    println!(
        "Settings > Language:  {}",
        runtime_language_label(r.runtime_language)
    );
    if !r.mapping_hits.is_empty() {
        let used = r.mapping_hits.iter().filter(|(_, n)| *n > 0).count();
        println!(
            "Mapping entries used:   {} of {}",
            used,
            r.mapping_hits.len()
        );
        for (k, n) in r.mapping_hits.iter().filter(|(_, n)| *n == 0) {
            println!("  no match for \"{k}\" ({n} times)");
        }
    }
    if !r.unresolved.is_empty() {
        println!("Unconverted ({}):", r.unresolved.len());
        for u in &r.unresolved {
            println!("  {} -> {} x{} ({})", u.source, u.target, u.count, u.reason);
        }
    }
    if !r.remaining_simplified.is_empty() {
        let list: String = r.remaining_simplified.iter().map(|(c, _)| *c).collect();
        println!("Simplified characters left: {list}");
    }
    if dry_run {
        println!("Dry run: nothing written");
    } else {
        std::fs::write(&out_path, &result.bytes)
            .with_context(|| format!("writing {}", out_path.display()))?;
        println!("Wrote {}", out_path.display());
    }
    Ok(())
}

fn runtime_language_label(r: RuntimeLanguage) -> &'static str {
    match r {
        RuntimeLanguage::Disabled => "not requested",
        RuntimeLanguage::Added => "added (experimental)",
        RuntimeLanguage::AlreadyPresent => "already present",
        RuntimeLanguage::Unsupported => "skipped: firmware does not match a known build",
    }
}

fn read_mapping(path: &Path) -> Result<Mapping> {
    let text =
        std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    Ok(Mapping::parse(text.trim_start_matches('\u{feff}'))?)
}

fn do_strings(
    rom_path: &Path,
    grep: Option<&str>,
    export: Option<&Path>,
    variant: Variant,
) -> Result<()> {
    let rom = read_rom(rom_path)?;
    let cs = Charset::standard();
    let table = rom.table(rom.info().zh_table)?;
    if let Some(path) = export {
        let conv = Converter::with_mapping(variant, Mapping::default(), &cs)?;
        let json = export_template(&table, &cs, &conv, grep);
        std::fs::write(path, json)?;
        println!("Wrote {}", path.display());
        return Ok(());
    }
    let startup = rom.startup()?;
    let mut scopes = vec![("Main", table)];
    if let Some(startup) = &startup {
        scopes.push(("Startup", startup.table(startup.info().zh_table)?));
    }
    for (label, table) in scopes {
        if startup.is_some() {
            println!("[{label}]");
        }
        for (i, s) in table.strings.iter().enumerate() {
            let text = describe(&cs, s);
            if !text.is_empty() && grep.is_none_or(|g| text.contains(g)) {
                println!("{:>4}  {text}", i + 1);
            }
        }
    }
    Ok(())
}

fn do_verify(original: &Path, patched: &Path, mapping: Option<&Path>) -> Result<()> {
    let a = std::fs::read(original)?;
    let b = std::fs::read(patched)?;
    let mapping = mapping.map(read_mapping).transpose()?;
    let issues = verify_with(&a, &b, mapping.as_ref())?;
    if issues.is_empty() {
        println!("OK: patched ROM is structurally valid and differs only where expected");
        Ok(())
    } else {
        for i in &issues {
            println!("problem: {i}");
        }
        bail!("verification failed")
    }
}

fn do_preview(original: &Path, patched: &Path, output: &Path, limit: usize) -> Result<()> {
    let a = read_rom(original)?;
    let b = read_rom(patched)?;
    let changed: HashSet<usize> = changed_glyphs(&a, &b)?.into_iter().collect();
    let (ta, tb) = (a.table(a.info().zh_table)?, b.table(b.info().zh_table)?);
    let mut ids = affected_ids(&ta, &tb, &changed);
    ids.truncate(limit);
    let (fa, fb) = (a.font()?, b.font()?);
    let img = render_comparison((&fa, &ta), (&fb, &tb), &ids, 480);
    img.write_png(output)?;
    println!("Wrote {} ({} strings)", output.display(), ids.len());
    Ok(())
}

fn main() -> Result<()> {
    match Cli::parse().command {
        Command::Inspect { rom } => inspect(&rom),
        Command::Strings {
            rom,
            grep,
            export,
            variant,
        } => do_strings(&rom, grep.as_deref(), export.as_deref(), variant.into()),
        Command::Patch {
            rom,
            output,
            variant,
            font,
            mapping,
            no_runtime_language,
            keep_shared_glyphs,
            keep_latin_glyphs,
            dry_run,
            force,
        } => do_patch(
            &rom,
            PatchArgs {
                output,
                variant,
                font,
                mapping,
                runtime_language: !no_runtime_language,
                redraw_shared: !keep_shared_glyphs,
                redraw_latin: !keep_latin_glyphs,
                dry_run,
                force,
            },
        ),
        Command::Verify {
            original,
            patched,
            mapping,
        } => do_verify(&original, &patched, mapping.as_deref()),
        Command::Preview {
            original,
            patched,
            output,
            limit,
        } => do_preview(&original, &patched, &output, limit),
    }
}
