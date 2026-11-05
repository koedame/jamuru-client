//! Lossless packing of 24-bit PCM audio packets (ADR-111)
//!
//! Neighbouring samples of a waveform are close to each other, so what is left
//! of a sample after predicting it from the ones before is a small number, and
//! small numbers take few bits. This is the method of FLAC (RFC 9639): a fixed
//! polynomial predictor of order 0 to 4 (section 9.2.5), and the residual coded
//! with Rice codes in partitions (section 9.2.7). A stereo packet may be coded as
//! left and side, side and right, or mid and side (section 9.1.3), which is what
//! makes a mono source panned to the middle - the same samples on both sides -
//! cost one channel. What is not taken over is the FLAC frame: a packet here
//! holds 64 to 256 samples, and the frame header would be a sizeable part of that.
//!
//! A packet decodes by itself. The first `order` samples of each channel go in
//! unpredicted, so a lost packet costs nothing to the ones after it.
//!
//! ```text
//! byte 0           format (bits 0-1), channels - 1 (bits 2-4),
//!                  stereo mode (bits 5-6, 0 unless two channels), 0 (bit 7)
//! verbatim (0):    the samples as 24-bit little-endian integers, byte 0 is 0
//! packed (1):      samples per channel (u16, big-endian), then for each channel,
//!                  in a bit stream, most significant bit first:
//!                    predictor order       3 bits (0-4)
//!                    partition order       2 bits (0-3): 2^p partitions
//!                    warm-up               `order` samples, 24 bits each (25 for a side)
//!                    per partition:  Rice parameter (5 bits), then its residuals
//!                  A residual is zigzag-folded (0, -1, 1, -2 ... = 0, 1, 2, 3 ...),
//!                  then q = value >> k as q zeros and a one, and the low k bits.
//! ```
//!
//! Anything after the bit stream is padding and is ignored.
//!
//! A packet that does not get smaller (noise) goes as `verbatim`: one byte more
//! than the samples themselves, and never more than that.
//!
//! The size of a packet follows the audio, which an observer of the encrypted
//! traffic can read. The packed size is therefore rounded up to one of
//! [`SIZE_STEPS`] sizes ([`pad_to_step`]).

use super::codec::{quantize, CodecError, PCM_BYTES_PER_SAMPLE, PCM_FULL_SCALE};

/// Bytes of a packet that are not samples: the format byte
pub const PACKED_HEADER_BYTES: usize = 1;

/// The sizes a packed packet is rounded up to: this many, spread evenly up to
/// the size of the same packet sent as it is
pub const SIZE_STEPS: usize = 8;

const FORMAT_VERBATIM: u8 = 0;
const FORMAT_PACKED: u8 = 1;
const FORMAT_MASK: u8 = 0b11;
const CHANNELS_SHIFT: u32 = 2;
const CHANNELS_MASK: u8 = 0b111;
const STEREO_SHIFT: u32 = 5;
const STEREO_MASK: u8 = 0b11;
const MAX_CHANNELS: usize = 8;

/// The most samples a packet is unpacked to, in all channels: a bound for the stack. The
/// sender never makes a packet of more than a packet carries as it is (486), and a frame
/// of 1024 stereo frames, which no setting allows, would still fit.
const MAX_PACKET_SAMPLES: usize = 2048;

/// Bytes ahead of the bit stream: the format byte and the sample count
const PACKED_HEADER: usize = 3;

const MAX_ORDER: usize = 4;
const ORDER_BITS: u32 = 3;
const PARTITION_ORDER_BITS: u32 = 2;
const MAX_PARTITION_ORDER: u32 = (1 << PARTITION_ORDER_BITS) - 1;
const PARAMETER_BITS: u32 = 5;
const MAX_PARAMETER: u32 = 30;
const SAMPLE_BITS: u32 = 24;

/// The largest value a valid residual folds to (a side of 25 bits predicted at order
/// 4 is off by under 2^30); a decoder reads anything above it as a damaged packet
/// instead of looping on a run of zeros
const MAX_FOLDED: u64 = 1 << 31;

/// How the two channels of a stereo packet are coded
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Stereo {
    /// Left, right
    Independent = 0,
    /// Left, side (left - right)
    LeftSide = 1,
    /// Side, right
    SideRight = 2,
    /// Mid ((left + right) >> 1), side
    MidSide = 3,
}

impl Stereo {
    fn from_bits(bits: u8) -> Self {
        match bits {
            0 => Stereo::Independent,
            1 => Stereo::LeftSide,
            2 => Stereo::SideRight,
            _ => Stereo::MidSide,
        }
    }

