#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

use convalgen::app::ConvalgenApp;
use eframe::egui;

const PRODUCT_NAME: &str = "PhonoScript GUI";

fn native_options() -> eframe::NativeOptions {
    eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1280.0, 820.0])
            .with_min_inner_size([640.0, 480.0])
            .with_title(PRODUCT_NAME),
        ..Default::default()
    }
}

#[cfg(target_os = "macos")]
fn main() -> eframe::Result {
    use winit::event_loop::EventLoop;

    let initial_path = std::env::args_os().nth(1).map(std::path::PathBuf::from);
    let event_loop = EventLoop::<eframe::UserEvent>::with_user_event().build()?;
    convalgen::macos::install_file_open_handler();
    let mut application = eframe::create_native(
        PRODUCT_NAME,
        native_options(),
        Box::new(move |context| {
            Ok(Box::new(ConvalgenApp::new_with_path(
                context,
                initial_path.clone(),
            )))
        }),
        &event_loop,
    );
    event_loop.run_app(&mut application)?;
    Ok(())
}

#[cfg(not(target_os = "macos"))]
fn main() -> eframe::Result {
    let initial_path = std::env::args_os().nth(1).map(std::path::PathBuf::from);
    eframe::run_native(
        PRODUCT_NAME,
        native_options(),
        Box::new(move |context| {
            Ok(Box::new(ConvalgenApp::new_with_path(
                context,
                initial_path.clone(),
            )))
        }),
    )
}
