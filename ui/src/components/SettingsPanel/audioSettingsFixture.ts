/**
 * Audio settings as the app reports them (`settings_get`), for tests of the
 * settings panel adapter. Defaults describe a machine with no devices.
 */
import type { AudioSettings } from "../../lib/tauri";

export function audioSettings(overrides: Partial<AudioSettings> = {}): AudioSettings {
  return {
    revision: 0,
    input_devices: [],
    output_devices: [],
    input_device_id: null,
    output_device_id: null,
    input_channel_count: null,
    output_channel_count: null,
    input_channels: { left: 1, right: 2 },
    output_channels: { left: 1, right: 2 },
    transmit_channels: 2,
    buffer_size: 64,
    buffer_sizes: [32, 64, 128, 256],
    sample_rate: 48000,
    sample_rates: [
      { rate: 44100, label: "44100 Hz", recommended: false },
      { rate: 48000, label: "48000 Hz", recommended: true },
      { rate: 96000, label: "96000 Hz", recommended: false },
    ],
    ...overrides,
  };
}
