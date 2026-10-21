#![cfg_attr(
    all(not(debug_assertions), target_os = "windows"),
    windows_subsystem = "windows"
)]

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver};
use std::sync::Arc;

use anyhow::{Context as _, Result};
use eframe::egui;
use paradise_core::charset::Charset;
use paradise_core::convert::{Converter, Variant};
use paradise_core::glyphpack::FontChoice;
use paradise_core::mapping::{export_template, Mapping};
use paradise_core::patch::{changed_glyphs, patch, PatchOptions, PatchReport, RuntimeLanguage};
use paradise_core::preview::{affected_ids, render_comparison, Image};
use paradise_core::rom::{Rom, RomInfo};

const PREVIEW_LIMIT: usize = 60;
const PREVIEW_WIDTH: usize = 480;
const WINDOW_WIDTH: f32 = 1100.0;

const CJK_FONT_CANDIDATES: &[&str] = &[
    r"C:\Windows\Fonts\msjh.ttc",
    r"C:\Windows\Fonts\msyh.ttc",
    "/System/Library/Fonts/PingFang.ttc",
    "/System/Library/Fonts/STHeiti Medium.ttc",
    "/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc",
    "/usr/share/fonts/truetype/noto/NotoSansCJK-Regular.ttc",
    "/usr/share/fonts/noto-cjk/NotoSansCJK-Regular.ttc",
    "/usr/share/fonts/google-noto-cjk/NotoSansCJK-Regular.ttc",
];

enum Msg {
    Log(String),
    Done(Result<Box<Finished>, String>),
}

struct Finished {
    report: PatchReport,
    preview: Option<Image>,
    written: Option<PathBuf>,
}

struct LoadedRom {
    path: PathBuf,
    info: RomInfo,
}

struct App {
    loaded: Option<LoadedRom>,
    load_error: Option<String>,
    output: String,
    variant: Variant,
    font: FontChoice,
    mapping_path: String,
    runtime_language: bool,
    dry_run: bool,
    log: Vec<String>,
    rx: Option<Receiver<Msg>>,
    report: Option<PatchReport>,
    written: Option<PathBuf>,
    preview: Option<egui::TextureHandle>,
    error: Option<String>,
}

fn install_cjk_font(ctx: &egui::Context) {
    for path in CJK_FONT_CANDIDATES {
        if let Ok(bytes) = std::fs::read(path) {
            let mut fonts = egui::FontDefinitions::default();
            fonts
                .font_data
                .insert("cjk".into(), Arc::new(egui::FontData::from_owned(bytes)));
            for family in [egui::FontFamily::Proportional, egui::FontFamily::Monospace] {
                if let Some(list) = fonts.families.get_mut(&family) {
                    list.push("cjk".into());
                }
            }
            ctx.set_fonts(fonts);
            return;
        }
    }
}

fn default_output(input: &Path) -> String {
    let stem = input.file_stem().and_then(|s| s.to_str()).unwrap_or("rom");
    input
        .with_file_name(format!("{stem}_zh-hk.bin"))
        .display()
        .to_string()
}

fn variant_label(v: Variant) -> &'static str {
    v.label()
}

impl App {
    fn new() -> App {
        App {
            loaded: None,
            load_error: None,
            output: String::new(),
            variant: Variant::HongKong,
            font: FontChoice::default(),
            mapping_path: String::new(),
            runtime_language: true,
            dry_run: false,
            log: Vec::new(),
            rx: None,
            report: None,
            written: None,
            preview: None,
            error: None,
        }
    }

    fn load(&mut self, path: PathBuf) {
        self.report = None;
        self.preview = None;
        self.written = None;
        self.error = None;
        self.log.clear();
        let result = std::fs::read(&path)
            .with_context(|| format!("reading {}", path.display()))
            .and_then(|b| Rom::load(b).map_err(anyhow::Error::from));
        match result {
            Ok(rom) => {
                self.output = default_output(&path);
                self.loaded = Some(LoadedRom {
                    info: rom.info().clone(),
                    path,
                });
                self.load_error = None;
            }
            Err(e) => {
                self.loaded = None;
                self.load_error = Some(format!("{e:#}"));
            }
        }
    }

