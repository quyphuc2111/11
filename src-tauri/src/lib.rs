use base64::{engine::general_purpose, Engine};
use rdev::{simulate, EventType, Key, Button, SimulateError};
use std::io::Cursor;
use std::net::UdpSocket;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::{thread, time::Duration};
use tauri::{Emitter, Manager};

// Platform-specific capture module
#[cfg(target_os = "windows")]
mod windows_capture_handler;

// Cross-platform capture using screenshots crate
#[cfg(not(target_os = "windows"))]
use screenshots::Screen;

static CAPTURING: AtomicBool = AtomicBool::new(false);
static STREAMING: AtomicBool = AtomicBool::new(false);

lazy_static::lazy_static! {
    static ref STREAMER: Arc<Mutex<Option<UdpStreamer>>> = Arc::new(Mutex::new(None));
}

// ============== Input Simulation (RustDesk style) ==============

fn send_event(event_type: &EventType) -> Result<(), String> {
    match simulate(event_type) {
        Ok(()) => {
            // Small delay for OS to process (RustDesk does this too)
            thread::sleep(Duration::from_millis(10));
            Ok(())
        }
        Err(SimulateError) => Err(format!("Failed to simulate event: {:?}", event_type)),
    }
}

// Convert JavaScript key code to rdev Key
fn js_key_to_rdev(key: &str, code: &str) -> Option<Key> {
    // First try by code (physical key position)
    match code {
        // Letters
        "KeyA" => Some(Key::KeyA),
        "KeyB" => Some(Key::KeyB),
        "KeyC" => Some(Key::KeyC),
        "KeyD" => Some(Key::KeyD),
        "KeyE" => Some(Key::KeyE),
        "KeyF" => Some(Key::KeyF),
        "KeyG" => Some(Key::KeyG),
        "KeyH" => Some(Key::KeyH),
        "KeyI" => Some(Key::KeyI),
        "KeyJ" => Some(Key::KeyJ),
        "KeyK" => Some(Key::KeyK),
        "KeyL" => Some(Key::KeyL),
        "KeyM" => Some(Key::KeyM),
        "KeyN" => Some(Key::KeyN),
        "KeyO" => Some(Key::KeyO),
        "KeyP" => Some(Key::KeyP),
        "KeyQ" => Some(Key::KeyQ),
        "KeyR" => Some(Key::KeyR),
        "KeyS" => Some(Key::KeyS),
        "KeyT" => Some(Key::KeyT),
        "KeyU" => Some(Key::KeyU),
        "KeyV" => Some(Key::KeyV),
        "KeyW" => Some(Key::KeyW),
        "KeyX" => Some(Key::KeyX),
        "KeyY" => Some(Key::KeyY),
        "KeyZ" => Some(Key::KeyZ),
        
        // Numbers
        "Digit0" => Some(Key::Num0),
        "Digit1" => Some(Key::Num1),
        "Digit2" => Some(Key::Num2),
        "Digit3" => Some(Key::Num3),
        "Digit4" => Some(Key::Num4),
        "Digit5" => Some(Key::Num5),
        "Digit6" => Some(Key::Num6),
        "Digit7" => Some(Key::Num7),
        "Digit8" => Some(Key::Num8),
        "Digit9" => Some(Key::Num9),
        
        // Function keys
        "F1" => Some(Key::F1),
        "F2" => Some(Key::F2),
        "F3" => Some(Key::F3),
        "F4" => Some(Key::F4),
        "F5" => Some(Key::F5),
        "F6" => Some(Key::F6),
        "F7" => Some(Key::F7),
        "F8" => Some(Key::F8),
        "F9" => Some(Key::F9),
        "F10" => Some(Key::F10),
        "F11" => Some(Key::F11),
        "F12" => Some(Key::F12),
        
        // Special keys
        "Enter" => Some(Key::Return),
        "NumpadEnter" => Some(Key::Return),
        "Escape" => Some(Key::Escape),
        "Backspace" => Some(Key::Backspace),
        "Tab" => Some(Key::Tab),
        "Space" => Some(Key::Space),
        "Delete" => Some(Key::Delete),
        "Insert" => Some(Key::Insert),
        "Home" => Some(Key::Home),
        "End" => Some(Key::End),
        "PageUp" => Some(Key::PageUp),
        "PageDown" => Some(Key::PageDown),
        
        // Arrow keys
        "ArrowUp" => Some(Key::UpArrow),
        "ArrowDown" => Some(Key::DownArrow),
        "ArrowLeft" => Some(Key::LeftArrow),
        "ArrowRight" => Some(Key::RightArrow),
        
        // Modifiers
        "ShiftLeft" | "ShiftRight" => Some(Key::ShiftLeft),
        "ControlLeft" | "ControlRight" => Some(Key::ControlLeft),
        "AltLeft" | "AltRight" => Some(Key::Alt),
        "MetaLeft" | "MetaRight" => Some(Key::MetaLeft),
        
        // Punctuation
        "Minus" => Some(Key::Minus),
        "Equal" => Some(Key::Equal),
        "BracketLeft" => Some(Key::LeftBracket),
        "BracketRight" => Some(Key::RightBracket),
        "Backslash" => Some(Key::BackSlash),
        "Semicolon" => Some(Key::SemiColon),
        "Quote" => Some(Key::Quote),
        "Backquote" => Some(Key::BackQuote),
        "Comma" => Some(Key::Comma),
        "Period" => Some(Key::Dot),
        "Slash" => Some(Key::Slash),
        
        // Numpad
        "Numpad0" => Some(Key::Kp0),
        "Numpad1" => Some(Key::Kp1),
        "Numpad2" => Some(Key::Kp2),
        "Numpad3" => Some(Key::Kp3),
        "Numpad4" => Some(Key::Kp4),
        "Numpad5" => Some(Key::Kp5),
        "Numpad6" => Some(Key::Kp6),
        "Numpad7" => Some(Key::Kp7),
        "Numpad8" => Some(Key::Kp8),
        "Numpad9" => Some(Key::Kp9),
        "NumpadMultiply" => Some(Key::KpMultiply),
        "NumpadAdd" => Some(Key::KpPlus),
        "NumpadSubtract" => Some(Key::KpMinus),
        "NumpadDecimal" => Some(Key::KpDelete),
        "NumpadDivide" => Some(Key::KpDivide),
        
        // CapsLock, etc
        "CapsLock" => Some(Key::CapsLock),
        "PrintScreen" => Some(Key::PrintScreen),
        "ScrollLock" => Some(Key::ScrollLock),
        "Pause" => Some(Key::Pause),
        
        _ => {
            // Fallback: try by key name
            match key {
                " " => Some(Key::Space),
                _ => None,
            }
        }
    }
}

