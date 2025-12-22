use base64::{engine::general_purpose, Engine};
use openh264::encoder::{Encoder, EncoderConfig, BitRate, FrameRate};
use openh264::formats::YUVBuffer;
use parking_lot::Mutex;
use rdev::{simulate, Button, EventType, Key, SimulateError};
use scrap::{Capturer, Display};
use std::io::ErrorKind::WouldBlock;
use std::net::{UdpSocket, IpAddr};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::time::{Duration, Instant};
use std::thread;
use tauri::{Emitter, Manager};

// ============== Constants ==============
const STREAM_WIDTH: usize = 640;
const STREAM_HEIGHT: usize = 360;

// ============== Global State ==============
lazy_static::lazy_static! {
    static ref CAPTURING: AtomicBool = AtomicBool::new(false);
    static ref STREAMING: AtomicBool = AtomicBool::new(false);
    static ref UDP_RECEIVER_RUNNING: AtomicBool = AtomicBool::new(false);
    static ref FRAME_COUNT: AtomicU32 = AtomicU32::new(0);
    static ref LAST_H264_FRAME: Mutex<Option<Vec<u8>>> = Mutex::new(None);
    static ref LAST_JPEG_FRAME: Mutex<Option<Vec<u8>>> = Mutex::new(None);
}

// ============== Screen Capture ==============
struct ScreenCapturer {
    capturer: Capturer,
    width: usize,
    height: usize,
}

impl ScreenCapturer {
    fn new() -> Result<Self, String> {
        let display = Display::primary().map_err(|e| format!("No display: {}", e))?;
        let width = display.width();
        let height = display.height();
        let capturer = Capturer::new(display).map_err(|e| format!("Capturer error: {}", e))?;
        Ok(Self { capturer, width, height })
    }

    fn capture(&mut self) -> Option<Vec<u8>> {
        match self.capturer.frame() {
            Ok(frame) => Some(frame.to_vec()),
            Err(ref e) if e.kind() == WouldBlock => None,
            Err(_) => None,
        }
    }
}

// ============== H.264 Encoder ==============
struct H264Encoder {
    encoder: Encoder,
    width: usize,
    height: usize,
    frame_count: u32,
}

impl H264Encoder {
    fn new(width: usize, height: usize) -> Result<Self, String> {
        let config = EncoderConfig::new()
            .bitrate(BitRate::from_bps(500_000)) // 500 kbps
            .max_frame_rate(FrameRate::from_hz(30.0));
        
        let encoder = Encoder::with_api_config(
            openh264::OpenH264API::from_source(),
            config
        ).map_err(|e| format!("H264 encoder error: {:?}", e))?;
        
        Ok(Self {
            encoder,
            width,
            height,
            frame_count: 0,
        })
    }

    fn encode(&mut self, bgra: &[u8], src_width: usize, src_height: usize) -> Option<Vec<u8>> {
        // Resize and convert BGRA to YUV420
        let yuv = bgra_to_yuv420_resized(bgra, src_width, src_height, self.width, self.height)?;
        
        let yuv_buf = YUVBuffer::from_vec(yuv, self.width, self.height);
        
        // Encode to H.264
        let bitstream = self.encoder.encode(&yuv_buf).ok()?;
        
        // Get raw H.264 data
        let h264_data = bitstream.to_vec();
        
        self.frame_count += 1;
        
        if !h264_data.is_empty() {
            Some(h264_data)
        } else {
            None
        }
    }
}

// BGRA to YUV420 with resize
fn bgra_to_yuv420_resized(
    bgra: &[u8], 
    src_w: usize, 
    src_h: usize, 
    dst_w: usize, 
    dst_h: usize
) -> Option<Vec<u8>> {
    let stride = bgra.len() / src_h;
    let scale_x = src_w as f32 / dst_w as f32;
    let scale_y = src_h as f32 / dst_h as f32;
    
    let y_size = dst_w * dst_h;
    let uv_size = (dst_w / 2) * (dst_h / 2);
    let mut yuv = vec![0u8; y_size + uv_size * 2];
    
    let (y_plane, uv_planes) = yuv.split_at_mut(y_size);
    let (u_plane, v_plane) = uv_planes.split_at_mut(uv_size);
    
    // Convert to Y plane
    for y in 0..dst_h {
        let src_y = (y as f32 * scale_y) as usize;
        for x in 0..dst_w {
            let src_x = (x as f32 * scale_x) as usize;
            let i = src_y * stride + src_x * 4;
            
            if i + 2 < bgra.len() {
                let b = bgra[i] as i32;
                let g = bgra[i + 1] as i32;
                let r = bgra[i + 2] as i32;
                
                // RGB to Y
                let y_val = ((66 * r + 129 * g + 25 * b + 128) >> 8) + 16;
                y_plane[y * dst_w + x] = y_val.clamp(0, 255) as u8;
            }
        }
    }
    
    // Convert to U and V planes (subsampled 2x2)
    for y in 0..(dst_h / 2) {
        let src_y = ((y * 2) as f32 * scale_y) as usize;
        for x in 0..(dst_w / 2) {
            let src_x = ((x * 2) as f32 * scale_x) as usize;
            let i = src_y * stride + src_x * 4;
            
            if i + 2 < bgra.len() {
                let b = bgra[i] as i32;
                let g = bgra[i + 1] as i32;
                let r = bgra[i + 2] as i32;
                
                // RGB to U, V
                let u_val = ((-38 * r - 74 * g + 112 * b + 128) >> 8) + 128;
                let v_val = ((112 * r - 94 * g - 18 * b + 128) >> 8) + 128;
                
                let uv_idx = y * (dst_w / 2) + x;
                u_plane[uv_idx] = u_val.clamp(0, 255) as u8;
                v_plane[uv_idx] = v_val.clamp(0, 255) as u8;
            }
        }
    }
    
    Some(yuv)
}

