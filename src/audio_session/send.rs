//! The thread that sends what is captured.

use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};

use crate::audio::{
    capture_to_wire, discard_queued, taps, BurstProbe, CaptureRing, GainRamp, WIRE_CHANNELS,
};
use crate::link::Senders;

/// What the send thread reads and is told, besides the audio itself
pub(super) struct SendFeed {
    /// The ring the capture callback fills. Replaced whole when the input is opened again.
    pub ring: Arc<Mutex<CaptureRing>>,
    /// The connections of the links that are up
    pub senders: Senders,
    pub is_muted: Arc<AtomicBool>,
    pub local_volume: Arc<AtomicU32>,
    pub local_pan: Arc<AtomicI32>,
    /// Cleared to stop the thread
    pub running: Arc<AtomicBool>,
    pub frame_size: usize,
    pub sample_rate: u32,
    /// Sees the frames as they are sent
    pub probe: Option<Arc<BurstProbe>>,
}

/// Starts the dedicated thread that sends the captured audio (no tokio overhead on the capture
/// side): it reads frames from the capture ring and sends each, through every link that is up
pub(super) fn spawn(feed: SendFeed) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        let SendFeed {
            ring,
            senders,
            is_muted,
            local_volume,
            local_pan,
            running,
            frame_size: send_buffer_size,
            sample_rate,
            probe,
        } = feed;

        // A mini tokio runtime just for async send operations
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("Failed to create send thread runtime");

        let mut timestamp: u32 = 0;
        // Room for the widest capture, so a switch between mono and stereo
        // needs no new allocation
        let mut send_buffer = vec![0.0f32; send_buffer_size * WIRE_CHANNELS];
        // Stereo buffer for pan-converted output (2x mono size)
        let mut stereo_send_buffer = vec![0.0f32; send_buffer_size * 2];
        // Muting fades the microphone out (and unmuting in) rather than cutting
        // it, so the other side does not hear a click.
        let mut mute_ramp = GainRamp::new(sample_rate);

        // Capture opened before this thread started, so the ring holds what
        // was captured meanwhile. Sent, that would be a burst of old audio
        // ahead of the live frames, and the other side's play-out buffer
        // overflows on it. From here on the ring is read as it fills whether
        // or not a link is up, so nothing old waits for the next link either.
        if let Ok(mut ring) = ring.lock() {
            discard_queued(&mut ring.consumer);
        }

        while running.load(Ordering::SeqCst) {
            // Try to read a frame from capture ring buffer. Yields the channel
            // count of the frame read: a frame is `send_buffer_size` samples
            // per channel.
            let channels_read = {
                if let Ok(mut ring) = ring.try_lock() {
                    let channels = ring.channels;
                    let frame_samples = send_buffer_size * channels;
                    let available = ring.consumer.slots();
                    if available >= frame_samples {
                        if let Ok(chunk) = ring.consumer.read_chunk(frame_samples) {
                            let slices = chunk.as_slices();
                            send_buffer[..slices.0.len()].copy_from_slice(slices.0);
                            if !slices.1.is_empty() {
                                send_buffer[slices.0.len()..frame_samples]
                                    .copy_from_slice(slices.1);
                            }
                            chunk.commit_all();
                            Some(channels)
                        } else {
                            None
                        }
                    } else {
                        None
                    }
                } else {
                    None
                }
            };

            if let Some(channels) = channels_read {
                let mute_target = if is_muted.load(Ordering::SeqCst) {
                    0.0
                } else {
                    1.0
                };
                // Nothing is sent once the fade-out has finished
                if !mute_ramp.is_silent_at(mute_target) {
                    let volume = local_volume.load(Ordering::SeqCst) as f32 / 100.0;
                    let pan = local_pan.load(Ordering::SeqCst);
                    capture_to_wire(
                        &send_buffer[..send_buffer_size * channels],
                        channels,
                        volume,
                        pan,
                        &mut stereo_send_buffer,
                    );
                    mute_ramp.apply(&mut stereo_send_buffer, WIRE_CHANNELS, mute_target);
                    if let Some(taps) = taps() {
                        taps.sent(&mut stereo_send_buffer);
                    }

                    // Everyone gets the same frame, each through their own link
                    let targets = senders
                        .read()
                        .map(|senders| senders.clone())
                        .unwrap_or_else(|e| e.into_inner().clone());
                    rt.block_on(async {
                        let mut noted = false;
                        for connection in targets.iter() {
                            let conn = connection.lock().await;
                            if conn.is_connected() {
                                if let (Some(probe), false) = (&probe, noted) {
                                    probe.note_sent(&stereo_send_buffer);
                                    noted = true;
                                }
                                if let Err(e) =
                                    conn.send_audio(&stereo_send_buffer, timestamp).await
                                {
                                    tracing::warn!("Failed to send audio: {}", e);
                                }
                            }
                        }
                    });
                }
                timestamp = timestamp.wrapping_add(send_buffer_size as u32);
            } else {
                // No data available, sleep briefly (100µs for low latency)
                std::thread::sleep(std::time::Duration::from_micros(100));
            }
        }
    })
}