    /// Bits a sample of the first and of the second channel has: a side is one bit wider
    fn widths(self) -> [u32; 2] {
        match self {
            Stereo::Independent => [SAMPLE_BITS, SAMPLE_BITS],
            Stereo::LeftSide | Stereo::MidSide => [SAMPLE_BITS, SAMPLE_BITS + 1],
            Stereo::SideRight => [SAMPLE_BITS + 1, SAMPLE_BITS],
        }
    }

    /// The left and right samples that the coded pair `a`, `b` stand for
    fn to_left_right(self, a: i32, b: i32) -> (i32, i32) {
        match self {
            Stereo::Independent => (a, b),
            Stereo::LeftSide => (a, a - b),
            Stereo::SideRight => (a + b, b),
            Stereo::MidSide => {
                let mid = (a << 1) | (b & 1);
                ((mid + b) >> 1, (mid - b) >> 1)
            }
        }
    }
}

/// The bytes a packet of `samples` takes if it is sent as it is
pub fn verbatim_len(samples: usize) -> usize {
    PACKED_HEADER_BYTES + samples * PCM_BYTES_PER_SAMPLE
}

/// Pack `samples`, interleaved over `channels`, onto the end of `out` as small as
/// it goes, rounded up to one of [`SIZE_STEPS`] sizes
///
/// Returns the number of samples that were clipped to 24 bits (see
/// [`super::encode_pcm24`]).
pub fn encode_packed(samples: &[f32], channels: usize, out: &mut Vec<u8>) -> u64 {
    let start = out.len();
    let clipped = encode_packed_exact(samples, channels, out);
    pad_to_step(out, start, verbatim_len(samples.len()));
    clipped
}

/// [`encode_packed`], without the rounding up
pub fn encode_packed_exact(samples: &[f32], channels: usize, out: &mut Vec<u8>) -> u64 {
    let mut clipped = 0;
    let quantized: Vec<i32> = samples
        .iter()
        .map(|&sample| {
            let (value, was_clipped) = quantize(sample);
            clipped += u64::from(was_clipped);
            value
        })
        .collect();

    let per_channel = samples.len().checked_div(channels).unwrap_or(0);
    let packable = (1..=MAX_CHANNELS).contains(&channels)
        && per_channel > 0
        && per_channel * channels == samples.len()
        && per_channel <= usize::from(u16::MAX);
    if packable {
        if let Some(packed) = pack(&quantized, channels, per_channel) {
            out.extend_from_slice(&packed);
            return clipped;
        }
    }

    out.reserve(verbatim_len(samples.len()));
    out.push(FORMAT_VERBATIM);
    for value in quantized {
        out.extend_from_slice(&value.to_le_bytes()[..PCM_BYTES_PER_SAMPLE]);
    }
    clipped
}

/// `quantized` (interleaved over `channels`) as a packed packet, or `None` if that is
/// no smaller than sending it as it is
fn pack(quantized: &[i32], channels: usize, per_channel: usize) -> Option<Vec<u8>> {
    let mut signals: Vec<Vec<i32>> = (0..channels)
        .map(|c| {
            quantized
                .iter()
                .skip(c)
                .step_by(channels)
                .copied()
                .collect()
        })
        .collect();
    let mut widths = vec![SAMPLE_BITS; channels];
    let mut scratch = Vec::with_capacity(per_channel);

    // Which signals are coded, in which order: left and right, unless two channels are
    // better coded as one of them and the difference
    let mut coded: Vec<usize> = (0..channels).collect();
    let mut stereo = Stereo::Independent;
    if channels == 2 {
        let side: Vec<i32> = (0..per_channel)
            .map(|i| signals[0][i] - signals[1][i])
            .collect();
        let mid: Vec<i32> = (0..per_channel)
            .map(|i| (signals[0][i] + signals[1][i]) >> 1)
            .collect();
        signals.push(side);
        signals.push(mid);
        widths.extend([SAMPLE_BITS + 1, SAMPLE_BITS]);
    }
    let plans: Vec<Plan> = signals
        .iter()
        .zip(&widths)
        .map(|(signal, &width)| Plan::search(signal, width, &mut scratch))
        .collect();
    let mut bits: usize = coded.iter().map(|&c| plans[c].bits).sum();
    if channels == 2 {
        // left, right, side, mid
        for (mode, pair) in [
            (Stereo::LeftSide, [0, 2]),
            (Stereo::SideRight, [2, 1]),
            (Stereo::MidSide, [3, 2]),
        ] {
            let pair_bits = plans[pair[0]].bits + plans[pair[1]].bits;
            if pair_bits < bits {
                bits = pair_bits;
                coded = pair.to_vec();
                stereo = mode;
            }
        }
    }

    if PACKED_HEADER + bits.div_ceil(8) >= verbatim_len(quantized.len()) {
        return None;
    }
    let mut out = Vec::with_capacity(PACKED_HEADER + bits.div_ceil(8));
    out.push(
        FORMAT_PACKED
            | (((channels - 1) as u8) << CHANNELS_SHIFT)
            | ((stereo as u8) << STEREO_SHIFT),
    );
    out.extend_from_slice(&(per_channel as u16).to_be_bytes());
    let mut writer = BitWriter::new(&mut out);
    for &c in &coded {
        plans[c].write(&signals[c], widths[c], &mut scratch, &mut writer);
    }
    writer.finish();
    Some(out)
}