// JPEG encoding for fallback/preview
fn encode_jpeg(bgra: &[u8], src_w: usize, src_h: usize, quality: u8) -> Option<Vec<u8>> {
    let stride = bgra.len() / src_h;
    let dst_w = STREAM_WIDTH;
    let dst_h = STREAM_HEIGHT;
    let scale_x = src_w as f32 / dst_w as f32;
    let scale_y = src_h as f32 / dst_h as f32;
    
    let mut rgb = Vec::with_capacity(dst_w * dst_h * 3);
    
    for y in 0..dst_h {
        let src_y = (y as f32 * scale_y) as usize;
        for x in 0..dst_w {
            let src_x = (x as f32 * scale_x) as usize;
            let i = src_y * stride + src_x * 4;
            if i + 2 < bgra.len() {
                rgb.push(bgra[i + 2]); // R
                rgb.push(bgra[i + 1]); // G
                rgb.push(bgra[i]);     // B
            } else {
                rgb.extend_from_slice(&[0, 0, 0]);
            }
        }
    }
    
    let img = image::RgbImage::from_raw(dst_w as u32, dst_h as u32, rgb)?;
    let mut buffer = std::io::Cursor::new(Vec::with_capacity(50000));
    img.write_to(&mut buffer, image::ImageOutputFormat::Jpeg(quality)).ok()?;
    Some(buffer.into_inner())
}


// ============== H.264 UDP Streaming ==============
fn start_h264_streaming(server_addr: String, fps: u32) -> Result<(), String> {
    if STREAMING.swap(true, Ordering::SeqCst) {
        return Err("Already streaming".to_string());
    }
    
    thread::spawn(move || {
        let socket = match UdpSocket::bind("0.0.0.0:0") {
            Ok(s) => s,
            Err(e) => {
                eprintln!("UDP bind error: {}", e);
                STREAMING.store(false, Ordering::SeqCst);
                return;
            }
        };
        
        let mut capturer = match ScreenCapturer::new() {
            Ok(c) => c,
            Err(e) => {
                eprintln!("Capturer error: {}", e);
                STREAMING.store(false, Ordering::SeqCst);
                return;
            }
        };
        
        let mut encoder = match H264Encoder::new(STREAM_WIDTH, STREAM_HEIGHT) {
            Ok(e) => e,
            Err(e) => {
                eprintln!("H264 encoder error: {}", e);
                STREAMING.store(false, Ordering::SeqCst);
                return;
            }
        };
        
        let frame_interval = Duration::from_micros(1_000_000 / fps as u64);
        let mut sequence: u32 = 0;
        let mut last_frame_time = Instant::now();
        
        println!("H.264 UDP streaming started to {} at {} FPS ({}x{})", 
                 server_addr, fps, STREAM_WIDTH, STREAM_HEIGHT);
        
        let mut encode_errors = 0u32;
        
        while STREAMING.load(Ordering::SeqCst) {
            let now = Instant::now();
            
            if let Some(bgra) = capturer.capture() {
                // Encode to H.264
                if let Some(h264_data) = encoder.encode(&bgra, capturer.width, capturer.height) {
                    // Send via UDP with H264 magic header
                    if send_h264_udp(&socket, &server_addr, &h264_data, sequence).is_ok() {
                        sequence = sequence.wrapping_add(1);
                        FRAME_COUNT.fetch_add(1, Ordering::Relaxed);
                        if sequence % 30 == 0 {
                            println!("Sent {} H.264 frames ({} bytes)", sequence, h264_data.len());
                        }
                    }
                    
                    *LAST_H264_FRAME.lock() = Some(h264_data);
                } else {
                    encode_errors += 1;
                    if encode_errors % 30 == 1 {
                        println!("H.264 encode failed (errors: {})", encode_errors);
                    }
                }
                
                // Also encode JPEG for preview/fallback
                if let Some(jpeg) = encode_jpeg(&bgra, capturer.width, capturer.height, 60) {
                    *LAST_JPEG_FRAME.lock() = Some(jpeg);
                }
                
                let elapsed = now.elapsed();
                if elapsed < frame_interval {
                    thread::sleep(frame_interval - elapsed);
                }
                last_frame_time = Instant::now();
            } else {
                thread::sleep(Duration::from_millis(1));
                
                if last_frame_time.elapsed() > Duration::from_secs(2) {
                    if let Ok(new_capturer) = ScreenCapturer::new() {
                        capturer = new_capturer;
                        last_frame_time = Instant::now();
                    }
                }
            }
        }
        
        println!("H.264 streaming stopped");
    });
    
    Ok(())
}

fn send_h264_udp(socket: &UdpSocket, addr: &str, data: &[u8], sequence: u32) -> Result<(), String> {
    const MAX_PAYLOAD: usize = 1400;
    const HEADER_SIZE: usize = 12;
    
    let chunk_size = MAX_PAYLOAD - HEADER_SIZE;
    let total_chunks = (data.len() + chunk_size - 1) / chunk_size;
    
    for (i, chunk) in data.chunks(chunk_size).enumerate() {
        let mut packet = Vec::with_capacity(HEADER_SIZE + chunk.len());
        
        // Header: magic(2) + type(1) + flags(1) + seq(4) + idx(2) + total(2)
        packet.extend_from_slice(b"H4");  // H.264 magic
        packet.push(if i == 0 { 0x01 } else { 0x00 }); // type: 1=keyframe start
        packet.push(0x00); // flags reserved
        packet.extend_from_slice(&sequence.to_le_bytes());
        packet.extend_from_slice(&(i as u16).to_le_bytes());
        packet.extend_from_slice(&(total_chunks as u16).to_le_bytes());
        packet.extend_from_slice(chunk);
        
        if socket.send_to(&packet, addr).is_err() {
            return Err("Send failed".to_string());
        }
    }
    
    Ok(())
}


