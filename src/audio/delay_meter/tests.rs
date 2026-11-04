use super::*;
use std::sync::{Arc, Condvar, Mutex};

/// The meter is one per process, so the tests that run it take turns.
static TURN: Mutex<()> = Mutex::new(());

const RATE: u32 = 48_000;

// ---------------------------------------------------------------------
// A clock the device below drives, so a run of tens of seconds is over in
// a moment and the stamps are exact

struct Virtual {
    t0: Instant,
    inner: Mutex<Moment>,
    changed: Condvar,
}

struct Moment {
    elapsed: Duration,
    sleeping_until: Option<Duration>,
    over: bool,
}

impl Virtual {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            t0: Instant::now(),
            inner: Mutex::new(Moment {
                elapsed: Duration::ZERO,
                sleeping_until: None,
                over: false,
            }),
            changed: Condvar::new(),
        })
    }

    fn at(&self, frame: u64) -> Instant {
        self.t0 + Duration::from_secs_f64(frame as f64 / f64::from(RATE))
    }

    fn finish(&self) {
        self.inner.lock().unwrap().over = true;
        self.changed.notify_all();
    }

    /// Waits until the meter is asleep with time left to sleep, then moves
    /// time to `frame`. False when the run is over.
    fn advance_to(&self, frame: u64) -> bool {
        let mut moment = self.inner.lock().unwrap();
        while !moment.over
            && !moment
                .sleeping_until
                .is_some_and(|until| until > moment.elapsed)
        {
            moment = self.changed.wait(moment).unwrap();
        }
        if moment.over {
            return false;
        }
        moment.elapsed = moment
            .elapsed
            .max(Duration::from_secs_f64(frame as f64 / f64::from(RATE)));
        drop(moment);
        self.changed.notify_all();
        true
    }
}

impl Clock for Virtual {
    fn now(&self) -> Instant {
        self.t0 + self.inner.lock().unwrap().elapsed
    }

    fn sleep(&self, duration: Duration) {
        let mut moment = self.inner.lock().unwrap();
        let until = moment.elapsed + duration;
        moment.sleeping_until = Some(until);
        self.changed.notify_all();
        while moment.elapsed < until && !moment.over {
            moment = self.changed.wait(moment).unwrap();
        }
        moment.sleeping_until = None;
    }
}

// ---------------------------------------------------------------------
// A device pair that loops the output to the input

#[derive(Clone)]
struct Device {
    output_frames: usize,
    input_frames: usize,
    channels: usize,
    /// Frames from the output to the input.
    delay_frames: u64,
    /// What survives of a sample, or of the clicks and the tone apart.
    gain: f32,
    /// Peak of white noise on the input.
    noise: f32,
    /// A leak of the output into the input, with no delay but this many frames.
    leak: Option<(u64, f32)>,
    /// A different delay for every sound, cycling.
    delays_per_sound: Option<Vec<u64>>,
}

impl Device {
    fn loop_of(delay_ms: f64) -> Self {
        Self {
            output_frames: 64,
            input_frames: 64,
            channels: 2,
            delay_frames: (delay_ms * f64::from(RATE) / 1000.0).round() as u64,
            gain: 0.8,
            noise: 0.0,
            leak: None,
            delays_per_sound: None,
        }
    }
}

struct Noise(u32);

impl Noise {
    fn next(&mut self) -> f32 {
        self.0 = self.0.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        (self.0 >> 8) as f32 / (1u32 << 24) as f32 * 2.0 - 1.0
    }
}

