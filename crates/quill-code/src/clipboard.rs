use std::fs;
use std::path::PathBuf;

use arboard::{Clipboard, Error, ImageData};

use crate::thread::now_ms;

pub fn save_image() -> Result<Option<PathBuf>, String> {
    let image = match Clipboard::new().and_then(|mut clipboard| clipboard.get_image()) {
        Ok(image) => image,
        Err(Error::ContentNotAvailable) => return Ok(None),
        Err(error) => return Err(error.to_string()),
    };
    let bytes = encode(&image).map_err(|error| error.to_string())?;
    let path = std::env::temp_dir().join(format!("quill-clipboard-{}.png", now_ms()));
    fs::write(&path, bytes).map_err(|error| error.to_string())?;
    Ok(Some(path))
}

fn encode(image: &ImageData) -> Result<Vec<u8>, png::EncodingError> {
    let mut bytes = Vec::new();
    let mut encoder = png::Encoder::new(&mut bytes, image.width as u32, image.height as u32);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    encoder.write_header()?.write_image_data(&image.bytes)?;
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use std::borrow::Cow;
    use std::io::Cursor;

    use super::*;

    #[test]
    fn clipboard_pixels_encode_as_png() {
        let pixels = vec![255, 0, 0, 255, 0, 255, 0, 128];
        let image = ImageData { width: 2, height: 1, bytes: Cow::Owned(pixels.clone()) };
        let bytes = encode(&image).unwrap();
        let mut reader = png::Decoder::new(Cursor::new(bytes)).read_info().unwrap();
        let mut decoded = vec![0; reader.output_buffer_size().unwrap()];
        let info = reader.next_frame(&mut decoded).unwrap();
        assert_eq!((info.width, info.height), (2, 1));
        assert_eq!(decoded, pixels);
    }
}