// ============== H.264 UDP Receiver ==============
fn start_h264_receiver(app: tauri::AppHandle, port: u16) -> Result<(), String> {
    if UDP_RECEIVER_RUNNING.swap(true, Ordering::SeqCst) {
        return Err("Already running".to_string());
    }
    
    thread::spawn(move || {
        let socket = match UdpSocket::bind(format!("0.0.0.0:{}", port)) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("UDP bind error on port {}: {}", port, e);
                UDP_RECEIVER_RUNNING.store(false, Ordering::SeqCst);
                return;
            }
        };
        
        let _ = socket.set_read_timeout(Some(Duration::from_millis(100)));
        
        let mut frame_buffer = H264FrameAssembler::new();
        let mut buf = [0u8; 1500];
        let mut last_emit = Instant::now();
        let emit_interval = Duration::from_millis(33);
        
        println!("H.264 UDP receiver started on port {}", port);
        
        while UDP_RECEIVER_RUNNING.load(Ordering::SeqCst) {
            match socket.recv_from(&mut buf) {
                Ok((len, addr)) => {
                    if len < 12 {
                        continue;
                    }
                    
                    // Check magic header
                    if &buf[0..2] == b"H4" {
                        // H.264 frame
                        let _frame_type = buf[2];
                        let seq = u32::from_le_bytes([buf[4], buf[5], buf[6], buf[7]]);
                        let idx = u16::from_le_bytes([buf[8], buf[9]]) as usize;
                        let total = u16::from_le_bytes([buf[10], buf[11]]) as usize;
                        let payload = &buf[12..len];
                        
                        if let Some(h264_frame) = frame_buffer.add_chunk(seq, idx, total, payload) {
                            if last_emit.elapsed() >= emit_interval {
                                let base64_str = general_purpose::STANDARD.encode(&h264_frame);
                                let _ = app.emit("h264-frame", (&addr.ip().to_string(), base64_str));
                                last_emit = Instant::now();
                            }
                        }
                    } else if &buf[0..2] == b"SF" {
                        // Legacy JPEG frame (backward compatible)
                        let seq = u32::from_le_bytes([buf[2], buf[3], buf[4], buf[5]]);
                        let idx = u16::from_le_bytes([buf[6], buf[7]]) as usize;
                        let total = u16::from_le_bytes([buf[8], buf[9]]) as usize;
                        let payload = &buf[10..len];
                        
                        if let Some(jpeg_frame) = frame_buffer.add_chunk(seq, idx, total, payload) {
                            if last_emit.elapsed() >= emit_interval {
                                let base64_str = general_purpose::STANDARD.encode(&jpeg_frame);
                                let data_url = format!("data:image/jpeg;base64,{}", base64_str);
                                let _ = app.emit("udp-frame", (&addr.ip().to_string(), data_url));
                                last_emit = Instant::now();
                            }
                        }
                    }
                }
                Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock || 
                             e.kind() == std::io::ErrorKind::TimedOut => {
                    continue;
                }
                Err(_) => {
                    thread::sleep(Duration::from_millis(10));
                }
            }
        }
        
        println!("H.264 receiver stopped");
    });
    
    Ok(())
}

struct H264FrameAssembler {
    current_seq: u32,
    chunks: Vec<Option<Vec<u8>>>,
    total: usize,
    received: usize,
}

impl H264FrameAssembler {
    fn new() -> Self {
        Self {
            current_seq: u32::MAX,
            chunks: Vec::new(),
            total: 0,
            received: 0,
        }
    }
    
    fn add_chunk(&mut self, seq: u32, idx: usize, total: usize, data: &[u8]) -> Option<Vec<u8>> {
        if seq != self.current_seq {
            self.current_seq = seq;
            self.chunks = vec![None; total];
            self.total = total;
            self.received = 0;
        }
        
        if idx < self.total && self.chunks[idx].is_none() {
            self.chunks[idx] = Some(data.to_vec());
            self.received += 1;
        }
        
        if self.received == self.total {
            let mut result = Vec::with_capacity(self.total * 1400);
            for chunk in &self.chunks {
                if let Some(data) = chunk {
                    result.extend_from_slice(data);
                }
            }
            
            self.current_seq = u32::MAX;
            self.chunks.clear();
            self.received = 0;
            
            return Some(result);
        }
        
        None
    }
}