    fn start(&mut self, ctx: &egui::Context) {
        let Some(loaded) = &self.loaded else { return };
        let input = loaded.path.clone();
        let output = PathBuf::from(self.output.trim());
        let dry_run = self.dry_run;
        let variant = self.variant;
        let font = self.font;
        let mapping_path = self.mapping_path.trim().to_string();
        let runtime_language = self.runtime_language;
        let (tx, rx) = channel();
        self.rx = Some(rx);
        self.report = None;
        self.preview = None;
        self.written = None;
        self.error = None;
        self.log.clear();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let send = |m: Msg| {
                let _ = tx.send(m);
                ctx.request_repaint();
            };
            let run = || -> Result<Finished> {
                if !dry_run {
                    if output == input {
                        anyhow::bail!("the output file must differ from the input ROM");
                    }
                    if output.as_os_str().is_empty() {
                        anyhow::bail!("choose an output file");
                    }
                }
                let bytes = std::fs::read(&input)?;
                let rom = Rom::load(bytes)?;
                let mapping = if mapping_path.is_empty() {
                    None
                } else {
                    let text = std::fs::read_to_string(&mapping_path)
                        .with_context(|| format!("reading {mapping_path}"))?;
                    Some(Mapping::parse(text.trim_start_matches('\u{feff}'))?)
                };
                let opts = PatchOptions {
                    variant,
                    font,
                    runtime_language,
                    mapping,
                };
                let result = patch(&rom, &opts, &mut |m| send(Msg::Log(m.to_string())))?;
                let after = Rom::load(result.bytes.clone())?;
                let changed: HashSet<usize> = changed_glyphs(&rom, &after)?.into_iter().collect();
                let (ta, tb) = (
                    rom.table(rom.info().zh_table)?,
                    after.table(after.info().zh_table)?,
                );
                let mut ids = affected_ids(&ta, &tb, &changed);
                ids.truncate(PREVIEW_LIMIT);
                let preview = render_comparison(
                    (&rom.font()?, &ta),
                    (&after.font()?, &tb),
                    &ids,
                    PREVIEW_WIDTH,
                );
                let mut written = None;
                if !dry_run {
                    std::fs::write(&output, &result.bytes)
                        .with_context(|| format!("writing {}", output.display()))?;
                    written = Some(output.clone());
                }
                Ok(Finished {
                    report: result.report,
                    preview: Some(preview),
                    written,
                })
            };
            send(Msg::Done(run().map(Box::new).map_err(|e| format!("{e:#}"))));
        });
    }

    fn export_mapping(&mut self, path: &Path) -> Result<()> {
        let loaded = self.loaded.as_ref().context("no ROM loaded")?;
        let bytes = std::fs::read(&loaded.path)
            .with_context(|| format!("reading {}", loaded.path.display()))?;
        let rom = Rom::load(bytes)?;
        let cs = Charset::standard();
        let table = rom.table(rom.info().zh_table)?;
        let conv = Converter::with_mapping(self.variant, Mapping::default(), &cs)?;
        std::fs::write(path, export_template(&table, &cs, &conv))
            .with_context(|| format!("writing {}", path.display()))?;
        Ok(())
    }

    fn poll(&mut self, ctx: &egui::Context) {
        let Some(rx) = &self.rx else { return };
        let mut finished = false;
        while let Ok(msg) = rx.try_recv() {
            match msg {
                Msg::Log(m) => self.log.push(m),
                Msg::Done(res) => {
                    finished = true;
                    match res {
                        Ok(f) => {
                            self.log.push(if f.written.is_some() {
                                "Done".into()
                            } else {
                                "Dry run finished".into()
                            });
                            self.report = Some(f.report);
                            self.written = f.written;
                            if let Some(img) = f.preview {
                                let ci = egui::ColorImage::from_rgba_unmultiplied(
                                    [img.width, img.height],
                                    &img.rgba,
                                );
                                self.preview = Some(ctx.load_texture(
                                    "preview",
                                    ci,
                                    egui::TextureOptions::NEAREST,
                                ));
                            }
                        }
                        Err(e) => self.error = Some(e),
                    }
                }
            }
        }
        if finished {
            self.rx = None;
        }
    }

    fn rom_section(&mut self, ui: &mut egui::Ui) {
        ui.heading("1. Your ROM");
        ui.label("Use your own Tamagotchi Paradise \"Glacier\" ROM (.bin). Drop a file here or browse. The original file is never modified.");
        ui.horizontal_wrapped(|ui| {
            if ui.button("Browse...").clicked() {
                if let Some(p) = rfd::FileDialog::new()
                    .add_filter("ROM", &["bin", "rom"])
                    .pick_file()
                {
                    self.load(p);
                }
            }
            if let Some(l) = &self.loaded {
                ui.monospace(l.path.display().to_string());
            }
        });
        if let Some(e) = &self.load_error {
            ui.colored_label(
                egui::Color32::LIGHT_RED,
                format!("Cannot use this file: {e}"),
            );
        }
        if let Some(l) = &self.loaded {
            let i = &l.info;
            egui::Grid::new("info").num_columns(2).show(ui, |ui| {
                ui.label("Size");
                ui.label(format!("{} bytes", i.rom_len));
                ui.end_row();
                ui.label("Signature");
                ui.label(if i.signature_ok {
                    "Sonix header found"
                } else {
                    "Sonix header missing (continuing anyway)"
                });
                ui.end_row();
                ui.label("Archive checksums");
                ui.label(if i.top_checksum_ok && i.strings_checksum_ok {
                    "valid"
                } else {
                    "mismatch"
                });
                ui.end_row();
                ui.label("CJK font");
                ui.label(format!(
                    "sprite #{} ({} glyphs)",
                    i.font_sprite, i.font_glyphs
                ));
                ui.end_row();
                ui.label("Chinese text");
                ui.label(format!(
                    "table #{}: {} strings, {} distinct hanzi",
                    i.zh_table, i.zh_string_count, i.zh_hanzi_distinct
                ));
                ui.end_row();
            });
        }
    }

    fn options_section(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        ui.heading("2. Convert");
        let busy = self.rx.is_some();
        egui::Grid::new("opts").num_columns(2).show(ui, |ui| {
            ui.label("Target");
            egui::ComboBox::from_id_salt("variant")
                .selected_text(variant_label(self.variant))
                .show_ui(ui, |ui| {
                    for v in [Variant::HongKong, Variant::Taiwan, Variant::Standard] {
                        ui.selectable_value(&mut self.variant, v, variant_label(v));
                    }
                });
            ui.end_row();
            ui.label("Glyph font");
            ui.horizontal_wrapped(|ui| {
                egui::ComboBox::from_id_salt("font")
                    .selected_text(self.font.label())
                    .show_ui(ui, |ui| {
                        for f in FontChoice::ALL {
                            ui.selectable_value(&mut self.font, f, f.label());
                        }
                    });
                ui.hyperlink_to("project page", self.font.url());
            });
            ui.end_row();
            ui.label("Text mapping (JSON)");
            ui.horizontal_wrapped(|ui| {
                ui.add(
                    egui::TextEdit::singleline(&mut self.mapping_path)
                        .hint_text("optional, e.g. { \"^信息$\": \"資訊\"}")
                        .desired_width(380.0),
                );
                if ui.button("Browse...").clicked() {
                    if let Some(p) = rfd::FileDialog::new()
                        .add_filter("JSON", &["json"])
                        .pick_file()
                    {
                        self.mapping_path = p.display().to_string();
                    }
                }
                let can_export = self.loaded.is_some() && !busy;
                if ui
                    .add_enabled(can_export, egui::Button::new("Export all strings..."))
                    .on_hover_text(
                        "Save every string with its converted text as an editable mapping file",
                    )
                    .clicked()
                {
                    if let Some(p) = rfd::FileDialog::new()
                        .add_filter("JSON", &["json"])
                        .set_file_name("mapping.json")
                        .save_file()
                    {
                        match self.export_mapping(&p) {
                            Ok(()) => {
                                self.log
                                    .push(format!("Exported strings to {}", p.display()));
                                self.mapping_path = p.display().to_string();
                                self.error = None;
                            }
                            Err(e) => self.error = Some(format!("{e:#}")),
                        }
                    }
                }
            });
            ui.end_row();
            ui.label("Output file");
            ui.horizontal_wrapped(|ui| {
                ui.add(egui::TextEdit::singleline(&mut self.output).desired_width(380.0));
                if ui.button("Browse...").clicked() {
                    let mut d = rfd::FileDialog::new().add_filter("ROM", &["bin"]);
                    if let Some(name) = Path::new(&self.output).file_name().and_then(|n| n.to_str())
                    {
                        d = d.set_file_name(name);
                    }
                    if let Some(p) = d.save_file() {
                        self.output = p.display().to_string();
                    }
                }
            });
            ui.end_row();
        });
        ui.checkbox(
            &mut self.runtime_language,
            "Add a Language entry to Settings (4th item) so the language can be changed any time (experimental)",
        );
        ui.checkbox(
            &mut self.dry_run,
            "Dry run (preview and report only, do not write a ROM)",
        );
        ui.horizontal_wrapped(|ui| {
            let can_run = self.loaded.is_some() && !busy;
            if ui
                .add_enabled(
                    can_run,
                    egui::Button::new(if self.dry_run {
                        "Run dry run"
                    } else {
                        "Create patched ROM"
                    }),
                )
                .clicked()
            {
                self.start(ctx);
            }
            if busy {
                ui.spinner();
            }
        });
        if !self.log.is_empty() {
            egui::ScrollArea::vertical()
                .id_salt("log")
                .max_height(110.0)
                .stick_to_bottom(true)
                .show(ui, |ui| {
                    for l in &self.log {
                        ui.monospace(l);
                    }
                });
        }
        if let Some(e) = &self.error {
            ui.colored_label(egui::Color32::LIGHT_RED, format!("Failed: {e}"));
        }
    }

    fn result_section(&self, ui: &mut egui::Ui) {
        let Some(r) = &self.report else { return };
        ui.heading("3. Result");
        ui.columns(2, |cols| {
            self.result_details(&mut cols[0], r);
            self.preview_panel(&mut cols[1]);
        });
    }

    fn result_details(&self, ui: &mut egui::Ui, r: &PatchReport) {
        if let Some(p) = &self.written {
            ui.colored_label(egui::Color32::LIGHT_GREEN, format!("Saved {}", p.display()));
        } else {
            ui.label("Dry run: nothing was written.");
        }
        egui::Grid::new("stats").num_columns(2).show(ui, |ui| {
            let s = &r.stats;
            ui.label("Strings changed");
            ui.label(format!("{} of {}", s.strings_changed, s.strings_total));
            ui.end_row();
            ui.label("Characters converted");
            ui.label(s.chars_converted.to_string());
            ui.end_row();
            ui.label("Glyphs redrawn in place");
            ui.label(s.reskinned.to_string());
            ui.end_row();
            ui.label("Glyphs in spare slots");
            ui.label(s.overflow.to_string());
            ui.end_row();
            ui.label("Asset size");
            ui.label(format!(
                "{} -> {} bytes",
                r.asset_len_before, r.asset_len_after
            ));
            ui.end_row();
            if r.fallback_glyphs > 0 {
                ui.label("Glyphs from Fusion Pixel");
                ui.add(
                    egui::Label::new(format!(
                        "{} (missing in the chosen font)",
                        r.fallback_glyphs
                    ))
                    .wrap(),
                );
                ui.end_row();
            }
            if !r.mapping_hits.is_empty() {
                ui.label("Mapping entries used");
                let used = r.mapping_hits.iter().filter(|(_, n)| *n > 0).count();
                ui.label(format!("{used} of {}", r.mapping_hits.len()));
                ui.end_row();
            }
            ui.label("Settings > Language");
            ui.add(
                egui::Label::new(match r.runtime_language {
                    RuntimeLanguage::Disabled => "not requested",
                    RuntimeLanguage::Added => "added",
                    RuntimeLanguage::AlreadyPresent => "already present",
                    RuntimeLanguage::Unsupported => {
                        "skipped (firmware does not match the known build)"
                    }
                })
                .wrap(),
            );
            ui.end_row();
        });
        let unused: Vec<&str> = r
            .mapping_hits
            .iter()
            .filter(|(_, n)| *n == 0)
            .map(|(k, _)| k.as_str())
            .collect();
        if !unused.is_empty() {
            ui.colored_label(
                egui::Color32::YELLOW,
                format!("Mapping keys that matched nothing: {}", unused.join(", ")),
            );
        }
        if r.unresolved.is_empty() && r.remaining_simplified.is_empty() {
            ui.label("Every Simplified character was converted.");
        }
        if !r.unresolved.is_empty() {
            ui.colored_label(egui::Color32::YELLOW, "Could not convert:");
            for u in &r.unresolved {
                ui.label(format!(
                    "{} -> {} x{} ({})",
                    u.source, u.target, u.count, u.reason
                ));
            }
        }
        if !r.remaining_simplified.is_empty() {
            let list: String = r.remaining_simplified.iter().map(|(c, _)| *c).collect();
            ui.colored_label(
                egui::Color32::YELLOW,
                format!("Simplified characters left: {list}"),
            );
        }
    }

    fn preview_panel(&self, ui: &mut egui::Ui) {
        let Some(tex) = &self.preview else {
            ui.label("No preview: no string changed.");
            return;
        };
        ui.label("Preview: original (top line) and converted (bottom line) for changed strings");
        let native = tex.size_vec2();
        let scale = (ui.available_width() / native.x).min(1.0);
        egui::ScrollArea::vertical()
            .id_salt("preview")
            .max_height(600.0)
            .auto_shrink([false, true])
            .show(ui, |ui| {
                ui.add(egui::Image::new(tex).fit_to_exact_size(native * scale));
            });
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.poll(&ctx);
        let dropped = ctx.input(|i| i.raw.dropped_files.first().map(|f| f.path().to_path_buf()));
        if let Some(p) = dropped {
            if self.rx.is_none() {
                self.load(p);
            }
        }
        egui::CentralPanel::default().show(ui, |ui| {
            egui::ScrollArea::vertical().id_salt("main").show(ui, |ui| {
                ui.heading("Tamagotchi Paradise: Simplified to Traditional Chinese");
                ui.label("Converts the Simplified Chinese text and font in your own ROM. Keep a backup of the original ROM.");
                ui.separator();
                self.rom_section(ui);
                ui.separator();
                self.options_section(ui, &ctx);
                ui.separator();
                self.result_section(ui);
            });
        });
    }
}

fn main() -> eframe::Result {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([WINDOW_WIDTH, 900.0])
            .with_min_inner_size([WINDOW_WIDTH, 480.0])
            .with_max_inner_size([WINDOW_WIDTH, 4000.0])
            .with_drag_and_drop(true),
        ..Default::default()
    };
    eframe::run_native(
        "Paradise ZH Patcher",
        options,
        Box::new(|cc| {
            install_cjk_font(&cc.egui_ctx);
            let mut app = App::new();
            if let Some(p) = std::env::args_os().nth(1) {
                app.load(PathBuf::from(p));
            }
            Ok(Box::new(app))
        }),
    )
}
