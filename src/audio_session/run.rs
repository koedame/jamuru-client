//! The audio thread of a session: opens the devices, serves the commands, and keeps the links
//! (ADR-077).

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::mpsc as std_mpsc;
use std::sync::{Arc, RwLock};
#[cfg(target_os = "windows")]
use std::thread;

use super::links::{senders_of, Link, LinkSetup, LinkStage};
use super::shared::{LinkReport, Shared, MAX_LINKS};
use super::stand_in::StandIns;
use super::{send, Command, Params};
use crate::audio::mic_access::SystemMic;
use crate::audio::{
    capture_or_silence, clear_device_problem, microphone_allows_input, note_device_problem,
    note_trouble, playout_source, start_capture_ring, start_playout, AudioConfig, AudioEngine,
    CaptureRing, DeviceId, DeviceSide, DeviceSlot, DeviceTrouble, FlightKind, LocalMonitor,
    PeerMix, RecordingFeed, Verdict, ADAPT_INTERVAL, MIC_ANSWER_WAIT, WIRE_CHANNELS,
};
#[cfg(target_os = "windows")]
use crate::audio::{open_asio, shared_asio_driver, AsioDuplex, AsioSwitch};
use crate::link::Senders;
use crate::network::{audio_packet_duration_ms, required_bps, AudioEncodingConfig, UdpMux};
use crate::protocol::LatencyInfoMessage;

/// How long the receive loop waits when it has nothing to hand to playback.
///
/// Short enough to be irrelevant to the latency budget (ADR-008) and long
/// enough to hand the runtime back to the task that receives packets, which
/// shares this thread. The playback ring buffer holds eight frames, so a wake
/// that the OS timer rounds up to a millisecond still refills it in time.
const POP_IDLE: tokio::time::Duration = tokio::time::Duration::from_micros(250);

/// How often connection stats and the meter levels are refreshed for the UI,
/// which polls at the same interval.
const STATS_INTERVAL: std::time::Duration = std::time::Duration::from_millis(100);

