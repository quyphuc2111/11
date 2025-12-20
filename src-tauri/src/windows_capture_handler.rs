// Windows-specific screen capture using windows-capture crate
// This module is only compiled on Windows

#![cfg(target_os = "windows")]

use base64::{engine::general_purpose, Engine};
use std::io::Cursor;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use tauri::Emitter;
use windows_capture::{
    capture::{Context, GraphicsCaptureApiHandler},
    frame::Frame,
    graphics_capture_api::InternalCaptureControl,
    monitor::Monitor,
    settings::{
        ColorFormat, CursorCaptureSettings, DrawBorderSettings, Settings,
        SecondaryWindowSettings, MinimumUpdateIntervalSettings, DirtyRegionSettings,
    },
};

// Shared state
lazy_static::lazy_static! {
    pub static ref WC_CAPTURING: AtomicBool = AtomicBool::new(false);
    pub static ref LAST_FRAME: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
}

/// Screen capture handler for continuous streaming
pub struct StreamingCapture {
    app_handle: tauri::AppHandle,
    frame_count: u64,
}

impl GraphicsCaptureApiHandler for StreamingCapture {
    type Flags = tauri::AppHandle;
    type Error = Box<dyn std::error::Error + Send + Sync>;

    fn new(ctx: Context<Self::Flags>) -> Result<Self, Self::Error> {
        Ok(Self {
            app_handle: ctx.flags,
            frame_count: 0,
        })
    }

    fn on_frame_arrived(
        &mut self,
        frame: &mut Frame,
        capture_control: InternalCaptureControl,
    ) -> Result<(), Self::Error> {
        // Check if we should stop
        if !WC_CAPTURING.load(Ordering::SeqCst) {
            capture_control.stop();
            return Ok(());
        }

        self.frame_count += 1;

        // Get frame buffer
        let mut buffer = frame.buffer()?;
        let width = buffer.width();
        let height = buffer.height();
        
        // Get raw BGRA data (Windows uses BGRA)
        let raw_data = buffer.as_raw_buffer();
        
        // Convert BGRA to RGBA
        let mut rgba_data = Vec::with_capacity(raw_data.len());
        for chunk in raw_data.chunks(4) {
            if chunk.len() == 4 {
                rgba_data.push(chunk[2]); // R (was B)
                rgba_data.push(chunk[1]); // G
                rgba_data.push(chunk[0]); // B (was R)
                rgba_data.push(chunk[3]); // A
            }
        }

        // Create image and resize
        if let Some(img) = image::RgbaImage::from_raw(width, height, rgba_data) {
            let target_width = 640u32;
            let target_height = (target_width as f32 * height as f32 / width as f32) as u32;
            
            let resized = image::imageops::resize(
                &img,
                target_width,
                target_height,
                image::imageops::FilterType::Nearest,
            );
            
            // Encode to JPEG
            let mut jpeg_buffer = Cursor::new(Vec::new());
            if resized.write_to(&mut jpeg_buffer, image::ImageOutputFormat::Jpeg(50)).is_ok() {
                let base64_str = general_purpose::STANDARD.encode(jpeg_buffer.into_inner());
                let data_url = format!("data:image/jpeg;base64,{}", base64_str);
                
                // Store last frame
                if let Ok(mut guard) = LAST_FRAME.lock() {
                    *guard = Some(data_url.clone());
                }
                
                // Emit to frontend
                let _ = self.app_handle.emit("screen-frame", data_url);
            }
        }

        Ok(())
    }

    fn on_closed(&mut self) -> Result<(), Self::Error> {
        WC_CAPTURING.store(false, Ordering::SeqCst);
        println!("Windows capture session closed after {} frames", self.frame_count);
        Ok(())
    }
}