/// Extend the packet that starts at `start` in `out` with zeros up to the next of
/// [`SIZE_STEPS`] sizes, `max_len` being the last
fn pad_to_step(out: &mut Vec<u8>, start: usize, max_len: usize) {
    let len = out.len() - start;
    let step = max_len.div_ceil(SIZE_STEPS);
    let padded = (len.div_ceil(step) * step).min(max_len);
    out.resize(start + padded.max(len), 0);
}

/// The number of samples a packed packet holds
pub fn packed_sample_count(data: &[u8]) -> Result<usize, CodecError> {
    match packet_format(data)? {
        Format::Verbatim => verbatim_count(data),
        Format::Packed {
            per_channel,
            channels,
            ..
        } => {
            let total = per_channel * channels;
            // A sample takes at least a bit, so a count the data cannot hold is not a count
            if total > data.len() * 8 || total > MAX_PACKET_SAMPLES {
                return Err(CodecError::InvalidData(format!(
                    "{total} samples in {} bytes",
                    data.len()
                )));
            }
            Ok(total)
        }
    }
}

/// Unpack a packet into `output`, returning the number of samples written
///
/// # Errors
/// [`CodecError::InvalidData`] if the packet is not one [`encode_packed`] makes,
/// [`CodecError::InvalidFrameSize`] if `output` is too small.
pub fn decode_packed(data: &[u8], output: &mut [f32]) -> Result<usize, CodecError> {
    let total = packed_sample_count(data)?;
    if output.len() < total {
        return Err(CodecError::InvalidFrameSize {
            expected: total,
            actual: output.len(),
        });
    }
    match packet_format(data)? {
        Format::Verbatim => {
            for (slot, bytes) in output.iter_mut().zip(
                data[PACKED_HEADER_BYTES..]
                    .as_chunks::<PCM_BYTES_PER_SAMPLE>()
                    .0,
            ) {
                // Put the three bytes at the top of an i32 and shift back down: the shift carries the sign
                let value = i32::from_le_bytes([0, bytes[0], bytes[1], bytes[2]]) >> 8;
                *slot = value as f32 / PCM_FULL_SCALE;
            }
        }
        Format::Packed {
            per_channel,
            channels,
            stereo,
        } => {
            // The channels are decoded to integers first, and only then turned into samples:
            // a side has a bit more than a float holds. On the stack, so that nothing is
            // allocated here
            let mut ints = [0i32; MAX_PACKET_SAMPLES];
            let widths = if channels == 2 {
                stereo.widths()
            } else {
                [SAMPLE_BITS; 2]
            };
            let mut reader = BitReader::new(&data[PACKED_HEADER..]);
            for c in 0..channels {
                let width = widths[c.min(1)];
                decode_channel(
                    &mut reader,
                    per_channel,
                    width,
                    &mut ints[c..total],
                    channels,
                )?;
            }
            for (frame, ints) in output[..total]
                .chunks_exact_mut(channels)
                .zip(ints.chunks_exact(channels))
            {
                if channels == 2 {
                    let (left, right) = stereo.to_left_right(ints[0], ints[1]);
                    frame[0] = to_sample(left)?;
                    frame[1] = to_sample(right)?;
                } else {
                    for (slot, &value) in frame.iter_mut().zip(ints) {
                        *slot = to_sample(value)?;
                    }
                }
            }
        }
    }
    Ok(total)
}

fn to_sample(value: i32) -> Result<f32, CodecError> {
    if !(-(1 << 23)..(1 << 23)).contains(&value) {
        return Err(CodecError::InvalidData("sample beyond 24 bits".into()));
    }
    Ok(value as f32 / PCM_FULL_SCALE)
}

