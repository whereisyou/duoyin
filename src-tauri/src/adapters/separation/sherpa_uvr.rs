#![cfg(feature = "inference")]

use std::ffi::{c_char, c_float, c_int, c_void, CString};
use std::path::{Path, PathBuf};

use crate::pipeline::runner::CancelToken;
use crate::ports::separator::{AudioSeparator, SeparationOutput, SeparatorError, SeparatorFuture};

#[repr(C)]
struct SpleeterConfig {
    vocals: *const c_char,
    accompaniment: *const c_char,
}

#[repr(C)]
struct UvrConfig {
    model: *const c_char,
}

#[repr(C)]
struct ModelConfig {
    spleeter: SpleeterConfig,
    uvr: UvrConfig,
    num_threads: c_int,
    debug: c_int,
    provider: *const c_char,
}

#[repr(C)]
struct SeparationConfig {
    model: ModelConfig,
}

#[repr(C)]
struct Stem {
    samples: *mut *mut c_float,
    num_channels: c_int,
    n: c_int,
}

#[repr(C)]
struct Output {
    stems: *const Stem,
    num_stems: c_int,
    sample_rate: c_int,
}

#[link(name = "sherpa-onnx-c-api")]
unsafe extern "C" {
    fn SherpaOnnxCreateOfflineSourceSeparation(config: *const SeparationConfig) -> *const c_void;
    fn SherpaOnnxDestroyOfflineSourceSeparation(engine: *const c_void);
    fn SherpaOnnxOfflineSourceSeparationProcess(
        engine: *const c_void,
        samples: *const *const c_float,
        num_channels: c_int,
        num_samples: c_int,
        sample_rate: c_int,
    ) -> *const Output;
    fn SherpaOnnxDestroySourceSeparationOutput(output: *const Output);
}

#[derive(Debug, Clone)]
pub struct SherpaUvrSeparator {
    model: PathBuf,
}

impl SherpaUvrSeparator {
    pub fn new(model: impl Into<PathBuf>) -> Self {
        Self {
            model: model.into(),
        }
    }
}

impl AudioSeparator for SherpaUvrSeparator {
    fn version(&self) -> String {
        "sherpa-onnx-uvr-v1".into()
    }

    fn resource_cost(&self) -> crate::scheduler::ResourceCost {
        crate::scheduler::SEPARATE.into()
    }

    fn separate<'a>(
        &'a self,
        input: &'a Path,
        staging_dir: &'a Path,
        cancel: &'a CancelToken,
    ) -> SeparatorFuture<'a> {
        Box::pin(async move {
            if !self.model.is_file() {
                return Err(SeparatorError::ModelUnavailable(format!(
                    "UVR 模型不存在: {}",
                    self.model.display()
                )));
            }
            if cancel.is_canceled() {
                return Err(SeparatorError::Canceled);
            }
            tokio::fs::create_dir_all(staging_dir)
                .await
                .map_err(|error| SeparatorError::Engine(error.to_string()))?;
            let converted = staging_dir.join("input-44k-stereo.wav");
            let status = tokio::process::Command::new("ffmpeg")
                .kill_on_drop(true)
                .args(["-v", "error", "-i"])
                .arg(input)
                .args(["-ar", "44100", "-ac", "2", "-c:a", "pcm_s16le", "-y"])
                .arg(&converted)
                .status()
                .await
                .map_err(|error| SeparatorError::Engine(error.to_string()))?;
            if !status.success() {
                return Err(SeparatorError::Engine(
                    "背景分离输入转 44.1kHz stereo 失败".into(),
                ));
            }
            let model = self.model.clone();
            let staging = staging_dir.to_owned();
            let cancel = cancel.clone();
            tokio::task::spawn_blocking(move || {
                run_separation(&model, &converted, &staging, &cancel)
            })
            .await
            .map_err(|error| SeparatorError::Engine(error.to_string()))?
        })
    }
}

struct Engine(*const c_void);
impl Drop for Engine {
    fn drop(&mut self) {
        unsafe { SherpaOnnxDestroyOfflineSourceSeparation(self.0) }
    }
}

struct OwnedOutput(*const Output);
impl Drop for OwnedOutput {
    fn drop(&mut self) {
        unsafe { SherpaOnnxDestroySourceSeparationOutput(self.0) }
    }
}