/// Plays the device until the meter is done: the output callback at the
/// start of its frames, the input callback when its last frame is in.
fn drive(clock: &Virtual, device: &Device) {
    let mut sent: Vec<f32> = Vec::new();
    let mut noise = Noise(12345);
    let (mut out_at, mut in_at) = (0u64, 0u64);
    let out_len = device.output_frames as u64;
    let in_len = device.input_frames as u64;
    let mut sound_starts: Vec<u64> = Vec::new();
    let mut seen_started = 0;
    loop {
        let next_out = out_at;
        let next_in = in_at + in_len - 1;
        let frame = next_out.min(next_in);
        if !clock.advance_to(frame) {
            return;
        }
        if next_out <= next_in {
            let mut stereo = vec![0.9f32; device.output_frames * 2];
            output_at(&mut stereo, RATE, clock.at(next_out));
            sent.extend(stereo.chunks_exact(2).map(|f| f[0]));
            let shared = SHARED.get().unwrap();
            let started = shared.started_gen.load(Ordering::Relaxed);
            if started != seen_started {
                seen_started = started;
                sound_starts.push(next_out);
            }
            out_at += out_len;
        } else {
            let mut data = Vec::with_capacity(device.input_frames * device.channels);
            for n in in_at..in_at + in_len {
                let sound_index = sound_starts.partition_point(|&s| s <= n);
                let delay = match (&device.delays_per_sound, sound_index) {
                    (Some(cycle), i) if i > 0 => cycle[(i - 1) % cycle.len()],
                    _ => device.delay_frames,
                };
                let mut value = if n >= delay {
                    sent.get((n - delay) as usize).copied().unwrap_or(0.0) * device.gain
                } else {
                    0.0
                };
                if let Some((lag, leak)) = device.leak {
                    if n >= lag {
                        value += sent.get((n - lag) as usize).copied().unwrap_or(0.0) * leak;
                    }
                }
                value += noise.next() * device.noise;
                data.extend(std::iter::repeat_n(value, device.channels));
            }
            input_at(&data, device.channels, RATE, clock.at(in_at + in_len - 1));
            in_at += in_len;
        }
    }
}

fn measure(device: &Device) -> Result<Measurement, Failure> {
    let _turn = TURN.lock().unwrap_or_else(|e| e.into_inner());
    let clock = Virtual::new();
    let meter_clock: Arc<dyn Clock> = clock.clone();
    let meter = std::thread::spawn({
        let clock = clock.clone();
        move || {
            begin(meter_clock, false, Box::new(|_| {})).unwrap();
            clock.finish();
        }
    });
    drive(&clock, device);
    meter.join().unwrap();
    match state() {
        State::Done(result) => result,
        other => panic!("the run did not end: {other:?}"),
    }
}

fn ms_of(frames: u64) -> f64 {
    frames as f64 * 1000.0 / f64::from(RATE)
}

// ---------------------------------------------------------------------
// The run, against a device

/// Verifies: REQ-AUD-050
#[test]
fn when_the_output_is_cabled_to_the_input_the_figure_is_the_delay_of_the_cable_to_a_sample() {
    let device = Device::loop_of(23.0);

    let found = measure(&device).expect("a figure");

    assert_eq!(found.stimulus, Stimulus::Clicks);
    assert_eq!(found.level_db, LEVELS_DB[0], "the quietest step was enough");
    assert!(
        (found.delay_ms - ms_of(device.delay_frames)).abs() <= 0.03,
        "{} ms against {} ms",
        found.delay_ms,
        ms_of(device.delay_frames)
    );
    assert_eq!(found.stability, Stability::Steady);
    assert_eq!(found.kept, REPEATS);
}

/// Verifies: REQ-AUD-050
#[test]
fn when_the_buffers_are_large_the_arrival_is_still_read_to_a_sample_not_to_a_buffer() {
    let device = Device {
        output_frames: 480,
        input_frames: 441,
        channels: 1,
        ..Device::loop_of(13.0)
    };

    let found = measure(&device).expect("a figure");

    assert!(
        (found.delay_ms - ms_of(device.delay_frames)).abs() <= 0.03,
        "{} ms against {} ms with 10 ms buffers",
        found.delay_ms,
        ms_of(device.delay_frames)
    );
}

/// Verifies: REQ-AUD-050
#[test]
fn when_the_path_is_quiet_the_volume_goes_up_until_the_sound_is_heard() {
    let device = Device {
        gain: 0.01,
        ..Device::loop_of(40.0)
    };

    let found = measure(&device).expect("a figure");

    assert!(found.level_db > LEVELS_DB[0], "{}", found.level_db);
    assert!((found.delay_ms - 40.0).abs() <= 0.03, "{}", found.delay_ms);
}

