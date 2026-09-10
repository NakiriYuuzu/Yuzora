/// Update the running application's icon from the renderer's shared brand master.
#[tauri::command]
pub async fn set_brand_icon(app: tauri::AppHandle, png: Vec<u8>) -> Result<(), String> {
    if png.len() > 128 * 1024 {
        return Err("Brand icon is too large".into());
    }
    let icon = tauri::image::Image::from_bytes(&png).map_err(|error| error.to_string())?;
    if icon.width() != 256 || icon.height() != 256 {
        return Err("Brand icon must be 256 by 256 pixels".into());
    }
    #[cfg(target_os = "macos")]
    {
        let (sender, receiver) = tokio::sync::oneshot::channel();
        app.run_on_main_thread(move || {
            use objc2::{AnyThread, MainThreadMarker};
            use objc2_app_kit::{NSApplication, NSImage};
            use objc2_foundation::NSData;
            let result = (|| {
                let mtm = MainThreadMarker::new().ok_or("Main thread unavailable")?;
                let data = NSData::with_bytes(&png);
                let image =
                    NSImage::initWithData(NSImage::alloc(), &data).ok_or("Invalid brand image")?;
                // SAFETY: AppKit is accessed on the main thread, and the image
                // stays alive for the synchronous setter (AppKit retains it).
                unsafe {
                    NSApplication::sharedApplication(mtm).setApplicationIconImage(Some(&image))
                };
                Ok::<(), &'static str>(())
            })();
            let _ = sender.send(result);
        })
        .map_err(|error| error.to_string())?;
        receiver
            .await
            .map_err(|error| error.to_string())?
            .map_err(str::to_owned)
    }
    #[cfg(not(target_os = "macos"))]
    {
        use tauri::Manager;
        for window in app.webview_windows().values() {
            window
                .set_icon(icon.clone())
                .map_err(|error| error.to_string())?;
        }
        Ok(())
    }
}