fn run_separation(
    model: &Path,
    input: &Path,
    staging: &Path,
    cancel: &CancelToken,
) -> Result<SeparationOutput, SeparatorError> {
    let model = CString::new(model.to_string_lossy().as_bytes())
        .map_err(|error| SeparatorError::Engine(error.to_string()))?;
    let provider = CString::new("cpu").unwrap();
    let config = SeparationConfig {
        model: ModelConfig {
            spleeter: SpleeterConfig {
                vocals: std::ptr::null(),
                accompaniment: std::ptr::null(),
            },
            uvr: UvrConfig {
                model: model.as_ptr(),
            },
            num_threads: 2,
            debug: 0,
            provider: provider.as_ptr(),
        },
    };
    let engine = unsafe { SherpaOnnxCreateOfflineSourceSeparation(&config) };
    if engine.is_null() {
        return Err(SeparatorError::Engine("创建 sherpa UVR 分离器失败".into()));
    }
    let engine = Engine(engine);
    let mut reader =
        hound::WavReader::open(input).map_err(|error| SeparatorError::Engine(error.to_string()))?;
    let spec = reader.spec();
    if spec.channels != 2 || spec.bits_per_sample != 16 {
        return Err(SeparatorError::InvalidInput(
            "UVR 输入必须是 stereo PCM16 WAV".into(),
        ));
    }
    let window = spec.sample_rate as usize * 30;
    let overlap = spec.sample_rate as usize;
    let step = window - overlap;
    let mut samples = reader.samples::<i16>();
    let vocals_path = staging.join("vocals.wav");
    let background_path = staging.join("background.wav");
    let out_spec = hound::WavSpec {
        channels: 2,
        sample_rate: spec.sample_rate,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut vocals_writer = hound::WavWriter::create(&vocals_path, out_spec)
        .map_err(|error| SeparatorError::Engine(error.to_string()))?;
    let mut bg_writer = hound::WavWriter::create(&background_path, out_spec)
        .map_err(|error| SeparatorError::Engine(error.to_string()))?;
    let mut pending_vocals: Option<Vec<Vec<f32>>> = None;
    let mut pending_bg: Option<Vec<Vec<f32>>> = None;
    let mut channels = vec![Vec::with_capacity(window), Vec::with_capacity(window)];
    let initial_read = fill_stereo_window(&mut samples, &mut channels, window)?;
    let mut is_last = initial_read < window;
    while !channels[0].is_empty() {
        if cancel.is_canceled() {
            return Err(SeparatorError::Canceled);
        }
        let frames = channels[0].len();
        let pointers = [channels[0].as_ptr(), channels[1].as_ptr()];
        let output = unsafe {
            SherpaOnnxOfflineSourceSeparationProcess(
                engine.0,
                pointers.as_ptr(),
                2,
                frames as c_int,
                spec.sample_rate as c_int,
            )
        };
        if output.is_null() {
            return Err(SeparatorError::Engine("UVR 分离返回空结果".into()));
        }
        let output = OwnedOutput(output);
        let (bg, vocals) = unsafe { copy_stems(output.0)? };
        write_crossfaded(
            &mut vocals_writer,
            &mut pending_vocals,
            vocals,
            overlap.min(frames),
            is_last,
        )?;
        write_crossfaded(
            &mut bg_writer,
            &mut pending_bg,
            bg,
            overlap.min(frames),
            is_last,
        )?;
        if is_last {
            break;
        }
        for channel in &mut channels {
            let keep_from = channel.len().saturating_sub(overlap);
            channel.drain(..keep_from);
        }
        let read = fill_stereo_window(&mut samples, &mut channels, step)?;
        if read == 0 {
            flush_pending(&mut vocals_writer, &mut pending_vocals)?;
            flush_pending(&mut bg_writer, &mut pending_bg)?;
            break;
        }
        is_last = read < step;
    }
    vocals_writer
        .finalize()
        .map_err(|error| SeparatorError::Engine(error.to_string()))?;
    bg_writer
        .finalize()
        .map_err(|error| SeparatorError::Engine(error.to_string()))?;
    Ok(SeparationOutput {
        vocals: vocals_path,
        background: background_path,
    })
}

fn fill_stereo_window(
    samples: &mut impl Iterator<Item = Result<i16, hound::Error>>,
    channels: &mut [Vec<f32>],
    frames: usize,
) -> Result<usize, SeparatorError> {
    let mut read_frames = 0;
    for _ in 0..frames {
        let Some(left) = samples.next() else {
            break;
        };
        let left = left.map_err(|error| SeparatorError::Engine(error.to_string()))?;
        let right = samples
            .next()
            .ok_or_else(|| SeparatorError::InvalidInput("stereo WAV 样本不完整".into()))?
            .map_err(|error| SeparatorError::Engine(error.to_string()))?;
        channels[0].push(left as f32 / 32768.0);
        channels[1].push(right as f32 / 32768.0);
        read_frames += 1;
    }
    Ok(read_frames)
}

unsafe fn copy_stems(
    output: *const Output,
) -> Result<(Vec<Vec<f32>>, Vec<Vec<f32>>), SeparatorError> {
    let output = unsafe { &*output };
    if output.num_stems < 2 {
        return Err(SeparatorError::Engine("UVR 输出少于 2 个 stems".into()));
    }
    let stems = unsafe { std::slice::from_raw_parts(output.stems, output.num_stems as usize) };
    let copy = |stem: &Stem| -> Result<Vec<Vec<f32>>, SeparatorError> {
        if stem.num_channels <= 0 || stem.n <= 0 || stem.samples.is_null() {
            return Err(SeparatorError::Engine("UVR stem 为空".into()));
        }
        let pointers =
            unsafe { std::slice::from_raw_parts(stem.samples, stem.num_channels as usize) };
        Ok(pointers
            .iter()
            .map(|pointer| unsafe {
                std::slice::from_raw_parts(*pointer, stem.n as usize).to_vec()
            })
            .collect())
    };
    Ok((copy(&stems[0])?, copy(&stems[1])?))
}

fn flush_pending(
    writer: &mut hound::WavWriter<std::io::BufWriter<std::fs::File>>,
    pending: &mut Option<Vec<Vec<f32>>>,
) -> Result<(), SeparatorError> {
    let Some(channels) = pending.take() else {
        return Ok(());
    };
    let frames = channels.first().map(Vec::len).unwrap_or(0);
    for index in 0..frames {
        for channel in &channels {
            write_sample(writer, channel[index])?;
        }
    }
    Ok(())
}

fn write_crossfaded(
    writer: &mut hound::WavWriter<std::io::BufWriter<std::fs::File>>,
    pending: &mut Option<Vec<Vec<f32>>>,
    current: Vec<Vec<f32>>,
    overlap: usize,
    last: bool,
) -> Result<(), SeparatorError> {
    let channels = current.len();
    let mut current = current;
    if let Some(previous) = pending.take() {
        let blend = overlap
            .min(previous.first().map(Vec::len).unwrap_or(0))
            .min(current.first().map(Vec::len).unwrap_or(0));
        let previous_body = previous[0].len().saturating_sub(blend);
        for index in 0..previous_body {
            for channel in 0..channels {
                write_sample(writer, previous[channel][index])?;
            }
        }
        for index in 0..blend {
            let t = index as f32 / blend.max(1) as f32;
            for channel in 0..channels {
                let mixed = previous[channel][previous_body + index] * (1.0 - t)
                    + current[channel][index] * t;
                write_sample(writer, mixed)?;
            }
        }
        for channel in &mut current {
            channel.drain(..blend);
        }
    }
    if last {
        let frames = current.first().map(Vec::len).unwrap_or(0);
        for index in 0..frames {
            for channel in 0..channels {
                write_sample(writer, current[channel][index])?;
            }
        }
    } else {
        *pending = Some(current);
    }
    Ok(())
}

fn write_sample(
    writer: &mut hound::WavWriter<std::io::BufWriter<std::fs::File>>,
    value: f32,
) -> Result<(), SeparatorError> {
    writer
        .write_sample((value.clamp(-1.0, 1.0) * 32767.0) as i16)
        .map_err(|error| SeparatorError::Engine(error.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_model_is_reported_before_ffi_call() {
        let engine = SherpaUvrSeparator::new("missing.onnx");
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let result = runtime.block_on(engine.separate(
            Path::new("input.wav"),
            Path::new("out"),
            &CancelToken::default(),
        ));
        assert!(matches!(result, Err(SeparatorError::ModelUnavailable(_))));
    }

    #[tokio::test]
    #[ignore = "requires local UVR model"]
    async fn real_uvr_model_produces_two_full_length_stems() {
        let model = std::env::var("VT_UVR_MODEL")
            .map(PathBuf::from)
            .unwrap_or_else(|_| PathBuf::from("../../models/onnx/UVR-MDX-NET-Inst_HQ_4.onnx"));
        if !model.is_file() {
            eprintln!("skip: UVR model not found at {}", model.display());
            return;
        }
        let root = std::env::temp_dir().join(format!("uvr-e2e-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let input = root.join("mixture.wav");
        let status = tokio::process::Command::new("ffmpeg")
            .args([
                "-v",
                "error",
                "-f",
                "lavfi",
                "-i",
                "sine=frequency=220:duration=3",
                "-f",
                "lavfi",
                "-i",
                "sine=frequency=880:duration=3",
                "-filter_complex",
                "[0:a][1:a]amix=inputs=2:duration=longest,pan=stereo|c0=c0|c1=c0",
                "-ar",
                "44100",
                "-y",
            ])
            .arg(&input)
            .status()
            .await
            .unwrap();
        assert!(status.success());
        let output = SherpaUvrSeparator::new(model)
            .separate(&input, &root.join("staging"), &CancelToken::default())
            .await
            .unwrap();
        for path in [output.vocals, output.background] {
            let reader = hound::WavReader::open(path).unwrap();
            assert_eq!(reader.spec().channels, 2);
            assert_eq!(reader.spec().sample_rate, 44100);
            assert!(reader.duration() > 44100 * 2);
        }
        std::fs::remove_dir_all(root).unwrap();
    }
}
