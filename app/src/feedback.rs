//! Explicit, person-reviewed beta feedback. No usage-data consent or install id.
//! Screenshots are what the person pastes or picks, each shown before sending.
use serde_json::{Value, json};
use std::path::PathBuf;

pub const MAX_IMAGES: usize = 3;
const MAX_IMAGE_BYTES: usize = 5 * 1024 * 1024;

const ENDPOINT: &str = "https://telemetry.herdr.dev/v1/feedback";

fn endpoint() -> Option<String> {
    std::env::var("HERDR_FEEDBACK_URL").ok().filter(|u| !u.is_empty())
        .or_else(|| option_env!("HERDR_APP_RELEASE").map(|_| ENDPOINT.to_owned()))
}

pub fn draft(message: &str, email: &str, images: usize, from: &str, layout: &str) -> Value {
    json!({
        "v": 1,
        "from": if from == "agent" { "agent" } else { "person" },
        "message": message.trim(),
        "email": if email.trim().is_empty() { None } else { Some(email.trim()) },
        "app": {
            "version": env!("CARGO_PKG_VERSION"),
            "os": std::env::consts::OS,
            "arch": std::env::consts::ARCH,
            "channel": "stable",
        },
        "core": { "version": crate::update::core().version.as_deref().map(crate::telemetry::version) },
        "layout": if crate::window::shipped_layouts().contains(&layout) { layout } else { "other" },
        "images": images,
    })
}

/// Where the form keeps the screenshots it shows (its data folder: `image()` reads from the app home).
fn shots_dir() -> PathBuf { crate::mods::app_dir().join("data").join("frame-feedback") }

fn sniff(bytes: &[u8]) -> Option<&'static str> {
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") { Some("png") }
    else if bytes.starts_with(&[0xff, 0xd8, 0xff]) { Some("jpg") }
    else { None }
}

/// Keeps a pasted or picked image for the form: PNG or JPEG as is, any other
/// image as PNG. Returns its path from the app home.
pub fn attach(bytes: &[u8]) -> Result<String, String> {
    let (bytes, ext) = match sniff(bytes) {
        Some(ext) => (bytes.to_vec(), ext),
        None => {
            let img = image::load_from_memory(bytes).map_err(|_| "That isn't an image Herdr can read.".to_owned())?;
            let mut png = std::io::Cursor::new(Vec::new());
            img.write_to(&mut png, image::ImageFormat::Png).map_err(|_| "Couldn't convert that image.".to_owned())?;
            (png.into_inner(), "png")
        }
    };
    if bytes.len() > MAX_IMAGE_BYTES { return Err("That image is over 5 MB.".into()); }
    let dir = shots_dir();
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or_default();
    let name = format!("shot-{stamp}.{ext}");
    std::fs::write(dir.join(&name), bytes).map_err(|e| e.to_string())?;
    Ok(format!("data/frame-feedback/{name}"))
}

/// Picker and external drops share the same conversion and size limits.
pub fn attach_files(paths: &[PathBuf]) -> Vec<Value> {
    paths.iter().take(MAX_IMAGES).map(|p| {
        match std::fs::read(p) {
            Ok(bytes) => match attach(&bytes) {
                Ok(path) => json!({ "path": path }),
                Err(error) => json!({ "error": error }),
            },
            Err(_) => json!({ "error": "Couldn't read that file." }),
        }
    }).collect()
}

/// Drops the form's screenshots (a new form starts empty; a sent one is done with them).
pub fn clear_shots() {
    let Ok(entries) = std::fs::read_dir(shots_dir()) else { return };
    for e in entries.flatten() {
        if e.file_name().to_string_lossy().starts_with("shot-") { let _ = std::fs::remove_file(e.path()); }
    }
}

/// A path `attach` returned, checked again before it leaves the machine.
fn shot_file(rel: &str) -> Result<PathBuf, String> {
    let name = rel.strip_prefix("data/frame-feedback/").filter(|n| n.starts_with("shot-") && !n.contains(['/', '\\']))
        .ok_or("Unknown screenshot.")?;
    let path = shots_dir().join(name);
    let bytes = std::fs::read(&path).map_err(|_| "A screenshot is gone. Remove it and add it again.".to_owned())?;
    if bytes.len() > MAX_IMAGE_BYTES || sniff(&bytes).is_none() { return Err("A screenshot isn't a PNG or JPEG under 5 MB.".into()); }
    Ok(path)
}

pub fn available() -> bool { endpoint().is_some() }

