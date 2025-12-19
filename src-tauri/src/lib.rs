use base64::{engine::general_purpose, Engine};
use enigo::{Enigo, Keyboard, Mouse, Settings};
use openh264::encoder::Encoder;
use openh264::formats::YUVBuffer;
use screenshots::Screen;
use std::io::Cursor;
use std::net::UdpSocket;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use tauri::{Emitter, Manager};

static CAPTURING: AtomicBool = AtomicBool::new(false);
static STREAMING: AtomicBool = AtomicBool::new(false);

// H.264 Encoder wrapper
struct H264Encoder {
    encoder: Encoder,
    width: usize,
    height: usize,
}

impl H264Encoder {
    fn new(width: usize, height: usize) -> Result<Self, String> {
        let encoder = Encoder::new().map_err(|e| e.to_string())?;
        Ok(Self { encoder, width, height })
    }

    fn encode_frame(&mut self, rgba_data: &[u8]) -> Result<Vec<u8>, String> {
        let yuv = Self::rgba_to_yuv420(rgba_data, self.width, self.height);
        let yuv_buffer = YUVBuffer::from_vec(yuv, self.width, self.height);
        
        let bitstream = self.encoder
            .encode(&yuv_buffer)
            .map_err(|e| e.to_string())?;
        
        Ok(bitstream.to_vec())
    }

    fn rgba_to_yuv420(rgba: &[u8], width: usize, height: usize) -> Vec<u8> {
        let mut yuv = vec![0u8; width * height * 3 / 2];
        
        let (y_plane, uv_planes) = yuv.split_at_mut(width * height);
        let (u_plane, v_plane) = uv_planes.split_at_mut(width * height / 4);

        for y in 0..height {
            for x in 0..width {
                let idx = (y * width + x) * 4;
                let r = rgba[idx] as f32;
                let g = rgba[idx + 1] as f32;
                let b = rgba[idx + 2] as f32;

                // RGB to YUV conversion (BT.601)
                let y_val = (0.299 * r + 0.587 * g + 0.114 * b) as u8;
                y_plane[y * width + x] = y_val;

                // Subsample U and V (2x2 blocks)
                if y % 2 == 0 && x % 2 == 0 {
                    let u_val = (128.0 - 0.169 * r - 0.331 * g + 0.5 * b) as u8;
                    let v_val = (128.0 + 0.5 * r - 0.419 * g - 0.081 * b) as u8;
                    let uv_idx = (y / 2) * (width / 2) + (x / 2);
                    u_plane[uv_idx] = u_val;
                    v_plane[uv_idx] = v_val;
                }
            }
        }
        yuv
    }
}

// UDP Streamer for video frames
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

    fn send_frame(&mut self, h264_data: &[u8]) -> Result<(), String> {
        const MAX_PACKET_SIZE: usize = 1400; // MTU safe size
        
        let chunks: Vec<&[u8]> = h264_data.chunks(MAX_PACKET_SIZE - 8).collect();
        let total_chunks = chunks.len() as u16;

        for (i, chunk) in chunks.iter().enumerate() {
            let mut packet = Vec::with_capacity(chunk.len() + 8);
            // Header: sequence(4) + chunk_index(2) + total_chunks(2)
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

// Global state for streaming
lazy_static::lazy_static! {
    static ref ENCODER: Arc<Mutex<Option<H264Encoder>>> = Arc::new(Mutex::new(None));
    static ref STREAMER: Arc<Mutex<Option<UdpStreamer>>> = Arc::new(Mutex::new(None));
}

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
fn capture_screen_h264() -> Result<Vec<u8>, String> {
    let screens = Screen::all().map_err(|e| e.to_string())?;
    let screen = screens.first().ok_or("No screen found")?;
    let image = screen.capture().map_err(|e| e.to_string())?;

    let width = 640usize;
    let height = (640.0 * image.height() as f32 / image.width() as f32) as usize;
    let height = height - (height % 2); // Must be even for YUV420

    let resized = image::imageops::resize(
        &image,
        width as u32,
        height as u32,
        image::imageops::FilterType::Nearest,
    );

    let rgba_data: Vec<u8> = resized.into_raw();

    let mut encoder_guard = ENCODER.lock().map_err(|e| e.to_string())?;
    if encoder_guard.is_none() {
        *encoder_guard = Some(H264Encoder::new(width, height)?);
    }

    let encoder = encoder_guard.as_mut().unwrap();
    encoder.encode_frame(&rgba_data)
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

// Start H.264 UDP streaming
#[tauri::command]
fn start_h264_stream(app: tauri::AppHandle, target_addr: String, fps: u32) -> Result<(), String> {
    if STREAMING.load(Ordering::SeqCst) {
        return Err("Already streaming".to_string());
    }

    // Initialize streamer
    {
        let mut streamer_guard = STREAMER.lock().map_err(|e| e.to_string())?;
        *streamer_guard = Some(UdpStreamer::new(&target_addr)?);
    }

    STREAMING.store(true, Ordering::SeqCst);
    let interval = std::time::Duration::from_millis(1000 / fps as u64);

    std::thread::spawn(move || {
        while STREAMING.load(Ordering::SeqCst) {
            match capture_screen_h264() {
                Ok(h264_data) => {
                    if let Ok(mut streamer_guard) = STREAMER.lock() {
                        if let Some(streamer) = streamer_guard.as_mut() {
                            if let Err(e) = streamer.send_frame(&h264_data) {
                                let _ = app.emit("stream-error", e);
                            }
                        }
                    }
                }
                Err(e) => {
                    let _ = app.emit("stream-error", e);
                }
            }
            std::thread::sleep(interval);
        }
    });

    Ok(())
}

#[tauri::command]
fn stop_h264_stream() {
    STREAMING.store(false, Ordering::SeqCst);
    if let Ok(mut streamer_guard) = STREAMER.lock() {
        *streamer_guard = None;
    }
}

// Get streaming stats
#[tauri::command]
fn get_stream_stats() -> Result<serde_json::Value, String> {
    let is_streaming = STREAMING.load(Ordering::SeqCst);
    let sequence = if let Ok(guard) = STREAMER.lock() {
        guard.as_ref().map(|s| s.sequence).unwrap_or(0)
    } else {
        0
    };

    Ok(serde_json::json!({
        "streaming": is_streaming,
        "frames_sent": sequence
    }))
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
            capture_screen_h264,
            start_capture_loop,
            stop_capture_loop,
            start_h264_stream,
            stop_h264_stream,
            get_stream_stats,
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
