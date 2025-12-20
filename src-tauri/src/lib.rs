use base64::{engine::general_purpose, Engine};
use parking_lot::Mutex;
use rdev::{simulate, Button, EventType, Key, SimulateError};
use scrap::{Capturer, Display};
use std::io::Cursor;
use std::io::ErrorKind::WouldBlock;
use std::net::UdpSocket;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use std::thread;
use tauri::{Emitter, Manager};
use tokio::sync::broadcast;

// ============== Global State ==============
lazy_static::lazy_static! {
    static ref CAPTURING: AtomicBool = AtomicBool::new(false);
    static ref STREAMING: AtomicBool = AtomicBool::new(false);
    static ref UDP_RECEIVER_RUNNING: AtomicBool = AtomicBool::new(false);
    static ref FRAME_SENDER: Mutex<Option<broadcast::Sender<Vec<u8>>>> = Mutex::new(None);
}

// ============== Screen Capture with scrap ==============

fn capture_frame_bgra() -> Result<(Vec<u8>, usize, usize), String> {
    let display = Display::primary().map_err(|e| format!("No display: {}", e))?;
    let mut capturer = Capturer::new(display).map_err(|e| format!("Capturer error: {}", e))?;
    
    let width = capturer.width();
    let height = capturer.height();
    
    // Try to get a frame (may need multiple attempts)
    for _ in 0..10 {
        match capturer.frame() {
            Ok(frame) => {
                return Ok((frame.to_vec(), width, height));
            }
            Err(ref e) if e.kind() == WouldBlock => {
                thread::sleep(Duration::from_millis(10));
                continue;
            }
            Err(e) => return Err(format!("Frame error: {}", e)),
        }
    }
    Err("Timeout waiting for frame".to_string())
}

fn bgra_to_jpeg(bgra: &[u8], width: usize, height: usize, quality: u8) -> Result<Vec<u8>, String> {
    // Convert BGRA to RGB
    let mut rgb = Vec::with_capacity(width * height * 3);
    let stride = bgra.len() / height;
    
    for y in 0..height {
        for x in 0..width {
            let i = y * stride + x * 4;
            if i + 2 < bgra.len() {
                rgb.push(bgra[i + 2]); // R
                rgb.push(bgra[i + 1]); // G
                rgb.push(bgra[i]);     // B
            }
        }
    }
    
    // Create RGB image and resize
    let img = image::RgbImage::from_raw(width as u32, height as u32, rgb)
        .ok_or("Failed to create image")?;
    
    // Resize to 640px width for bandwidth
    let target_width = 640u32;
    let target_height = (target_width as f32 * height as f32 / width as f32) as u32;
    let resized = image::imageops::resize(&img, target_width, target_height, image::imageops::FilterType::Nearest);
    
    // Encode to JPEG
    let mut buffer = Cursor::new(Vec::new());
    resized.write_to(&mut buffer, image::ImageOutputFormat::Jpeg(quality))
        .map_err(|e| format!("JPEG encode error: {}", e))?;
    
    Ok(buffer.into_inner())
}

// ============== UDP Streaming ==============

/// Start UDP streaming to server
fn start_udp_stream(server_addr: &str, fps: u32) -> Result<(), String> {
    if STREAMING.load(Ordering::SeqCst) {
        return Err("Already streaming".to_string());
    }
    
    let socket = UdpSocket::bind("0.0.0.0:0").map_err(|e| e.to_string())?;
    socket.set_nonblocking(false).map_err(|e| e.to_string())?;
    
    let server = server_addr.to_string();
    let interval = Duration::from_millis(1000 / fps as u64);
    
    STREAMING.store(true, Ordering::SeqCst);
    
    thread::spawn(move || {
        let mut sequence: u32 = 0;
        
        while STREAMING.load(Ordering::SeqCst) {
            match capture_frame_bgra() {
                Ok((bgra, width, height)) => {
                    if let Ok(jpeg) = bgra_to_jpeg(&bgra, width, height, 50) {
                        // Send frame via UDP with chunking
                        let _ = send_udp_frame(&socket, &server, &jpeg, sequence);
                        sequence = sequence.wrapping_add(1);
                    }
                }
                Err(_) => {}
            }
            thread::sleep(interval);
        }
    });
    
    Ok(())
}

