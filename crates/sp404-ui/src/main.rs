//! Native pad-sampler window.
//!
//! Audio device I/O stays in this crate. The engine, effects, and chain JSON
//! all come from `sp404-core`, which is what also targets `wasm32`.

#![forbid(unsafe_code)]

mod app;
mod audio;

fn main() -> eframe::Result<()> {
    let options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_inner_size([1200.0, 780.0])
            .with_min_inner_size([960.0, 640.0]),
        ..Default::default()
    };
    eframe::run_native(
        "Pad404",
        options,
        Box::new(|cc| {
            app::apply_style(&cc.egui_ctx);
            Ok(Box::new(app::SamplerApp::new()))
        }),
    )
}
