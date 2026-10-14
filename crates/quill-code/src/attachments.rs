use std::fs;
use std::path::{Path, PathBuf};

use base64::Engine;

const MAX_TEXT: usize = 60_000;
const MAX_IMAGE: u64 = 8 * 1024 * 1024;

#[derive(Clone, Debug, PartialEq)]
pub struct Attachment {
    pub path: PathBuf,
    pub name: String,
    pub image: bool,
}

fn image_type(path: &Path) -> Option<&'static str> {
    match path.extension()?.to_string_lossy().to_ascii_lowercase().as_str() {
        "png" => Some("image/png"),
        "jpg" | "jpeg" => Some("image/jpeg"),
        "webp" => Some("image/webp"),
        "gif" => Some("image/gif"),
        _ => None,
    }
}

impl Attachment {
    pub fn from_path(path: PathBuf) -> Option<Attachment> {
        if !path.is_file() {
            return None;
        }
        let name = path.file_name()?.to_string_lossy().into_owned();
        let image = image_type(&path).is_some();
        Some(Attachment { path, name, image })
    }
}

#[derive(Default, Debug, PartialEq)]
pub struct Prepared {
    pub context: String,
    pub images: Vec<String>,
    pub skipped: Vec<String>,
}

fn fence(name: &str, text: &str) -> String {
    let ext = name.rsplit('.').next().filter(|e| *e != name).unwrap_or_default();
    let mut body = text.to_string();
    if body.len() > MAX_TEXT {
        let mut cut = MAX_TEXT;
        while !body.is_char_boundary(cut) {
            cut -= 1;
        }
        body.truncate(cut);
        body.push_str("\n[file cut at 60 KB]");
    }
    format!("\n\nFile `{name}`:\n```{ext}\n{body}\n```")
}

pub fn prepare(attachments: &[Attachment], mentions: &[(String, PathBuf)], vision: bool) -> Prepared {
    let mut prepared = Prepared::default();
    for attachment in attachments {
        if attachment.image {
            if !vision {
                prepared.skipped.push(format!("{} (this model cannot see images)", attachment.name));
                continue;
            }
            match (image_type(&attachment.path), fs::metadata(&attachment.path).map(|m| m.len()), fs::read(&attachment.path)) {
                (Some(kind), Ok(size), Ok(bytes)) if size <= MAX_IMAGE => {
                    let data = base64::engine::general_purpose::STANDARD.encode(bytes);
                    prepared.images.push(format!("data:{kind};base64,{data}"));
                }
                _ => prepared.skipped.push(format!("{} (too large or unreadable)", attachment.name)),
            }
            continue;
        }
        add_text(&mut prepared, &attachment.name, &attachment.path);
    }
    for (label, path) in mentions {
        add_text(&mut prepared, label, path);
    }
    prepared
}

fn add_text(prepared: &mut Prepared, label: &str, path: &Path) {
    match fs::read(path) {
        Ok(bytes) if bytes.iter().take(8192).any(|b| *b == 0) => prepared.skipped.push(format!("{label} (binary file)")),
        Ok(bytes) => prepared.context.push_str(&fence(label, &String::from_utf8_lossy(&bytes))),
        Err(_) => prepared.skipped.push(format!("{label} (could not be read)")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_files_are_fenced_and_images_encoded() {
        let dir = std::env::temp_dir().join(format!("quill-attach-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("notes.md"), "hello").unwrap();
        fs::write(dir.join("pic.png"), [137u8, 80, 78, 71]).unwrap();
        let notes = Attachment::from_path(dir.join("notes.md")).unwrap();
        let pic = Attachment::from_path(dir.join("pic.png")).unwrap();
        assert!(pic.image && !notes.image);
        let prepared = prepare(&[notes.clone(), pic.clone()], &[], true);
        assert!(prepared.context.contains("File `notes.md`:\n```md\nhello\n```"));
        assert!(prepared.images[0].starts_with("data:image/png;base64,"));
        let blind = prepare(&[pic], &[], false);
        assert!(blind.images.is_empty() && blind.skipped[0].contains("cannot see images"));
        fs::remove_dir_all(&dir).unwrap();
    }
}