//! Deterministic two-tone launcher background generation.

use std::fmt;
use std::fs::File;
use std::io::{BufRead, BufReader, Seek};
use std::path::Path;

use slint::{Rgb8Pixel, SharedPixelBuffer};

/// Width of the compact source buffer Slint scales to the launcher surface.
pub const JACQUARD_WIDTH: u32 = 632;
/// Height of the compact source buffer Slint scales to the launcher surface.
pub const JACQUARD_HEIGHT: u32 = 843;

const MAX_PIXELS: usize = 4_194_304;
const MAX_BACKGROUND_PNG_BYTES: u64 = 1_048_576;
const TAU: f32 = std::f32::consts::TAU;
const THRESHOLD: f32 = -0.12;
const ROUNDED_WEIGHT: f32 = 0.28;
const DIAGONAL_WEIGHT: f32 = -0.10;
const SUBPIXEL_OFFSETS: [f32; 2] = [0.25, 0.75];
const BAYER_8: [[u8; 8]; 8] = [
    [0, 48, 12, 60, 3, 51, 15, 63],
    [32, 16, 44, 28, 35, 19, 47, 31],
    [8, 56, 4, 52, 11, 59, 7, 55],
    [40, 24, 36, 20, 43, 27, 39, 23],
    [2, 50, 14, 62, 1, 49, 13, 61],
    [34, 18, 46, 30, 33, 17, 45, 29],
    [10, 58, 6, 54, 9, 57, 5, 53],
    [42, 26, 38, 22, 41, 25, 37, 21],
];

/// A validated basename within Ferrink's one background directory.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct LauncherBackgroundFileName(String);

impl LauncherBackgroundFileName {
    /// Validates one bounded ASCII `.png` basename.
    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        let valid_length = (5..=64).contains(&value.len());
        let valid_start = value
            .as_bytes()
            .first()
            .is_some_and(u8::is_ascii_alphanumeric);
        let valid_bytes = value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'));
        (valid_length && valid_start && valid_bytes && value.ends_with(".png"))
            .then(|| Self(value.to_owned()))
    }

    /// Returns the validated basename.
    #[must_use]
    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }

    fn label(&self) -> String {
        match self.as_str() {
            "waves.png" => "Waves".to_owned(),
            "topography.png" => "Topography".to_owned(),
            value => {
                let stem = value.strip_suffix(".png").unwrap_or(value);
                let mut label = stem.replace(['-', '_'], " ");
                if let Some(first) = label.get_mut(..1) {
                    first.make_ascii_uppercase();
                }
                label
            }
        }
    }
}

/// User-selectable launcher background source.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LauncherBackgroundChoice {
    /// Deterministic built-in two-tone pattern.
    Pattern,
    /// Validated exact-size PNG from Ferrink's single background directory.
    File(LauncherBackgroundFileName),
}

impl LauncherBackgroundChoice {
    /// Short settings label.
    #[must_use]
    pub fn label(&self) -> String {
        match self {
            Self::Pattern => "Pattern".to_owned(),
            Self::File(filename) => filename.label(),
        }
    }

    /// Secondary picker label describing the source.
    #[must_use]
    pub const fn detail(&self) -> &'static str {
        match self {
            Self::Pattern => "Generated on device",
            Self::File(_) => "Image from ferrink/backgrounds",
        }
    }

    /// Stable value used by the versioned preference file.
    #[must_use]
    pub fn setting_value(&self) -> String {
        match self {
            Self::Pattern => "pattern".to_owned(),
            Self::File(filename) => format!("file:{}", filename.as_str()),
        }
    }

    /// Validated image filename, if this is not the generated choice.
    #[must_use]
    pub const fn filename(&self) -> Option<&LauncherBackgroundFileName> {
        match self {
            Self::Pattern => None,
            Self::File(filename) => Some(filename),
        }
    }

    /// Parses one exact persisted value.
    #[must_use]
    pub fn from_setting_value(value: &str) -> Option<Self> {
        match value {
            "pattern" => Some(Self::Pattern),
            // Accept the two values written by the earlier closed picker.
            "waves" => LauncherBackgroundFileName::parse("waves.png").map(Self::File),
            "topography" => LauncherBackgroundFileName::parse("topography.png").map(Self::File),
            _ => value
                .strip_prefix("file:")
                .and_then(LauncherBackgroundFileName::parse)
                .map(Self::File),
        }
    }
}