/// Verifies: REQ-AUD-050
#[test]
fn when_the_clicks_are_lost_in_the_noise_the_tone_is_heard_through_the_filter() {
    let device = Device {
        gain: 0.02,
        noise: 0.02,
        ..Device::loop_of(31.0)
    };

    let found = measure(&device).expect("a figure");

    assert_eq!(found.stimulus, Stimulus::Tone);
    assert!(
        (found.delay_ms - 31.0).abs() <= 1.5,
        "{} ms for a 31 ms path",
        found.delay_ms
    );
}

/// Verifies: REQ-AUD-050
#[test]
fn when_nothing_comes_back_there_is_no_figure_and_the_run_ends_within_its_time() {
    let device = Device {
        gain: 0.0,
        ..Device::loop_of(10.0)
    };

    assert_eq!(measure(&device), Err(Failure::NoSound));
}

/// Verifies: REQ-AUD-050
#[test]
fn a_sound_that_leaks_from_the_output_into_the_input_at_once_is_not_taken_for_the_path() {
    let device = Device {
        gain: 0.0,
        leak: Some((48, 0.5)),
        ..Device::loop_of(10.0)
    };

    assert_eq!(measure(&device), Err(Failure::NoSound), "1 ms is crosstalk");
}

/// Verifies: REQ-AUD-050
#[test]
fn a_path_just_over_the_crosstalk_limit_is_measured() {
    let device = Device::loop_of(3.0);

    let found = measure(&device).expect("a figure");

    assert!((found.delay_ms - 3.0).abs() <= 0.03, "{}", found.delay_ms);
}

/// Verifies: REQ-AUD-050
#[test]
fn when_every_sound_comes_back_at_another_time_the_run_says_the_delay_is_unsteady() {
    let ms = |ms: f64| (ms * f64::from(RATE) / 1000.0).round() as u64;
    let device = Device {
        delays_per_sound: Some(vec![ms(10.0), ms(48.0), ms(15.0), ms(70.0), ms(30.0)]),
        ..Device::loop_of(10.0)
    };

    assert_eq!(measure(&device), Err(Failure::Unsteady));
}

/// Verifies: REQ-AUD-050
#[test]
fn while_a_run_is_under_way_the_session_gets_silence_and_after_it_the_input_again() {
    let _turn = TURN.lock().unwrap_or_else(|e| e.into_inner());
    let clock = Virtual::new();
    let meter_clock: Arc<dyn Clock> = clock.clone();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let meter = std::thread::spawn({
        let clock = clock.clone();
        move || {
            begin(meter_clock, false, Box::new(|_| {})).unwrap();
            clock.finish();
        }
    });
    let mut out_at = 0;
    while clock.advance_to(out_at) {
        output_at(&mut vec![0.9f32; 128], RATE, clock.at(out_at));
        seen.lock().unwrap().push(on_input(&[0.0; 128], 2, RATE));
        out_at += 64;
        if out_at > 20 * u64::from(RATE) {
            cancel();
        }
    }
    meter.join().unwrap();

    assert!(seen.lock().unwrap().iter().all(|muted| *muted));
    assert!(!on_input(&[0.0; 128], 2, RATE), "the run is over");
    assert_eq!(state(), State::Done(Err(Failure::Cancelled)));
}

/// Verifies: REQ-AUD-050
#[test]
fn when_no_stream_is_calling_back_the_run_says_so() {
    let _turn = TURN.lock().unwrap_or_else(|e| e.into_inner());
    let clock = Virtual::new();
    let meter_clock: Arc<dyn Clock> = clock.clone();
    let meter = std::thread::spawn({
        let clock = clock.clone();
        move || {
            begin(meter_clock, false, Box::new(|_| {})).unwrap();
            clock.finish();
        }
    });
    let mut at = 0;
    while clock.advance_to(at) {
        at += 480;
    }
    meter.join().unwrap();

    assert_eq!(state(), State::Done(Err(Failure::NoStream)));
}

// ---------------------------------------------------------------------
// The parts