// ============== Input Simulation ==============
fn send_event(event_type: &EventType) -> Result<(), String> {
    match simulate(event_type) {
        Ok(()) => {
            thread::sleep(Duration::from_millis(5));
            Ok(())
        }
        Err(SimulateError) => Err(format!("Failed: {:?}", event_type)),
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
    if let Some(jpeg) = LAST_JPEG_FRAME.lock().clone() {
        let base64_str = general_purpose::STANDARD.encode(&jpeg);
        return Ok(format!("data:image/jpeg;base64,{}", base64_str));
    }
    
    let mut capturer = ScreenCapturer::new()?;
    
    for _ in 0..30 {
        if let Some(bgra) = capturer.capture() {
            if let Some(jpeg) = encode_jpeg(&bgra, capturer.width, capturer.height, 60) {
                let base64_str = general_purpose::STANDARD.encode(&jpeg);
                return Ok(format!("data:image/jpeg;base64,{}", base64_str));
            }
        }
        thread::sleep(Duration::from_millis(10));
    }
    
    Err("Capture timeout".to_string())
}

#[tauri::command]
fn start_capture_loop(app: tauri::AppHandle, interval_ms: u64) {
    if CAPTURING.swap(true, Ordering::SeqCst) {
        return;
    }
    
    thread::spawn(move || {
        let mut capturer = match ScreenCapturer::new() {
            Ok(c) => c,
            Err(_) => {
                CAPTURING.store(false, Ordering::SeqCst);
                return;
            }
        };
        
        let interval = Duration::from_millis(interval_ms);
        
        while CAPTURING.load(Ordering::SeqCst) {
            let start = Instant::now();
            
            if let Some(bgra) = capturer.capture() {
                if let Some(jpeg) = encode_jpeg(&bgra, capturer.width, capturer.height, 60) {
                    let base64_str = general_purpose::STANDARD.encode(&jpeg);
                    let data_url = format!("data:image/jpeg;base64,{}", base64_str);
                    let _ = app.emit("screen-frame", data_url);
                }
            }
            
            let elapsed = start.elapsed();
            if elapsed < interval {
                thread::sleep(interval - elapsed);
            }
        }
    });
}

#[tauri::command]
fn stop_capture_loop() {
    CAPTURING.store(false, Ordering::SeqCst);
}

#[tauri::command]
fn start_stream(server_addr: String, fps: u32) -> Result<(), String> {
    start_h264_streaming(server_addr, fps)
}

#[tauri::command]
fn stop_stream() {
    STREAMING.store(false, Ordering::SeqCst);
}

#[tauri::command]
fn start_frame_receiver(app: tauri::AppHandle, port: u16) -> Result<(), String> {
    start_h264_receiver(app, port)
}

#[tauri::command]
fn stop_frame_receiver() {
    UDP_RECEIVER_RUNNING.store(false, Ordering::SeqCst);
}

#[tauri::command]
fn get_stream_stats() -> serde_json::Value {
    serde_json::json!({
        "streaming": STREAMING.load(Ordering::SeqCst),
        "capturing": CAPTURING.load(Ordering::SeqCst),
        "frames_sent": FRAME_COUNT.load(Ordering::Relaxed),
        "codec": "H.264",
        "resolution": format!("{}x{}", STREAM_WIDTH, STREAM_HEIGHT)
    })
}

#[tauri::command]
fn get_screen_size() -> Result<serde_json::Value, String> {
    let display = Display::primary().map_err(|e| e.to_string())?;
    Ok(serde_json::json!({ "width": display.width(), "height": display.height() }))
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

// ============== LAN Scan ==============
#[tauri::command]
async fn scan_lan(app: tauri::AppHandle) -> Result<Vec<serde_json::Value>, String> {
    use std::net::{IpAddr, Ipv4Addr, TcpStream, SocketAddr};
    use std::sync::Arc;
    use std::sync::atomic::AtomicUsize;
    
    // Get local IP to determine subnet
    let local_ip = local_ip_address::local_ip()
        .map_err(|e| format!("Cannot get local IP: {}", e))?;
    
    let base_ip = match local_ip {
        IpAddr::V4(ip) => {
            let octets = ip.octets();
            format!("{}.{}.{}", octets[0], octets[1], octets[2])
        }
        _ => return Err("IPv6 not supported".to_string()),
    };
    
    println!("Scanning LAN: {}.1-254", base_ip);
    let _ = app.emit("scan-progress", serde_json::json!({ "status": "scanning", "base": base_ip }));
    
    let found_hosts: Arc<parking_lot::Mutex<Vec<serde_json::Value>>> = Arc::new(parking_lot::Mutex::new(Vec::new()));
    let scanned = Arc::new(AtomicUsize::new(0));
    
    let mut handles = vec![];
    
    for i in 1..=254u8 {
        let ip_str = format!("{}.{}", base_ip, i);
        let found = Arc::clone(&found_hosts);
        let count = Arc::clone(&scanned);
        
        let handle = thread::spawn(move || {
            let ip: Ipv4Addr = ip_str.parse().unwrap();
            let addr = SocketAddr::new(IpAddr::V4(ip), 3001); // Check if our app port is open
            
            // Quick TCP connect check with timeout
            let is_online = TcpStream::connect_timeout(&addr, Duration::from_millis(100)).is_ok();
            
            // Also try ping-like check (any common port)
            let has_any_service = if !is_online {
                // Try common ports: 22 (SSH), 445 (SMB), 139 (NetBIOS)
                [22, 445, 139, 80, 443].iter().any(|&port| {
                    let addr = SocketAddr::new(IpAddr::V4(ip), port);
                    TcpStream::connect_timeout(&addr, Duration::from_millis(50)).is_ok()
                })
            } else {
                true
            };
            
            count.fetch_add(1, Ordering::Relaxed);
            
            if is_online || has_any_service {
                let mut hosts = found.lock();
                hosts.push(serde_json::json!({
                    "ip": ip_str,
                    "hasApp": is_online,
                    "online": true
                }));
            }
        });
        
        handles.push(handle);
        
        // Limit concurrent threads
        if handles.len() >= 50 {
            if let Some(h) = handles.pop() {
                let _ = h.join();
            }
        }
    }
    
    // Wait for all threads
    for h in handles {
        let _ = h.join();
    }
    
    let results = found_hosts.lock().clone();
    println!("Scan complete: {} hosts found", results.len());
    let _ = app.emit("scan-progress", serde_json::json!({ "status": "complete", "count": results.len() }));
    
    Ok(results)
}

// ============== Wake-on-LAN ==============
#[tauri::command]
fn wake_on_lan(mac_address: String) -> Result<String, String> {
    // Parse MAC address (formats: AA:BB:CC:DD:EE:FF or AA-BB-CC-DD-EE-FF)
    let mac_str = mac_address.replace("-", ":").to_uppercase();
    let mac_bytes: Vec<u8> = mac_str
        .split(':')
        .map(|s| u8::from_str_radix(s, 16))
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| format!("Invalid MAC address: {}", mac_address))?;
    
    if mac_bytes.len() != 6 {
        return Err(format!("MAC address must have 6 bytes, got {}", mac_bytes.len()));
    }
    
    // Build magic packet: 6 bytes of 0xFF + MAC repeated 16 times
    let mut magic_packet = vec![0xFFu8; 6];
    for _ in 0..16 {
        magic_packet.extend_from_slice(&mac_bytes);
    }
    
    // Send to broadcast address on port 9 (standard WOL port)
    let socket = UdpSocket::bind("0.0.0.0:0")
        .map_err(|e| format!("Cannot create socket: {}", e))?;
    
    socket.set_broadcast(true)
        .map_err(|e| format!("Cannot enable broadcast: {}", e))?;
    
    // Send to multiple broadcast addresses for better compatibility
    let broadcasts = ["255.255.255.255:9", "255.255.255.255:7"];
    
    for addr in broadcasts {
        if let Err(e) = socket.send_to(&magic_packet, addr) {
            println!("WOL send to {} failed: {}", addr, e);
        }
    }
    
    // Also try subnet broadcast
    if let Ok(local_ip) = local_ip_address::local_ip() {
        if let IpAddr::V4(ip) = local_ip {
            let octets = ip.octets();
            let subnet_broadcast = format!("{}.{}.{}.255:9", octets[0], octets[1], octets[2]);
            let _ = socket.send_to(&magic_packet, &subnet_broadcast);
        }
    }
    
    println!("WOL packet sent to {}", mac_address);
    Ok(format!("Wake-on-LAN packet sent to {}", mac_address))
}

// ============== Get Local Network Info ==============
#[tauri::command]
fn get_network_info() -> Result<serde_json::Value, String> {
    let local_ip = local_ip_address::local_ip()
        .map_err(|e| format!("Cannot get local IP: {}", e))?;
    
    let mac = mac_address::get_mac_address()
        .map_err(|e| format!("Cannot get MAC: {}", e))?
        .map(|m| m.to_string())
        .unwrap_or_else(|| "unknown".to_string());
    
    Ok(serde_json::json!({
        "ip": local_ip.to_string(),
        "mac": mac
    }))
}

// ============== File Transfer with Chunk + Resume ==============
use sha2::{Sha256, Digest};
use std::fs::{self, File};
use std::io::{Read, Write, Seek, SeekFrom};
use std::path::PathBuf;
use std::collections::HashMap;

const CHUNK_SIZE: usize = 64 * 1024; // 64KB chunks

lazy_static::lazy_static! {
    // Track ongoing transfers: transfer_id -> TransferState
    static ref TRANSFERS: Mutex<HashMap<String, TransferState>> = Mutex::new(HashMap::new());
}

#[derive(Clone, serde::Serialize)]
struct TransferState {
    file_name: String,
    file_size: u64,
    total_chunks: u32,
    received_chunks: u32,
    file_hash: String,
    temp_path: String,
    completed: bool,
}

// Admin: Read file and prepare for transfer
#[tauri::command]
fn prepare_file_transfer(file_path: String) -> Result<serde_json::Value, String> {
    let path = PathBuf::from(&file_path);
    
    if !path.exists() {
        return Err(format!("File not found: {}", file_path));
    }
    
    let metadata = fs::metadata(&path).map_err(|e| e.to_string())?;
    let file_size = metadata.len();
    let file_name = path.file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("unknown")
        .to_string();
    
    // Calculate file hash
    let mut file = File::open(&path).map_err(|e| e.to_string())?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0u8; CHUNK_SIZE];
    
    loop {
        let bytes_read = file.read(&mut buffer).map_err(|e| e.to_string())?;
        if bytes_read == 0 { break; }
        hasher.update(&buffer[..bytes_read]);
    }
    
    let file_hash = hex::encode(hasher.finalize());
    let total_chunks = ((file_size as usize + CHUNK_SIZE - 1) / CHUNK_SIZE) as u32;
    let transfer_id = format!("{}_{}", file_hash[..16].to_string(), chrono_lite_timestamp());
    
    Ok(serde_json::json!({
        "transfer_id": transfer_id,
        "file_name": file_name,
        "file_path": file_path,
        "file_size": file_size,
        "total_chunks": total_chunks,
        "file_hash": file_hash,
        "chunk_size": CHUNK_SIZE
    }))
}

