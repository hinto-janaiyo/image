use std::{error, fmt, io::{self, Write}};

use super::header::{Header, ImageType, ALPHA_BIT_MASK, SCREEN_ORIGIN_BIT_MASK};
use crate::color::ColorType;
use crate::error::{EncodingError, ImageResult, UnsupportedError, UnsupportedErrorKind};
use crate::{image::ImageEncoder, ImageError, ImageFormat};
use byteorder::{LittleEndian, WriteBytesExt};
use std::convert::TryFrom;

/// Errors that can occur during encoding and saving of a TGA image.
#[derive(Debug, Copy, Clone, Hash, PartialEq, Eq, PartialOrd, Ord)]
enum EncoderError {
    /// Invalid TGA width.
    WidthInvalid(u32),

    /// Invalid TGA height.
    HeightInvalid(u32),
}

impl fmt::Display for EncoderError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            EncoderError::WidthInvalid(s) => f.write_fmt(format_args!("Invalid TGA width: {}", s)),
            EncoderError::HeightInvalid(s) => {
                f.write_fmt(format_args!("Invalid TGA height: {}", s))
            }
        }
    }
}

impl From<EncoderError> for ImageError {
    fn from(e: EncoderError) -> ImageError {
        ImageError::Encoding(EncodingError::new(ImageFormat::Tga.into(), e))
    }
}

impl error::Error for EncoderError {}

/// TGA encoder.
pub struct TgaEncoder<W: Write> {
    writer: W,
}

impl<W: Write> TgaEncoder<W> {
    /// Create a new encoder that writes its output to ```w```.
    pub fn new(w: W) -> TgaEncoder<W> {
        TgaEncoder { writer: w }
    }

    /// Encodes the image ```buf``` that has dimensions ```width```
    /// and ```height``` and ```ColorType``` ```color_type```.
    ///
    /// The dimensions of the image must be between 0 and 65535 (inclusive) or
    /// an error will be returned.
    pub fn encode(
        mut self,
        buf: &[u8],
        width: u32,
        height: u32,
        color_type: ColorType,
    ) -> ImageResult<()> {
        // Write out TGA header.
        let header = Header::from_pixel_info(color_type, width, height)?;
        header.write_to(&mut self.writer)?;

        // Write out Bgr(a)8 or L(a)8 image data.
        let mut image = Vec::from(buf);

        match color_type {
            ColorType::Rgb8 | ColorType::Rgba8 => {
                for chunk in image.chunks_mut(usize::from(color_type.bytes_per_pixel())) {
                    chunk.swap(0, 2);
                }
            }
            _ => {}
        }

        self.writer.write_all(&image)?;
        Ok(())
    }
}

impl<W: Write> ImageEncoder for TgaEncoder<W> {
    fn write_image(
        self,
        buf: &[u8],
        width: u32,
        height: u32,
        color_type: ColorType,
    ) -> ImageResult<()> {
        self.encode(buf, width, height, color_type)
    }
}

impl Header {
    /// Load the header with values from pixel information.
    pub(crate) fn from_pixel_info(
        color_type: ColorType,
        width: u32,
        height: u32,
    ) -> ImageResult<Self> {
        let mut header = Self::default();

        if width > 0 && height > 0 {
            header.image_width = u16::try_from(width)
                .map_err(|_| ImageError::from(EncoderError::WidthInvalid(width)))?;

            header.image_height = u16::try_from(height)
                .map_err(|_| ImageError::from(EncoderError::HeightInvalid(height)))?;

            let (num_alpha_bits, other_channel_bits, image_type) = match color_type {
                ColorType::Rgba8 | ColorType::Bgra8 => (8, 24, ImageType::RawTrueColor),
                ColorType::Rgb8 | ColorType::Bgr8 => (0, 24, ImageType::RawTrueColor),
                ColorType::La8 => (8, 8, ImageType::RawGrayScale),
                ColorType::L8 => (0, 8, ImageType::RawGrayScale),
                _ => {
                    return Err(ImageError::Unsupported(
                        UnsupportedError::from_format_and_kind(
                            ImageFormat::Tga.into(),
                            UnsupportedErrorKind::Color(color_type.into()),
                        ),
                    ))
                }
            };

            header.image_type = image_type as u8;
            header.pixel_depth = num_alpha_bits + other_channel_bits;
            header.image_desc = num_alpha_bits & ALPHA_BIT_MASK;
            header.image_desc |= SCREEN_ORIGIN_BIT_MASK; // Upper left origin.
        }

        Ok(header)
    }