/// Start Windows Graphics Capture
pub fn start_capture(app_handle: tauri::AppHandle) -> Result<(), String> {
    if WC_CAPTURING.load(Ordering::SeqCst) {
        return Err("Already capturing".to_string());
    }

    WC_CAPTURING.store(true, Ordering::SeqCst);

    std::thread::spawn(move || {
        let monitor = match Monitor::primary() {
            Ok(m) => m,
            Err(e) => {
                let _ = app_handle.emit("capture-error", format!("Failed to get monitor: {}", e));
                WC_CAPTURING.store(false, Ordering::SeqCst);
                return;
            }
        };

        let settings = Settings::new(
            monitor,
            CursorCaptureSettings::WithCursor,
            DrawBorderSettings::WithoutBorder,
            SecondaryWindowSettings::Default,
            MinimumUpdateIntervalSettings::Default,
            DirtyRegionSettings::Default,
            ColorFormat::Bgra8,
            app_handle,
        );

        match StreamingCapture::start(settings) {
            Ok(_) => println!("Windows capture started successfully"),
            Err(e) => {
                eprintln!("Windows capture error: {}", e);
                WC_CAPTURING.store(false, Ordering::SeqCst);
            }
        }
    });

    Ok(())
}

/// Stop Windows Graphics Capture
pub fn stop_capture() {
    WC_CAPTURING.store(false, Ordering::SeqCst);
}

/// Get the last captured frame
pub fn get_last_frame() -> Option<String> {
    LAST_FRAME.lock().ok().and_then(|guard| guard.clone())
}

/// Single frame capture using windows-capture
pub fn capture_single_frame() -> Result<String, String> {
    use std::sync::mpsc;
    use std::time::Duration;

    let (tx, rx) = mpsc::channel();
    
    struct SingleCapture {
        sender: mpsc::Sender<String>,
    }

    impl GraphicsCaptureApiHandler for SingleCapture {
        type Flags = mpsc::Sender<String>;
        type Error = Box<dyn std::error::Error + Send + Sync>;

        fn new(ctx: Context<Self::Flags>) -> Result<Self, Self::Error> {
            Ok(Self { sender: ctx.flags })
        }

        fn on_frame_arrived(
            &mut self,
            frame: &mut Frame,
            capture_control: InternalCaptureControl,
        ) -> Result<(), Self::Error> {
            let mut buffer = frame.buffer()?;
            let width = buffer.width();
            let height = buffer.height();
            let raw_data = buffer.as_raw_buffer();
            
            // Convert BGRA to RGBA
            let mut rgba_data = Vec::with_capacity(raw_data.len());
            for chunk in raw_data.chunks(4) {
                if chunk.len() == 4 {
                    rgba_data.push(chunk[2]);
                    rgba_data.push(chunk[1]);
                    rgba_data.push(chunk[0]);
                    rgba_data.push(chunk[3]);
                }
            }

            if let Some(img) = image::RgbaImage::from_raw(width, height, rgba_data) {
                let target_width = 640u32;
                let target_height = (target_width as f32 * height as f32 / width as f32) as u32;
                
                let resized = image::imageops::resize(
                    &img,
                    target_width,
                    target_height,
                    image::imageops::FilterType::Nearest,
                );
                
                let mut jpeg_buffer = Cursor::new(Vec::new());
                if resized.write_to(&mut jpeg_buffer, image::ImageOutputFormat::Jpeg(50)).is_ok() {
                    let base64_str = general_purpose::STANDARD.encode(jpeg_buffer.into_inner());
                    let data_url = format!("data:image/jpeg;base64,{}", base64_str);
                    let _ = self.sender.send(data_url);
                }
            }

            capture_control.stop();
            Ok(())
        }

        fn on_closed(&mut self) -> Result<(), Self::Error> {
            Ok(())
        }
    }

    let monitor = Monitor::primary().map_err(|e| e.to_string())?;
    
    std::thread::spawn(move || {
        let settings = Settings::new(
            monitor,
            CursorCaptureSettings::WithCursor,
            DrawBorderSettings::WithoutBorder,
            SecondaryWindowSettings::Default,
            MinimumUpdateIntervalSettings::Default,
            DirtyRegionSettings::Default,
            ColorFormat::Bgra8,
            tx,
        );
        let _ = SingleCapture::start(settings);
    });

    rx.recv_timeout(Duration::from_secs(5))
        .map_err(|_| "Capture timeout".to_string())
}
