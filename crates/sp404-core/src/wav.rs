//! Minimal WAV reader and writer.
//!
//! Reading supports PCM 8/16/24/32-bit and IEEE float 32-bit, mono or stereo.
//! Writing always emits 16-bit stereo PCM. Both operate on byte buffers so the
//! core stays free of filesystem types and can compile for `wasm32`.

use crate::buffer::AudioBuffer;
use crate::Error;

/// Decode a WAV file from memory.
pub fn decode_wav(bytes: &[u8]) -> Result<AudioBuffer, Error> {
    if bytes.len() < 44 {
        return Err(Error::Wav("file is too small to be a WAV".into()));
    }
    if &bytes[0..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        return Err(Error::Wav("missing RIFF/WAVE header".into()));
    }
    let mut pos = 12usize;
    let mut audio_format = None;
    let mut channels = None;
    let mut sample_rate = None;
    let mut bits = None;
    let mut data: Option<&[u8]> = None;

    while pos + 8 <= bytes.len() {
        let id = &bytes[pos..pos + 4];
        let size = u32::from_le_bytes(bytes[pos + 4..pos + 8].try_into().unwrap()) as usize;
        pos += 8;
        if pos + size > bytes.len() {
            return Err(Error::Wav("chunk extends past the end of the file".into()));
        }
        let chunk = &bytes[pos..pos + size];
        if id == b"fmt " {
            if chunk.len() < 16 {
                return Err(Error::Wav("fmt chunk is too small".into()));
            }
            audio_format = Some(u16::from_le_bytes([chunk[0], chunk[1]]));
            channels = Some(u16::from_le_bytes([chunk[2], chunk[3]]));
            sample_rate = Some(u32::from_le_bytes([chunk[4], chunk[5], chunk[6], chunk[7]]));
            bits = Some(u16::from_le_bytes([chunk[14], chunk[15]]));
        } else if id == b"data" && data.is_none() {
            data = Some(chunk);
        }
        pos += size + (size & 1);
    }

    let audio_format = audio_format.ok_or_else(|| Error::Wav("missing fmt chunk".into()))?;
    let channels = channels.ok_or_else(|| Error::Wav("missing fmt chunk".into()))?;
    let sample_rate = sample_rate.ok_or_else(|| Error::Wav("missing fmt chunk".into()))? as f32;
    let bits = bits.ok_or_else(|| Error::Wav("missing fmt chunk".into()))?;
    let data = data.ok_or_else(|| Error::Wav("missing data chunk".into()))?;

    if channels != 1 && channels != 2 {
        return Err(Error::Wav(format!(
            "only mono and stereo are supported, found {channels} channels"
        )));
    }
    if sample_rate < 1_000.0 {
        return Err(Error::Wav(format!(
            "sample rate {sample_rate} is unsupported"
        )));
    }

    let samples = decode_samples(audio_format, bits, data)?;
    if samples.is_empty() || samples.len() % channels as usize != 0 {
        return Err(Error::Wav(
            "data chunk is not a whole number of frames".into(),
        ));
    }
    AudioBuffer::new(sample_rate, channels, samples)
}

fn decode_samples(format: u16, bits: u16, data: &[u8]) -> Result<Vec<f32>, Error> {
    match (format, bits) {
        (1, 8) => Ok(data.iter().map(|b| (*b as f32 - 128.0) / 128.0).collect()),
        (1, 16) => {
            if data.len() % 2 != 0 {
                return Err(Error::Wav("16-bit data has an odd length".into()));
            }
            Ok(data
                .chunks_exact(2)
                .map(|c| i16::from_le_bytes([c[0], c[1]]) as f32 / 32_768.0)
                .collect())
        }
        (1, 24) => {
            if data.len() % 3 != 0 {
                return Err(Error::Wav(
                    "24-bit data length is not a multiple of 3".into(),
                ));
            }
            Ok(data
                .chunks_exact(3)
                .map(|c| {
                    let mut v = c[0] as i32 | ((c[1] as i32) << 8) | ((c[2] as i32) << 16);
                    if v & 0x0080_0000 != 0 {
                        v |= !0x00FF_FFFF;
                    }
                    v as f32 / 8_388_608.0
                })
                .collect())
        }
        (1, 32) => {
            if data.len() % 4 != 0 {
                return Err(Error::Wav(
                    "32-bit data length is not a multiple of 4".into(),
                ));
            }
            Ok(data
                .chunks_exact(4)
                .map(|c| i32::from_le_bytes([c[0], c[1], c[2], c[3]]) as f32 / 2_147_483_648.0)
                .collect())
        }
        (3, 32) => {
            if data.len() % 4 != 0 {
                return Err(Error::Wav(
                    "float data length is not a multiple of 4".into(),
                ));
            }
            Ok(data
                .chunks_exact(4)
                .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
                .collect())
        }
        _ => Err(Error::Wav(format!(
            "unsupported format {format} at {bits} bits"
        ))),
    }
}

/// Encode stereo 16-bit PCM WAV bytes.
///
/// Mono input is duplicated. The sample rate is taken from `buffer` unless it
/// is resampled by the caller first. Values outside -1..1 are clipped.
pub fn encode_wav(buffer: &AudioBuffer) -> Result<Vec<u8>, Error> {
    let stereo = buffer.to_stereo(buffer.sample_rate)?;
    let frames = stereo.frames();
    let data_bytes = frames
        .checked_mul(4)
        .ok_or_else(|| Error::Wav("WAV is too large".into()))?;
    let riff_size = 36u32
        .checked_add(data_bytes as u32)
        .ok_or_else(|| Error::Wav("WAV is too large".into()))?;
    let mut out = Vec::with_capacity(44 + data_bytes);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&riff_size.to_le_bytes());
    out.extend_from_slice(b"WAVE");
    out.extend_from_slice(b"fmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&2u16.to_le_bytes());
    let sr = stereo.sample_rate.round() as u32;
    out.extend_from_slice(&sr.to_le_bytes());
    out.extend_from_slice(&(sr * 4).to_le_bytes());
    out.extend_from_slice(&4u16.to_le_bytes());
    out.extend_from_slice(&16u16.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&(data_bytes as u32).to_le_bytes());
    for sample in &stereo.data {
        let value = (sample.clamp(-1.0, 1.0) * 32_767.0).round() as i16;
        out.extend_from_slice(&value.to_le_bytes());
    }
    Ok(out)
}
