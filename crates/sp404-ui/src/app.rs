//! Pad grid, chain editor, and the small amount of UI state around the engine.

use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::sync::MutexGuard;

use eframe::egui::{
    self, Align2, Color32, CornerRadius, FontId, Pos2, RichText, Sense, Stroke, StrokeKind,
    TextStyle, Vec2, Visuals,
};
use sp404_core::{
    chain_presets, decode_wav, effect_schema, effect_schemas, encode_wav, render, ChainDesc,
    Engine, PadView, ParamScale, ParamSchema, PAD_COUNT,
};

use crate::audio::{self, Session};

const KEYS: [(egui::Key, usize); 16] = [
    (egui::Key::Num1, 0),
    (egui::Key::Num2, 1),
    (egui::Key::Num3, 2),
    (egui::Key::Num4, 3),
    (egui::Key::Q, 4),
    (egui::Key::W, 5),
    (egui::Key::E, 6),
    (egui::Key::R, 7),
    (egui::Key::A, 8),
    (egui::Key::S, 9),
    (egui::Key::D, 10),
    (egui::Key::F, 11),
    (egui::Key::Z, 12),
    (egui::Key::X, 13),
    (egui::Key::C, 14),
    (egui::Key::V, 15),
];

const KEY_LABELS: [&str; 16] = [
    "1", "2", "3", "4", "Q", "W", "E", "R", "A", "S", "D", "F", "Z", "X", "C", "V",
];

const CREAM: Color32 = Color32::from_rgb(244, 230, 214);
const DIM: Color32 = Color32::from_rgb(186, 164, 148);
const INK: Color32 = Color32::from_rgb(18, 14, 12);

pub struct SamplerApp {
    session: Session,
    selected: usize,
    edit_bus: bool,
    status: String,
    meter: f32,
    path: String,
    hover_pad: Option<usize>,
    import_json: String,
}

pub fn apply_style(ctx: &egui::Context) {
    let mut style = (*ctx.style()).clone();
    style.spacing.item_spacing = Vec2::new(8.0, 8.0);
    style.spacing.button_padding = Vec2::new(10.0, 6.0);
    style
        .text_styles
        .insert(TextStyle::Heading, FontId::proportional(28.0));
    style
        .text_styles
        .insert(TextStyle::Body, FontId::proportional(15.0));
    style
        .text_styles
        .insert(TextStyle::Button, FontId::proportional(14.0));
    style
        .text_styles
        .insert(TextStyle::Small, FontId::proportional(12.0));
    style
        .text_styles
        .insert(TextStyle::Monospace, FontId::monospace(13.0));
    ctx.set_style(style);

    let mut visuals = Visuals::dark();
    visuals.panel_fill = Color32::from_rgb(18, 16, 15);
    visuals.window_fill = Color32::from_rgb(18, 16, 15);
    visuals.faint_bg_color = Color32::from_rgb(32, 27, 24);
    visuals.extreme_bg_color = Color32::from_rgb(12, 10, 9);
    visuals.widgets.noninteractive.bg_fill = Color32::from_rgb(28, 24, 22);
    visuals.widgets.inactive.bg_fill = Color32::from_rgb(54, 40, 32);
    visuals.widgets.inactive.weak_bg_fill = Color32::from_rgb(42, 32, 28);
    visuals.widgets.hovered.bg_fill = Color32::from_rgb(196, 92, 36);
    visuals.widgets.active.bg_fill = Color32::from_rgb(232, 120, 48);
    visuals.widgets.inactive.fg_stroke.color = CREAM;
    visuals.widgets.hovered.fg_stroke.color = Color32::from_rgb(255, 244, 230);
    visuals.widgets.active.fg_stroke.color = Color32::WHITE;
    visuals.selection.bg_fill = Color32::from_rgb(196, 92, 36);
    visuals.hyperlink_color = Color32::from_rgb(255, 160, 80);
    visuals.window_corner_radius = CornerRadius::same(8);
    visuals.widgets.inactive.corner_radius = CornerRadius::same(6);
    visuals.widgets.hovered.corner_radius = CornerRadius::same(6);
    visuals.widgets.active.corner_radius = CornerRadius::same(6);
    ctx.set_visuals(visuals);
}