fn chrono_lite_timestamp() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

// Admin: Read a specific chunk from file
#[tauri::command]
fn read_file_chunk(file_path: String, chunk_index: u32) -> Result<serde_json::Value, String> {
    let mut file = File::open(&file_path).map_err(|e| e.to_string())?;
    let offset = chunk_index as u64 * CHUNK_SIZE as u64;
    
    file.seek(SeekFrom::Start(offset)).map_err(|e| e.to_string())?;
    
    let mut buffer = vec![0u8; CHUNK_SIZE];
    let bytes_read = file.read(&mut buffer).map_err(|e| e.to_string())?;
    buffer.truncate(bytes_read);
    
    let data_base64 = general_purpose::STANDARD.encode(&buffer);
    
    Ok(serde_json::json!({
        "chunk_index": chunk_index,
        "data": data_base64,
        "size": bytes_read
    }))
}

// Client: Initialize file receive
#[tauri::command]
fn init_file_receive(
    app: tauri::AppHandle,
    transfer_id: String,
    file_name: String,
    file_size: u64,
    total_chunks: u32,
    file_hash: String,
    save_dir: String
) -> Result<serde_json::Value, String> {
    let save_path = PathBuf::from(&save_dir);
    if !save_path.exists() {
        fs::create_dir_all(&save_path).map_err(|e| e.to_string())?;
    }
    
    let temp_path = save_path.join(format!("{}.tmp", transfer_id));
    
    // Check if we have partial transfer (for resume)
    let received_chunks = if temp_path.exists() {
        let existing_size = fs::metadata(&temp_path)
            .map(|m| m.len())
            .unwrap_or(0);
        (existing_size / CHUNK_SIZE as u64) as u32
    } else {
        // Create empty temp file
        File::create(&temp_path).map_err(|e| e.to_string())?;
        0
    };
    
    let state = TransferState {
        file_name: file_name.clone(),
        file_size,
        total_chunks,
        received_chunks,
        file_hash,
        temp_path: temp_path.to_string_lossy().to_string(),
        completed: false,
    };
    
    TRANSFERS.lock().insert(transfer_id.clone(), state.clone());
    
    let _ = app.emit("file-transfer-init", serde_json::json!({
        "transfer_id": transfer_id,
        "file_name": file_name,
        "resume_from": received_chunks
    }));
    
    Ok(serde_json::json!({
        "transfer_id": transfer_id,
        "resume_from": received_chunks,
        "status": "ready"
    }))
}