fn validate(body: &Value) -> Result<(), String> {
    let message = body["message"].as_str().unwrap_or_default();
    if message.is_empty() { return Err("Write a message first.".into()); }
    if message.encode_utf16().count() > 4000 { return Err("Keep the message to 4000 characters.".into()); }
    if let Some(email) = body["email"].as_str() {
        let valid = email.split_once('@').is_some_and(|(local, domain)| {
            !local.is_empty() && domain.split_once('.').is_some_and(|(a, b)| !a.is_empty() && !b.is_empty())
                && !domain.contains('@')
        });
        if email.len() > 254 || !valid || email.chars().any(char::is_whitespace) {
            return Err("Enter a plain email address, or leave it blank.".into());
        }
    }
    Ok(())
}

pub async fn send(body: Value, shots: Vec<String>) -> Result<(), String> {
    validate(&body)?;
    if shots.len() > MAX_IMAGES || body["images"].as_u64() != Some(shots.len() as u64) {
        return Err("Up to 3 screenshots.".into());
    }
    let files = shots.iter().map(|s| shot_file(s)).collect::<Result<Vec<_>, _>>()?;
    let url = endpoint().ok_or("This build was made from source and doesn't send feedback. Use a release build.")?;
    let (tx, rx) = async_channel::bounded(1);
    std::thread::spawn(move || {
        let result = post(&url, &body.to_string(), &files);
        if result.is_ok() { clear_shots(); }
        let _ = tx.send_blocking(result);
    });
    rx.recv().await.map_err(|_| "The feedback request was lost.".to_owned())?
}

/// Text alone goes as JSON (what every version sends); with screenshots, as a
/// form: the same JSON in `feedback`, each image in `image`.
fn post(url: &str, body: &str, files: &[PathBuf]) -> Result<(), String> {
    use std::io::Write;
    use std::process::Stdio;
    let null = if cfg!(windows) { "NUL" } else { "/dev/null" };
    let network = || "Couldn't reach the feedback server. Check your connection and try again.".to_owned();
    let mut args: Vec<String> = ["-sS", "--max-time", "60", "-X", "POST"].map(String::from).to_vec();
    if files.is_empty() {
        args.extend(["-H", "content-type: application/json", "--data-binary", "@-"].map(String::from));
    } else {
        args.extend(["-F".into(), "feedback=<-;type=application/json".into()]);
        for f in files {
            let kind = if f.extension().is_some_and(|e| e == "png") { "image/png" } else { "image/jpeg" };
            args.extend(["-F".into(), format!("image=@{};type={kind}", f.display())]);
        }
    }
    args.extend(["-o", null, "-w", "%{http_code}", url].map(String::from));
    let mut child = crate::spawn::command(crate::update::system("curl"))
        .args(&args)
        .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null()).spawn().map_err(|_| network())?;
    if child.stdin.take().ok_or_else(network)?.write_all(body.as_bytes()).is_err() {
        let _ = child.kill();
        let _ = child.wait();
        return Err(network());
    }
    let out = child.wait_with_output().map_err(|_| network())?;
    if !out.status.success() { return Err(network()); }
    match String::from_utf8_lossy(&out.stdout).trim() {
        "204" => Ok(()),
        "429" => Err("Too many, try in a minute.".into()),
        "503" => Err("Not taking feedback right now. Try again later.".into()),
        "400" => Err("The server couldn't accept this feedback. Check the message and email.".into()),
        "413" => Err("The feedback is too large. Shorten the message or remove a screenshot.".into()),
        _ => Err("The feedback server returned an unexpected answer. Try again later.".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn payload_is_text_only_and_custom_layouts_are_hidden() {
        let body = draft("  broken  ", "  ", 2, "agent", "private-layout");
        assert_eq!(body["message"], "broken");
        assert_eq!(body["email"], Value::Null);
        assert_eq!(body["from"], "agent");
        assert_eq!(body["layout"], "other");
        assert_eq!(body["images"], 2);
        assert_eq!(body.as_object().unwrap().len(), 8);
        assert!(body.get("install").is_none());
        assert_eq!(draft("ok", "a@b.c", 0, "person", "main")["layout"], "main");
    }
    #[test]
    fn invalid_message_and_email_are_explained() {
        for (message, email) in [("", ""), (&"x".repeat(4001), ""), ("ok", "bad"), ("ok", "a b@c.d")] {
            assert!(validate(&draft(message, email, 0, "person", "main")).is_err());
        }
        assert!(validate(&draft("ok", "a@b.c", 0, "person", "main")).is_ok());
    }
    #[test]
    fn only_png_and_jpeg_leave_as_they_are() {
        assert_eq!(sniff(b"\x89PNG\r\n\x1a\nrest"), Some("png"));
        assert_eq!(sniff(&[0xff, 0xd8, 0xff, 0xe0]), Some("jpg"));
        assert_eq!(sniff(b"GIF89a"), None);
        assert!(shot_file("data/frame-feedback/../../settings.json").is_err());
        assert!(shot_file("/etc/passwd").is_err());
    }
}