impl SamplerApp {
    pub fn new() -> Self {
        let session = audio::start();
        {
            let mut engine = session.engine.lock().unwrap_or_else(|err| err.into_inner());
            engine.load_demo_bank();
            if let Some(preset) = preset("lofi-phone") {
                let _ = engine.set_pad_chain(0, &preset);
            }
            if let Some(preset) = preset("room") {
                let _ = engine.set_bus_chain(&preset);
            }
        }
        Self {
            session,
            selected: 0,
            edit_bus: false,
            status: "Demo bank loaded. Keys 1–4, Q–R, A–F, Z–V trigger pads. Drop a WAV on a pad to load it.".into(),
            meter: 0.0,
            path: String::new(),
            hover_pad: None,
            import_json: String::new(),
        }
    }

    fn engine(&self) -> MutexGuard<'_, Engine> {
        self.session
            .engine
            .lock()
            .unwrap_or_else(|err| err.into_inner())
    }

    fn trigger(&mut self, index: usize) {
        if !self.engine().trigger(index) {
            self.status = format!("Pad {} is empty", index + 1);
        }
    }

    fn note_err(&mut self, result: Result<(), sp404_core::Error>) {
        if let Err(err) = result {
            self.status = format!("Error: {err}");
        }
    }

    fn current_chain(&self, pads: &[PadView], bus: &ChainDesc) -> ChainDesc {
        if self.edit_bus {
            bus.clone()
        } else {
            pads.get(self.selected)
                .map(|pad| pad.chain.clone())
                .unwrap_or_default()
        }
    }

    fn set_chain(&mut self, desc: ChainDesc) {
        let result = if self.edit_bus {
            self.engine().set_bus_chain(&desc)
        } else {
            self.engine().set_pad_chain(self.selected, &desc)
        };
        self.note_err(result);
    }

    fn set_param(&mut self, index: usize, id: &str, value: f32) {
        let result = if self.edit_bus {
            self.engine().set_bus_param(index, id, value)
        } else {
            self.engine().set_pad_param(self.selected, index, id, value)
        };
        self.note_err(result);
    }

    fn set_bypass(&mut self, index: usize, bypassed: bool) {
        let result = if self.edit_bus {
            self.engine().set_bus_bypass(index, bypassed)
        } else {
            self.engine().set_pad_bypass(self.selected, index, bypassed)
        };
        self.note_err(result);
    }

    fn load_bytes(&mut self, bytes: &[u8], name: &str, pad: usize) {
        match decode_wav(bytes) {
            Ok(buffer) => {
                let result = self.engine().load_pad(pad, buffer, name);
                match result {
                    Ok(()) => {
                        self.selected = pad;
                        self.status = format!("Loaded {name} onto pad {}", pad + 1);
                    }
                    Err(err) => self.status = format!("Error: {err}"),
                }
            }
            Err(err) => self.status = format!("Error: {err}"),
        }
    }

    fn load_path(&mut self, path: &std::path::Path, pad: usize) {
        match std::fs::read(path) {
            Ok(bytes) => {
                let name = path
                    .file_stem()
                    .and_then(|stem| stem.to_str())
                    .unwrap_or("sample");
                self.load_bytes(&bytes, name, pad);
            }
            Err(err) => self.status = format!("Error: {err}"),
        }
    }

    fn bounce(&mut self) {
        let (sample, chain) = {
            let engine = self.engine();
            let sample = engine.pad_sample(self.selected).ok().flatten();
            let pad_chain = engine.pad_view(self.selected).map(|pad| pad.chain);
            let bus = engine.bus_chain();
            (
                sample,
                pad_chain.map(|mut desc| {
                    desc.effects.extend(bus.effects);
                    desc
                }),
            )
        };
        let Some(sample) = sample else {
            self.status = "Selected pad has no sample to render".into();
            return;
        };
        let Ok(chain) = chain else {
            return;
        };
        match render(&sample, &chain).and_then(|buffer| encode_wav(&buffer)) {
            Ok(bytes) => {
                let path = format!("pad404-pad{:02}.wav", self.selected + 1);
                match std::fs::write(&path, bytes) {
                    Ok(()) => self.status = format!("Wrote {path}"),
                    Err(err) => self.status = format!("Error: {err}"),
                }
            }
            Err(err) => self.status = format!("Error: {err}"),
        }
    }

    fn handle_keys(&mut self, ctx: &egui::Context) {
        if ctx.wants_keyboard_input() {
            return;
        }
        let pressed: Vec<usize> = ctx.input(|input| {
            KEYS.iter()
                .filter(|(key, _)| input.key_pressed(*key))
                .map(|(_, pad)| *pad)
                .collect()
        });
        for pad in pressed {
            self.selected = pad;
            self.edit_bus = false;
            self.trigger(pad);
        }
        if ctx.input(|input| input.key_pressed(egui::Key::Space)) {
            self.trigger(self.selected);
        }
    }

    fn handle_drops(&mut self, ctx: &egui::Context) {
        let drops: Vec<(Option<PathBuf>, Option<Vec<u8>>, String)> = ctx.input(|input| {
            input
                .raw
                .dropped_files
                .iter()
                .map(|file| {
                    (
                        file.path.clone(),
                        file.bytes.as_ref().map(|bytes| bytes.as_ref().to_vec()),
                        file.name.clone(),
                    )
                })
                .collect()
        });
        let pad = self.hover_pad.unwrap_or(self.selected);
        for (path, bytes, name) in drops {
            if let Some(path) = path {
                self.load_path(&path, pad);
            } else if let Some(bytes) = bytes {
                let label = if name.is_empty() {
                    "dropped".to_string()
                } else {
                    name
                };
                self.load_bytes(&bytes, &label, pad);
            }
        }
    }
}