enum Format {
    Verbatim,
    Packed {
        per_channel: usize,
        channels: usize,
        stereo: Stereo,
    },
}

fn packet_format(data: &[u8]) -> Result<Format, CodecError> {
    let Some(&head) = data.first() else {
        return Err(CodecError::InvalidData("empty audio packet".into()));
    };
    let channels = usize::from((head >> CHANNELS_SHIFT) & CHANNELS_MASK) + 1;
    let stereo = (head >> STEREO_SHIFT) & STEREO_MASK;
    match head & FORMAT_MASK {
        FORMAT_VERBATIM if head == FORMAT_VERBATIM => Ok(Format::Verbatim),
        FORMAT_PACKED if head >> 7 == 0 && (channels == 2 || stereo == 0) => {
            if data.len() < PACKED_HEADER {
                return Err(CodecError::InvalidData("packed packet is cut short".into()));
            }
            let per_channel = usize::from(u16::from_be_bytes([data[1], data[2]]));
            if per_channel == 0 {
                return Err(CodecError::InvalidData(
                    "packed packet has no samples".into(),
                ));
            }
            Ok(Format::Packed {
                per_channel,
                channels,
                stereo: Stereo::from_bits(stereo),
            })
        }
        _ => Err(CodecError::InvalidData(format!(
            "unknown audio packet format {head:#04x}"
        ))),
    }
}

fn verbatim_count(data: &[u8]) -> Result<usize, CodecError> {
    let body = data.len() - PACKED_HEADER_BYTES;
    #[allow(clippy::manual_is_multiple_of)]
    if body % PCM_BYTES_PER_SAMPLE != 0 {
        return Err(CodecError::InvalidData(format!(
            "PCM data length {body} is not a multiple of {PCM_BYTES_PER_SAMPLE}"
        )));
    }
    Ok(body / PCM_BYTES_PER_SAMPLE)
}

/// What FLAC calls the fixed predictor of `order`: the value the `order`
/// samples before `i` lead to, by extrapolating the polynomial through them
fn predict(x: &[i32], i: usize, order: usize) -> i64 {
    let at = |back: usize| i64::from(x[i - back]);
    match order {
        0 => 0,
        1 => at(1),
        2 => 2 * at(1) - at(2),
        3 => 3 * at(1) - 3 * at(2) + at(3),
        _ => 4 * at(1) - 6 * at(2) + 4 * at(3) - at(4),
    }
}

/// The zigzag fold of a residual: 0, -1, 1, -2, 2 ... become 0, 1, 2, 3, 4 ...
fn fold(residual: i64) -> u32 {
    ((residual << 1) ^ (residual >> 63)) as u32
}

fn unfold(folded: u32) -> i64 {
    i64::from(folded >> 1) ^ -i64::from(folded & 1)
}

/// How one channel of a packet is coded
struct Plan {
    order: usize,
    partition_order: u32,
    parameters: [u8; 1 << MAX_PARTITION_ORDER],
    bits: usize,
}

impl Plan {
    /// The predictor order, partitioning and Rice parameters that code `x` in the fewest bits
    fn search(x: &[i32], width: u32, folded: &mut Vec<u32>) -> Plan {
        let n = x.len();
        let mut best: Option<Plan> = None;
        for order in 0..=MAX_ORDER.min(n) {
            fold_residuals(x, order, folded);
            let fixed = (ORDER_BITS + PARTITION_ORDER_BITS) as usize + order * width as usize;
            for partition_order in 0..=MAX_PARTITION_ORDER {
                let parts = 1usize << partition_order;
                if !n.is_multiple_of(parts) || n / parts < order {
                    continue;
                }
                let mut plan = Plan {
                    order,
                    partition_order,
                    parameters: [0; 1 << MAX_PARTITION_ORDER],
                    bits: fixed,
                };
                for part in 0..parts {
                    let range = partition_range(n, order, partition_order, part);
                    let (parameter, bits) = best_parameter(&folded[range]);
                    plan.parameters[part] = parameter;
                    plan.bits += PARAMETER_BITS as usize + bits;
                }
                if best.as_ref().is_none_or(|b| plan.bits < b.bits) {
                    best = Some(plan);
                }
            }
        }
        // Order 0 with one partition always qualifies
        best.expect("order 0 has a plan")
    }