/// Verifies: REQ-AUD-050
#[test]
fn the_click_train_is_thirty_one_sample_pulses_a_millisecond_apart() {
    let len = stimulus_frames(Stimulus::Clicks, RATE);
    let pulses: Vec<usize> = (0..len)
        .filter(|&n| stimulus_sample(Stimulus::Clicks, n, len, RATE) != 0.0)
        .collect();

    assert_eq!(len, 30 * 48);
    assert_eq!(pulses.len(), 30);
    assert!(pulses.windows(2).all(|pair| pair[1] - pair[0] == 48));
}

/// Verifies: REQ-AUD-050
#[test]
fn the_tone_is_a_one_kilohertz_sine_for_fifty_milliseconds() {
    let len = stimulus_frames(Stimulus::Tone, RATE);
    let samples: Vec<f32> = (0..len)
        .map(|n| stimulus_sample(Stimulus::Tone, n, len, RATE))
        .collect();
    let crossings = samples
        .windows(2)
        .filter(|pair| pair[0] <= 0.0 && pair[1] > 0.0)
        .count();

    assert_eq!(len, 2400);
    assert_eq!(crossings, 50, "50 cycles of 1 kHz in 50 ms");
    assert_eq!(samples[0], 0.0, "it starts from zero");
    assert!(samples[len - 1].abs() < 0.1, "and fades out");
}

/// Verifies: REQ-AUD-050
#[test]
fn the_noise_is_the_amplitude_ninety_percent_of_the_samples_stay_under() {
    let window: Vec<f32> = (0..1000).map(|n| (n as f32 / 1000.0) * -1.0).collect();

    let noise = noise_of(&window);

    assert!((noise - 0.9).abs() < 0.01, "{noise}");
    assert_eq!(
        noise_of(&[0.0; 10]),
        NOISE_MIN,
        "digital silence has a floor"
    );
}

/// Verifies: REQ-AUD-050
#[test]
fn the_arrival_is_traced_back_to_where_the_rise_began() {
    let mut signal = vec![0.0f32; 1000];
    for (n, level) in [0.002f32, 0.01, 0.05, 0.2, 0.4].into_iter().enumerate() {
        signal[200 + n] = level;
    }
    signal[205..320].fill(0.4);

    // The 20 dB threshold over 1e-4 noise is 1e-3: crossed by the first
    // sample of the rise.
    let arrival = seek_arrival(&signal, 100, 1e-4, 20.0, RATE, Stimulus::Clicks);
    assert_eq!(arrival, Seek::Found(200));

    // A louder floor means the same rise is first seen later
    let arrival = seek_arrival(&signal, 100, 1e-3, 20.0, RATE, Stimulus::Clicks);
    assert_eq!(
        arrival,
        Seek::Found(201),
        "0.01 is the first over 10 x 1e-3"
    );
}

/// Verifies: REQ-AUD-050
#[test]
fn a_rise_is_not_traced_back_to_before_where_the_search_began() {
    let signal = vec![0.5f32; 1000];

    assert_eq!(
        seek_arrival(&signal, 120, 1e-4, 20.0, RATE, Stimulus::Clicks),
        Seek::Found(120)
    );
}

/// Verifies: REQ-AUD-050
#[test]
fn a_spike_that_does_not_go_on_is_not_taken_for_the_sound() {
    let mut signal = vec![0.0f32; 2000];
    signal[300] = 0.5;
    signal[1000..1500].fill(0.5);

    let clicks = seek_arrival(&signal, 100, 1e-4, 20.0, RATE, Stimulus::Clicks);
    let tone = seek_arrival(&signal, 100, 1e-4, 20.0, RATE, Stimulus::Tone);

    assert_eq!(clicks, Seek::Found(1000), "one spike is not three");
    assert_eq!(tone, Seek::Found(1000), "a spike is gone 6 ms later");
}

/// Verifies: REQ-AUD-050
#[test]
fn a_crossing_too_near_the_end_of_the_signal_waits_for_more_of_it() {
    let mut signal = vec![0.0f32; 700];
    signal[500..].fill(0.5);

    assert_eq!(
        seek_arrival(&signal, 100, 1e-4, 20.0, RATE, Stimulus::Clicks),
        Seek::Pending(500)
    );
}