// ============== UDP Streamer ==============
struct UdpStreamer {
    socket: UdpSocket,
    target_addr: String,
    sequence: u32,
}

impl UdpStreamer {
    fn new(target_addr: &str) -> Result<Self, String> {
        let socket = UdpSocket::bind("0.0.0.0:0").map_err(|e| e.to_string())?;
        socket.set_nonblocking(true).map_err(|e| e.to_string())?;
        Ok(Self {
            socket,
            target_addr: target_addr.to_string(),
            sequence: 0,
        })
    }

    fn send_frame(&mut self, data: &[u8]) -> Result<(), String> {
        const MAX_PACKET_SIZE: usize = 1400;
        
        let chunks: Vec<&[u8]> = data.chunks(MAX_PACKET_SIZE - 8).collect();
        let total_chunks = chunks.len() as u16;

        for (i, chunk) in chunks.iter().enumerate() {
            let mut packet = Vec::with_capacity(chunk.len() + 8);
            packet.extend_from_slice(&self.sequence.to_be_bytes());
            packet.extend_from_slice(&(i as u16).to_be_bytes());
            packet.extend_from_slice(&total_chunks.to_be_bytes());
            packet.extend_from_slice(chunk);

            self.socket
                .send_to(&packet, &self.target_addr)
                .map_err(|e| e.to_string())?;
        }
        
        self.sequence = self.sequence.wrapping_add(1);
        Ok(())
    }
}

