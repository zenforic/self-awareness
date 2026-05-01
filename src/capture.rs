use anyhow::Result;
use chrono::Timelike;
use std::path::Path;
use image::{DynamicImage, ImageBuffer, Rgba};

use crate::config::ImageFormat;
use crate::crypto;

/// Capture the screen using GDI BitBlt and save it to the specified directory.
/// If `encrypt` is true, the image is encrypted with AES-256-GCM and saved as `.enc`.
pub fn capture_and_save(
    output_dir: &str,
    format: &ImageFormat,
    encrypt: bool,
    hash_chain: bool,
    mut prev_chain_hash: Option<&mut [u8; 32]>,
) -> Result<()> {
    std::fs::create_dir_all(output_dir)?;

    let (rgba, width, height) = capture_screen()?;
    let img = ImageBuffer::<Rgba<u8>, _>::from_raw(width, height, rgba)
        .ok_or_else(|| anyhow::anyhow!("Failed to create image from screen capture"))?;
    let dyn_img = DynamicImage::ImageRgba8(img);

    let now = chrono::Local::now();
    let timestamp = now.format("%Y%m%d_%H%M%S");
    let millis = now.nanosecond() / 1_000_000;
    let timestamp_ms = now.timestamp_millis();

    if encrypt {
        let key = crypto::load_key()?;
        let encoded = encode_to_bytes(&dyn_img, format)?;
        
        let (encrypted, new_hash) = crypto::encrypt_image(
            &key,
            &encoded,
            *format,
            if hash_chain {
                prev_chain_hash.as_deref().map(|prev| (prev, timestamp_ms))
            } else {
                None
            }
        )?;
        
        if let Some(hash) = new_hash {
            if let Some(prev) = prev_chain_hash.as_deref_mut() {
                *prev = hash;
            }
        }
        
        let filename = format!("{}_{}.{}", timestamp, millis, crypto::ENCRYPTED_EXTENSION);
        let path = Path::new(output_dir).join(filename);
        // Atomic write: write to temp file, then rename
        let tmp_path = path.with_extension("tmp");
        std::fs::write(&tmp_path, &encrypted)?;
        std::fs::rename(&tmp_path, &path)?;
    } else {
        let filename = format!("{}_{}.{}", timestamp, millis, format.extension());
        let path = Path::new(output_dir).join(filename);
        save_image(&dyn_img, &path, format)?;
    }

    Ok(())
}

/// Capture the screen using GDI BitBlt.
/// Returns RGBA pixel data, width, and height.
fn capture_screen() -> Result<(Vec<u8>, u32, u32)> {
    use windows::Win32::Foundation::{CloseHandle, HANDLE};
    use windows::Win32::Graphics::Gdi::{
        BitBlt, CreateCompatibleBitmap, CreateCompatibleDC, DeleteObject,
        GetDIBits, GetDC, ReleaseDC, SelectObject, SRCCOPY, HBITMAP, HGDIOBJ,
    };

    let (width, height): (u32, u32) = unsafe {
        let w = windows::Win32::UI::WindowsAndMessaging::GetSystemMetrics(
            windows::Win32::UI::WindowsAndMessaging::SM_CXSCREEN,
        );
        let h = windows::Win32::UI::WindowsAndMessaging::GetSystemMetrics(
            windows::Win32::UI::WindowsAndMessaging::SM_CYSCREEN,
        );
        (w as u32, h as u32)
    };

    if width == 0 || height == 0 {
        return Err(anyhow::anyhow!("Screen dimensions are zero"));
    }

    let rgba = unsafe {
        // Get desktop DC
        let desktop = windows::Win32::UI::WindowsAndMessaging::GetDesktopWindow();
        let hdc = GetDC(desktop);
        if hdc.0.is_null() {
            return Err(anyhow::anyhow!("GetDC failed"));
        }

        // Create compatible DC and bitmap
        let memdc = CreateCompatibleDC(hdc);
        if memdc.0.is_null() {
            ReleaseDC(desktop, hdc);
            return Err(anyhow::anyhow!("CreateCompatibleDC failed"));
        }

        let bitmap: HBITMAP =
            CreateCompatibleBitmap(hdc, width as i32, height as i32);
        if bitmap.0.is_null() {
            ReleaseDC(desktop, hdc);
            let _ = DeleteObject(HGDIOBJ(memdc.0));
            return Err(anyhow::anyhow!("CreateCompatibleBitmap failed"));
        }

        let old_obj = SelectObject(memdc, HGDIOBJ(bitmap.0));

        // Blit from desktop DC to memory DC
        BitBlt(
            memdc,
            0,
            0,
            width as i32,
            height as i32,
            hdc,
            0,
            0,
            SRCCOPY,
        )?;

        // Get bitmap bits (BGRA format from Windows)
        // Use negative height to get top-down bitmap (matches BitBlt capture)
        // Positive height would give bottom-up, causing a vertical flip
        let mut bmi: windows::Win32::Graphics::Gdi::BITMAPINFO =
            std::mem::zeroed();
        bmi.bmiHeader.biSize =
            std::mem::size_of::<windows::Win32::Graphics::Gdi::BITMAPINFOHEADER>() as u32;
        bmi.bmiHeader.biWidth = width as i32;
        bmi.bmiHeader.biHeight = -(height as i32); // negative = top-down
        bmi.bmiHeader.biPlanes = 1;
        bmi.bmiHeader.biBitCount = 32;
        bmi.bmiHeader.biCompression = 0; // BI_RGB

        let mut bits = vec![0u8; width as usize * height as usize * 4];
        let result = GetDIBits(
            memdc,
            bitmap,
            0,
            height,
            Some(bits.as_mut_ptr() as *mut _),
            &mut bmi,
            windows::Win32::Graphics::Gdi::DIB_RGB_COLORS,
        );

        // Restore old object and clean up
        SelectObject(memdc, old_obj);
        ReleaseDC(desktop, hdc);
        let _ = DeleteObject(HGDIOBJ(memdc.0));
        let _ = DeleteObject(HGDIOBJ(bitmap.0));
        let _ = CloseHandle(HANDLE(memdc.0));

        if result == 0 {
            return Err(anyhow::anyhow!("GetDIBits failed"));
        }

        // Convert BGRA (Windows format) to RGBA (image crate format)
        let mut rgba = Vec::with_capacity(bits.len());
        for i in (0..bits.len()).step_by(4) {
            rgba.push(bits[i + 2]); // R
            rgba.push(bits[i + 1]); // G
            rgba.push(bits[i]);     // B
            rgba.push(255);         // A (opaque)
        }

        rgba
    };

    Ok((rgba, width, height))
}

/// Encode a DynamicImage to raw bytes in the specified format.
fn encode_to_bytes(img: &DynamicImage, format: &ImageFormat) -> Result<Vec<u8>> {
    use image::ImageOutputFormat;

    let image_format = match format {
        ImageFormat::Webp => ImageOutputFormat::WebP,
        ImageFormat::Jpeg => ImageOutputFormat::Jpeg(95),
        ImageFormat::Png => ImageOutputFormat::Png,
    };

    let mut buffer = Vec::new();
    img.write_to(&mut std::io::Cursor::new(&mut buffer), image_format)?;
    Ok(buffer)
}

fn save_image(img: &DynamicImage, path: &Path, _format: &ImageFormat) -> Result<()> {
    // The image crate's save() method uses the codec system and supports
    // WebP when the "webp" feature is enabled.
    img.save(path)?;
    Ok(())
}