/// Verifies: REQ-AUD-050
#[test]
fn the_band_pass_filter_passes_one_kilohertz_and_holds_back_the_rest() {
    let amplitude_of = |hz: f64| {
        let mut filter = BandPass::new(RATE);
        let out: Vec<f32> = (0..9600)
            .map(|n| {
                let x = (2.0 * std::f64::consts::PI * hz * n as f64 / f64::from(RATE)).sin();
                filter.process(x as f32)
            })
            .collect();
        out[4800..].iter().fold(0.0f32, |peak, s| peak.max(s.abs()))
    };

    assert!(amplitude_of(1000.0) > 0.95);
    assert!(amplitude_of(100.0) < 0.2);
    assert!(amplitude_of(8000.0) < 0.2);
}

/// Verifies: REQ-AUD-050
#[test]
fn a_result_far_from_the_rest_is_dropped_and_the_figure_is_the_median_of_the_others() {
    let (figure, spread, kept) = evaluate(&[20.0, 20.4, 19.8, 20.2, 95.0]).unwrap();

    assert_eq!(kept, 4);
    assert!((figure - 20.1).abs() < 1e-9, "{figure}");
    assert!((spread - 0.6).abs() < 1e-9, "{spread}");
}

/// Verifies: REQ-AUD-050
#[test]
fn results_within_three_milliseconds_of_the_median_are_never_dropped() {
    let (_, _, kept) = evaluate(&[20.0, 20.0, 20.0, 20.0, 22.9]).unwrap();

    assert_eq!(kept, 5, "the MAD is 0 here, and 3 ms is the least room");
}

/// Verifies: REQ-AUD-050
#[test]
fn a_figure_needs_three_results_that_are_close_and_not_longer_than_five_hundred_milliseconds() {
    assert_eq!(evaluate(&[20.0, 21.0]), Err(Failure::NoSound));
    assert_eq!(
        evaluate(&[10.0, 30.0, 50.0, 12.0, 52.0]),
        Err(Failure::Unsteady)
    );
    assert_eq!(
        evaluate(&[510.0, 511.0, 512.0, 509.0, 510.5]),
        Err(Failure::TooLong)
    );
    assert!(evaluate(&[498.0, 499.0, 499.5]).is_ok());
}

/// Verifies: REQ-AUD-050
#[test]
fn a_spread_up_to_two_milliseconds_is_steady_and_up_to_six_slightly_shaky() {
    assert_eq!(Stability::of_spread(0.0), Stability::Steady);
    assert_eq!(Stability::of_spread(2.0), Stability::Steady);
    assert_eq!(Stability::of_spread(2.1), Stability::SlightlyShaky);
    assert_eq!(Stability::of_spread(6.0), Stability::SlightlyShaky);
    assert_eq!(Stability::of_spread(6.1), Stability::Shaky);
}

/// Verifies: REQ-AUD-050
#[test]
fn a_delay_over_a_hundred_milliseconds_asks_for_a_second_look_at_the_settings() {
    let at = |delay_ms| Measurement {
        delay_ms,
        spread_ms: 0.5,
        stability: Stability::Steady,
        kept: 5,
        heard: 5,
        stimulus: Stimulus::Clicks,
        level_db: -48.0,
    };

    assert!(!at(100.0).needs_review());
    assert!(at(100.5).needs_review());
}

/// Verifies: REQ-AUD-050
#[test]
fn the_steps_get_louder_by_six_decibels_and_the_last_three_ask_for_less_over_the_noise() {
    assert!(LEVELS_DB.windows(2).all(|pair| pair[1] - pair[0] == 6.0));
    assert_eq!(LEVELS_DB[7], -6.0, "never louder than half of full scale");
    assert_eq!(&MARGIN_DB[..5], &[20.0; 5]);
    assert_eq!(&MARGIN_DB[5..], &[12.0, 9.0, 6.0]);
}