// Client: Receive and save a chunk
#[tauri::command]
fn receive_file_chunk(
    app: tauri::AppHandle,
    transfer_id: String,
    chunk_index: u32,
    data: String // base64
) -> Result<serde_json::Value, String> {
    let mut transfers = TRANSFERS.lock();
    let state = transfers.get_mut(&transfer_id)
        .ok_or_else(|| format!("Transfer not found: {}", transfer_id))?;
    
    // Decode chunk data
    let chunk_data = general_purpose::STANDARD.decode(&data)
        .map_err(|e| format!("Base64 decode error: {}", e))?;
    
    // Write chunk to temp file
    let mut file = fs::OpenOptions::new()
        .write(true)
        .open(&state.temp_path)
        .map_err(|e| e.to_string())?;
    
    let offset = chunk_index as u64 * CHUNK_SIZE as u64;
    file.seek(SeekFrom::Start(offset)).map_err(|e| e.to_string())?;
    file.write_all(&chunk_data).map_err(|e| e.to_string())?;
    
    state.received_chunks = chunk_index + 1;
    
    let progress = (state.received_chunks as f64 / state.total_chunks as f64 * 100.0) as u32;
    
    let _ = app.emit("file-transfer-progress", serde_json::json!({
        "transfer_id": transfer_id,
        "chunk_index": chunk_index,
        "received": state.received_chunks,
        "total": state.total_chunks,
        "progress": progress
    }));
    
    Ok(serde_json::json!({
        "chunk_index": chunk_index,
        "received": state.received_chunks,
        "progress": progress
    }))
}

// Client: Finalize transfer - verify and rename
#[tauri::command]
fn finalize_file_transfer(
    app: tauri::AppHandle,
    transfer_id: String,
    save_dir: String
) -> Result<serde_json::Value, String> {
    let mut transfers = TRANSFERS.lock();
    let state = transfers.get_mut(&transfer_id)
        .ok_or_else(|| format!("Transfer not found: {}", transfer_id))?;
    
    // Verify file hash
    let mut file = File::open(&state.temp_path).map_err(|e| e.to_string())?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0u8; CHUNK_SIZE];
    
    loop {
        let bytes_read = file.read(&mut buffer).map_err(|e| e.to_string())?;
        if bytes_read == 0 { break; }
        hasher.update(&buffer[..bytes_read]);
    }
    
    let computed_hash = hex::encode(hasher.finalize());
    
    if computed_hash != state.file_hash {
        return Err(format!("Hash mismatch! Expected: {}, Got: {}", state.file_hash, computed_hash));
    }
    
    // Rename temp file to final name
    let final_path = PathBuf::from(&save_dir).join(&state.file_name);
    fs::rename(&state.temp_path, &final_path).map_err(|e| e.to_string())?;
    
    state.completed = true;
    
    let _ = app.emit("file-transfer-complete", serde_json::json!({
        "transfer_id": transfer_id,
        "file_name": state.file_name,
        "file_path": final_path.to_string_lossy(),
        "file_size": state.file_size
    }));
    
    // Cleanup
    transfers.remove(&transfer_id);
    
    Ok(serde_json::json!({
        "status": "complete",
        "file_path": final_path.to_string_lossy()
    }))
}

// Client: Get transfer status (for resume)
#[tauri::command]
fn get_transfer_status(transfer_id: String) -> Result<serde_json::Value, String> {
    let transfers = TRANSFERS.lock();
    
    if let Some(state) = transfers.get(&transfer_id) {
        Ok(serde_json::json!({
            "found": true,
            "received_chunks": state.received_chunks,
            "total_chunks": state.total_chunks,
            "completed": state.completed
        }))
    } else {
        Ok(serde_json::json!({
            "found": false
        }))
    }
}

// Client: Cancel and cleanup transfer
#[tauri::command]
fn cancel_file_transfer(transfer_id: String) -> Result<(), String> {
    let mut transfers = TRANSFERS.lock();
    
    if let Some(state) = transfers.remove(&transfer_id) {
        // Delete temp file
        let _ = fs::remove_file(&state.temp_path);
    }
    
    Ok(())
}

// ============== Direct TCP File Transfer ==============
use std::net::{TcpListener, TcpStream, SocketAddr};
use std::io::{BufReader, BufWriter};

const TCP_FILE_PORT: u16 = 3003;
const TCP_CHUNK_SIZE: usize = 256 * 1024; // 256KB for TCP (larger than UDP)

lazy_static::lazy_static! {
    static ref TCP_SERVER_RUNNING: AtomicBool = AtomicBool::new(false);
    static ref TCP_TRANSFER_ACTIVE: AtomicBool = AtomicBool::new(false);
}