    /// Write out the header values.
    pub(crate) fn write_to(&self, w: &mut dyn Write) -> io::Result<()> {
        w.write_u8(self.id_length)?;
        w.write_u8(self.map_type)?;
        w.write_u8(self.image_type)?;
        w.write_u16::<LittleEndian>(self.map_origin)?;
        w.write_u16::<LittleEndian>(self.map_length)?;
        w.write_u8(self.map_entry_size)?;
        w.write_u16::<LittleEndian>(self.x_origin)?;
        w.write_u16::<LittleEndian>(self.y_origin)?;
        w.write_u16::<LittleEndian>(self.image_width)?;
        w.write_u16::<LittleEndian>(self.image_height)?;
        w.write_u8(self.pixel_depth)?;
        w.write_u8(self.image_desc)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::{EncoderError, TgaEncoder};
    use crate::tga::TgaDecoder;

    use crate::color::ColorType;
    use crate::image::ImageDecoder;
    use crate::ImageError;

    use std::error::Error;
    use std::io::Cursor;

    fn round_trip_image(image: &[u8], width: u32, height: u32, c: ColorType) -> Vec<u8> {
        let mut encoded_data = Vec::new();
        {
            let encoder = TgaEncoder::new(&mut encoded_data);
            encoder
                .encode(&image, width, height, c)
                .expect("could not encode image");
        }

        let decoder = TgaDecoder::new(Cursor::new(&encoded_data)).expect("failed to decode");

        let mut buf = vec![0; decoder.total_bytes() as usize];
        decoder.read_image(&mut buf).expect("failed to decode");
        buf
    }

    #[test]
    fn test_image_width_too_large() {
        // TGA cannot encode images larger than 65,535×65,535
        // create a 65,536×1 8-bit black image buffer
        let size = usize::from(u16::MAX) + 1;
        let dimension = size as u32;
        let img = vec![0u8; size];
        // Try to encode an image that is too large
        let mut encoded = Vec::new();
        let encoder = TgaEncoder::new(&mut encoded);
        let result = encoder.encode(&img, dimension, 1, ColorType::L8);
        match result {
            Err(ImageError::Encoding(err)) => {
                let err = err
                    .source()
                    .unwrap()
                    .downcast_ref::<EncoderError>()
                    .unwrap();
                assert_eq!(*err, EncoderError::WidthInvalid(dimension));
            }
            other => panic!(
                "Encoding an image that is too wide should return a InvalidWidth \
                it returned {:?} instead",
                other
            ),
        }
    }

    #[test]
    fn test_image_height_too_large() {
        // TGA cannot encode images larger than 65,535×65,535
        // create a 65,536×1 8-bit black image buffer
        let size = usize::from(u16::MAX) + 1;
        let dimension = size as u32;
        let img = vec![0u8; size];
        // Try to encode an image that is too large
        let mut encoded = Vec::new();
        let encoder = TgaEncoder::new(&mut encoded);
        let result = encoder.encode(&img, 1, dimension, ColorType::L8);
        match result {
            Err(ImageError::Encoding(err)) => {
                let err = err
                    .source()
                    .unwrap()
                    .downcast_ref::<EncoderError>()
                    .unwrap();
                assert_eq!(*err, EncoderError::HeightInvalid(dimension));
            }
            other => panic!(
                "Encoding an image that is too tall should return a InvalidHeight \
                it returned {:?} instead",
                other
            ),
        }
    }

    #[test]
    fn round_trip_single_pixel_rgb() {
        let image = [0, 1, 2];
        let decoded = round_trip_image(&image, 1, 1, ColorType::Rgb8);
        assert_eq!(decoded.len(), image.len());
        assert_eq!(decoded.as_slice(), image);
    }

    #[test]
    fn round_trip_single_pixel_rgba() {
        let image = [0, 1, 2, 3];
        let decoded = round_trip_image(&image, 1, 1, ColorType::Rgba8);
        assert_eq!(decoded.len(), image.len());
        assert_eq!(decoded.as_slice(), image);
    }

    #[test]
    fn round_trip_single_pixel_bgr() {
        let image = [0, 1, 2];
        let decoded = round_trip_image(&image, 1, 1, ColorType::Bgr8);
        assert_eq!(decoded.len(), image.len());
        assert_eq!(decoded.as_slice(), [2, 1, 0]);
    }

    #[test]
    fn round_trip_single_pixel_bgra() {
        let image = [0, 1, 2, 3];
        let decoded = round_trip_image(&image, 1, 1, ColorType::Bgra8);
        assert_eq!(decoded.len(), image.len());
        assert_eq!(decoded.as_slice(), [2, 1, 0, 3]);
    }

    #[test]
    fn round_trip_gray() {
        let image = [0, 1, 2];
        let decoded = round_trip_image(&image, 3, 1, ColorType::L8);
        assert_eq!(decoded.len(), image.len());
        assert_eq!(decoded.as_slice(), image);
    }

    #[test]
    fn round_trip_graya() {
        let image = [0, 1, 2, 3, 4, 5];
        let decoded = round_trip_image(&image, 1, 3, ColorType::La8);
        assert_eq!(decoded.len(), image.len());
        assert_eq!(decoded.as_slice(), image);
    }

    #[test]
    fn round_trip_3px_rgb() {
        let image = [0; 3 * 3 * 3]; // 3x3 pixels, 3 bytes per pixel
        let _decoded = round_trip_image(&image, 3, 3, ColorType::Rgb8);
    }
}