/// Reviewed parameter sets derived from the supplied pixel-field experiment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JacquardPreset {
    /// Original 150-pixel motif and stronger interlock.
    Original,
    /// Calmer 220-pixel motif and gentler bend.
    Calm,
    /// Tighter, lower-contrast upholstery treatment.
    Upholstery,
    /// Calm geometry with two tones separated for a 16-level E-Ink panel.
    EinkCalm,
}

impl JacquardPreset {
    /// Every preset in stable comparison order.
    pub const ALL: [Self; 4] = [Self::Original, Self::Calm, Self::Upholstery, Self::EinkCalm];

    /// Returns the stable filename label used by render fixtures.
    #[must_use]
    pub const fn slug(self) -> &'static str {
        match self {
            Self::Original => "original",
            Self::Calm => "calm",
            Self::Upholstery => "upholstery",
            Self::EinkCalm => "eink-calm",
        }
    }

    const fn parameters(self) -> Parameters {
        match self {
            Self::Original => Parameters {
                dark: [31, 33, 37],
                light: [43, 46, 51],
                period: 150.0,
                bend: 0.42,
            },
            Self::Calm => Parameters {
                dark: [31, 33, 37],
                light: [43, 46, 51],
                period: 220.0,
                bend: 0.28,
            },
            Self::Upholstery => Parameters {
                dark: [32, 34, 37],
                light: [39, 42, 46],
                period: 110.0,
                bend: 0.42,
            },
            Self::EinkCalm => Parameters {
                dark: [32, 34, 37],
                light: [62, 66, 72],
                period: 220.0,
                bend: 0.28,
            },
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct Parameters {
    dark: [u8; 3],
    light: [u8; 3],
    period: f32,
    bend: f32,
}

#[derive(Debug, Clone, Copy)]
struct XTerms {
    sin_u: f32,
    cos_u: f32,
    cos_2u: f32,
    sin_3u: f32,
    cos_3u: f32,
    sin_bend_2u: f32,
    cos_bend_2u: f32,
}

#[derive(Debug, Clone, Copy)]
struct YTerms {
    sin_v: f32,
    cos_v: f32,
    cos_2v: f32,
    sin_3v: f32,
    cos_3v: f32,
    sin_bend_2v: f32,
    cos_bend_2v: f32,
}

/// A requested background buffer was empty or exceeded its fixed memory bound.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JacquardError {
    /// Width or height was zero.
    EmptyDimensions,
    /// Pixel count overflowed or exceeded the four-megapixel safety bound.
    TooManyPixels,
}

impl fmt::Display for JacquardError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyDimensions => formatter.write_str("jacquard dimensions must be nonzero"),
            Self::TooManyPixels => {
                formatter.write_str("jacquard dimensions exceed the pixel bound")
            }
        }
    }
}

impl std::error::Error for JacquardError {}

/// An optional launcher PNG failed its strict file or image contract.
#[derive(Debug)]
pub enum LauncherBackgroundImageError {
    /// The path is not a bounded regular file or could not be read.
    Io(std::io::Error),
    /// The PNG stream is malformed.
    Decode(png::DecodingError),
    /// The PNG is not the exact reviewed 632×843 RGB8 shape.
    InvalidFormat,
}

impl fmt::Display for LauncherBackgroundImageError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => write!(formatter, "cannot read launcher background: {error}"),
            Self::Decode(error) => write!(formatter, "cannot decode launcher background: {error}"),
            Self::InvalidFormat => formatter
                .write_str("launcher background must be an exact 632×843 eight-bit RGB PNG"),
        }
    }
}

impl std::error::Error for LauncherBackgroundImageError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            Self::Decode(error) => Some(error),
            Self::InvalidFormat => None,
        }
    }
}