fn send_udp_frame(socket: &UdpSocket, addr: &str, data: &[u8], sequence: u32) -> Result<(), String> {
    const MAX_CHUNK: usize = 1400;
    
    let chunks: Vec<&[u8]> = data.chunks(MAX_CHUNK - 12).collect();
    let total = chunks.len() as u16;
    
    for (i, chunk) in chunks.iter().enumerate() {
        let mut packet = Vec::with_capacity(chunk.len() + 12);
        // Header: magic(2) + seq(4) + idx(2) + total(2) + len(2)
        packet.extend_from_slice(b"SC"); // Magic bytes
        packet.extend_from_slice(&sequence.to_be_bytes());
        packet.extend_from_slice(&(i as u16).to_be_bytes());
        packet.extend_from_slice(&total.to_be_bytes());
        packet.extend_from_slice(&(chunk.len() as u16).to_be_bytes());
        packet.extend_from_slice(chunk);
        
        socket.send_to(&packet, addr).map_err(|e| e.to_string())?;
    }
    
    Ok(())
}

// ============== TCP Server for Admin ==============

/// Start TCP server to receive and broadcast frames
fn start_frame_server(port: u16) -> Result<broadcast::Receiver<Vec<u8>>, String> {
    let (tx, rx) = broadcast::channel::<Vec<u8>>(16);
    
    {
        let mut sender = FRAME_SENDER.lock();
        *sender = Some(tx.clone());
    }
    
    // Start UDP receiver
    if !UDP_RECEIVER_RUNNING.load(Ordering::SeqCst) {
        UDP_RECEIVER_RUNNING.store(true, Ordering::SeqCst);
        
        let udp_port = port;
        thread::spawn(move || {
            let socket = match UdpSocket::bind(format!("0.0.0.0:{}", udp_port)) {
                Ok(s) => s,
                Err(e) => {
                    eprintln!("UDP bind error: {}", e);
                    UDP_RECEIVER_RUNNING.store(false, Ordering::SeqCst);
                    return;
                }
            };
            
            let mut frame_buffers: std::collections::HashMap<u32, FrameBuffer> = std::collections::HashMap::new();
            let mut buf = [0u8; 1500];
            
            while UDP_RECEIVER_RUNNING.load(Ordering::SeqCst) {
                match socket.recv_from(&mut buf) {
                    Ok((len, _addr)) => {
                        if len < 12 || &buf[0..2] != b"SC" {
                            continue;
                        }
                        
                        let seq = u32::from_be_bytes([buf[2], buf[3], buf[4], buf[5]]);
                        let idx = u16::from_be_bytes([buf[6], buf[7]]) as usize;
                        let total = u16::from_be_bytes([buf[8], buf[9]]) as usize;
                        let chunk_len = u16::from_be_bytes([buf[10], buf[11]]) as usize;
                        
                        if 12 + chunk_len > len {
                            continue;
                        }
                        
                        let chunk = &buf[12..12 + chunk_len];
                        
                        let fb = frame_buffers.entry(seq).or_insert_with(|| FrameBuffer::new(total));
                        
                        if fb.add_chunk(idx, chunk) {
                            if let Some(frame) = fb.complete() {
                                if let Some(sender) = FRAME_SENDER.lock().as_ref() {
                                    let _ = sender.send(frame);
                                }
                            }
                            frame_buffers.remove(&seq);
                            
                            // Clean old buffers
                            frame_buffers.retain(|&k, _| k > seq.saturating_sub(10));
                        }
                    }
                    Err(_) => {}
                }
            }
        });
    }
    
    Ok(rx)
}

struct FrameBuffer {
    chunks: Vec<Option<Vec<u8>>>,
    received: usize,
    total: usize,
}

impl FrameBuffer {
    fn new(total: usize) -> Self {
        Self {
            chunks: vec![None; total],
            received: 0,
            total,
        }
    }
    
    fn add_chunk(&mut self, idx: usize, data: &[u8]) -> bool {
        if idx < self.total && self.chunks[idx].is_none() {
            self.chunks[idx] = Some(data.to_vec());
            self.received += 1;
        }
        self.received == self.total
    }
    
    fn complete(&self) -> Option<Vec<u8>> {
        if self.received != self.total {
            return None;
        }
        
        let mut result = Vec::new();
        for chunk in &self.chunks {
            if let Some(data) = chunk {
                result.extend_from_slice(data);
            }
        }
        Some(result)
    }
}

