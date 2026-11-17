use super::*;

pub(super) fn wav(root: &Path, meeting: &Meeting, start_ms: u64) -> Result<Vec<u8>> {
    if meeting.audio_deleted {
        return Err(err("MeetingAudioDeleted", "录音已删除，仍可查看逐字稿。"));
    }
    let from = usize::try_from(start_ms)
        .ok()
        .and_then(|ms| ms.checked_mul(16))
        .ok_or_else(|| err("MeetingAudioRange", "片段时间超出录音范围。"))?;
    if from >= meeting.samples() {
        return Err(
            if ["preparing", "recording", "paused"].contains(&meeting.state.as_str()) {
                err("MeetingAudioPending", "本段录音还在保存，请稍后再试。")
            } else {
                err(
                    "MeetingAudioRange",
                    "此片段没有可回听的录音，仍可查看逐字稿。",
                )
            },
        );
    }
    let pcm = store::samples(root, meeting, from, 30 * store::RATE as usize).map_err(|_| {
        err(
            "MeetingAudioMissing",
            "本段录音文件缺失或损坏，仍可查看逐字稿。",
        )
    })?;
    let mut out = std::io::Cursor::new(Vec::new());
    {
        let mut writer = hound::WavWriter::new(
            &mut out,
            hound::WavSpec {
                channels: 1,
                sample_rate: store::RATE,
                bits_per_sample: 16,
                sample_format: hound::SampleFormat::Int,
            },
        )
        .map_err(store::io)?;
        for sample in pcm {
            writer
                .write_sample((sample.clamp(-1.0, 1.0) * 32767.0) as i16)
                .map_err(store::io)?;
        }
        writer.finalize().map_err(store::io)?;
    }
    Ok(out.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn playback_crosses_chunks_and_clamps_to_thirty_seconds_and_recording_end() {
        let root = tempfile::tempdir().unwrap();
        let mut m = Meeting::new("playback".into(), "microphone", "zh".into());
        store::save(root.path(), &m).unwrap();
        for value in [0.1, 0.2, 0.3, 0.4, 0.5, 0.6, 0.7] {
            store::append(root.path(), &mut m, &vec![value; 80_000]).unwrap();
        }
        let bytes = wav(root.path(), &m, 4000).unwrap();
        let reader = hound::WavReader::new(std::io::Cursor::new(bytes)).unwrap();
        assert_eq!(reader.spec().sample_rate, 16000);
        assert_eq!(reader.spec().bits_per_sample, 16);
        let samples = reader
            .into_samples::<i16>()
            .collect::<std::result::Result<Vec<_>, _>>()
            .unwrap();
        assert_eq!(samples.len(), 30 * 16000);
        assert_eq!(samples[0], (0.1 * 32767.0) as i16);
        assert_eq!(samples[16000], (0.2 * 32767.0) as i16);
        let tail =
            hound::WavReader::new(std::io::Cursor::new(wav(root.path(), &m, 34_000).unwrap()))
                .unwrap();
        assert_eq!(tail.duration(), 16000);
    }

    #[test]
    fn unavailable_audio_returns_a_specific_error_instead_of_an_empty_wav() {
        let root = tempfile::tempdir().unwrap();
        let mut m = Meeting::new("missing".into(), "microphone", "zh".into());
        m.state = "recording".into();
        store::save(root.path(), &m).unwrap();
        assert_eq!(
            wav(root.path(), &m, 0).unwrap_err().code,
            "MeetingAudioPending"
        );
        m.state = "ready".into();
        assert_eq!(
            wav(root.path(), &m, 0).unwrap_err().code,
            "MeetingAudioRange"
        );
        store::append(root.path(), &mut m, &[0.25; 16000]).unwrap();
        assert_eq!(
            wav(root.path(), &m, u64::MAX).unwrap_err().code,
            "MeetingAudioRange"
        );
        std::fs::remove_file(root.path().join(m.id.to_string()).join("00000.pcm")).unwrap();
        assert_eq!(
            wav(root.path(), &m, 0).unwrap_err().code,
            "MeetingAudioMissing"
        );
        m.audio_deleted = true;
        assert_eq!(
            wav(root.path(), &m, 0).unwrap_err().code,
            "MeetingAudioDeleted"
        );
    }
}