/// Checks the bounded file and PNG header without allocating its pixel buffer.
///
/// # Errors
///
/// Returns [`LauncherBackgroundImageError`] unless `path` is a non-symlinked,
/// non-empty, at-most-1-MiB exact 632×843 RGB8 PNG.
pub fn inspect_launcher_background_png(path: &Path) -> Result<(), LauncherBackgroundImageError> {
    let input = open_background_png(path)?;
    inspect_background_png(input)
}

/// Loads one exact bounded launcher PNG without enabling Slint image decoders.
///
/// # Errors
///
/// Returns [`LauncherBackgroundImageError`] for an unsafe path, malformed PNG,
/// wrong dimensions/color format, or an unexpected decoded byte count.
pub fn load_launcher_background_png(
    path: &Path,
) -> Result<slint::Image, LauncherBackgroundImageError> {
    decode_launcher_background_png(open_background_png(path)?)
}

fn open_background_png(path: &Path) -> Result<BufReader<File>, LauncherBackgroundImageError> {
    let metadata = path
        .symlink_metadata()
        .map_err(LauncherBackgroundImageError::Io)?;
    if !metadata.file_type().is_file()
        || metadata.len() == 0
        || metadata.len() > MAX_BACKGROUND_PNG_BYTES
    {
        return Err(LauncherBackgroundImageError::Io(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "background must be a non-empty regular file no larger than 1 MiB",
        )));
    }
    File::open(path)
        .map(BufReader::new)
        .map_err(LauncherBackgroundImageError::Io)
}

fn inspect_background_png<R: BufRead + Seek>(input: R) -> Result<(), LauncherBackgroundImageError> {
    let reader = png::Decoder::new(input)
        .read_info()
        .map_err(LauncherBackgroundImageError::Decode)?;
    validate_background_info(reader.info())
}

fn decode_launcher_background_png<R: BufRead + Seek>(
    input: R,
) -> Result<slint::Image, LauncherBackgroundImageError> {
    let mut reader = png::Decoder::new(input)
        .read_info()
        .map_err(LauncherBackgroundImageError::Decode)?;
    validate_background_info(reader.info())?;
    let expected = usize::try_from(JACQUARD_WIDTH)
        .ok()
        .and_then(|width| {
            usize::try_from(JACQUARD_HEIGHT)
                .ok()
                .and_then(|height| width.checked_mul(height))
        })
        .and_then(|pixels| pixels.checked_mul(3))
        .ok_or(LauncherBackgroundImageError::InvalidFormat)?;
    let output_size = reader
        .output_buffer_size()
        .ok_or(LauncherBackgroundImageError::InvalidFormat)?;
    if output_size != expected {
        return Err(LauncherBackgroundImageError::InvalidFormat);
    }
    let mut bytes = vec![0_u8; output_size];
    let frame = reader
        .next_frame(bytes.as_mut_slice())
        .map_err(LauncherBackgroundImageError::Decode)?;
    if frame.buffer_size() != expected
        || frame.color_type != png::ColorType::Rgb
        || frame.bit_depth != png::BitDepth::Eight
    {
        return Err(LauncherBackgroundImageError::InvalidFormat);
    }
    if bytes.len() != expected {
        return Err(LauncherBackgroundImageError::InvalidFormat);
    }
    let mut buffer = SharedPixelBuffer::<Rgb8Pixel>::new(JACQUARD_WIDTH, JACQUARD_HEIGHT);
    for (pixel, rgb) in buffer
        .make_mut_slice()
        .iter_mut()
        .zip(bytes.chunks_exact(3))
    {
        *pixel = Rgb8Pixel::new(rgb[0], rgb[1], rgb[2]);
    }
    Ok(slint::Image::from_rgb8(buffer))
}

fn validate_background_info(info: &png::Info<'_>) -> Result<(), LauncherBackgroundImageError> {
    if info.width != JACQUARD_WIDTH
        || info.height != JACQUARD_HEIGHT
        || info.color_type != png::ColorType::Rgb
        || info.bit_depth != png::BitDepth::Eight
    {
        return Err(LauncherBackgroundImageError::InvalidFormat);
    }
    Ok(())
}