// ============== Input Simulation ==============

fn send_event(event_type: &EventType) -> Result<(), String> {
    match simulate(event_type) {
        Ok(()) => {
            thread::sleep(Duration::from_millis(10));
            Ok(())
        }
        Err(SimulateError) => Err(format!("Failed to simulate: {:?}", event_type)),
    }
}

fn js_key_to_rdev(key: &str, code: &str) -> Option<Key> {
    match code {
        "KeyA" => Some(Key::KeyA), "KeyB" => Some(Key::KeyB), "KeyC" => Some(Key::KeyC),
        "KeyD" => Some(Key::KeyD), "KeyE" => Some(Key::KeyE), "KeyF" => Some(Key::KeyF),
        "KeyG" => Some(Key::KeyG), "KeyH" => Some(Key::KeyH), "KeyI" => Some(Key::KeyI),
        "KeyJ" => Some(Key::KeyJ), "KeyK" => Some(Key::KeyK), "KeyL" => Some(Key::KeyL),
        "KeyM" => Some(Key::KeyM), "KeyN" => Some(Key::KeyN), "KeyO" => Some(Key::KeyO),
        "KeyP" => Some(Key::KeyP), "KeyQ" => Some(Key::KeyQ), "KeyR" => Some(Key::KeyR),
        "KeyS" => Some(Key::KeyS), "KeyT" => Some(Key::KeyT), "KeyU" => Some(Key::KeyU),
        "KeyV" => Some(Key::KeyV), "KeyW" => Some(Key::KeyW), "KeyX" => Some(Key::KeyX),
        "KeyY" => Some(Key::KeyY), "KeyZ" => Some(Key::KeyZ),
        "Digit0" => Some(Key::Num0), "Digit1" => Some(Key::Num1), "Digit2" => Some(Key::Num2),
        "Digit3" => Some(Key::Num3), "Digit4" => Some(Key::Num4), "Digit5" => Some(Key::Num5),
        "Digit6" => Some(Key::Num6), "Digit7" => Some(Key::Num7), "Digit8" => Some(Key::Num8),
        "Digit9" => Some(Key::Num9),
        "F1" => Some(Key::F1), "F2" => Some(Key::F2), "F3" => Some(Key::F3), "F4" => Some(Key::F4),
        "F5" => Some(Key::F5), "F6" => Some(Key::F6), "F7" => Some(Key::F7), "F8" => Some(Key::F8),
        "F9" => Some(Key::F9), "F10" => Some(Key::F10), "F11" => Some(Key::F11), "F12" => Some(Key::F12),
        "Enter" | "NumpadEnter" => Some(Key::Return),
        "Escape" => Some(Key::Escape), "Backspace" => Some(Key::Backspace),
        "Tab" => Some(Key::Tab), "Space" => Some(Key::Space),
        "Delete" => Some(Key::Delete), "Insert" => Some(Key::Insert),
        "Home" => Some(Key::Home), "End" => Some(Key::End),
        "PageUp" => Some(Key::PageUp), "PageDown" => Some(Key::PageDown),
        "ArrowUp" => Some(Key::UpArrow), "ArrowDown" => Some(Key::DownArrow),
        "ArrowLeft" => Some(Key::LeftArrow), "ArrowRight" => Some(Key::RightArrow),
        "ShiftLeft" | "ShiftRight" => Some(Key::ShiftLeft),
        "ControlLeft" | "ControlRight" => Some(Key::ControlLeft),
        "AltLeft" | "AltRight" => Some(Key::Alt),
        "MetaLeft" | "MetaRight" => Some(Key::MetaLeft),
        _ => if key == " " { Some(Key::Space) } else { None }
    }
}

// ============== Tauri Commands ==============

#[tauri::command]
fn capture_screen() -> Result<String, String> {
    let (bgra, width, height) = capture_frame_bgra()?;
    let jpeg = bgra_to_jpeg(&bgra, width, height, 50)?;
    let base64_str = general_purpose::STANDARD.encode(&jpeg);
    Ok(format!("data:image/jpeg;base64,{}", base64_str))
}