/// Runs the audio session on the audio thread until it is told to stop
pub(super) async fn run(
    params: Params,
    cmd_rx: std_mpsc::Receiver<Command>,
    shared: &Shared,
    ready: &std_mpsc::Sender<Result<(), String>>,
) -> Result<(), String> {
    let Params {
        prepared_socket,
        input_device_id,
        output_device_id,
        buffer_size,
        preset,
        sample_rate,
        transmit_channels,
        mut input_channels,
        mut output_channels,
        input_signal,
        output_file,
        probe,
    } = params;
    let Shared {
        is_active,
        is_muted,
        is_monitoring,
        input_level,
        output_level,
        links: shared_links,
        ended,
        master_volume,
        local_volume,
        local_pan,
        device_problems,
        underrun_count,
        delay_adjustments,
        adjusted_delay_us,
        ..
    } = shared;

    // Capture config: mono (for network transmission)
    // Sample rate from config (ADR-013: user-selectable)
    let capture_config = AudioConfig {
        sample_rate,
        channels: 1,
        frame_size: buffer_size,
    };

    // Playback config: stereo (for pan support)
    let playback_config = AudioConfig {
        sample_rate,
        channels: 2,
        frame_size: buffer_size,
    };

    // Everyone in the room sends to the one address this app told it, so every
    // link goes through that socket (the one `streaming_prepare` bound).
    let mux = UdpMux::from_std(prepared_socket)
        .map_err(|e| format!("Failed to adopt the prepared audio socket: {}", e))?;

    // Create separate audio engines for capture (mono) and playback (stereo)
    let mut capture_engine = AudioEngine::new(capture_config.clone());
    let mut playback_engine = AudioEngine::new(playback_config.clone());

    let input_id = input_device_id.map(DeviceId);
    let output_id = output_device_id.map(DeviceId);

    // Zero-allocation audio pipeline using rtrb for both capture and playback:
    // - Capture: rtrb ring buffer (FnMut callback, no Sync requirement)
    // - Playback: rtrb ring buffer (wait-free network callback)
    // - Send thread: dedicated std::thread (no tokio overhead)

    // Configure the codec and FEC from the preset (ADR-021).
    let encoding = AudioEncodingConfig {
        codec_type: preset.codec_type(),
        sample_rate,
        channels: 2, // stereo on the wire, with local_pan already applied
        frame_size: buffer_size,
        bitrate: 0, // unused for PCM
        fec_group_size: preset.fec_group_size(),
    };

    // The play-out delay a link starts with. A preset switch moves it for the
    // links there are and for those opened later.
    let mut jitter_buffer_frames = preset.jitter_buffer_frames();
    // The buffer counts packets, and a frame too big for one packet goes as
    // several (ADR-070), so its depth lasts packets long, not frames long.
    let packet_duration_ms = audio_packet_duration_ms(buffer_size, WIRE_CHANNELS, sample_rate);
    // Everyone the session has a link to is mixed here for the output callback
    let mix = PeerMix::new(MAX_LINKS, sample_rate, buffer_size, underrun_count.clone());
    let stereo_frame_size = mix.frame_samples();

    let bandwidth_required_bps = required_bps(&preset, sample_rate, 2);

    let input_level_for_capture = Arc::new(AtomicU32::new(0));

    // Local monitoring: what the capture callback hears goes straight to the
    // output callback, never through the network (ADR-033).
    let monitor = LocalMonitor::new(buffer_size);
    monitor.set_enabled(is_monitoring.load(Ordering::SeqCst));

    // Recording/streaming virtual output (rm:1068): self (A) / peer (B) / mix
    // (C) as one 6ch device a DAW or OBS can pick as a recording input.
    // `None` where the platform or environment cannot provide one; the
    // session runs the same either way.
    // Not with stand-ins for the devices: they run where there is no sound card.
    let recording_feed = if input_signal.is_some() || output_file.is_some() {
        None
    } else {
        RecordingFeed::start(sample_rate).map(Arc::new)
    };

    // Start audio capture with level metering. Mono or stereo follows the
    // transmit channel setting (REQ-AUD-107/108).
    let mut wanted_channels = transmit_channels.clamp(1, WIRE_CHANNELS as u32) as u16;

    // The input and output settings name the same ASIO driver (Windows
    // only): one ASIO session serves both directions instead of the two
    // engines below (see `audio::asio`).
    #[cfg(target_os = "windows")]
    let asio_driver = if input_signal.is_some() || output_file.is_some() {
        None
    } else {
        shared_asio_driver(input_id.as_ref(), output_id.as_ref())
    };
    #[cfg(not(target_os = "windows"))]
    let asio_driver: Option<String> = None;
    #[cfg(target_os = "windows")]
    let mut asio_session: Option<AsioDuplex> = None;

    let mut stand_ins = StandIns::new(sample_rate, buffer_size);

    // The ring, and whether the input opened: one that did not has no stream
    // to watch
    let (capture_ring, capture_opened) = match &asio_driver {
        #[cfg(target_os = "windows")]
        Some(driver_name) => match open_asio(
            driver_name,
            input_id.as_ref(),
            output_id.as_ref(),
            sample_rate,
            buffer_size,
            stereo_frame_size,
            wanted_channels,
            input_channels,
            output_channels,
            input_level_for_capture.clone(),
            monitor.clone(),
            mix.clone(),
            master_volume.clone(),
            output_level.clone(),
            recording_feed.as_deref(),
        ) {
            Ok((session, ring)) => {
                tracing::info!("ASIO session started on {}", driver_name);
                clear_device_problem(device_problems, DeviceSide::Input);
                clear_device_problem(device_problems, DeviceSide::Output);
                asio_session = Some(session);
                (ring, true)
            }
            Err(e) => {
                note_device_problem(
                    device_problems,
                    DeviceSide::Input,
                    input_id.as_ref(),
                    sample_rate,
                    &e,
                );
                note_device_problem(
                    device_problems,
                    DeviceSide::Output,
                    output_id.as_ref(),
                    sample_rate,
                    &e,
                );
                return Err(format!("Failed to start the ASIO session: {}", e));
            }
        },
        #[cfg(not(target_os = "windows"))]
        Some(_) => unreachable!("asio_driver is always None off Windows"),
        None => {
            let (ring, opened_ok) = match &input_signal {
                Some(signal) => (
                    stand_ins.input(signal, &monitor, &input_level_for_capture),
                    false,
                ),
                None if !microphone_allows_input(
                    &SystemMic,
                    device_problems,
                    sample_rate,
                    MIC_ANSWER_WAIT,
                ) =>
                {
                    (CaptureRing::silent(wanted_channels as usize), false)
                }
                None => {
                    let opened = start_capture_ring(
                        &mut capture_engine,
                        input_id.as_ref(),
                        wanted_channels,
                        input_channels,
                        buffer_size as usize,
                        &input_level_for_capture,
                        &monitor,
                        recording_feed.as_deref(),
                    );
                    let opened_ok = opened.is_ok();
                    let ring = capture_or_silence(
                        opened,
                        wanted_channels,
                        device_problems,
                        input_id.as_ref(),
                        sample_rate,
                    );
                    (ring, opened_ok)
                }
            };

            // Start audio playback (stereo). The callback takes frames from
            // the play-out buffer itself and applies the mixer gains, so a
            // volume change is heard on the next frame and nothing queues
            // behind the buffer (ADR-028).
            match &output_file {
                Some(path) => {
                    let source = playout_source(
                        mix.clone(),
                        master_volume.clone(),
                        output_level.clone(),
                        monitor.clone(),
                        None,
                        probe.clone(),
                    );
                    stand_ins.output_file(path, stereo_frame_size, source)?;
                }
                None => {
                    if let Err(e) = start_playout(
                        &mut playback_engine,
                        output_id.as_ref(),
                        output_channels,
                        stereo_frame_size,
                        || {
                            playout_source(
                                mix.clone(),
                                master_volume.clone(),
                                output_level.clone(),
                                monitor.clone(),
                                recording_feed
                                    .as_deref()
                                    .map(|feed| (feed.tap_b(), feed.tap_c())),
                                probe.clone(),
                            )
                        },
                    ) {
                        note_device_problem(
                            device_problems,
                            DeviceSide::Output,
                            output_id.as_ref(),
                            sample_rate,
                            &e,
                        );
                        return Err(format!("Failed to start playback: {}", e));
                    }
                    tracing::info!("Playback started on {:?}", output_id);
                }
            }
            (ring, opened_ok)
        }
    };
    let capture_channels = capture_ring.channels;
    let capture_ring = Arc::new(std::sync::Mutex::new(capture_ring));

    let mut capture = DeviceSlot::new("input", capture_config, capture_engine, input_id.clone());
    if !capture_opened {
        capture = capture.without_stream();
    }
    let mut playback: DeviceSlot<()> = DeviceSlot::new(
        "output",
        playback_config,
        playback_engine,
        output_id.clone(),
    );
    if output_file.is_some() {
        playback = playback.without_stream();
    }

    // What the thread that opens the input device runs, for a new device, new
    // channels, and a stream that stopped calling back: the ring is made
    // anew, and the send thread takes it once the open has finished.
    let capture_job = |device: Option<DeviceId>, wanted: u16, picks: (u32, Option<u32>)| {
        let input_level = input_level_for_capture.clone();
        let monitor = monitor.clone();
        let recording_feed = recording_feed.clone();
        move |engine: &mut AudioEngine| {
            start_capture_ring(
                engine,
                device.as_ref(),
                wanted,
                picks,
                buffer_size as usize,
                &input_level,
                &monitor,
                recording_feed.as_deref(),
            )
        }
    };

    // What the thread that switches the output device runs: the stream is
    // rebuilt around the same play-out buffer, so the audio already waiting
    // in it survives the switch.
    let playout_job = |device: Option<DeviceId>, channels: (u32, Option<u32>)| {
        let mix = mix.clone();
        let master_volume = master_volume.clone();
        let output_level = output_level.clone();
        let monitor = monitor.clone();
        let recording_feed = recording_feed.clone();
        let probe = probe.clone();
        move |engine: &mut AudioEngine| {
            engine.stop_playback();
            start_playout(engine, device.as_ref(), channels, stereo_frame_size, || {
                playout_source(
                    mix.clone(),
                    master_volume.clone(),
                    output_level.clone(),
                    monitor.clone(),
                    recording_feed
                        .as_deref()
                        .map(|feed| (feed.tap_b(), feed.tap_c())),
                    probe.clone(),
                )
            })
        }
    };

    tracing::info!("Streaming active");

    // What every peer is told about our audio when its link comes up (ADR-013:
    // sample rate communication). The delay is that of the peer's own
    // play-out buffer, filled in as it is sent.
    let mut local_latency_info = LatencyInfoMessage {
        capture_buffer_ms: (buffer_size as f32 / sample_rate as f32) * 1000.0,
        playback_buffer_ms: (buffer_size as f32 / sample_rate as f32) * 1000.0,
        encode_ms: 0.0, // PCM, no encoding delay
        decode_ms: 0.0, // PCM, no decoding delay
        jitter_buffer_ms: jitter_buffer_frames as f32 * packet_duration_ms,
        frame_size: buffer_size,
        sample_rate,
        codec: "pcm".to_string(),
        channel_count: capture_channels as u8,
    };

    let mut links: Vec<Link> = Vec::new();
    let link_setup = LinkSetup {
        mux: &mux,
        mix: &mix,
        encoding: &encoding,
        sample_rate,
        buffer_size,
        packet_duration_ms,
        shared,
    };
    let senders: Senders = Arc::new(RwLock::new(Arc::from(Vec::new())));

    // The send thread has copies of what it reads, which the commands below keep up to date
    let is_muted_for_send = Arc::new(AtomicBool::new(is_muted.load(Ordering::SeqCst)));
    let local_volume_for_send = Arc::new(AtomicU32::new(local_volume.load(Ordering::SeqCst)));
    let local_pan_for_send = Arc::new(std::sync::atomic::AtomicI32::new(
        local_pan.load(Ordering::SeqCst),
    ));

    // Flag to signal send thread to stop
    let send_thread_running = Arc::new(AtomicBool::new(true));

    // Spawn dedicated thread to send captured audio (no tokio overhead)
    // This thread reads from rtrb consumer and sends via connection
    let send_thread = send::spawn(send::SendFeed {
        ring: capture_ring.clone(),
        senders: senders.clone(),
        is_muted: is_muted_for_send.clone(),
        local_volume: local_volume_for_send.clone(),
        local_pan: local_pan_for_send.clone(),
        running: send_thread_running.clone(),
        frame_size: buffer_size as usize,
        sample_rate,
        probe: probe.clone(),
    });

    // Everything the session needs is open: whoever started it can go on
    let _ = ready.send(Ok(()));

    // Main loop: process received audio and check for stop command
    let mut next_stats_at = std::time::Instant::now();
    let mut next_adapt_at = std::time::Instant::now() + ADAPT_INTERVAL;

    // Pre-allocate buffers to avoid allocation in hot path
    // Receive buffer is stereo (frame_size * 2) since sender transmits stereo with local_pan applied

    // This thread carries the network task as well as this loop, so a turn of
    // the loop that comes late is a stretch in which no packet was taken off
    // the socket either.
    let mut last_turn = std::time::Instant::now();
    let stall_after = std::time::Duration::from_micros(2 * mix.frame_us());

    // The ASIO driver currently in use, if any - what the reconciliation
    // below compares a device or channel change against to decide whether
    // the path in use (ASIO, or the two engines) needs to change.
    #[cfg(target_os = "windows")]
    let mut asio_driver_in_use = asio_driver;
    #[cfg(target_os = "windows")]
    let mut asio_switch: AsioSwitch<CaptureRing> = AsioSwitch::idle();

    loop {
        let pass_started = crate::perf::start();
        let turn_started = std::time::Instant::now();
        let away = turn_started.duration_since(last_turn);
        if away > stall_after {
            for link in &links {
                link.receive
                    .flight()
                    .note(FlightKind::ThreadStall, away.as_micros() as u64);
            }
        }
        last_turn = turn_started;

        // Set by a command that has capture, or playback, start over: a new
        // device, or a new channel count on the current one. Deferred to
        // after the match below (rather than switched right away, the way
        // this loop used to handle playback) so the two can be reconciled
        // with ASIO together: whichever changes, the pair might now share
        // (or stop sharing) an ASIO driver, which takes over both directions
        // at once.
        let mut reopen_capture: Option<Option<DeviceId>> = None;
        let mut reopen_playback: Option<Option<DeviceId>> = None;

        // Check for commands (non-blocking)
        match cmd_rx.try_recv() {
            Ok(Command::Stop) => {
                tracing::info!("Stopping streaming...");
                break;
            }
            Ok(Command::SetInputDevice(device_id)) => {
                tracing::info!("Switching input device to: {:?}", device_id);
                reopen_capture = Some(device_id.map(DeviceId));
            }
            Ok(Command::SetTransmitChannels(count)) => {
                tracing::info!("Setting transmit channels to: {}", count);
                wanted_channels = count.clamp(1, WIRE_CHANNELS as u32) as u16;
                reopen_capture = Some(capture.device().cloned());
            }
            Ok(Command::SetOutputDevice(device_id)) => {
                tracing::info!("Switching output device to: {:?}", device_id);
                reopen_playback = Some(device_id.map(DeviceId));
            }
            Ok(Command::SetOutputChannels(left, right)) => {
                tracing::info!("Setting output channels to: {} / {:?}", left, right);
                output_channels = (left, right);
                reopen_playback = Some(playback.device().cloned());
            }
            Ok(Command::SetInputChannels(left, right)) => {
                tracing::info!("Setting input channels to: {} / {:?}", left, right);
                input_channels = (left, right);
                reopen_capture = Some(capture.device().cloned());
            }
            Ok(Command::SetMute(muted)) => {
                tracing::info!("Setting mute state to: {}", muted);
                is_muted_for_send.store(muted, Ordering::SeqCst);
            }
            Ok(Command::SetMonitoring(enabled)) => {
                monitor.set_enabled(enabled);
            }
            Ok(Command::SetMasterVolume(vol)) => {
                tracing::debug!("Setting master volume to: {}", vol);
                master_volume.store((vol * 100.0) as u32, Ordering::SeqCst);
            }
            Ok(Command::SetLocalVolume(vol)) => {
                tracing::debug!("Setting local volume to: {}", vol);
                let volume_u32 = (vol * 100.0) as u32;
                local_volume.store(volume_u32, Ordering::SeqCst);
                // Also update the send thread's copy
                local_volume_for_send.store(volume_u32, Ordering::SeqCst);
            }
            Ok(Command::SetLocalPan(pan)) => {
                tracing::debug!("Setting local pan to: {}", pan);
                local_pan.store(pan, Ordering::SeqCst);
                // Also update the send thread's copy
                local_pan_for_send.store(pan, Ordering::SeqCst);
            }
            Ok(Command::Reconnect) => {
                tracing::info!("User requested reconnection");
                // A link that is up, or not up yet, has no loss to recover from
                for link in &links {
                    if let Some(connection) = link.connection() {
                        if let Err(e) = connection.lock().await.reconnect() {
                            tracing::warn!("Reconnect refused: {}", e);
                        }
                    }
                }
            }
            Ok(Command::SetJitterBufferFrames(frames)) => {
                tracing::info!("Setting the play-out delay to {} frame(s)", frames);
                jitter_buffer_frames = frames;
                // What is held moves to the new delay (ADR-031). The display
                // and the peers hear about it from the stats tick below.
                for link in &links {
                    link.receive.set_delay_frames(frames);
                }
            }
            Ok(Command::Link(target)) => {
                let peer = target.peer;
                // Asked for twice: the link there is is the one to keep
                if links.iter().any(|link| link.peer == peer) {
                    tracing::debug!("There is a link to {} already", peer);
                } else {
                    // What is left of an attempt that could not be started
                    if let Ok(mut shared) = shared_links.write() {
                        shared.retain(|(linked, _)| *linked != peer);
                    }
                    match link_setup.open(*target, jitter_buffer_frames) {
                        Ok(link) => {
                            if let Ok(mut shared) = shared_links.write() {
                                shared.push((peer, link.report.clone()));
                            }
                            links.push(link);
                        }
                        Err(e) => {
                            tracing::error!(
                                "The audio link to {} could not be started: {}",
                                peer,
                                e
                            );
                            // Told like a link that gave up, so whoever
                            // decides about the links hears of it
                            let report = Arc::new(LinkReport::default());
                            report.set_state("failed", None);
                            if let Ok(mut shared) = shared_links.write() {
                                shared.push((peer, report));
                            }
                        }
                    }
                }
            }
            Ok(Command::Unlink(peer)) => {
                if let Ok(mut shared) = shared_links.write() {
                    shared.retain(|(linked, _)| *linked != peer);
                }
                if let Some(at) = links.iter().position(|link| link.peer == peer) {
                    let link = links.remove(at);
                    if let Ok(mut senders) = senders.write() {
                        *senders = senders_of(&links);
                    }
                    let report = link.close().await;
                    if let Ok(mut ended) = ended.write() {
                        ended.push((peer, report));
                    }
                    tracing::info!("Audio link to {} closed", peer);
                }
            }
            Err(std_mpsc::TryRecvError::Disconnected) => {
                tracing::info!("Command channel disconnected");
                break;
            }
            Err(std_mpsc::TryRecvError::Empty) => {
                // No command, continue
            }
        }

        // Whether this turn's command could put the pair on, or take it off,
        // a shared ASIO driver: a device pick on either side, or - while
        // ASIO is active - a channel setting, since ASIO needs the new picks
        // and route from the moment it reopens (unlike `start_capture_ring`,
        // it does not retry with a narrower channel selection afterwards).
        // Windows only: off Windows there is no ASIO path, and
        // `reopen_capture`/`reopen_playback` proceed exactly as before.
        #[cfg(target_os = "windows")]
        if reopen_capture.is_some() || reopen_playback.is_some() {
            let wanted_input = reopen_capture
                .clone()
                .unwrap_or_else(|| capture.device().cloned());
            let wanted_output = reopen_playback
                .clone()
                .unwrap_or_else(|| playback.device().cloned());
            let wanted_asio_driver =
                shared_asio_driver(wanted_input.as_ref(), wanted_output.as_ref());

            if let Some(driver_name) = wanted_asio_driver {
                capture.park(wanted_input.clone());
                playback.park(wanted_output.clone());
                let previous = asio_session.take();
                let input_level = input_level_for_capture.clone();
                let monitor = monitor.clone();
                let mix = mix.clone();
                let master_volume = master_volume.clone();
                let output_level = output_level.clone();
                let recording_feed = recording_feed.clone();
                let frame_samples = mix.frame_samples();
                let wanted = wanted_channels;
                let input_ch = input_channels;
                let output_ch = output_channels;
                let open_input = wanted_input.clone();
                let open_output = wanted_output.clone();
                let open_driver_name = driver_name.clone();
                asio_switch.switch(previous, move || {
                    open_asio(
                        &open_driver_name,
                        open_input.as_ref(),
                        open_output.as_ref(),
                        sample_rate,
                        buffer_size,
                        frame_samples,
                        wanted,
                        input_ch,
                        output_ch,
                        input_level,
                        monitor,
                        mix,
                        master_volume,
                        output_level,
                        recording_feed.as_deref(),
                    )
                });
                asio_driver_in_use = Some(driver_name);
                reopen_capture = None;
                reopen_playback = None;
            } else if asio_driver_in_use.is_some() {
                // Leaving ASIO: it governed both directions, so both need a
                // normal open now, even if only one side's command fired
                // this turn.
                if let Some(previous) = asio_session.take() {
                    let _ = thread::Builder::new()
                        .name("audio-switch-asio".to_string())
                        .spawn(move || previous.stop());
                }
                reopen_capture.get_or_insert(wanted_input);
                reopen_playback.get_or_insert(wanted_output);
                asio_driver_in_use = None;
            }
        }

        if let Some(device_id) = reopen_capture {
            let job = capture_job(device_id.clone(), wanted_channels, input_channels);
            capture.switch(device_id, job);
        }
        if let Some(device_id) = reopen_playback {
            let device = device_id.clone();
            playback.switch(device_id, playout_job(device, output_channels));
        }

        // How the last switch of each device went, if it has finished. Until
        // it has, the session goes on: a device that does not answer holds
        // up only its own switch.
        if let Some(opened) = capture.poll() {
            match opened {
                Ok(ring) => {
                    // The peer is told below if the channel count changed
                    local_latency_info.channel_count = ring.channels as u8;
                    // Replace the ring in the shared reference (send thread will pick this up)
                    if let Ok(mut guard) = capture_ring.lock() {
                        *guard = ring;
                    }
                    clear_device_problem(device_problems, DeviceSide::Input);
                }
                Err(e) => {
                    tracing::warn!("Failed to restart capture: {}", e);
                    note_device_problem(
                        device_problems,
                        DeviceSide::Input,
                        capture.device(),
                        sample_rate,
                        &e,
                    );
                }
            }
        }
        if let Some(opened) = playback.poll() {
            match opened {
                Ok(()) => clear_device_problem(device_problems, DeviceSide::Output),
                Err(e) => {
                    tracing::warn!("Failed to switch output device: {}", e);
                    note_device_problem(
                        device_problems,
                        DeviceSide::Output,
                        playback.device(),
                        sample_rate,
                        &e,
                    );
                }
            }
        }
        // How the last switch to an ASIO driver went, if it has finished
        // (Windows only; see the reconciliation above).
        #[cfg(target_os = "windows")]
        if let Some(result) = asio_switch.poll() {
            match result {
                Ok((session, ring)) => {
                    tracing::info!("ASIO session started on {:?}", asio_driver_in_use);
                    asio_session = Some(session);
                    local_latency_info.channel_count = ring.channels as u8;
                    if let Ok(mut guard) = capture_ring.lock() {
                        *guard = ring;
                    }
                    clear_device_problem(device_problems, DeviceSide::Input);
                    clear_device_problem(device_problems, DeviceSide::Output);
                }
                Err(e) => {
                    tracing::warn!("Failed to switch the ASIO driver: {}", e);
                    note_device_problem(
                        device_problems,
                        DeviceSide::Input,
                        capture.device(),
                        sample_rate,
                        &e,
                    );
                    note_device_problem(
                        device_problems,
                        DeviceSide::Output,
                        playback.device(),
                        sample_rate,
                        &e,
                    );
                }
            }
        }

        // A stream that has stopped calling back raises no error and leaves
        // the session connected, so the callbacks are watched: one silent
        // for a second is opened again, and one that stays silent is told to
        // the user (REQ-AUD-128). ASIO has its own session, not watched here.
        #[cfg(target_os = "windows")]
        let streams_watched = asio_driver_in_use.is_none();
        #[cfg(not(target_os = "windows"))]
        let streams_watched = true;
        if streams_watched {
            let now = std::time::Instant::now();
            match capture.watch(now) {
                Verdict::Flowing => {}
                Verdict::Reopen => {
                    tracing::warn!("The input stopped calling back; opening it again");
                    crate::perf::count_input_reopen();
                    let device = capture.device().cloned();
                    let job = capture_job(device.clone(), wanted_channels, input_channels);
                    capture.switch(device, job);
                }
                Verdict::GiveUp => {
                    tracing::error!("The input is still silent after being opened again");
                    note_trouble(
                        device_problems,
                        DeviceSide::Input,
                        capture.device(),
                        sample_rate,
                        DeviceTrouble::Stalled,
                    );
                }
                Verdict::Resumed => {
                    tracing::info!("The input calls back again");
                    clear_device_problem(device_problems, DeviceSide::Input);
                }
            }
            match playback.watch(now) {
                Verdict::Flowing => {}
                Verdict::Reopen => {
                    tracing::warn!("The output stopped calling back; opening it again");
                    crate::perf::count_output_reopen();
                    let device = playback.device().cloned();
                    playback.switch(device.clone(), playout_job(device, output_channels));
                }
                Verdict::GiveUp => {
                    tracing::error!("The output is still silent after being opened again");
                    note_trouble(
                        device_problems,
                        DeviceSide::Output,
                        playback.device(),
                        sample_rate,
                        DeviceTrouble::Stalled,
                    );
                }
                Verdict::Resumed => {
                    tracing::info!("The output calls back again");
                    clear_device_problem(device_problems, DeviceSide::Output);
                }
            }
        }

        // Check if we should still be active
        if !is_active.load(Ordering::SeqCst) {
            break;
        }

        // A link whose connectivity check has ended is up: its peer is told
        // about our audio, and the send thread starts sending to it.
        let mut came_up = false;
        for link in &mut links {
            let LinkStage::Checking(checking) = &mut link.stage else {
                continue;
            };
            if !checking.is_finished() {
                continue;
            }
            link.stage = match checking.await {
                Ok(Ok(connection)) => {
                    let info = LatencyInfoMessage {
                        jitter_buffer_ms: link.delay_us(packet_duration_ms) as f32 / 1000.0,
                        ..local_latency_info.clone()
                    };
                    if let Err(e) = connection.send_latency_info(&info).await {
                        tracing::warn!("Failed to send initial latency info: {}", e);
                    }
                    link.announced_channel_count = info.channel_count;
                    came_up = true;
                    LinkStage::Up(Arc::new(tokio::sync::Mutex::new(connection)))
                }
                Ok(Err(e)) => {
                    tracing::warn!("The audio link to {} could not be set up: {}", link.peer, e);
                    link.report.set_state("failed", None);
                    LinkStage::Down
                }
                Err(e) => {
                    tracing::warn!(
                        "The check of the audio link to {} ended early: {}",
                        link.peer,
                        e
                    );
                    link.report.set_state("failed", None);
                    LinkStage::Down
                }
            };
        }
        if came_up {
            if let Ok(mut senders) = senders.write() {
                *senders = senders_of(&links);
            }
        }

        // Refresh stats and the meters on a wall-clock interval. Counting loop
        // iterations would tie the cadence to how fast this loop happens to
        // spin, which is a property of the audio clock, not of anything the UI
        // cares about.
        if std::time::Instant::now() >= next_adapt_at {
            next_adapt_at = std::time::Instant::now() + ADAPT_INTERVAL;
            for link in &links {
                let before = link.receive.delay_frames();
                if let Some(frames) = link.receive.adapt() {
                    tracing::info!(
                        "Play-out delay of {} adjusted to {} frame(s)",
                        link.peer,
                        frames
                    );
                    // Only a delay that grew is told to the user: giving back
                    // the preset's spare frames on a clean link happens on
                    // every join and says nothing about the link
                    if frames > before {
                        adjusted_delay_us
                            .store(link.delay_us(packet_duration_ms), Ordering::Relaxed);
                        delay_adjustments.fetch_add(1, Ordering::Relaxed);
                    }
                }
            }
        }

        // Whatever moved a delay, or the channels we send, the display and the
        // peer follow. The peer shows a total latency that includes our
        // buffer, so it has to hear about the change - otherwise its display
        // keeps the value from connect time. If the connection is busy the
        // peer is told on the next tick.
        let channel_count = local_latency_info.channel_count;
        for link in &mut links {
            let delay_frames = link.receive.delay_frames();
            if delay_frames == link.announced_delay_frames
                && channel_count == link.announced_channel_count
            {
                continue;
            }
            let Some(connection) = link.connection().cloned() else {
                // Told when the link comes up
                link.announced_delay_frames = delay_frames;
                continue;
            };
            let told = match connection.try_lock() {
                Ok(conn) => {
                    let updated = LatencyInfoMessage {
                        jitter_buffer_ms: link.delay_us(packet_duration_ms) as f32 / 1000.0,
                        ..local_latency_info.clone()
                    };
                    if let Err(e) = conn.send_latency_info(&updated).await {
                        tracing::warn!("Failed to send updated latency info: {}", e);
                    }
                    true
                }
                Err(_) => false,
            };
            if told {
                link.announced_delay_frames = delay_frames;
                link.announced_channel_count = channel_count;
            }
        }

        if std::time::Instant::now() >= next_stats_at {
            next_stats_at = std::time::Instant::now() + STATS_INTERVAL;
            for link in &mut links {
                link.refresh(&preset, bandwidth_required_bps, sample_rate);
                // Whatever moved the delay - a preset switch, an adaptation,
                // a reconnect, the peer saying how long its packets are - the
                // display follows (ADR-031)
                link.report
                    .jitter_buffer_delay_us
                    .store(link.delay_us(packet_duration_ms), Ordering::SeqCst);
                link.publish_playout();
            }
            // Update shared input level from capture callback
            input_level.store(
                input_level_for_capture.load(Ordering::SeqCst),
                Ordering::SeqCst,
            );
        }

        // Nothing to do here for audio: the output callback takes frames
        // from the play-out buffer itself, at the device's clock (ADR-028).
        // This loop only carries commands and statistics, so it waits.
        crate::perf::RECEIVE_LOOP.stop(pass_started);
        tokio::time::sleep(POP_IDLE).await;
    }

    // Cleanup: signal send thread to stop and wait for it
    send_thread_running.store(false, Ordering::SeqCst);
    let _ = send_thread.join();

    if let Ok(mut senders) = senders.write() {
        *senders = Arc::from(Vec::new());
    }
    for link in links {
        let peer = link.peer;
        let report = link.close().await;
        if let Ok(mut ended) = ended.write() {
            ended.push((peer, report));
        }
    }

    drop(capture);
    drop(playback);
    stand_ins.stop();
    #[cfg(target_os = "windows")]
    if let Some(session) = asio_session.take() {
        session.stop();
    }

    tracing::info!("Streaming stopped.");

    Ok(())
}