    fn write(&self, x: &[i32], width: u32, folded: &mut Vec<u32>, writer: &mut BitWriter) {
        let n = x.len();
        writer.put(self.order as u32, ORDER_BITS);
        writer.put(self.partition_order, PARTITION_ORDER_BITS);
        for &value in &x[..self.order] {
            writer.put(value as u32 & ((1 << width) - 1), width);
        }
        fold_residuals(x, self.order, folded);
        for part in 0..1usize << self.partition_order {
            let parameter = u32::from(self.parameters[part]);
            writer.put(parameter, PARAMETER_BITS);
            for &value in &folded[partition_range(n, self.order, self.partition_order, part)] {
                writer.put_zeros_then_one(value >> parameter);
                writer.put(value & ((1 << parameter) - 1), parameter);
            }
        }
    }
}

/// The folded residuals of `x` at `order`, from sample `order` on
fn fold_residuals(x: &[i32], order: usize, folded: &mut Vec<u32>) {
    folded.clear();
    folded.extend((order..x.len()).map(|i| fold(i64::from(x[i]) - predict(x, i, order))));
}

/// Where partition `part` lies in the residuals of a channel of `n` samples. The
/// partitions are of equal length in samples; the first is short by the warm-up.
fn partition_range(
    n: usize,
    order: usize,
    partition_order: u32,
    part: usize,
) -> std::ops::Range<usize> {
    let len = n >> partition_order;
    let start = if part == 0 { 0 } else { part * len - order };
    start..(part + 1) * len - order
}

/// The Rice parameter that codes `folded` in the fewest bits, and the bits
fn best_parameter(folded: &[u32]) -> (u8, usize) {
    if folded.is_empty() {
        return (0, 0);
    }
    let sum: u64 = folded.iter().map(|&v| u64::from(v)).sum();
    let mean = sum / folded.len() as u64;
    let estimate = mean.checked_ilog2().unwrap_or(0);
    let mut best = (0u8, usize::MAX);
    for parameter in estimate.saturating_sub(1)..=(estimate + 1).min(MAX_PARAMETER) {
        let quotients: usize = folded.iter().map(|&v| (v >> parameter) as usize).sum();
        let bits = quotients + folded.len() * (parameter as usize + 1);
        if bits < best.1 {
            best = (parameter as u8, bits);
        }
    }
    best
}

/// Decode one channel of `n` samples, `width` bits each, into every `stride`th slot of `output`
fn decode_channel(
    reader: &mut BitReader,
    n: usize,
    width: u32,
    output: &mut [i32],
    stride: usize,
) -> Result<(), CodecError> {
    let order = reader.read(ORDER_BITS)? as usize;
    if order > MAX_ORDER {
        return Err(CodecError::InvalidData(format!("predictor order {order}")));
    }
    let partition_order = reader.read(PARTITION_ORDER_BITS)?;
    let parts = 1usize << partition_order;
    if !n.is_multiple_of(parts) || n / parts < order {
        return Err(CodecError::InvalidData(format!(
            "{n} samples do not divide into {parts} partitions of {order} or more"
        )));
    }

    // The latest four samples, newest first
    let mut history = [0i64; MAX_ORDER];
    for i in 0..order {
        let raw = reader.read(width)? as i32;
        // Sign-extend from `width` bits
        let shift = 32 - width;
        store(
            &mut history,
            output,
            i * stride,
            width,
            i64::from((raw << shift) >> shift),
        )?;
    }

    let len = n / parts;
    let mut i = order;
    for part in 0..parts {
        let parameter = reader.read(PARAMETER_BITS)?;
        if parameter > MAX_PARAMETER {
            return Err(CodecError::InvalidData(format!(
                "Rice parameter {parameter}"
            )));
        }
        let end = (part + 1) * len;
        while i < end {
            let quotient = reader.zeros_before_one((MAX_FOLDED >> parameter) as u32)?;
            let folded = (u64::from(quotient) << parameter) | u64::from(reader.read(parameter)?);
            if folded >= MAX_FOLDED {
                return Err(CodecError::InvalidData("residual out of range".into()));
            }
            let h = history;
            let predicted = match order {
                0 => 0,
                1 => h[0],
                2 => 2 * h[0] - h[1],
                3 => 3 * h[0] - 3 * h[1] + h[2],
                _ => 4 * h[0] - 6 * h[1] + 4 * h[2] - h[3],
            };
            store(
                &mut history,
                output,
                i * stride,
                width,
                predicted + unfold(folded as u32),
            )?;
            i += 1;
        }
    }
    Ok(())
}

/// Put a decoded sample at `index` of `output`, and in the history the next ones are
/// predicted from
fn store(
    history: &mut [i64; MAX_ORDER],
    output: &mut [i32],
    index: usize,
    width: u32,
    value: i64,
) -> Result<(), CodecError> {
    if !(-(1i64 << (width - 1))..(1i64 << (width - 1))).contains(&value) {
        return Err(CodecError::InvalidData(format!(
            "sample beyond {width} bits"
        )));
    }
    history.rotate_right(1);
    history[0] = value;
    output[index] = value as i32;
    Ok(())
}