// ============== Tauri Commands ==============

/// Capture a single screenshot
#[tauri::command]
fn capture_screen() -> Result<String, String> {
    #[cfg(target_os = "windows")]
    {
        windows_capture_handler::capture_single_frame()
    }

    #[cfg(not(target_os = "windows"))]
    {
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
}

/// Start continuous screen capture loop
#[tauri::command]
fn start_capture_loop(app: tauri::AppHandle, interval_ms: u64) {
    if CAPTURING.load(Ordering::SeqCst) {
        return;
    }
    CAPTURING.store(true, Ordering::SeqCst);

    #[cfg(target_os = "windows")]
    {
        if let Err(e) = windows_capture_handler::start_capture(app.clone()) {
            let _ = app.emit("capture-error", e);
            CAPTURING.store(false, Ordering::SeqCst);
        }
    }

    #[cfg(not(target_os = "windows"))]
    {
        std::thread::spawn(move || {
            while CAPTURING.load(Ordering::SeqCst) {
                if let Ok(data) = capture_screen() {
                    let _ = app.emit("screen-frame", data);
                }
                std::thread::sleep(std::time::Duration::from_millis(interval_ms));
            }
        });
    }
}

/// Stop screen capture loop
#[tauri::command]
fn stop_capture_loop() {
    CAPTURING.store(false, Ordering::SeqCst);
    
    #[cfg(target_os = "windows")]
    {
        windows_capture_handler::stop_capture();
    }
}

/// Start UDP streaming
#[tauri::command]
fn start_h264_stream(app: tauri::AppHandle, target_addr: String, fps: u32) -> Result<(), String> {
    if STREAMING.load(Ordering::SeqCst) {
        return Err("Already streaming".to_string());
    }

    {
        let mut streamer_guard = STREAMER.lock().map_err(|e| e.to_string())?;
        *streamer_guard = Some(UdpStreamer::new(&target_addr)?);
    }

    STREAMING.store(true, Ordering::SeqCst);
    let interval = std::time::Duration::from_millis(1000 / fps as u64);

    std::thread::spawn(move || {
        while STREAMING.load(Ordering::SeqCst) {
            #[cfg(target_os = "windows")]
            let frame_data = windows_capture_handler::get_last_frame();
            
            #[cfg(not(target_os = "windows"))]
            let frame_data = capture_screen().ok();

            if let Some(data) = frame_data {
                if let Some(base64_part) = data.strip_prefix("data:image/jpeg;base64,") {
                    if let Ok(bytes) = general_purpose::STANDARD.decode(base64_part) {
                        if let Ok(mut guard) = STREAMER.lock() {
                            if let Some(streamer) = guard.as_mut() {
                                if let Err(e) = streamer.send_frame(&bytes) {
                                    let _ = app.emit("stream-error", e);
                                }
                            }
                        }
                    }
                }
            }
            std::thread::sleep(interval);
        }
    });

    Ok(())
}

/// Stop UDP streaming
#[tauri::command]
fn stop_h264_stream() {
    STREAMING.store(false, Ordering::SeqCst);
    if let Ok(mut streamer_guard) = STREAMER.lock() {
        *streamer_guard = None;
    }
}

/// Get streaming statistics
#[tauri::command]
fn get_stream_stats() -> Result<serde_json::Value, String> {
    let is_streaming = STREAMING.load(Ordering::SeqCst);
    let is_capturing = CAPTURING.load(Ordering::SeqCst);
    let sequence = if let Ok(guard) = STREAMER.lock() {
        guard.as_ref().map(|s| s.sequence).unwrap_or(0)
    } else {
        0
    };

    Ok(serde_json::json!({
        "streaming": is_streaming,
        "capturing": is_capturing,
        "frames_sent": sequence
    }))
}

/// Get screen size for remote control coordinate mapping
#[tauri::command]
fn get_screen_size() -> Result<serde_json::Value, String> {
    match rdev::display_size() {
        Ok((width, height)) => Ok(serde_json::json!({
            "width": width,
            "height": height
        })),
        Err(_) => {
            // Fallback to screenshots crate
            let screens = screenshots::Screen::all().map_err(|e| e.to_string())?;
            let screen = screens.first().ok_or("No screen found")?;
            let info = screen.display_info;
            Ok(serde_json::json!({
                "width": info.width,
                "height": info.height
            }))
        }
    }
}

/// Lock/unlock client screen
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

/// Remote mouse move (RustDesk style using rdev)
#[tauri::command]
fn remote_mouse_move(x: f64, y: f64) -> Result<(), String> {
    send_event(&EventType::MouseMove { x, y })
}

/// Remote mouse click (RustDesk style using rdev)
#[tauri::command]
fn remote_mouse_click(button: String) -> Result<(), String> {
    let btn = match button.as_str() {
        "right" => Button::Right,
        "middle" => Button::Middle,
        _ => Button::Left,
    };
    
    // Press and release (click)
    send_event(&EventType::ButtonPress(btn))?;
    send_event(&EventType::ButtonRelease(btn))
}

/// Remote mouse scroll (RustDesk style)
#[tauri::command]
fn remote_mouse_scroll(delta_x: i64, delta_y: i64) -> Result<(), String> {
    send_event(&EventType::Wheel { delta_x, delta_y })
}

/// Remote key press (RustDesk style using rdev)
#[tauri::command]
fn remote_key_press(key: String, code: String, ctrl: bool, alt: bool, shift: bool, meta: bool) -> Result<(), String> {
    // Press modifiers first
    if ctrl {
        send_event(&EventType::KeyPress(Key::ControlLeft))?;
    }
    if alt {
        send_event(&EventType::KeyPress(Key::Alt))?;
    }
    if shift {
        send_event(&EventType::KeyPress(Key::ShiftLeft))?;
    }
    if meta {
        send_event(&EventType::KeyPress(Key::MetaLeft))?;
    }

    // Press and release the main key
    if let Some(rdev_key) = js_key_to_rdev(&key, &code) {
        send_event(&EventType::KeyPress(rdev_key))?;
        send_event(&EventType::KeyRelease(rdev_key))?;
    }

    // Release modifiers
    if meta {
        send_event(&EventType::KeyRelease(Key::MetaLeft))?;
    }
    if shift {
        send_event(&EventType::KeyRelease(Key::ShiftLeft))?;
    }
    if alt {
        send_event(&EventType::KeyRelease(Key::Alt))?;
    }
    if ctrl {
        send_event(&EventType::KeyRelease(Key::ControlLeft))?;
    }

    Ok(())
}

/// Remote key down (for held keys)
#[tauri::command]
fn remote_key_down(key: String, code: String) -> Result<(), String> {
    if let Some(rdev_key) = js_key_to_rdev(&key, &code) {
        send_event(&EventType::KeyPress(rdev_key))
    } else {
        Ok(())
    }
}

/// Remote key up (for released keys)
#[tauri::command]
fn remote_key_up(key: String, code: String) -> Result<(), String> {
    if let Some(rdev_key) = js_key_to_rdev(&key, &code) {
        send_event(&EventType::KeyRelease(rdev_key))
    } else {
        Ok(())
    }
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
            start_h264_stream,
            stop_h264_stream,
            get_stream_stats,
            get_screen_size,
            set_lock_screen,
            remote_mouse_move,
            remote_mouse_click,
            remote_mouse_scroll,
            remote_key_press,
            remote_key_down,
            remote_key_up
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