impl eframe::App for SamplerApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.hover_pad = None;
        self.handle_keys(ctx);
        let snap = self.snapshot();
        let block_peak = f32::from_bits(self.session.peak.load(Ordering::Relaxed));
        let dt = ctx.input(|input| input.stable_dt).clamp(0.0, 0.1);
        self.meter = (self.meter * (-3.5 * dt).exp()).max(block_peak);

        self.header(ctx, &snap);
        self.footer(ctx, &snap);
        self.inspector(ctx, &snap);
        self.pads(ctx, &snap);
        self.handle_drops(ctx);
        ctx.request_repaint_after(std::time::Duration::from_millis(16));
    }
}

struct Snap {
    pads: Vec<PadView>,
    bus: ChainDesc,
    master: f32,
    sample_rate: f32,
    waveform: Vec<f32>,
}

impl SamplerApp {
    fn snapshot(&self) -> Snap {
        let engine = self.engine();
        let pads = (0..PAD_COUNT)
            .map(|index| engine.pad_view(index).expect("pad index"))
            .collect();
        let waveform = engine.waveform(self.selected, 160).unwrap_or_default();
        Snap {
            pads,
            bus: engine.bus_chain(),
            master: engine.master(),
            sample_rate: engine.sample_rate(),
            waveform,
        }
    }

