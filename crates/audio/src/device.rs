use crate::error::{AudioError, Result};
use cpal::{
    traits::{DeviceTrait, HostTrait},
    Device, DeviceId, SampleFormat, SupportedStreamConfig,
};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Format {
    F32,
    I16,
    U16,
}
impl TryFrom<SampleFormat> for Format {
    type Error = AudioError;
    fn try_from(format: SampleFormat) -> Result<Self> {
        match format {
            SampleFormat::F32 => Ok(Self::F32),
            SampleFormat::I16 => Ok(Self::I16),
            SampleFormat::U16 => Ok(Self::U16),
            _ => Err(AudioError::UnsupportedAudioFormat),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Configuration {
    pub sample_rate: u32,
    pub channels: u16,
    pub sample_format: Format,
}
impl Configuration {
    pub fn validate(self) -> Result<Self> {
        if !(8000..=192000).contains(&self.sample_rate) || !(1..=8).contains(&self.channels) {
            return Err(AudioError::UnsupportedAudioFormat);
        }
        Ok(self)
    }
    pub fn stream_config(self) -> cpal::StreamConfig {
        cpal::StreamConfig {
            channels: self.channels,
            sample_rate: self.sample_rate,
            buffer_size: cpal::BufferSize::Default,
        }
    }
}
impl TryFrom<&SupportedStreamConfig> for Configuration {
    type Error = AudioError;
    fn try_from(config: &SupportedStreamConfig) -> Result<Self> {
        Self {
            sample_rate: config.sample_rate(),
            channels: config.channels(),
            sample_format: Format::try_from(config.sample_format())?,
        }
        .validate()
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InputDevice {
    pub id: String,
    pub name: String,
    pub is_default: bool,
    pub configuration: Option<Configuration>,
}

pub(crate) fn map_error(error: cpal::Error) -> AudioError {
    match error.kind() {
        cpal::ErrorKind::PermissionDenied => AudioError::PermissionDenied,
        cpal::ErrorKind::DeviceBusy => AudioError::DeviceBusy,
        cpal::ErrorKind::UnsupportedConfig
        | cpal::ErrorKind::UnsupportedOperation
        | cpal::ErrorKind::InvalidInput => AudioError::UnsupportedAudioFormat,
        cpal::ErrorKind::ResourceExhausted => AudioError::MemoryUnavailable,
        _ => AudioError::DeviceUnavailable,
    }
}

/// Querying devices does not construct or play an input stream.
pub fn list_inputs() -> Result<Vec<InputDevice>> {
    let host = cpal::default_host();
    let default = host
        .default_input_device()
        .and_then(|device| device.id().ok());
    let devices = host.input_devices().map_err(map_error)?;
    let mut result = Vec::new();
    for device in devices.take(256) {
        let Ok(id) = device.id() else {
            continue;
        };
        let name = device
            .description()
            .map(|description| description.name().to_owned())
            .unwrap_or_else(|_| "Audio device".into());
        result.push(InputDevice {
            is_default: default.as_ref() == Some(&id),
            id: id.to_string(),
            name,
            configuration: negotiate(&device).ok(),
        });
    }
    Ok(result)
}

/// Negotiates only the selected input (an ID from `list_inputs`), or the current
/// default when `None`, using the same rules as capture. Opens no stream.
/// A missing or invalid selection is `DeviceUnavailable` and never falls back to
/// another device; no default input is `NoInputDevice`.
pub fn probe(selected: Option<&str>) -> Result<Configuration> {
    let (device, _) = resolve(selected)?;
    negotiate(&device)
}

pub(crate) fn resolve(selected: Option<&str>) -> Result<(Device, String)> {
    let host = cpal::default_host();
    let id = match selected {
        Some(value) if value.len() <= 1024 && !value.chars().any(char::is_control) => value
            .parse::<DeviceId>()
            .map_err(|_| AudioError::DeviceUnavailable)?,
        Some(_) => return Err(AudioError::DeviceUnavailable),
        None => host
            .default_input_device()
            .ok_or(AudioError::NoInputDevice)?
            .id()
            .map_err(map_error)?,
    };
    if id.host() != host.id() {
        return Err(AudioError::DeviceUnavailable);
    }
    // Resolve the default to a concrete ID, freezing this recording's device.
    // A persisted selection never silently falls back to another microphone.
    let device = host
        .device_by_id(&id)
        .ok_or(AudioError::DeviceUnavailable)?;
    if !device.supports_input() {
        return Err(AudioError::DeviceUnavailable);
    }
    Ok((device, id.to_string()))
}

pub(crate) fn negotiate(device: &Device) -> Result<Configuration> {
    let default = device.default_input_config().ok();
    if let Some(config) = default
        .as_ref()
        .and_then(|config| Configuration::try_from(config).ok())
    {
        return Ok(config);
    }
    let desired = default
        .as_ref()
        .map_or(48000, SupportedStreamConfig::sample_rate);
    let mut candidates = Vec::new();
    for range in device.supported_input_configs().map_err(map_error)? {
        let Ok(format) = Format::try_from(range.sample_format()) else {
            continue;
        };
        let min = range.min_sample_rate().max(8000);
        let max = range.max_sample_rate().min(192000);
        if min > max || !(1..=8).contains(&range.channels()) {
            continue;
        }
        let rate = desired.clamp(min, max);
        candidates.push(Configuration {
            sample_rate: rate,
            channels: range.channels(),
            sample_format: format,
        });
    }
    candidates.sort_by_key(|config| {
        (
            config.sample_rate.abs_diff(desired),
            config.channels,
            match config.sample_format {
                Format::F32 => 0,
                Format::I16 => 1,
                Format::U16 => 2,
            },
        )
    });
    candidates
        .into_iter()
        .next()
        .ok_or(AudioError::UnsupportedAudioFormat)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn probe_rejects_malformed_selection_without_falling_back() {
        for selected in ["bad\u{7}id".to_owned(), "x".repeat(1025)] {
            assert_eq!(probe(Some(&selected)), Err(AudioError::DeviceUnavailable));
        }
    }
}