struct BitWriter<'a> {
    out: &'a mut Vec<u8>,
    acc: u64,
    bits: u32,
}

impl<'a> BitWriter<'a> {
    fn new(out: &'a mut Vec<u8>) -> Self {
        Self {
            out,
            acc: 0,
            bits: 0,
        }
    }

    /// Append the low `count` (at most 32) bits of `value`
    fn put(&mut self, value: u32, count: u32) {
        let mask = if count >= 32 {
            u32::MAX
        } else {
            (1u32 << count) - 1
        };
        self.acc = (self.acc << count) | u64::from(value & mask);
        self.bits += count;
        while self.bits >= 8 {
            self.bits -= 8;
            self.out.push((self.acc >> self.bits) as u8);
        }
    }

    fn put_zeros_then_one(&mut self, mut zeros: u32) {
        while zeros >= 32 {
            self.put(0, 32);
            zeros -= 32;
        }
        self.put(0, zeros);
        self.put(1, 1);
    }

    fn finish(mut self) {
        if self.bits > 0 {
            self.put(0, 8 - self.bits);
        }
    }
}

struct BitReader<'a> {
    data: &'a [u8],
    position: usize,
}

impl<'a> BitReader<'a> {
    fn new(data: &'a [u8]) -> Self {
        Self { data, position: 0 }
    }

    fn short() -> CodecError {
        CodecError::InvalidData("packed packet is cut short".into())
    }

    /// Read `count` (at most 32) bits
    fn read(&mut self, count: u32) -> Result<u32, CodecError> {
        if self.position + count as usize > self.data.len() * 8 {
            return Err(Self::short());
        }
        let mut value = 0u64;
        let mut left = count;
        while left > 0 {
            let byte = self.data[self.position / 8];
            let available = 8 - (self.position % 8) as u32;
            let take = available.min(left);
            let bits = (u32::from(byte) >> (available - take)) & ((1 << take) - 1);
            value = (value << take) | u64::from(bits);
            self.position += take as usize;
            left -= take;
        }
        Ok(value as u32)
    }