/// Generates one deterministic RGB8 two-tone pixel field.
///
/// Trigonometric terms are precomputed for the two subpixel samples on each
/// axis. The per-pixel loop then uses only arithmetic and the ordered-dither
/// lookup; it performs no trigonometric calls and uses no randomness.
///
/// # Errors
///
/// Returns [`JacquardError`] for empty, overflowing, or oversized dimensions.
pub fn render_jacquard_background(
    width: u32,
    height: u32,
    preset: JacquardPreset,
) -> Result<SharedPixelBuffer<Rgb8Pixel>, JacquardError> {
    let pixel_count = bounded_pixel_count(width, height)?;
    let parameters = preset.parameters();
    let x_terms = precompute_x_terms(width, parameters);
    let y_terms = precompute_y_terms(height, parameters);
    let dark = Rgb8Pixel::new(parameters.dark[0], parameters.dark[1], parameters.dark[2]);
    let light = Rgb8Pixel::new(
        parameters.light[0],
        parameters.light[1],
        parameters.light[2],
    );
    let mut output = SharedPixelBuffer::new(width, height);
    let pixels = output.make_mut_slice();
    debug_assert_eq!(pixels.len(), pixel_count);

    for (y, row) in pixels.chunks_exact_mut(width as usize).enumerate() {
        let y_samples = &y_terms[y * 2..y * 2 + 2];
        for (x, pixel) in row.iter_mut().enumerate() {
            let x_samples = &x_terms[x * 2..x * 2 + 2];
            let mut coverage = 0_u8;
            for y_sample in y_samples {
                for x_sample in x_samples {
                    coverage += u8::from(pattern_field(*x_sample, *y_sample) > THRESHOLD);
                }
            }
            let dither = BAYER_8[y & 7][x & 7];
            *pixel = if coverage.saturating_mul(16) > dither {
                light
            } else {
                dark
            };
        }
    }

    Ok(output)
}

fn bounded_pixel_count(width: u32, height: u32) -> Result<usize, JacquardError> {
    if width == 0 || height == 0 {
        return Err(JacquardError::EmptyDimensions);
    }
    let width = usize::try_from(width).map_err(|_| JacquardError::TooManyPixels)?;
    let height = usize::try_from(height).map_err(|_| JacquardError::TooManyPixels)?;
    let count = width
        .checked_mul(height)
        .ok_or(JacquardError::TooManyPixels)?;
    if count > MAX_PIXELS {
        return Err(JacquardError::TooManyPixels);
    }
    Ok(count)
}

fn precompute_x_terms(width: u32, parameters: Parameters) -> Vec<XTerms> {
    axis_samples(width)
        .map(|coordinate| {
            let u = TAU * coordinate / parameters.period;
            let (sin_u, cos_u) = u.sin_cos();
            let (sin_2u, cos_2u) = (u * 2.0).sin_cos();
            let (sin_3u, cos_3u) = (u * 3.0).sin_cos();
            let (sin_bend_2u, cos_bend_2u) = (parameters.bend * sin_2u).sin_cos();
            XTerms {
                sin_u,
                cos_u,
                cos_2u,
                sin_3u,
                cos_3u,
                sin_bend_2u,
                cos_bend_2u,
            }
        })
        .collect()
}

fn precompute_y_terms(height: u32, parameters: Parameters) -> Vec<YTerms> {
    axis_samples(height)
        .map(|coordinate| {
            let v = TAU * coordinate / parameters.period;
            let (sin_v, cos_v) = v.sin_cos();
            let (sin_2v, cos_2v) = (v * 2.0).sin_cos();
            let (sin_3v, cos_3v) = (v * 3.0).sin_cos();
            let (sin_bend_2v, cos_bend_2v) = (parameters.bend * sin_2v).sin_cos();
            YTerms {
                sin_v,
                cos_v,
                cos_2v,
                sin_3v,
                cos_3v,
                sin_bend_2v,
                cos_bend_2v,
            }
        })
        .collect()
}

fn axis_samples(extent: u32) -> impl Iterator<Item = f32> {
    (0..extent).flat_map(|pixel| SUBPIXEL_OFFSETS.map(move |offset| pixel as f32 + offset))
}