    fn header(&mut self, ctx: &egui::Context, snap: &Snap) {
        egui::TopBottomPanel::top("header").show(ctx, |ui| {
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new("PAD404")
                        .heading()
                        .color(Color32::from_rgb(255, 140, 54)),
                );
                ui.label(RichText::new("sampler").color(DIM));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(
                        RichText::new(format!("{:.0} Hz", snap.sample_rate))
                            .monospace()
                            .color(DIM),
                    );
                    ui.label(RichText::new(&self.session.status).small().color(DIM));
                });
            });
            ui.horizontal(|ui| {
                ui.label("WAV");
                ui.add(
                    egui::TextEdit::singleline(&mut self.path)
                        .desired_width(280.0)
                        .hint_text("path/to/sample.wav"),
                );
                if ui.button("Load").clicked() {
                    let path = self.path.trim().to_string();
                    if path.is_empty() {
                        self.status = "Type a WAV path, or drop a file on a pad".into();
                    } else {
                        self.load_path(std::path::Path::new(&path), self.selected);
                    }
                }
                if ui.button("Demo bank").clicked() {
                    self.engine().load_demo_bank();
                    self.status = "Demo bank reloaded".into();
                }
                if ui.button("Stop").clicked() {
                    self.engine().stop_all();
                    self.status = "Stopped".into();
                }
            });
            ui.add_space(4.0);
        });
    }

    fn footer(&mut self, ctx: &egui::Context, snap: &Snap) {
        egui::TopBottomPanel::bottom("footer").show(ctx, |ui| {
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                ui.label("Master");
                let mut master = snap.master;
                if ui
                    .add(egui::Slider::new(&mut master, 0.0..=1.5).show_value(false))
                    .changed()
                {
                    self.engine().set_master(master);
                }
                meter(ui, self.meter);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(
                        RichText::new("1–4   Q–R   A–F   Z–V    space retriggers")
                            .small()
                            .color(DIM),
                    );
                });
            });
            let color = if self.status.starts_with("Error") {
                Color32::from_rgb(255, 140, 120)
            } else {
                DIM
            };
            ui.label(RichText::new(&self.status).small().color(color));
            ui.add_space(2.0);
        });
    }

    fn pads(&mut self, ctx: &egui::Context, snap: &Snap) {
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.add_space(8.0);
            let gap = 12.0;
            let avail = ui.available_size();
            let size = ((avail.x - gap * 3.0) / 4.0)
                .min((avail.y - gap * 3.0) / 4.0)
                .clamp(78.0, 168.0);
            for row in 0..4 {
                ui.horizontal(|ui| {
                    let row_w = size * 4.0 + gap * 3.0;
                    let extra = (ui.available_width() - row_w).max(0.0);
                    ui.add_space(extra * 0.5);
                    for col in 0..4 {
                        let index = row * 4 + col;
                        let pad = &snap.pads[index];
                        let response = draw_pad(
                            ui,
                            Vec2::splat(size),
                            KEY_LABELS[index],
                            &pad.name,
                            pad.has_sample,
                            pad.active,
                            index == self.selected,
                        );
                        if response.hovered() {
                            self.hover_pad = Some(index);
                        }
                        if response.clicked() {
                            self.selected = index;
                            self.edit_bus = false;
                            self.trigger(index);
                        }
                        if col != 3 {
                            ui.add_space(gap);
                        }
                    }
                });
                if row != 3 {
                    ui.add_space(gap);
                }
            }
        });
    }

    fn inspector(&mut self, ctx: &egui::Context, snap: &Snap) {
        egui::SidePanel::right("inspector")
            .exact_width(430.0)
            .resizable(false)
            .show(ctx, |ui| {
                egui::ScrollArea::vertical().show(ui, |ui| {
                    self.pad_controls(ui, snap);
                    ui.separator();
                    self.chain_editor(ui, snap);
                });
            });
    }

    fn pad_controls(&mut self, ui: &mut egui::Ui, snap: &Snap) {
        let pad = &snap.pads[self.selected];
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            ui.label(
                RichText::new(format!("PAD {:02}", self.selected + 1))
                    .strong()
                    .color(Color32::from_rgb(255, 150, 70)),
            );
            ui.label(RichText::new(&pad.name).color(CREAM));
        });
        waveform(ui, &snap.waveform);
        let mut volume = pad.volume;
        if ui
            .add(egui::Slider::new(&mut volume, 0.0..=1.5).text("Volume"))
            .changed()
        {
            let result = self.engine().set_pad_volume(self.selected, volume);
            self.note_err(result);
        }
        let mut pitch = pad.pitch_semitones;
        if ui
            .add(egui::Slider::new(&mut pitch, -24.0..=24.0).text("Pitch"))
            .changed()
        {
            let result = self.engine().set_pad_pitch(self.selected, pitch);
            self.note_err(result);
        }
        let mut start = pad.start;
        let mut end = pad.end;
        let start_changed = ui
            .add(egui::Slider::new(&mut start, 0.0..=1.0).text("Start"))
            .changed();
        let end_changed = ui
            .add(egui::Slider::new(&mut end, 0.0..=1.0).text("End"))
            .changed();
        if start_changed || end_changed {
            let result = self.engine().set_pad_range(self.selected, start, end);
            self.note_err(result);
        }
        ui.horizontal(|ui| {
            if ui.button("Clear pad").clicked() {
                let result = self.engine().clear_pad(self.selected);
                self.note_err(result);
                self.status = format!("Cleared pad {}", self.selected + 1);
            }
            if ui.button("Render WAV").clicked() {
                self.bounce();
            }
        });
    }

    fn chain_editor(&mut self, ui: &mut egui::Ui, snap: &Snap) {
        ui.horizontal(|ui| {
            ui.label("Chain");
            if ui
                .selectable_label(!self.edit_bus, format!("Pad {:02}", self.selected + 1))
                .clicked()
            {
                self.edit_bus = false;
            }
            if ui.selectable_label(self.edit_bus, "Bus").clicked() {
                self.edit_bus = true;
            }
        });
        let mut apply_preset: Option<ChainDesc> = None;
        egui::ComboBox::from_id_salt("preset")
            .selected_text("Load preset…")
            .show_ui(ui, |ui| {
                for preset in chain_presets() {
                    if ui.selectable_label(false, preset.name).clicked() {
                        if let Ok(desc) = ChainDesc::from_json(preset.chain_json) {
                            apply_preset = Some(desc);
                            self.status = format!("Loaded preset {}", preset.name);
                        }
                    }
                }
            });
        if let Some(desc) = apply_preset {
            self.set_chain(desc);
        }

        let chain = self.current_chain(&snap.pads, &snap.bus);
        let mut remove: Option<usize> = None;
        let mut move_by: Option<(usize, isize)> = None;
        for (index, slot) in chain.effects.iter().enumerate() {
            let Some(schema) = effect_schema(&slot.effect) else {
                ui.label(format!("Unknown effect {}", slot.effect));
                continue;
            };
            ui.group(|ui| {
                ui.horizontal(|ui| {
                    ui.label(RichText::new(schema.name).strong());
                    let mut bypass = slot.bypass;
                    if ui.checkbox(&mut bypass, "Bypass").changed() {
                        self.set_bypass(index, bypass);
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.small_button("✕").clicked() {
                            remove = Some(index);
                        }
                        if ui.small_button("↓").clicked() {
                            move_by = Some((index, 1));
                        }
                        if ui.small_button("↑").clicked() {
                            move_by = Some((index, -1));
                        }
                    });
                });
                ui.label(RichText::new(schema.description).small().color(DIM));
                ui.add_enabled_ui(!slot.bypass, |ui| {
                    for spec in schema.params {
                        self.param_control(ui, index, spec, slot);
                    }
                });
            });
        }
        if let Some(index) = remove {
            let mut next = chain.clone();
            next.effects.remove(index);
            self.set_chain(next);
            self.status = "Removed effect".into();
        } else if let Some((index, delta)) = move_by {
            let mut next = chain.clone();
            let target = index as isize + delta;
            if target >= 0 && (target as usize) < next.effects.len() {
                next.effects.swap(index, target as usize);
                self.set_chain(next);
            }
        }

        let mut add: Option<String> = None;
        egui::ComboBox::from_id_salt("add-effect")
            .selected_text("Add effect…")
            .show_ui(ui, |ui| {
                for schema in effect_schemas() {
                    if ui.button(schema.name).clicked() {
                        add = Some(schema.id.to_string());
                    }
                }
            });
        if let Some(id) = add {
            let mut next = self.current_chain(&snap.pads, &snap.bus);
            next.effects.push(sp404_core::EffectDesc {
                effect: id,
                bypass: false,
                params: Default::default(),
            });
            self.set_chain(next);
        }

        ui.horizontal(|ui| {
            if ui.button("Copy JSON").clicked() {
                if let Ok(json) = chain.to_json() {
                    ui.ctx().copy_text(json);
                    self.status = "Copied chain JSON".into();
                }
            }
        });
        ui.collapsing("Import JSON", |ui| {
            ui.add(
                egui::TextEdit::multiline(&mut self.import_json)
                    .desired_rows(6)
                    .desired_width(f32::INFINITY)
                    .font(TextStyle::Monospace)
                    .hint_text(r#"{"effects":[{"effect":"filter","params":{"cutoff":1200}}]}"#),
            );
            if ui.button("Apply JSON").clicked() {
                match ChainDesc::from_json(&self.import_json) {
                    Ok(desc) => {
                        self.set_chain(desc);
                        self.status = "Applied chain JSON".into();
                    }
                    Err(err) => self.status = format!("Error: {err}"),
                }
            }
        });
        ui.add_space(8.0);
    }

    fn param_control(
        &mut self,
        ui: &mut egui::Ui,
        index: usize,
        spec: &ParamSchema,
        slot: &sp404_core::EffectDesc,
    ) {
        let mut value = slot.params.get(spec.id).copied().unwrap_or(spec.default);
        if spec.scale == ParamScale::Choice {
            let current = value.round() as usize;
            let label = spec.choices.get(current).copied().unwrap_or("?");
            let mut picked: Option<f32> = None;
            egui::ComboBox::from_id_salt(format!("p-{index}-{}", spec.id))
                .selected_text(format!("{}: {label}", spec.name))
                .show_ui(ui, |ui| {
                    for (choice, name) in spec.choices.iter().enumerate() {
                        if ui.selectable_label(choice == current, *name).clicked() {
                            picked = Some(choice as f32);
                        }
                    }
                });
            if let Some(picked) = picked {
                self.set_param(index, spec.id, picked);
            }
            return;
        }
        let mut slider = egui::Slider::new(&mut value, spec.min..=spec.max).text(spec.name);
        if spec.scale == ParamScale::Logarithmic {
            slider = slider.logarithmic(true);
        }
        if ui.add(slider).changed() {
            self.set_param(index, spec.id, value);
        }
        let _ = spec.unit;
    }
}