    /// Count the zeros up to the next one, and step over that one. More than
    /// `limit` zeros is an error.
    fn zeros_before_one(&mut self, limit: u32) -> Result<u32, CodecError> {
        let mut zeros = 0u32;
        loop {
            let Some(&byte) = self.data.get(self.position / 8) else {
                return Err(Self::short());
            };
            let offset = (self.position % 8) as u32;
            let rest = byte << offset;
            if rest == 0 {
                zeros += 8 - offset;
                self.position += (8 - offset) as usize;
            } else {
                let lead = rest.leading_zeros();
                zeros += lead;
                self.position += lead as usize + 1;
                return Ok(zeros);
            }
            if zeros > limit {
                return Err(CodecError::InvalidData("run of zeros too long".into()));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// What a sample comes back as: itself, to 24 bits, clipped to what 24 bits hold
    fn through_24_bits(sample: f32) -> f32 {
        quantize(sample).0 as f32 / PCM_FULL_SCALE
    }

    struct Rng(u64);

    impl Rng {
        fn next(&mut self) -> u64 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            self.0
        }

        /// In -1.0..1.0
        fn sample(&mut self) -> f32 {
            (self.next() >> 40) as f32 / (1u64 << 23) as f32 - 1.0
        }
    }

    fn sine(frames: usize, channels: usize, amplitude: f32) -> Vec<f32> {
        (0..frames)
            .flat_map(|n| {
                let s = amplitude * (n as f32 * 0.07).sin();
                (0..channels).map(move |c| s * (1.0 - 0.1 * c as f32))
            })
            .collect()
    }

    fn round_trip(samples: &[f32], channels: usize) -> (Vec<u8>, Vec<f32>) {
        let mut packet = Vec::new();
        encode_packed(samples, channels, &mut packet);
        let mut back = vec![0.0; samples.len()];
        let count = decode_packed(&packet, &mut back).expect("a packet we made decodes");
        assert_eq!(count, samples.len());
        (packet, back)
    }

    fn assert_same_bits(samples: &[f32], back: &[f32]) {
        for (i, (&sent, &got)) in samples.iter().zip(back).enumerate() {
            assert_eq!(
                got.to_bits(),
                through_24_bits(sent).to_bits(),
                "sample {i}: sent {sent}, got {got}"
            );
        }
    }

    /// Lossless is the point: what comes out is the 24-bit integer that went in.
    ///
    /// Verifies: REQ-AUD-131
    #[test]
    fn what_is_packed_comes_back_as_the_same_24_bit_samples() {
        let mut rng = Rng(0x2545_F491_4F6C_DD1D);
        let noise: Vec<f32> = (0..256).map(|_| rng.sample()).collect();
        let quiet_noise: Vec<f32> = (0..256).map(|_| rng.sample() * 1e-5).collect();
        let cases: Vec<(&str, Vec<f32>, usize)> = vec![
            ("silence", vec![0.0; 256], 2),
            ("a tone on both sides", sine(128, 2, 0.5), 2),
            ("a tone, mono", sine(256, 1, 0.5), 1),
            ("three channels", sine(64, 3, 0.5), 3),
            ("noise", noise.clone(), 2),
            ("the noise floor of an interface", quiet_noise, 2),
            (
                "one side of noise, one of silence",
                noise.chunks(2).flat_map(|p| [p[0], 0.0]).collect(),
                2,
            ),
            (
                "full scale, flipping every sample",
                (0..128)
                    .map(|n| if n % 2 == 0 { -1.0 } else { 0.999_999_9 })
                    .collect(),
                2,
            ),
            (
                "the two sides flipping against each other",
                (0..128)
                    .flat_map(|n| {
                        if n % 2 == 0 {
                            [-1.0, 0.999_999_9]
                        } else {
                            [0.999_999_9, -1.0]
                        }
                    })
                    .collect(),
                2,
            ),
            (
                "beyond full scale and NaN",
                vec![2.0, -3.0, f32::NAN, 0.5, 1.0, -1.0, 0.0, 0.1],
                2,
            ),
        ];
        for (name, samples, channels) in cases {
            let (packet, back) = round_trip(&samples, channels);
            assert_same_bits(&samples, &back);
            assert!(
                packet.len() <= verbatim_len(samples.len()),
                "{name}: {} bytes for {} samples",
                packet.len(),
                samples.len()
            );
        }
    }

    #[test]
    fn a_packet_of_any_length_from_one_sample_up_comes_back_the_same() {
        let signal = sine(80, 2, 0.3);
        for frames in 1..=80 {
            let samples = &signal[..frames * 2];
            let (_, back) = round_trip(samples, 2);
            assert_same_bits(samples, &back);
        }
        // Not a whole number of frames: sent as it is
        let (packet, back) = round_trip(&signal[..7], 2);
        assert_eq!(packet[0], FORMAT_VERBATIM);
        assert_same_bits(&signal[..7], &back);
    }

    #[test]
    fn noise_that_does_not_pack_goes_as_it_is_and_is_one_byte_more_than_the_samples() {
        let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
        let samples: Vec<f32> = (0..256).map(|_| rng.sample()).collect();
        let (packet, back) = round_trip(&samples, 2);
        assert_eq!(packet[0], FORMAT_VERBATIM);
        assert_eq!(
            packet.len(),
            samples.len() * PCM_BYTES_PER_SAMPLE + PACKED_HEADER_BYTES
        );
        assert_same_bits(&samples, &back);
    }

    #[test]
    fn a_tone_packs_to_a_fraction_of_its_samples() {
        let samples = sine(128, 2, 0.5);
        let (packet, _) = round_trip(&samples, 2);
        assert!(
            packet.len() * 2 < verbatim_len(samples.len()),
            "{} of {}",
            packet.len(),
            verbatim_len(samples.len())
        );
    }

    #[test]
    fn the_same_samples_on_both_sides_cost_about_one_channel() {
        let mut rng = Rng(7);
        let mut one = 0.0f32;
        let mono: Vec<f32> = (0..128)
            .map(|_| {
                one = (one + rng.sample() * 0.001).clamp(-0.5, 0.5);
                one
            })
            .collect();
        let both: Vec<f32> = mono.iter().flat_map(|&s| [s, s]).collect();
        let (one_channel, _) = round_trip(&mono, 1);
        let (two_channels, back) = round_trip(&both, 2);
        assert_same_bits(&both, &back);
        assert!(
            two_channels.len() < one_channel.len() + one_channel.len() / 4,
            "{} bytes for two equal channels, {} for one",
            two_channels.len(),
            one_channel.len()
        );
    }

    #[test]
    fn digital_silence_takes_almost_nothing_before_it_is_rounded_up() {
        let mut packet = Vec::new();
        encode_packed_exact(&vec![0.0; 256], 2, &mut packet);
        assert!(packet.len() < 40, "{} bytes", packet.len());
    }

    /// The size of a packet follows the audio and so tells a watcher of the encrypted
    /// traffic about it; there are only so many sizes it can be.
    ///
    /// Verifies: REQ-AUD-132
    #[test]
    fn the_size_of_a_packet_is_one_of_a_few_sizes() {
        let mut rng = Rng(11);
        let mut sizes = std::collections::BTreeSet::new();
        for scale in [0.0, 1e-7, 1e-6, 1e-5, 1e-4, 1e-3, 1e-2, 1e-1, 1.0] {
            for frames in [64usize, 128] {
                let samples: Vec<f32> = (0..frames * 2).map(|_| rng.sample() * scale).collect();
                let mut packet = Vec::new();
                encode_packed(&samples, 2, &mut packet);
                let max = verbatim_len(samples.len());
                let step = max.div_ceil(SIZE_STEPS);
                assert!(
                    packet.len() % step == 0 || packet.len() == max,
                    "{} bytes is not a multiple of {step} nor {max}",
                    packet.len()
                );
                assert!(packet.len() <= max);
                sizes.insert((frames, packet.len()));
            }
        }
        assert!(sizes.len() <= 2 * SIZE_STEPS, "{sizes:?}");
    }

    #[test]
    fn padding_is_ignored_when_the_packet_is_unpacked() {
        let samples = sine(64, 2, 0.1);
        let mut exact = Vec::new();
        encode_packed_exact(&samples, 2, &mut exact);
        let mut padded = exact.clone();
        padded.extend_from_slice(&[0; 200]);
        let (mut a, mut b) = (vec![0.0; samples.len()], vec![0.0; samples.len()]);
        decode_packed(&exact, &mut a).unwrap();
        decode_packed(&padded, &mut b).unwrap();
        assert_eq!(a, b);
    }

    /// A lost packet costs nothing to the ones after it.
    #[test]
    fn a_packet_unpacks_without_the_ones_before_it() {
        let signal = sine(256, 2, 0.4);
        let packets: Vec<Vec<u8>> = signal
            .chunks(128)
            .map(|chunk| {
                let mut packet = Vec::new();
                encode_packed(chunk, 2, &mut packet);
                packet
            })
            .collect();
        let mut back = vec![0.0; 128];
        decode_packed(&packets[3], &mut back).unwrap();
        assert_same_bits(&signal[3 * 128..4 * 128], &back);
    }

    #[test]
    fn a_packet_that_does_not_fit_the_output_is_refused() {
        let samples = sine(64, 2, 0.1);
        let mut packet = Vec::new();
        encode_packed(&samples, 2, &mut packet);
        let mut small = vec![0.0; samples.len() - 1];
        assert!(matches!(
            decode_packed(&packet, &mut small),
            Err(CodecError::InvalidFrameSize { .. })
        ));
    }

    /// Verifies: REQ-AUD-133
    #[test]
    fn a_damaged_packet_is_refused_and_does_not_panic() {
        let mut rng = Rng(99);
        let samples = sine(128, 2, 0.4);
        let mut good = Vec::new();
        encode_packed_exact(&samples, 2, &mut good);
        let mut out = vec![0.0; 1024];
        // Cut short at every length
        for len in 0..good.len() {
            let _ = decode_packed(&good[..len], &mut out);
        }
        // One byte changed, anywhere
        for i in 0..good.len() {
            for _ in 0..8 {
                let mut bad = good.clone();
                bad[i] = rng.next() as u8;
                let _ = decode_packed(&bad, &mut out);
            }
        }
        // Not a packet at all
        for _ in 0..2000 {
            let len = (rng.next() % 600) as usize;
            let junk: Vec<u8> = (0..len).map(|_| rng.next() as u8).collect();
            let _ = decode_packed(&junk, &mut out);
        }
        assert!(decode_packed(&[], &mut out).is_err());
        assert!(decode_packed(&[0xE1, 0, 8], &mut out).is_err());
        assert!(decode_packed(&[FORMAT_PACKED, 0xFF, 0xFF, 0, 0], &mut out).is_err());
        assert!(decode_packed(&[3], &mut out).is_err());
    }

    /// Verifies: REQ-AUD-133
    #[test]
    fn a_packet_that_claims_more_samples_than_a_packet_holds_is_refused_before_anything_is_made_for_them(
    ) {
        let mut packet = vec![FORMAT_PACKED | (7 << CHANNELS_SHIFT), 0xFF, 0xFF];
        packet.extend_from_slice(&[0; 1400]);
        assert!(packed_sample_count(&packet).is_err());
    }
}