fn pattern_field(x: XTerms, y: YTerms) -> f32 {
    let horizontal = x.cos_u * y.cos_bend_2v - x.sin_u * y.sin_bend_2v;
    let vertical = y.cos_v * x.cos_bend_2u + y.sin_v * x.sin_bend_2u;
    let woven = horizontal * vertical;
    let rounded = ROUNDED_WEIGHT * (x.cos_2u + y.cos_2v);
    let diagonal = DIAGONAL_WEIGHT * (x.cos_3u * y.cos_3v + x.sin_3u * y.sin_3v);
    woven + rounded + diagonal
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use super::*;

    #[test]
    fn every_preset_is_deterministic_and_strictly_two_tone() {
        for preset in JacquardPreset::ALL {
            let first = render_jacquard_background(96, 128, preset).unwrap();
            let second = render_jacquard_background(96, 128, preset).unwrap();
            assert_eq!(first.as_slice(), second.as_slice());

            let parameters = preset.parameters();
            let dark = Rgb8Pixel::new(parameters.dark[0], parameters.dark[1], parameters.dark[2]);
            let light = Rgb8Pixel::new(
                parameters.light[0],
                parameters.light[1],
                parameters.light[2],
            );
            assert!(first.as_slice().contains(&dark));
            assert!(first.as_slice().contains(&light));
            assert!(
                first
                    .as_slice()
                    .iter()
                    .all(|pixel| *pixel == dark || *pixel == light)
            );
        }
    }

    #[test]
    fn dimensions_are_bounded_before_allocation() {
        assert!(matches!(
            render_jacquard_background(0, 1, JacquardPreset::Original),
            Err(JacquardError::EmptyDimensions)
        ));
        assert!(matches!(
            render_jacquard_background(u32::MAX, u32::MAX, JacquardPreset::Original),
            Err(JacquardError::TooManyPixels)
        ));
        assert!(matches!(
            render_jacquard_background(2_049, 2_048, JacquardPreset::Original),
            Err(JacquardError::TooManyPixels)
        ));
    }

    #[test]
    fn compact_launcher_buffer_has_the_expected_memory_footprint() {
        let background =
            render_jacquard_background(JACQUARD_WIDTH, JACQUARD_HEIGHT, JacquardPreset::Calm)
                .unwrap();
        assert_eq!(background.width(), JACQUARD_WIDTH);
        assert_eq!(background.height(), JACQUARD_HEIGHT);
        assert_eq!(background.as_bytes().len(), 1_598_328);
    }

    #[test]
    fn eink_calm_tones_survive_four_bit_grayscale_quantization() {
        let background = render_jacquard_background(96, 128, JacquardPreset::EinkCalm).unwrap();
        let mut levels: Vec<_> = background
            .as_slice()
            .iter()
            .map(|pixel| {
                let luminance = (77 * u16::from(pixel.r)
                    + 150 * u16::from(pixel.g)
                    + 29 * u16::from(pixel.b)
                    + 128)
                    >> 8;
                luminance >> 4
            })
            .collect();
        levels.sort_unstable();
        levels.dedup();
        assert_eq!(levels, [2, 4]);
    }

    #[test]
    fn reviewed_topography_asset_is_exact_rgb8() {
        let image = decode_launcher_background_png(Cursor::new(include_bytes!(
            "../../../docs/design/launcher-background-topography-6shade.png"
        )))
        .unwrap();
        let size = image.size();
        assert_eq!((size.width, size.height), (632, 843));
    }

    #[test]
    fn background_choice_accepts_only_safe_png_basenames() {
        assert!(LauncherBackgroundFileName::parse("family-art.png").is_some());
        assert!(LauncherBackgroundFileName::parse("../art.png").is_none());
        assert!(LauncherBackgroundFileName::parse("nested/art.png").is_none());
        assert!(LauncherBackgroundFileName::parse(".hidden.png").is_none());
        assert!(LauncherBackgroundFileName::parse("art.jpg").is_none());
        assert_eq!(
            LauncherBackgroundChoice::from_setting_value("file:topography.png")
                .and_then(|choice| choice.filename().cloned())
                .map(|filename| filename.as_str().to_owned()),
            Some("topography.png".to_owned())
        );
        assert_eq!(LauncherBackgroundChoice::from_setting_value("wave"), None);
    }
}