fn preset(id: &str) -> Option<ChainDesc> {
    chain_presets()
        .iter()
        .find(|preset| preset.id == id)
        .and_then(|preset| ChainDesc::from_json(preset.chain_json).ok())
}

fn draw_pad(
    ui: &mut egui::Ui,
    size: Vec2,
    key: &str,
    name: &str,
    loaded: bool,
    active: bool,
    selected: bool,
) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(size, Sense::click());
    let mut fill = if active {
        Color32::from_rgb(255, 176, 72)
    } else if loaded {
        Color32::from_rgb(186, 84, 32)
    } else {
        Color32::from_rgb(48, 38, 34)
    };
    if response.hovered() {
        fill = lighten(fill);
    }
    let painter = ui.painter();
    painter.rect_filled(rect, CornerRadius::same(12), fill);
    if selected {
        painter.rect_stroke(
            rect.shrink(1.0),
            CornerRadius::same(12),
            Stroke::new(2.5, Color32::from_rgb(255, 220, 170)),
            StrokeKind::Inside,
        );
    }
    let text = if active { INK } else { CREAM };
    painter.text(
        rect.left_top() + Vec2::new(10.0, 8.0),
        Align2::LEFT_TOP,
        key,
        FontId::monospace(13.0),
        if active {
            Color32::from_rgb(80, 40, 16)
        } else {
            Color32::from_rgb(255, 214, 180)
        },
    );
    let label = if name.is_empty() {
        "Empty".to_string()
    } else {
        name.to_string()
    };
    painter.text(
        rect.center() + Vec2::new(0.0, 6.0),
        Align2::CENTER_CENTER,
        label,
        FontId::proportional(16.0),
        text,
    );
    response
}

