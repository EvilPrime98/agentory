
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, SyncSender, TrySendError};
use std::thread::JoinHandle;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, Sample, SampleFormat, SizedSample};

pub fn input_devices() -> Vec<String> {
    cpal::default_host()
        .input_devices()
        .map(|devices| devices.filter_map(|d| d.name().ok()).collect())
        .unwrap_or_default()
}

pub struct Capture {
    stop: Option<mpsc::Sender<()>>,
    thread: Option<JoinHandle<()>>,
    pub sample_rate: u32,
}

impl Drop for Capture {
    fn drop(&mut self) {
        drop(self.stop.take());
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

pub type ErrorSink = Arc<dyn Fn(String) + Send + Sync>;

pub fn start(
    device: &str,
    tx: SyncSender<Vec<f32>>,
    dropped: Arc<AtomicU64>,
    on_error: ErrorSink,
) -> Result<Capture, String> {
    let device = device.to_string();
    let (ready_tx, ready_rx) = mpsc::channel::<Result<u32, String>>();
    let (stop_tx, stop_rx) = mpsc::channel::<()>();
    let thread = std::thread::Builder::new()
        .name("agentory-mic".into())
        .spawn(move || match open(&device, tx, dropped, on_error) {
            Ok((stream, rate)) => {
                let _ = ready_tx.send(Ok(rate));
                let _ = stop_rx.recv();
                drop(stream);
            }
            Err(e) => {
                let _ = ready_tx.send(Err(e));
            }
        })
        .map_err(|e| e.to_string())?;
    match ready_rx.recv() {
        Ok(Ok(sample_rate)) => Ok(Capture {
            stop: Some(stop_tx),
            thread: Some(thread),
            sample_rate,
        }),
        Ok(Err(e)) => {
            let _ = thread.join();
            Err(e)
        }
        Err(_) => Err("microphone thread stopped unexpectedly".into()),
    }
}

fn pick(device: &str) -> Result<cpal::Device, String> {
    let host = cpal::default_host();
    if !device.is_empty() {
        if let Ok(mut all) = host.input_devices() {
            if let Some(d) = all.find(|d| d.name().is_ok_and(|n| n == device)) {
                return Ok(d);
            }
        }
        tracing::warn!("microphone `{device}` not found, using the default");
    }
    host.default_input_device()
        .ok_or_else(|| "no microphone found".to_string())
}

fn open(
    device: &str,
    tx: SyncSender<Vec<f32>>,
    dropped: Arc<AtomicU64>,
    on_error: ErrorSink,
) -> Result<(cpal::Stream, u32), String> {
    let device = pick(device)?;
    let supported = device
        .default_input_config()
        .map_err(|e| format!("cannot read the microphone configuration: {e}"))?;
    let format = supported.sample_format();
    let config: cpal::StreamConfig = supported.into();
    let rate = config.sample_rate.0;
    let err_cb = move |e: cpal::StreamError| on_error(e.to_string());
    let stream = match format {
        SampleFormat::F32 => build::<f32>(&device, &config, tx, dropped, err_cb),
        SampleFormat::I16 => build::<i16>(&device, &config, tx, dropped, err_cb),
        SampleFormat::U16 => build::<u16>(&device, &config, tx, dropped, err_cb),
        SampleFormat::I32 => build::<i32>(&device, &config, tx, dropped, err_cb),
        SampleFormat::F64 => build::<f64>(&device, &config, tx, dropped, err_cb),
        other => return Err(format!("unsupported microphone sample format {other:?}")),
    }
    .map_err(|e| format!("cannot open the microphone: {e}"))?;
    stream
        .play()
        .map_err(|e| format!("cannot start the microphone: {e}"))?;
    Ok((stream, rate))
}

fn build<T>(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    tx: SyncSender<Vec<f32>>,
    dropped: Arc<AtomicU64>,
    err_cb: impl FnMut(cpal::StreamError) + Send + 'static,
) -> Result<cpal::Stream, cpal::BuildStreamError>
where
    T: SizedSample,
    f32: FromSample<T>,
{
    let channels = usize::from(config.channels.max(1));
    device.build_input_stream(
        config,
        move |data: &[T], _| {
            let mono: Vec<f32> = data
                .chunks(channels)
                .map(|frame| {
                    frame.iter().map(|s| f32::from_sample(*s)).sum::<f32>() / channels as f32
                })
                .collect();
            if let Err(TrySendError::Full(_)) = tx.try_send(mono) {
                dropped.fetch_add(1, Ordering::Relaxed);
            }
        },
        err_cb,
        None,
    )
}