#[tauri::command]
fn start_capture_loop(app: tauri::AppHandle, interval_ms: u64) {
    if CAPTURING.load(Ordering::SeqCst) {
        return;
    }
    CAPTURING.store(true, Ordering::SeqCst);
    
    thread::spawn(move || {
        while CAPTURING.load(Ordering::SeqCst) {
            if let Ok(data) = capture_screen() {
                let _ = app.emit("screen-frame", data);
            }
            thread::sleep(Duration::from_millis(interval_ms));
        }
    });
}

#[tauri::command]
fn stop_capture_loop() {
    CAPTURING.store(false, Ordering::SeqCst);
}

/// Start UDP streaming to server (for client)
#[tauri::command]
fn start_stream(server_addr: String, fps: u32) -> Result<(), String> {
    start_udp_stream(&server_addr, fps)
}

#[tauri::command]
fn stop_stream() {
    STREAMING.store(false, Ordering::SeqCst);
}

/// Start frame receiver server (for admin)
#[tauri::command]
fn start_frame_receiver(app: tauri::AppHandle, port: u16) -> Result<(), String> {
    let mut rx = start_frame_server(port)?;
    
    thread::spawn(move || {
        loop {
            match rx.blocking_recv() {
                Ok(frame) => {
                    let base64_str = general_purpose::STANDARD.encode(&frame);
                    let data_url = format!("data:image/jpeg;base64,{}", base64_str);
                    let _ = app.emit("udp-frame", data_url);
                }
                Err(_) => {
                    thread::sleep(Duration::from_millis(10));
                }
            }
        }
    });
    
    Ok(())
}

#[tauri::command]
fn stop_frame_receiver() {
    UDP_RECEIVER_RUNNING.store(false, Ordering::SeqCst);
}

#[tauri::command]
fn get_screen_size() -> Result<serde_json::Value, String> {
    match rdev::display_size() {
        Ok((width, height)) => Ok(serde_json::json!({ "width": width, "height": height })),
        Err(_) => {
            let display = Display::primary().map_err(|e| e.to_string())?;
            Ok(serde_json::json!({ "width": display.width(), "height": display.height() }))
        }
    }
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

#[tauri::command]
fn remote_mouse_move(x: f64, y: f64) -> Result<(), String> {
    send_event(&EventType::MouseMove { x, y })
}

#[tauri::command]
fn remote_mouse_click(button: String) -> Result<(), String> {
    let btn = match button.as_str() {
        "right" => Button::Right,
        "middle" => Button::Middle,
        _ => Button::Left,
    };
    send_event(&EventType::ButtonPress(btn))?;
    send_event(&EventType::ButtonRelease(btn))
}

#[tauri::command]
fn remote_mouse_scroll(delta_x: i64, delta_y: i64) -> Result<(), String> {
    send_event(&EventType::Wheel { delta_x, delta_y })
}

#[tauri::command]
fn remote_key_press(key: String, code: String, ctrl: bool, alt: bool, shift: bool, meta: bool) -> Result<(), String> {
    if ctrl { send_event(&EventType::KeyPress(Key::ControlLeft))?; }
    if alt { send_event(&EventType::KeyPress(Key::Alt))?; }
    if shift { send_event(&EventType::KeyPress(Key::ShiftLeft))?; }
    if meta { send_event(&EventType::KeyPress(Key::MetaLeft))?; }

    if let Some(rdev_key) = js_key_to_rdev(&key, &code) {
        send_event(&EventType::KeyPress(rdev_key))?;
        send_event(&EventType::KeyRelease(rdev_key))?;
    }

    if meta { send_event(&EventType::KeyRelease(Key::MetaLeft))?; }
    if shift { send_event(&EventType::KeyRelease(Key::ShiftLeft))?; }
    if alt { send_event(&EventType::KeyRelease(Key::Alt))?; }
    if ctrl { send_event(&EventType::KeyRelease(Key::ControlLeft))?; }
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
            start_stream,
            stop_stream,
            start_frame_receiver,
            stop_frame_receiver,
            get_screen_size,
            set_lock_screen,
            remote_mouse_move,
            remote_mouse_click,
            remote_mouse_scroll,
            remote_key_press
        ])
        .setup(|_app| {
            #[cfg(debug_assertions)]
            {
                let window = _app.get_webview_window("main").unwrap();
                window.open_devtools();
            }
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