fn waveform(ui: &mut egui::Ui, peaks: &[f32]) {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 56.0), Sense::hover());
    let painter = ui.painter();
    painter.rect_filled(rect, CornerRadius::same(6), Color32::from_rgb(12, 10, 9));
    if peaks.is_empty() {
        return;
    }
    let step = rect.width() / peaks.len() as f32;
    for (index, peak) in peaks.iter().enumerate() {
        let x = rect.left() + (index as f32 + 0.5) * step;
        let half = peak.clamp(0.0, 1.0) * rect.height() * 0.42;
        painter.line_segment(
            [
                Pos2::new(x, rect.center().y - half),
                Pos2::new(x, rect.center().y + half),
            ],
            Stroke::new(step.max(1.0) * 0.7, Color32::from_rgb(255, 148, 64)),
        );
    }
}

fn meter(ui: &mut egui::Ui, level: f32) {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(120.0, 14.0), Sense::hover());
    let painter = ui.painter();
    painter.rect_filled(rect, CornerRadius::same(3), Color32::from_rgb(12, 10, 9));
    let frac = (level * 1.4).clamp(0.0, 1.0);
    let mut filled = rect;
    filled.max.x = rect.min.x + rect.width() * frac;
    let color = if frac > 0.92 {
        Color32::from_rgb(255, 90, 70)
    } else {
        Color32::from_rgb(255, 150, 60)
    };
    painter.rect_filled(filled, CornerRadius::same(3), color);
}

fn lighten(color: Color32) -> Color32 {
    Color32::from_rgb(
        color.r().saturating_add(22),
        color.g().saturating_add(16),
        color.b().saturating_add(10),
    )
}
