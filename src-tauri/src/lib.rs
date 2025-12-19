use base64::{engine::general_purpose, Engine};
use enigo::{Enigo, Keyboard, Mouse, Settings};
use screenshots::Screen;
use std::io::Cursor;
use std::sync::atomic::{AtomicBool, Ordering};
use tauri::{Emitter, Manager};

static CAPTURING: AtomicBool = AtomicBool::new(false);

#[tauri::command]
fn capture_screen() -> Result<String, String> {
    let screens = Screen::all().map_err(|e| e.to_string())?;
    let screen = screens.first().ok_or("No screen found")?;

    let image = screen.capture().map_err(|e| e.to_string())?;

    let resized = image::imageops::resize(
        &image,
        640,
        (640.0 * image.height() as f32 / image.width() as f32) as u32,
        image::imageops::FilterType::Nearest,
    );

    let mut buffer = Cursor::new(Vec::new());
    resized
        .write_to(&mut buffer, image::ImageOutputFormat::Jpeg(50))
        .map_err(|e| e.to_string())?;

    let base64_str = general_purpose::STANDARD.encode(buffer.into_inner());
    Ok(format!("data:image/jpeg;base64,{}", base64_str))
}

#[tauri::command]
fn start_capture_loop(app: tauri::AppHandle, interval_ms: u64) {
    if CAPTURING.load(Ordering::SeqCst) {
        return;
    }
    CAPTURING.store(true, Ordering::SeqCst);

    std::thread::spawn(move || {
        while CAPTURING.load(Ordering::SeqCst) {
            if let Ok(data) = capture_screen() {
                let _ = app.emit("screen-frame", data);
            }
            std::thread::sleep(std::time::Duration::from_millis(interval_ms));
        }
    });
}

#[tauri::command]
fn stop_capture_loop() {
    CAPTURING.store(false, Ordering::SeqCst);
}

#[tauri::command]
async fn set_lock_screen(app: tauri::AppHandle, lock: bool, _message: String) -> Result<(), String> {
    let window = app.get_webview_window("main").ok_or("Window not found")?;
    
    if lock {
        window.set_fullscreen(true).map_err(|e| e.to_string())?;
        window.set_always_on_top(true).map_err(|e| e.to_string())?;
        window.set_focus().map_err(|e| e.to_string())?;
    } else {
        window.set_always_on_top(false).map_err(|e| e.to_string())?;
        window.set_fullscreen(false).map_err(|e| e.to_string())?;
    }
    
    Ok(())
}

// Remote control commands
#[tauri::command]
fn remote_mouse_move(x: i32, y: i32) -> Result<(), String> {
    let mut enigo = Enigo::new(&Settings::default()).map_err(|e| e.to_string())?;
    enigo.move_mouse(x, y, enigo::Coordinate::Abs).map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
fn remote_mouse_click(button: String) -> Result<(), String> {
    let mut enigo = Enigo::new(&Settings::default()).map_err(|e| e.to_string())?;
    let btn = match button.as_str() {
        "right" => enigo::Button::Right,
        "middle" => enigo::Button::Middle,
        _ => enigo::Button::Left,
    };
    enigo.button(btn, enigo::Direction::Click).map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
fn remote_key_press(key: String) -> Result<(), String> {
    let mut enigo = Enigo::new(&Settings::default()).map_err(|e| e.to_string())?;
    enigo.text(&key).map_err(|e| e.to_string())?;
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
            set_lock_screen,
            remote_mouse_move,
            remote_mouse_click,
            remote_key_press
        ])
        .setup(|app| {
            #[cfg(debug_assertions)]
            {
                let window = app.get_webview_window("main").unwrap();
                window.open_devtools();
            }
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
