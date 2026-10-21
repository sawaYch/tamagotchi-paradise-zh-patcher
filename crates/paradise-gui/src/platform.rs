use std::sync::mpsc::Sender;

use anyhow::Result;
use eframe::egui;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Slot {
    Rom,
    Mapping,
}

pub struct Picked {
    pub slot: Slot,
    pub name: String,
    pub path: Option<std::path::PathBuf>,
    pub bytes: Result<Vec<u8>, String>,
}

pub const CAN_BROWSE_OUTPUT: bool = cfg!(not(target_arch = "wasm32"));
pub const IS_WEB: bool = cfg!(target_arch = "wasm32");

#[cfg(not(target_arch = "wasm32"))]
mod imp {
    use super::*;
    use std::path::Path;

    pub fn pick(ctx: &egui::Context, slot: Slot, label: &str, exts: &[&str], tx: &Sender<Picked>) {
        let Some(path) = rfd::FileDialog::new().add_filter(label, exts).pick_file() else {
            return;
        };
        send_path(ctx, slot, path, tx);
    }

    pub fn send_path(
        ctx: &egui::Context,
        slot: Slot,
        path: std::path::PathBuf,
        tx: &Sender<Picked>,
    ) {
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("file")
            .to_string();
        let bytes = std::fs::read(&path).map_err(|e| format!("reading {}: {e}", path.display()));
        let _ = tx.send(Picked {
            slot,
            name,
            path: Some(path),
            bytes,
        });
        ctx.request_repaint();
    }

    pub fn load_dropped(ctx: &egui::Context, file: egui::DroppedFileHandle, tx: &Sender<Picked>) {
        send_path(ctx, Slot::Rom, file.path().to_path_buf(), tx);
    }

    pub fn ask_save_target(default_name: &str, label: &str, exts: &[&str]) -> Option<String> {
        let mut d = rfd::FileDialog::new().add_filter(label, exts);
        if let Some(name) = Path::new(default_name).file_name().and_then(|n| n.to_str()) {
            d = d.set_file_name(name);
        }
        d.save_file().map(|p| p.display().to_string())
    }

    pub fn save(target: &str, bytes: &[u8]) -> Result<String> {
        std::fs::write(target, bytes).map_err(|e| anyhow::anyhow!("writing {target}: {e}"))?;
        Ok(target.to_string())
    }
}

#[cfg(target_arch = "wasm32")]
mod imp {
    use super::*;
    use anyhow::{anyhow, Context as _};
    use wasm_bindgen::{closure::Closure, JsCast, JsValue};

    pub fn pick(ctx: &egui::Context, slot: Slot, _label: &str, exts: &[&str], tx: &Sender<Picked>) {
        let Some(doc) = web_sys::window().and_then(|w| w.document()) else {
            return;
        };
        let Some(input) = doc
            .create_element("input")
            .ok()
            .and_then(|e| e.dyn_into::<web_sys::HtmlInputElement>().ok())
        else {
            return;
        };
        input.set_type("file");
        input.set_hidden(true);
        let accept: Vec<String> = exts.iter().map(|e| format!(".{e}")).collect();
        input.set_accept(&accept.join(","));
        if let Some(body) = doc.body() {
            let _ = body.append_child(&input);
        }
        let ctx = ctx.clone();
        let tx = tx.clone();
        let element = input.clone();
        let on_change = Closure::once(move || {
            let file = element.files().and_then(|list| list.get(0));
            element.remove();
            let Some(file) = file else { return };
            wasm_bindgen_futures::spawn_local(async move {
                let bytes = wasm_bindgen_futures::JsFuture::from(file.array_buffer())
                    .await
                    .map(|buf| js_sys::Uint8Array::new(&buf).to_vec())
                    .map_err(|e| format!("reading {}: {e:?}", file.name()));
                let _ = tx.send(Picked {
                    slot,
                    name: file.name(),
                    path: None,
                    bytes,
                });
                ctx.request_repaint();
            });
        });
        input.set_onchange(Some(on_change.as_ref().unchecked_ref()));
        on_change.forget();
        input.click();
    }

    pub fn load_dropped(ctx: &egui::Context, file: egui::DroppedFileHandle, tx: &Sender<Picked>) {
        let ctx = ctx.clone();
        let tx = tx.clone();
        wasm_bindgen_futures::spawn_local(async move {
            let name = file
                .path()
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("file")
                .to_string();
            let bytes = file.bytes_async().await;
            let _ = tx.send(Picked {
                slot: Slot::Rom,
                name,
                path: None,
                bytes,
            });
            ctx.request_repaint();
        });
    }

    pub fn ask_save_target(default_name: &str, _label: &str, _exts: &[&str]) -> Option<String> {
        Some(default_name.to_string())
    }

    pub fn save(target: &str, bytes: &[u8]) -> Result<String> {
        let js = |e: JsValue| anyhow!("{e:?}");
        let parts = js_sys::Array::of1(&js_sys::Uint8Array::from(bytes));
        let opts = web_sys::BlobPropertyBag::new();
        opts.set_type("application/octet-stream");
        let blob =
            web_sys::Blob::new_with_u8_array_sequence_and_options(&parts, &opts).map_err(js)?;
        let url = web_sys::Url::create_object_url_with_blob(&blob).map_err(js)?;
        let doc = web_sys::window()
            .and_then(|w| w.document())
            .context("no document")?;
        let anchor: web_sys::HtmlAnchorElement = doc
            .create_element("a")
            .map_err(js)?
            .dyn_into()
            .map_err(|_| anyhow!("cannot create a download link"))?;
        anchor.set_href(&url);
        anchor.set_download(target);
        anchor.click();
        web_sys::Url::revoke_object_url(&url).map_err(js)?;
        Ok(target.to_string())
    }
}

pub use imp::{ask_save_target, load_dropped, pick, save};

#[cfg(not(target_arch = "wasm32"))]
pub use imp::send_path;

/// Text with characters the web UI cannot draw replaced by their code points.
pub fn show_text(s: &str) -> String {
    if !IS_WEB {
        return s.to_string();
    }
    let mut out = String::new();
    for c in s.chars() {
        if c.is_ascii() {
            out.push(c);
        } else {
            out.push_str(&format!("U+{:04X} ", c as u32));
        }
    }
    out
}
