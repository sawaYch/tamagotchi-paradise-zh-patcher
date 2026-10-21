#![cfg_attr(
    all(not(debug_assertions), target_os = "windows"),
    windows_subsystem = "windows"
)]

mod platform;

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::mpsc::{channel, Receiver, Sender};
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
use platform::{show_text, Picked, Slot};

const PREVIEW_LIMIT: usize = 60;
const PREVIEW_WIDTH: usize = 480;
#[cfg(not(target_arch = "wasm32"))]
const WINDOW_WIDTH: f32 = 1100.0;

#[cfg(not(target_arch = "wasm32"))]
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

#[cfg_attr(target_arch = "wasm32", allow(dead_code))]
enum Msg {
    Log(String),
    Done(Result<Box<Finished>, String>),
}

struct Finished {
    report: PatchReport,
    preview: Option<Image>,
    written: Option<String>,
}

struct LoadedRom {
    name: String,
    path: Option<PathBuf>,
    bytes: Arc<Vec<u8>>,
    info: RomInfo,
}

struct MappingFile {
    name: String,
    text: String,
}

struct Job {
    bytes: Arc<Vec<u8>>,
    input_path: Option<PathBuf>,
    mapping_text: Option<String>,
    variant: Variant,
    font: FontChoice,
    runtime_language: bool,
    redraw_shared: bool,
    dry_run: bool,
    output: String,
}

struct App {
    loaded: Option<LoadedRom>,
    load_error: Option<String>,
    output: String,
    variant: Variant,
    font: FontChoice,
    mapping: Option<MappingFile>,
    runtime_language: bool,
    redraw_shared: bool,
    dry_run: bool,
    log: Vec<String>,
    rx: Option<Receiver<Msg>>,
    pending: Option<Job>,
    pending_frames: u8,
    pick_tx: Sender<Picked>,
    pick_rx: Receiver<Picked>,
    report: Option<PatchReport>,
    written: Option<String>,
    preview: Option<egui::TextureHandle>,
    error: Option<String>,
}

#[cfg(not(target_arch = "wasm32"))]
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

fn default_output(name: &str, dir: Option<&std::path::Path>) -> String {
    let stem = std::path::Path::new(name)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("rom");
    let file = format!("{stem}_zh-hk.bin");
    match dir {
        Some(d) => d.join(file).display().to_string(),
        None => file,
    }
}

fn run_job(job: &Job, log: &mut dyn FnMut(String)) -> Result<Finished> {
    if !job.dry_run {
        if job.output.trim().is_empty() {
            anyhow::bail!("choose an output file");
        }
        if let Some(input) = &job.input_path {
            if std::path::Path::new(job.output.trim()) == input.as_path() {
                anyhow::bail!("the output file must differ from the input ROM");
            }
        }
    }
    let rom = Rom::load(job.bytes.to_vec())?;
    let mapping = match &job.mapping_text {
        Some(text) => Some(Mapping::parse(text.trim_start_matches('\u{feff}'))?),
        None => None,
    };
    let opts = PatchOptions {
        variant: job.variant,
        font: job.font,
        runtime_language: job.runtime_language,
        redraw_shared: job.redraw_shared,
        mapping,
    };
    let result = patch(&rom, &opts, &mut |m| log(m.to_string()))?;
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
    let written = if job.dry_run {
        None
    } else {
        Some(platform::save(job.output.trim(), &result.bytes)?)
    };
    Ok(Finished {
        report: result.report,
        preview: Some(preview),
        written,
    })
}

impl App {
    fn new() -> App {
        let (pick_tx, pick_rx) = channel();
        App {
            loaded: None,
            load_error: None,
            output: String::new(),
            variant: Variant::HongKong,
            font: FontChoice::default(),
            mapping: None,
            runtime_language: true,
            redraw_shared: true,
            dry_run: false,
            log: Vec::new(),
            rx: None,
            pending: None,
            pending_frames: 0,
            pick_tx,
            pick_rx,
            report: None,
            written: None,
            preview: None,
            error: None,
        }
    }

    fn busy(&self) -> bool {
        self.rx.is_some() || self.pending.is_some()
    }

    fn pick(&self, ctx: &egui::Context, slot: Slot) {
        let (label, exts): (&str, &[&str]) = match slot {
            Slot::Rom => ("ROM", &["bin", "rom"]),
            Slot::Mapping => ("JSON", &["json"]),
        };
        platform::pick(ctx, slot, label, exts, &self.pick_tx);
    }

    fn accept(&mut self, picked: Picked) {
        match picked.slot {
            Slot::Rom => self.load(picked.name, picked.path, picked.bytes),
            Slot::Mapping => {
                let result = picked.bytes.and_then(|b| {
                    let text = String::from_utf8(b).map_err(|e| e.to_string())?;
                    Mapping::parse(text.trim_start_matches('\u{feff}'))
                        .map_err(|e| format!("{e}"))?;
                    Ok(text)
                });
                match result {
                    Ok(text) => {
                        self.mapping = Some(MappingFile {
                            name: picked.name,
                            text,
                        });
                        self.error = None;
                    }
                    Err(e) => self.error = Some(format!("mapping file: {e}")),
                }
            }
        }
    }