#[derive(Clone, serde::Serialize)]
struct TcpTransferProgress {
    transfer_id: String,
    bytes_transferred: u64,
    total_bytes: u64,
    progress: u32,
}

// Client: Start TCP server to receive file
#[tauri::command]
fn start_tcp_file_server(
    app: tauri::AppHandle,
    transfer_id: String,
    file_name: String,
    file_size: u64,
    file_hash: String,
    save_dir: String
) -> Result<u16, String> {
    if TCP_SERVER_RUNNING.swap(true, Ordering::SeqCst) {
        return Err("TCP server already running".to_string());
    }
    
    let listener = TcpListener::bind(format!("0.0.0.0:{}", TCP_FILE_PORT))
        .map_err(|e| {
            TCP_SERVER_RUNNING.store(false, Ordering::SeqCst);
            format!("Cannot bind TCP port {}: {}", TCP_FILE_PORT, e)
        })?;
    
    let port = listener.local_addr().map(|a| a.port()).unwrap_or(TCP_FILE_PORT);
    
    println!("TCP file server started on port {}", port);
    
    thread::spawn(move || {
        // Set timeout for accept
        let _ = listener.set_nonblocking(false);
        
        // Wait for connection (timeout 60s)
        let accept_result = listener.accept();
        
        match accept_result {
            Ok((stream, addr)) => {
                println!("TCP connection from: {}", addr);
                
                if let Err(e) = receive_file_via_tcp(
                    &app,
                    stream,
                    &transfer_id,
                    &file_name,
                    file_size,
                    &file_hash,
                    &save_dir
                ) {
                    eprintln!("TCP receive error: {}", e);
                    let _ = app.emit("tcp-transfer-error", serde_json::json!({
                        "transfer_id": transfer_id,
                        "error": e
                    }));
                }
            }
            Err(e) => {
                eprintln!("TCP accept error: {}", e);
                let _ = app.emit("tcp-transfer-error", serde_json::json!({
                    "transfer_id": transfer_id,
                    "error": format!("Accept failed: {}", e)
                }));
            }
        }
        
        TCP_SERVER_RUNNING.store(false, Ordering::SeqCst);
        println!("TCP file server stopped");
    });
    
    Ok(port)
}

fn receive_file_via_tcp(
    app: &tauri::AppHandle,
    stream: TcpStream,
    transfer_id: &str,
    file_name: &str,
    file_size: u64,
    expected_hash: &str,
    save_dir: &str
) -> Result<(), String> {
    let _ = stream.set_read_timeout(Some(Duration::from_secs(30)));
    
    let save_path = PathBuf::from(save_dir);
    let temp_path = save_path.join(format!("{}.tmp", transfer_id));
    let final_path = save_path.join(file_name);
    
    // Check for resume
    let resume_offset = if temp_path.exists() {
        fs::metadata(&temp_path).map(|m| m.len()).unwrap_or(0)
    } else {
        0
    };
    
    // Open file for writing (append if resuming)
    let mut file = fs::OpenOptions::new()
        .create(true)
        .write(true)
        .append(resume_offset > 0)
        .open(&temp_path)
        .map_err(|e| format!("Cannot create temp file: {}", e))?;
    
    if resume_offset == 0 {
        file.set_len(0).map_err(|e| e.to_string())?;
    }
    
    let mut reader = BufReader::with_capacity(TCP_CHUNK_SIZE, stream);
    let mut buffer = vec![0u8; TCP_CHUNK_SIZE];
    let mut bytes_received = resume_offset;
    let mut last_progress = 0u32;
    
    // Send resume offset to sender
    // (Protocol: first 8 bytes from client = resume offset)
    // Actually, we receive data, so we need to tell sender where to start
    // This is handled by signaling via Socket.IO
    
    println!("Receiving file: {} ({} bytes, resume from {})", file_name, file_size, resume_offset);
    
    while bytes_received < file_size {
        let to_read = std::cmp::min(TCP_CHUNK_SIZE, (file_size - bytes_received) as usize);
        
        match reader.read(&mut buffer[..to_read]) {
            Ok(0) => {
                // Connection closed
                if bytes_received < file_size {
                    return Err(format!("Connection closed early: {}/{} bytes", bytes_received, file_size));
                }
                break;
            }
            Ok(n) => {
                file.write_all(&buffer[..n]).map_err(|e| e.to_string())?;
                bytes_received += n as u64;
                
                let progress = (bytes_received as f64 / file_size as f64 * 100.0) as u32;
                
                // Emit progress every 5%
                if progress >= last_progress + 5 || bytes_received == file_size {
                    let _ = app.emit("tcp-transfer-progress", TcpTransferProgress {
                        transfer_id: transfer_id.to_string(),
                        bytes_transferred: bytes_received,
                        total_bytes: file_size,
                        progress,
                    });
                    last_progress = progress;
                }
            }
            Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock || 
                         e.kind() == std::io::ErrorKind::TimedOut => {
                // Timeout - save progress and return error for resume
                file.flush().map_err(|e| e.to_string())?;
                return Err(format!("Timeout at {}/{} bytes - can resume", bytes_received, file_size));
            }
            Err(e) => {
                file.flush().map_err(|e| e.to_string())?;
                return Err(format!("Read error: {} at {}/{} bytes", e, bytes_received, file_size));
            }
        }
    }
    
    file.flush().map_err(|e| e.to_string())?;
    drop(file);
    
    // Verify hash
    let mut verify_file = File::open(&temp_path).map_err(|e| e.to_string())?;
    let mut hasher = Sha256::new();
    let mut verify_buf = vec![0u8; TCP_CHUNK_SIZE];
    
    loop {
        let n = verify_file.read(&mut verify_buf).map_err(|e| e.to_string())?;
        if n == 0 { break; }
        hasher.update(&verify_buf[..n]);
    }
    
    let computed_hash = hex::encode(hasher.finalize());
    
    if computed_hash != expected_hash {
        return Err(format!("Hash mismatch! Expected: {}, Got: {}", expected_hash, computed_hash));
    }
    
    // Rename to final path
    fs::rename(&temp_path, &final_path).map_err(|e| e.to_string())?;
    
    let _ = app.emit("tcp-transfer-complete", serde_json::json!({
        "transfer_id": transfer_id,
        "file_name": file_name,
        "file_path": final_path.to_string_lossy(),
        "file_size": file_size
    }));
    
    println!("File received successfully: {}", final_path.display());
    
    Ok(())
}

