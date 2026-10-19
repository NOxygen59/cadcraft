//! The menu bar's resource readout (iPad): the memory and CPU CADCraft itself uses, the battery
//! level, and a 60 / 120 Hz switch, at the right of the menu bar. Shown only when the platform
//! provides the readings. They are cached and refreshed every two seconds, so the readout costs
//! one small redraw per period and nothing between.

use std::time::{Duration, Instant};

use egui::{CornerRadius, Ui};

use crate::theme::Tokens;
use crate::{CadApp, ResourceReading};

/// How often the readout is refreshed.
const PERIOD: Duration = Duration::from_secs(2);

/// Draw the readout in a right-to-left layout (the switch outermost).
pub fn show(app: &CadApp, ui: &mut Ui) {
    let Some(read) = app.services.resources else { return };
    let t = Tokens::get();
    let ctx = ui.ctx().clone();
    let cache_id = egui::Id::new("cadcraft.resource_readout");
    let fps_id = egui::Id::new("cadcraft.max_frame_rate");
    let now = Instant::now();
    let cached: Option<(Instant, ResourceReading)> = ctx.data(|d| d.get_temp(cache_id));
    let reading = match cached {
        Some((at, r)) if now.duration_since(at) < PERIOD => r,
        _ => {
            let r = read();
            ctx.data_mut(|d| d.insert_temp(cache_id, (now, r)));
            r
        }
    };
    ctx.request_repaint_after(PERIOD);

    // The frame-rate cap, remembered between launches by the host; applied once at startup.
    if let Some(set) = app.services.set_max_frame_rate {
        let mut fps: u32 = match ctx.data(|d| d.get_temp(fps_id)) {
            Some(fps) => fps,
            None => {
                let fps = app.services.saved_max_frame_rate.map_or(60, |saved| saved());
                set(fps);
                ctx.data_mut(|d| d.insert_temp(fps_id, fps));
                fps
            }
        };
        if ui.available_width() < 70.0 {
            return;
        }
        let label = egui::RichText::new(format!("{fps} Hz")).color(if fps > 60 { t.accent } else { t.text_dim }).size(11.0).monospace();
        let tip = crate::tl!("Maximum frame rate: 60 Hz saves battery and heat, 120 Hz is smoother. Tap to switch.");
        let button = egui::Button::new(label).fill(t.control).corner_radius(CornerRadius::same(4)).min_size(egui::vec2(48.0, 18.0));
        if ui.add(button).on_hover_text(tip).clicked() {
            fps = if fps > 60 { 60 } else { 120 };
            set(fps);
            ctx.data_mut(|d| d.insert_temp(fps_id, fps));
        }
    }

    let decimal = |v: f64| {
        let s = format!("{v:.1}");
        // Languages that write 2,5 rather than 2.5.
        let comma = matches!(crate::i18n::current(), "fr" | "de" | "es" | "it" | "pt-br" | "ru" | "uk" | "cs" | "sv" | "el" | "hu" | "tr");
        if comma { s.replace('.', ",") } else { s }
    };
    let mib = reading.memory_bytes as f64 / (1024.0 * 1024.0);
    let memory = if mib >= 1024.0 {
        crate::tl!("{n} GB").replace("{n}", &decimal(mib / 1024.0))
    } else {
        crate::tl!("{n} MB").replace("{n}", &format!("{mib:.0}"))
    };
    let base = format!("{} {memory}  ·  {} {:.0} %", crate::tl!("RAM"), crate::tl!("CPU"), reading.cpu_percent);
    let mut full = base.clone();
    if let Some(level) = reading.battery_percent {
        full.push_str(&format!("  ·  {} {level:.0} %", crate::tl!("Battery")));
        if reading.charging {
            full.push_str(" +");
        }
    }
    // A narrow window (Stage Manager, Aerial) drops the battery, then the whole readout.
    let font = egui::FontId::monospace(11.0);
    let width = |s: &str| ui.painter().layout_no_wrap(s.to_string(), font.clone(), t.text_dim).size().x + 8.0;
    let room = ui.available_width() - 24.0;
    let text = if width(&full) <= room {
        full
    } else if width(&base) <= room {
        base
    } else {
        return;
    };
    let tip = crate::tl!("Memory and processor used by CADCraft itself, and the battery level. Updated every 2 seconds.");
    ui.add_space(8.0);
    ui.add(egui::Label::new(egui::RichText::new(text).color(t.text_dim).size(11.0).monospace()).selectable(false)).on_hover_text(tip);
}