    fn load(&mut self, name: String, path: Option<PathBuf>, bytes: Result<Vec<u8>, String>) {
        self.report = None;
        self.preview = None;
        self.written = None;
        self.error = None;
        self.log.clear();
        let result = bytes.map_err(anyhow::Error::msg).and_then(|b| {
            Rom::load(b.clone())
                .map(|r| (b, r))
                .map_err(anyhow::Error::from)
        });
        match result {
            Ok((bytes, rom)) => {
                self.output = default_output(&name, path.as_deref().and_then(|p| p.parent()));
                self.loaded = Some(LoadedRom {
                    info: rom.info().clone(),
                    name,
                    path,
                    bytes: Arc::new(bytes),
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
        let job = Job {
            bytes: loaded.bytes.clone(),
            input_path: loaded.path.clone(),
            mapping_text: self.mapping.as_ref().map(|m| m.text.clone()),
            variant: self.variant,
            font: self.font,
            runtime_language: self.runtime_language,
            redraw_shared: self.redraw_shared,
            dry_run: self.dry_run,
            output: self.output.clone(),
        };
        self.report = None;
        self.preview = None;
        self.written = None;
        self.error = None;
        self.log.clear();
        self.dispatch(ctx, job);
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn dispatch(&mut self, ctx: &egui::Context, job: Job) {
        let (tx, rx) = channel();
        self.rx = Some(rx);
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let send = |m: Msg| {
                let _ = tx.send(m);
                ctx.request_repaint();
            };
            let res = run_job(&job, &mut |m| send(Msg::Log(m)));
            send(Msg::Done(res.map(Box::new).map_err(|e| format!("{e:#}"))));
        });
    }

    #[cfg(target_arch = "wasm32")]
    fn dispatch(&mut self, ctx: &egui::Context, job: Job) {
        self.pending = Some(job);
        self.pending_frames = 0;
        ctx.request_repaint();
    }

    /// The browser has no threads here, so the job runs on the UI thread one frame
    /// after the click, letting the "working" state paint first.
    fn run_pending(&mut self, ctx: &egui::Context) {
        if self.pending.is_none() {
            return;
        }
        if self.pending_frames < 2 {
            self.pending_frames += 1;
            ctx.request_repaint();
            return;
        }
        let Some(job) = self.pending.take() else {
            return;
        };
        let mut logs = Vec::new();
        let res = run_job(&job, &mut |m| logs.push(m));
        self.log.extend(logs);
        self.finish(ctx, res.map(Box::new).map_err(|e| format!("{e:#}")));
    }

    fn export_mapping(&mut self, target: &str) -> Result<()> {
        let loaded = self.loaded.as_ref().context("no ROM loaded")?;
        let rom = Rom::load(loaded.bytes.to_vec())?;
        let cs = Charset::standard();
        let table = rom.table(rom.info().zh_table)?;
        let conv = Converter::with_mapping(self.variant, Mapping::default(), &cs)?;
        let text = export_template(&table, &cs, &conv);
        let saved = platform::save(target, text.as_bytes())?;
        self.log.push(format!("Exported strings to {saved}"));
        self.mapping = Some(MappingFile { name: saved, text });
        Ok(())
    }

    fn finish(&mut self, ctx: &egui::Context, res: Result<Box<Finished>, String>) {
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
                    self.preview =
                        Some(ctx.load_texture("preview", ci, egui::TextureOptions::NEAREST));
                }
            }
            Err(e) => self.error = Some(e),
        }
    }

    fn poll(&mut self, ctx: &egui::Context) {
        while let Ok(picked) = self.pick_rx.try_recv() {
            self.accept(picked);
        }
        self.run_pending(ctx);
        let Some(rx) = &self.rx else { return };
        let mut done = None;
        while let Ok(msg) = rx.try_recv() {
            match msg {
                Msg::Log(m) => self.log.push(m),
                Msg::Done(res) => done = Some(res),
            }
        }
        if let Some(res) = done {
            self.rx = None;
            self.finish(ctx, res);
        }
    }

