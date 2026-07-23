use std::error::Error;
use std::fs::File;
use std::io::BufWriter;
use std::path::{Path, PathBuf};

use ferrink_shell::{JACQUARD_HEIGHT, JACQUARD_WIDTH, JacquardPreset, render_jacquard_background};
use slint::Rgb8Pixel;

fn main() -> Result<(), Box<dyn Error>> {
    let output = std::env::args_os()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("target/procedural-backgrounds"));
    std::fs::create_dir_all(&output)?;

    for preset in JacquardPreset::ALL {
        let background = render_jacquard_background(JACQUARD_WIDTH, JACQUARD_HEIGHT, preset)?;
        write_png(
            &output.join(format!("{}.png", preset.slug())),
            background.as_slice(),
            false,
        )?;
        write_png(
            &output.join(format!("{}-eink-4bit.png", preset.slug())),
            background.as_slice(),
            true,
        )?;
    }
    Ok(())
}

fn write_png(path: &Path, pixels: &[Rgb8Pixel], quantize: bool) -> Result<(), Box<dyn Error>> {
    let output = BufWriter::new(File::create(path)?);
    let mut encoder = png::Encoder::new(output, JACQUARD_WIDTH, JACQUARD_HEIGHT);
    encoder.set_color(png::ColorType::Rgb);
    encoder.set_depth(png::BitDepth::Eight);
    let mut bytes = Vec::with_capacity(pixels.len().saturating_mul(3));
    for pixel in pixels {
        if quantize {
            // Approximate the Kindle's 16-level grayscale output. Converting to
            // luminance first avoids pretending that tiny RGB hue differences
            // remain visible on an E-Ink panel.
            let luminance = (77 * u16::from(pixel.r)
                + 150 * u16::from(pixel.g)
                + 29 * u16::from(pixel.b)
                + 128)
                >> 8;
            let gray = u8::try_from((luminance >> 4) * 17)?;
            bytes.extend_from_slice(&[gray, gray, gray]);
        } else {
            bytes.extend_from_slice(&[pixel.r, pixel.g, pixel.b]);
        }
    }
    encoder.write_header()?.write_image_data(&bytes)?;
    Ok(())
}