// Admin: Send file directly to client via TCP
#[tauri::command]
async fn send_file_tcp(
    app: tauri::AppHandle,
    transfer_id: String,
    file_path: String,
    client_ip: String,
    client_port: u16,
    resume_offset: u64
) -> Result<(), String> {
    if TCP_TRANSFER_ACTIVE.swap(true, Ordering::SeqCst) {
        return Err("Another TCP transfer is active".to_string());
    }
    
    let app_clone = app.clone();
    let transfer_id_clone = transfer_id.clone();
    
    thread::spawn(move || {
        let result = send_file_via_tcp(
            &app_clone,
            &transfer_id_clone,
            &file_path,
            &client_ip,
            client_port,
            resume_offset
        );
        
        if let Err(e) = result {
            eprintln!("TCP send error: {}", e);
            let _ = app_clone.emit("tcp-send-error", serde_json::json!({
                "transfer_id": transfer_id_clone,
                "error": e
            }));
        }
        
        TCP_TRANSFER_ACTIVE.store(false, Ordering::SeqCst);
    });
    
    Ok(())
}

fn send_file_via_tcp(
    app: &tauri::AppHandle,
    transfer_id: &str,
    file_path: &str,
    client_ip: &str,
    client_port: u16,
    resume_offset: u64
) -> Result<(), String> {
    let addr = format!("{}:{}", client_ip, client_port);
    
    println!("Connecting to {} for file transfer...", addr);
    
    let stream = TcpStream::connect_timeout(
        &addr.parse::<SocketAddr>().map_err(|e| e.to_string())?,
        Duration::from_secs(10)
    ).map_err(|e| format!("Cannot connect to {}: {}", addr, e))?;
    
    let _ = stream.set_write_timeout(Some(Duration::from_secs(30)));
    let _ = stream.set_nodelay(true); // Disable Nagle for better throughput
    
    let mut file = File::open(file_path).map_err(|e| e.to_string())?;
    let file_size = file.metadata().map_err(|e| e.to_string())?.len();
    
    // Seek to resume position
    if resume_offset > 0 {
        file.seek(SeekFrom::Start(resume_offset)).map_err(|e| e.to_string())?;
        println!("Resuming from offset: {}", resume_offset);
    }
    
    let mut writer = BufWriter::with_capacity(TCP_CHUNK_SIZE, stream);
    let mut buffer = vec![0u8; TCP_CHUNK_SIZE];
    let mut bytes_sent = resume_offset;
    let mut last_progress = 0u32;
    
    println!("Sending file: {} ({} bytes)", file_path, file_size);
    
    while bytes_sent < file_size {
        let n = file.read(&mut buffer).map_err(|e| e.to_string())?;
        if n == 0 { break; }
        
        writer.write_all(&buffer[..n]).map_err(|e| format!("Write error: {}", e))?;
        bytes_sent += n as u64;
        
        let progress = (bytes_sent as f64 / file_size as f64 * 100.0) as u32;
        
        // Emit progress every 5%
        if progress >= last_progress + 5 || bytes_sent == file_size {
            let _ = app.emit("tcp-send-progress", TcpTransferProgress {
                transfer_id: transfer_id.to_string(),
                bytes_transferred: bytes_sent,
                total_bytes: file_size,
                progress,
            });
            last_progress = progress;
        }
    }
    
    writer.flush().map_err(|e| e.to_string())?;
    
    let _ = app.emit("tcp-send-complete", serde_json::json!({
        "transfer_id": transfer_id,
        "bytes_sent": bytes_sent
    }));
    
    println!("File sent successfully: {} bytes", bytes_sent);
    
    Ok(())
}

// Stop TCP server (for cleanup)
#[tauri::command]
fn stop_tcp_file_server() {
    TCP_SERVER_RUNNING.store(false, Ordering::SeqCst);
}

// Get TCP transfer status
#[tauri::command]
fn get_tcp_transfer_status() -> serde_json::Value {
    serde_json::json!({
        "server_running": TCP_SERVER_RUNNING.load(Ordering::SeqCst),
        "transfer_active": TCP_TRANSFER_ACTIVE.load(Ordering::SeqCst)
    })
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .invoke_handler(tauri::generate_handler![
            capture_screen,
            start_capture_loop,
            stop_capture_loop,
            start_stream,
            stop_stream,
            start_frame_receiver,
            stop_frame_receiver,
            get_stream_stats,
            get_screen_size,
            set_lock_screen,
            remote_mouse_move,
            remote_mouse_click,
            remote_mouse_scroll,
            remote_key_press,
            scan_lan,
            wake_on_lan,
            get_network_info,
            // File transfer (Socket.IO)
            prepare_file_transfer,
            read_file_chunk,
            init_file_receive,
            receive_file_chunk,
            finalize_file_transfer,
            get_transfer_status,
            cancel_file_transfer,
            // Direct TCP file transfer
            start_tcp_file_server,
            send_file_tcp,
            stop_tcp_file_server,
            get_tcp_transfer_status
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