    fn rom_section(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        ui.heading("1. Your ROM");
        ui.label(if platform::IS_WEB {
            "Use your own Tamagotchi Paradise \"Glacier\" ROM (.bin). Drop a file here or browse. The ROM is processed inside your browser and is never uploaded."
        } else {
            "Use your own Tamagotchi Paradise \"Glacier\" ROM (.bin). Drop a file here or browse. The original file is never modified."
        });
        ui.horizontal_wrapped(|ui| {
            if ui.button("Browse...").clicked() {
                self.pick(ctx, Slot::Rom);
            }
            if let Some(l) = &self.loaded {
                let shown = l
                    .path
                    .as_ref()
                    .map(|p| p.display().to_string())
                    .unwrap_or_else(|| l.name.clone());
                ui.monospace(shown);
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
        let busy = self.busy();
        egui::Grid::new("opts").num_columns(2).show(ui, |ui| {
            ui.label("Target");
            egui::ComboBox::from_id_salt("variant")
                .selected_text(self.variant.label())
                .show_ui(ui, |ui| {
                    for v in [Variant::HongKong, Variant::Taiwan, Variant::Standard] {
                        ui.selectable_value(&mut self.variant, v, v.label());
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
                if ui.button("Browse...").clicked() {
                    self.pick(ctx, Slot::Mapping);
                }
                match &self.mapping {
                    Some(m) => {
                        ui.monospace(&m.name);
                        if ui.button("Clear").clicked() {
                            self.mapping = None;
                        }
                    }
                    None => {
                        ui.weak("none (optional)");
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
                    if let Some(target) =
                        platform::ask_save_target("mapping.json", "JSON", &["json"])
                    {
                        match self.export_mapping(&target) {
                            Ok(()) => self.error = None,
                            Err(e) => self.error = Some(format!("{e:#}")),
                        }
                    }
                }
            });
            ui.end_row();
            ui.label(if platform::IS_WEB {
                "Download file name"
            } else {
                "Output file"
            });
            ui.horizontal_wrapped(|ui| {
                ui.add(egui::TextEdit::singleline(&mut self.output).desired_width(380.0));
                if platform::CAN_BROWSE_OUTPUT && ui.button("Browse...").clicked() {
                    if let Some(p) = platform::ask_save_target(&self.output, "ROM", &["bin"]) {
                        self.output = p;
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
            &mut self.redraw_shared,
            "Redraw characters shared with Simplified (定, 哦) in the chosen font so every glyph matches",
        );
        ui.checkbox(
            &mut self.dry_run,
            "Dry run (preview and report only, do not write a ROM)",
        );
        ui.horizontal_wrapped(|ui| {
            let can_run = self.loaded.is_some() && !busy;
            let label = match (self.dry_run, platform::IS_WEB) {
                (true, _) => "Run dry run",
                (false, true) => "Create and download patched ROM",
                (false, false) => "Create patched ROM",
            };
            if ui.add_enabled(can_run, egui::Button::new(label)).clicked() {
                self.start(ctx);
            }
            if busy {
                ui.spinner();
                if self.pending.is_some() {
                    ui.label("Working... the page may pause for a few seconds.");
                }
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
            let verb = if platform::IS_WEB {
                "Downloaded"
            } else {
                "Saved"
            };
            ui.colored_label(egui::Color32::LIGHT_GREEN, format!("{verb} {p}"));
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
        let unused: Vec<String> = r
            .mapping_hits
            .iter()
            .filter(|(_, n)| *n == 0)
            .map(|(k, _)| show_text(k))
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
                ui.label(show_text(&format!(
                    "{} -> {} x{} ({})",
                    u.source, u.target, u.count, u.reason
                )));
            }
        }
        if !r.remaining_simplified.is_empty() {
            let list: String = r.remaining_simplified.iter().map(|(c, _)| *c).collect();
            ui.colored_label(
                egui::Color32::YELLOW,
                format!("Simplified characters left: {}", show_text(&list)),
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
        let dropped = ctx.input(|i| i.raw.dropped_files.first().cloned());
        if let Some(file) = dropped {
            if !self.busy() {
                platform::load_dropped(&ctx, file, &self.pick_tx);
            }
        }
        egui::CentralPanel::default().show(ui, |ui| {
            egui::ScrollArea::vertical().id_salt("main").show(ui, |ui| {
                ui.heading("Tamagotchi Paradise: Simplified to Traditional Chinese");
                ui.label("Converts the Simplified Chinese text and font in your own ROM. Keep a backup of the original ROM.");
                ui.separator();
                self.rom_section(ui, &ctx);
                ui.separator();
                self.options_section(ui, &ctx);
                ui.separator();
                self.result_section(ui);
            });
        });
    }
}

#[cfg(not(target_arch = "wasm32"))]
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
            let app = App::new();
            if let Some(p) = std::env::args_os().nth(1) {
                platform::send_path(&cc.egui_ctx, Slot::Rom, PathBuf::from(p), &app.pick_tx);
            }
            Ok(Box::new(app))
        }),
    )
}

#[cfg(target_arch = "wasm32")]
fn main() {
    use wasm_bindgen::JsCast as _;

    wasm_bindgen_futures::spawn_local(async {
        let document = web_sys::window()
            .and_then(|w| w.document())
            .expect("no document");
        let canvas = document
            .get_element_by_id("paradise_canvas")
            .expect("canvas #paradise_canvas not found")
            .dyn_into::<web_sys::HtmlCanvasElement>()
            .expect("#paradise_canvas is not a canvas");
        let started = eframe::WebRunner::new()
            .start(
                canvas,
                eframe::WebOptions::default(),
                Box::new(|_cc| Ok(Box::new(App::new()))),
            )
            .await;
        if let Some(loading) = document.get_element_by_id("loading") {
            match started {
                Ok(()) => loading.remove(),
                Err(e) => loading.set_inner_html(&format!(
                    "The app could not start in this browser: {}",
                    e.as_string().unwrap_or_else(|| "unknown error".into())
                )),
            }
        }
    });
}
