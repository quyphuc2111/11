use base64::{engine::general_purpose, Engine};
use screenshots::Screen;
use std::io::Cursor;
use std::sync::atomic::{AtomicBool, Ordering};
use tauri::Emitter;

static CAPTURING: AtomicBool = AtomicBool::new(false);

#[tauri::command]
fn capture_screen() -> Result<String, String> {
    let screens = Screen::all().map_err(|e| e.to_string())?;
    let screen = screens.first().ok_or("No screen found")?;

    let image = screen.capture().map_err(|e| e.to_string())?;

    // Resize để giảm bandwidth
    let resized = image::imageops::resize(
        &image,
        800,
        (800.0 * image.height() as f32 / image.width() as f32) as u32,
        image::imageops::FilterType::Triangle,
    );

    // Encode to JPEG
    let mut buffer = Cursor::new(Vec::new());
    resized
        .write_to(&mut buffer, image::ImageOutputFormat::Jpeg(70))
        .map_err(|e| e.to_string())?;

    let base64_str = general_purpose::STANDARD.encode(buffer.into_inner());
    Ok(format!("data:image/jpeg;base64,{}", base64_str))
}

#[tauri::command]
fn start_capture_loop(app: tauri::AppHandle, interval_ms: u64) {
    if CAPTURING.load(Ordering::SeqCst) {
        println!("Capture already running");
        return;
    }
    CAPTURING.store(true, Ordering::SeqCst);
    println!("Starting capture loop with interval: {}ms", interval_ms);

    std::thread::spawn(move || {
        while CAPTURING.load(Ordering::SeqCst) {
            match capture_screen() {
                Ok(data) => {
                    println!("Captured frame, size: {} bytes", data.len());
                    let _ = app.emit("screen-frame", data);
                }
                Err(e) => {
                    println!("Capture error: {}", e);
                }
            }
            std::thread::sleep(std::time::Duration::from_millis(interval_ms));
        }
        println!("Capture loop stopped");
    });
}

#[tauri::command]
fn stop_capture_loop() {
    CAPTURING.store(false, Ordering::SeqCst);
}

#[tauri::command]
fn lock_input(lock: bool) -> Result<(), String> {
    println!("Lock input: {}", lock);
    Ok(())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_opener::init())
        .invoke_handler(tauri::generate_handler![
            capture_screen,
            start_capture_loop,
            stop_capture_loop,
            lock_input
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
