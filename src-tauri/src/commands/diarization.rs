use std::sync::Mutex;

use serde::Serialize;

#[cfg(feature = "inference")]
use crate::application::task_service::TaskService;
#[cfg(feature = "inference")]
use crate::domain::ids::TaskId;
use crate::types::AppConfig;
#[cfg(feature = "inference")]
use crate::types::Segment;

#[derive(Debug, Clone, Serialize)]
pub struct SpeakerAssignment {
    pub segment_idx: usize,
    pub speaker: String,
}

#[tauri::command]
pub async fn run_speaker_diarization(
    state: tauri::State<'_, Mutex<AppConfig>>,
    task_id: String,
) -> Result<Vec<SpeakerAssignment>, String> {
    let config = state.lock().map_err(|error| error.to_string())?.clone();
    #[cfg(not(feature = "inference"))]
    {
        let _ = (config, task_id);
        return Err("推理功能未启用".into());
    }
    #[cfg(feature = "inference")]
    {
        let service = TaskService::from_local_app_data()?;
        let root = service
            .store()
            .task_dir(&TaskId(task_id))
            .map_err(|error| error.to_string())?;
        let audio = root.join("shared/audio.wav");
        let segments: Vec<Segment> = serde_json::from_slice(
            &tokio::fs::read(root.join("shared/segments.json"))
                .await
                .map_err(|error| error.to_string())?,
        )
        .map_err(|error| error.to_string())?;
        let output = tokio::task::spawn_blocking(move || diarize(&config, &audio, &segments))
            .await
            .map_err(|error| error.to_string())??;
        tokio::fs::write(
            root.join("shared/speaker.json"),
            serde_json::to_vec_pretty(&output).map_err(|error| error.to_string())?,
        )
        .await
        .map_err(|error| error.to_string())?;
        Ok(output)
    }
}

#[cfg(feature = "inference")]
fn diarize(
    config: &AppConfig,
    audio: &std::path::Path,
    subtitles: &[Segment],
) -> Result<Vec<SpeakerAssignment>, String> {
    use sherpa_onnx::{
        FastClusteringConfig, OfflineSpeakerDiarization, OfflineSpeakerDiarizationConfig,
        OfflineSpeakerSegmentationModelConfig, OfflineSpeakerSegmentationPyannoteModelConfig,
        SpeakerEmbeddingExtractorConfig,
    };
    for path in [
        &config.diarization_seg_model,
        &config.diarization_embedding_model,
    ] {
        if !std::path::Path::new(path).is_file() {
            return Err(format!("说话人模型不存在: {path}"));
        }
    }
    let diarizer = OfflineSpeakerDiarization::create(&OfflineSpeakerDiarizationConfig {
        segmentation: OfflineSpeakerSegmentationModelConfig {
            pyannote: OfflineSpeakerSegmentationPyannoteModelConfig {
                model: Some(config.diarization_seg_model.clone()),
            },
            num_threads: 2,
            debug: false,
            provider: Some("cpu".into()),
        },
        embedding: SpeakerEmbeddingExtractorConfig {
            model: Some(config.diarization_embedding_model.clone()),
            num_threads: 2,
            debug: false,
            provider: Some("cpu".into()),
        },
        clustering: FastClusteringConfig {
            num_clusters: config.diarization_num_speakers,
            threshold: 0.5,
        },
        min_duration_on: 0.3,
        min_duration_off: 0.5,
    })
    .ok_or("创建说话人分离器失败")?;
    let reader = hound::WavReader::open(audio).map_err(|error| error.to_string())?;
    let spec = reader.spec();
    if spec.channels != 1 || spec.sample_rate != diarizer.sample_rate() as u32 {
        return Err(format!(
            "说话人音频格式不匹配：需要 {}Hz mono，实际 {}Hz {}ch",
            diarizer.sample_rate(),
            spec.sample_rate,
            spec.channels
        ));
    }
    let samples = reader
        .into_samples::<i16>()
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| error.to_string())?
        .into_iter()
        .map(|sample| sample as f32 / 32768.0)
        .collect::<Vec<_>>();
    let result = diarizer.process(&samples).ok_or("说话人分离无结果")?;
    let diarized = result.sort_by_start_time();
    let mut speaker_ids = std::collections::BTreeMap::new();
    let mut next_id = 0usize;
    let normalized = diarized
        .into_iter()
        .map(|segment| {
            let speaker = *speaker_ids.entry(segment.speaker).or_insert_with(|| {
                let value = next_id;
                next_id += 1;
                value
            });
            (segment.start as f64, segment.end as f64, speaker)
        })
        .collect::<Vec<_>>();
    Ok(subtitles
        .iter()
        .map(|subtitle| {
            let mut overlaps = std::collections::BTreeMap::<usize, f64>::new();
            for (start, end, speaker) in &normalized {
                let overlap = (subtitle.end.min(*end) - subtitle.start.max(*start)).max(0.0);
                if overlap > 0.0 {
                    *overlaps.entry(*speaker).or_default() += overlap;
                }
            }
            let speaker = overlaps
                .into_iter()
                .max_by(|a, b| a.1.total_cmp(&b.1))
                .map(|(speaker, _)| speaker)
                .unwrap_or(0);
            SpeakerAssignment {
                segment_idx: subtitle.idx,
                speaker: format!("spk{speaker}"),
            }
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn assignment_is_serializable() {
        assert!(serde_json::to_string(&SpeakerAssignment {
            segment_idx: 0,
            speaker: "spk0".into(),
        })
        .is_ok());
    }
}
