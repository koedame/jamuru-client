/**
 * Renders the components whose strings used to be written straight into JSX
 * (connection history, reactions, side panel, settings, diagnostics, session
 * stats, device selector, mixer labels) in each UI language.
 *
 * The bug this guards: with the language set to Japanese, these showed English
 * ("Settings", "Yesterday", "3 days ago", "Balanced", "Name is required", ...)
 * because they bypassed the locale bundles.
 */

import { describe, it, expect, beforeEach, afterAll, vi } from 'vitest';
import { render, screen, fireEvent, waitFor } from '@testing-library/react';

import i18n from './index';
import en from '../../locales/en.json';
import ja from '../../locales/ja.json';
import { ConnectionPanel } from '../components/ConnectionPanel';
import { ConnectionHistory } from '../components/ConnectionHistory';
import { QuickReactions, ReactionButton } from '../components/ChatPanel';
import { SidePanel } from '../components/SidePanel';
import { SessionStats } from '../components/SessionStats';
import { DeviceSelector } from '../components/DeviceSelector';
import { MixerPanel, type Channel } from '../components/MixerPanel';
import { DiagnosticsTab } from '../components/SettingsPanel/tabs/DiagnosticsTab';
import { SettingsPanelAdapter } from '../components/SettingsPanel';
import type {
  CompleteDiagnosticsResult,
  DetailedLatency,
  DiagnosticProblem,
  NetworkStats,
} from '../lib/tauri';

const invoke = vi.hoisted(() => vi.fn());
vi.mock('@tauri-apps/api/core', () => ({ invoke }));

type Bundle = typeof en;

/** Japanese has no `_one` plural forms, so its type differs from English's. */
const languages: [string, Bundle][] = [
  ['en', en],
  ['ja', ja as unknown as Bundle],
];

/** Fill `{{name}}` placeholders the way i18next does. */
function fill(template: string, values: Record<string, string | number>): string {
  return template.replace(/\{\{(\w+)\}\}/g, (_, name) => String(values[name]));
}

/** Every string a user or a screen reader gets from the rendered DOM, one per text node. */
function shownStrings(root: HTMLElement): string[] {
  const strings: string[] = [];
  const texts = document.createTreeWalker(root, NodeFilter.SHOW_TEXT);
  for (let node = texts.nextNode(); node; node = texts.nextNode()) strings.push(node.nodeValue ?? '');
  for (const el of root.querySelectorAll('*')) {
    for (const attr of ['aria-label', 'title', 'placeholder']) {
      const value = el.getAttribute(attr);
      if (value) strings.push(value);
    }
  }
  return strings;
}

/**
 * Latin words left on screen, minus the fixtures' own data (names, codes) and
 * terms the Japanese bundle itself keeps in Latin letters.
 */
function latinWords(root: HTMLElement, allowed: string[]): string[] {
  const kept = new Set(allowed);
  return shownStrings(root)
    .flatMap((s) => s.match(/[A-Za-z]{2,}/g) ?? [])
    .filter((word) => !kept.has(word));
}

const daysAgo = (days: number) => new Date(Date.now() - days * 86_400_000 - 3_600_000).toISOString();

const history = [
  { room_code: 'ABC123', label: '', connected_at: daysAgo(1) },
  { room_code: 'DEF456', label: '', connected_at: daysAgo(3) },
];

const channels: Channel[] = [
  {
    id: 'peer-1',
    name: 'Alice',
    type: 'remote',
    sampleRate: 48000,
    channelCount: 2,
    levelL: 10,
    levelR: 10,
    volume: 80,
    pan: 0,
    isMuted: false,
  },
];

const network: NetworkStats = {
  rtt_ms: 18.42,
  jitter_ms: 0.83,
  packet_loss_percent: 0.1,
  quality: 'good',
  measured_bps: 3_200_000,
  required_bps: 3_100_000,
  bandwidth_status: 'sufficient',
  uptime_seconds: 125,
  packets_sent: 1000,
  packets_received: 998,
  bytes_sent: 512_000,
  bytes_received: 511_000,
};

const latency: DetailedLatency = {
  upstream: [],
  upstream_total_ms: 1,
  downstream: [],
  downstream_total_ms: 2,
  roundtrip_total_ms: 3,
};

/**
 * One problem per `ProblemCode` variant (rm:887), so the "no English leaks
 * out of the backend-supplied problem codes" case is actually exercised
 * (previously this fixture always left `problems` empty).
 */
const diagnosticsProblems: DiagnosticProblem[] = [
  { severity: 'Error', category: 'network', code: { type: 'NoConnectivity' } },
  { severity: 'Info', category: 'network', code: { type: 'NoIpv6' } },
  { severity: 'Warning', category: 'network', code: { type: 'SymmetricNat' } },
  { severity: 'Warning', category: 'network', code: { type: 'UnstableConnection' } },
  { severity: 'Warning', category: 'network', code: { type: 'HighJitter', data: { jitter_ms: 12.3 } } },
  {
    severity: 'Error',
    category: 'network',
    code: { type: 'SignalingUnreachable', data: { url: 'signal.test:9000', error: 'ENODEV' } },
  },
  {
    severity: 'Error',
    category: 'audio',
    code: { type: 'InputEnumerationFailed', data: { error: 'ENODEV' } },
  },
  {
    severity: 'Error',
    category: 'audio',
    code: { type: 'OutputEnumerationFailed', data: { error: 'ENODEV' } },
  },
  { severity: 'Error', category: 'audio', code: { type: 'NoInputDevices' } },
  { severity: 'Error', category: 'audio', code: { type: 'NoOutputDevices' } },
  { severity: 'Warning', category: 'audio', code: { type: 'InputNot48kHz', data: { device_name: 'Mic' } } },
  {
    severity: 'Warning',
    category: 'audio',
    code: { type: 'OutputNot48kHz', data: { device_name: 'Speaker' } },
  },
  { severity: 'Warning', category: 'audio', code: { type: 'LowBufferUnsupported' } },
  { severity: 'Info', category: 'audio', code: { type: 'NoAsioDevices' } },
  { severity: 'Warning', category: 'cpu', code: { type: 'InsufficientRealtimeHeadroom' } },
  { severity: 'Warning', category: 'cpu', code: { type: 'HighCpuUsage', data: { usage_percent: 92.4 } } },
  { severity: 'Warning', category: 'cpu', code: { type: 'LowMemory', data: { available_mb: 256 } } },
  { severity: 'Warning', category: 'cpu', code: { type: 'SmallBufferGlitchRisk' } },
];

const diagnosticsResult: CompleteDiagnosticsResult = {
  network: {
    ip_support: {
      ipv4_available: true,
      ipv6_available: false,
      ipv4_addresses: [],
      ipv6_addresses: [],
      public_ipv4: null,
      public_ipv6: null,
    },
    nat_type: 'PortRestrictedCone',
    connection_stability: 'A',
    jitter_ms: null,
    stability_metrics: {
      avg_rtt_ms: null,
      min_rtt_ms: null,
      max_rtt_ms: null,
      successful_probes: 0,
      failed_probes: 0,
      packet_loss_rate: 0,
    },
    signaling: { connected: true, connection_time_ms: 120, error: null },
    problems: [],
  },
  audio: {
    input_devices: [],
    output_devices: [],
    selected_input: null,
    selected_output: null,
    input_source: 'OsDefault',
    output_source: 'OsDefault',
    low_latency_support: {
      asio_available: false,
      asio_devices: [],
      supports_32_samples: true,
      supports_64_samples: true,
      supports_128_samples: true,
      min_buffer_size: 32,
      estimated_min_latency_ms: 0.67,
    },
    overall_grade: 'A',
    problems: [],
  },
  cpu: {
    benchmarks: [],
    system: { cpu_cores: 10, cpu_usage: 0.15, available_memory_mb: 8192 },
    grade: 'A',
    realtime_capable: true,
    problems: [],
  },
  overall_score: 96,
  recommended_preset: 'UltraLowLatency',
  zero_latency_compatible: false,
  problems: diagnosticsProblems,
};

/** What the backend returns to the settings panel when it loads. */
const backend: Record<string, unknown> = {
  audio_list_input_devices: [
    { id: 'in1', name: 'Mic', supported_sample_rates: [48000], supported_channels: [1, 2, 3], is_default: true, is_asio: false },
  ],
  audio_list_output_devices: [
    { id: 'out1', name: 'Speaker', supported_sample_rates: [48000], supported_channels: [1, 2], is_default: true, is_asio: false },
  ],
  audio_get_current_devices: { input_device_id: 'in1', output_device_id: 'out1' },
  audio_get_buffer_size: 64,
  audio_get_device_channels: [1, 2, 3],
  config_get_peer_name: 'Taro',
  config_get_sample_rate: 48000,
  config_list_sample_rates: [
    { rate: 44100, label: '44.1 kHz', recommended: false },
    { rate: 48000, label: '48 kHz', recommended: true },
  ],
  config_get_input_channels: { channel_l: 1, channel_r: 2 },
  config_get_output_channels: { channel_l: 1, channel_r: 2 },
  config_get_transmit_channels: 2,
};

describe('components without hard-coded text', () => {
  beforeEach(() => {
    invoke.mockReset();
    invoke.mockImplementation(async (command: string) => backend[command]);
  });

  afterAll(async () => {
    await i18n.changeLanguage('en');
  });

  // Given the connection panel and the history list show recent rooms
  // When the language is English or Japanese
  // Then the settings / remove buttons and the dates read in that language
  //
  // Verifies: REQ-I18N-105
  it.each(languages)('shows the connection history in %s', async (language, bundle: Bundle) => {
    await i18n.changeLanguage(language);
    const { container } = render(
      <>
        <ConnectionPanel
          connectionHistory={history}
          onOpenSettings={() => {}}
          onHistorySelect={() => {}}
          onHistoryRemove={() => {}}
          onCreateRoom={() => {}}
          onJoinRoom={() => {}}
        />
        <ConnectionHistory history={history} onSelect={() => {}} onRemove={() => {}} />
      </>
    );

    expect(screen.getByLabelText(bundle.settings.title)).toBeInTheDocument();
    expect(screen.getAllByLabelText(bundle.connectionHistory.remove)).toHaveLength(4);
    expect(screen.getAllByText(bundle.connectionHistory.yesterday)).toHaveLength(2);
    expect(screen.getAllByText(fill(bundle.connectionHistory.daysAgo, { days: 3 }))).toHaveLength(2);
    if (language === 'ja') {
      expect(latinWords(container, ['ABC', 'DEF', 'TEST', 'jamjam'])).toEqual([]);
    }
  });

  // Given a reaction, the quick reactions and the side panel
  // When the language is English or Japanese
  // Then their accessible names read in that language, with the count's plural
  //
  // Verifies: REQ-I18N-105
  it.each(languages)('names the reaction and side panel controls in %s', async (language, bundle: Bundle) => {
    await i18n.changeLanguage(language);
    const { container } = render(
      <>
        <QuickReactions />
        <ReactionButton emoji="👍" count={1} />
        <ReactionButton emoji="❤️" count={2} />
        <SidePanel isOpen onClose={() => {}} title="Panel">
          <p>body</p>
        </SidePanel>
      </>
    );

    expect(screen.getByRole('group', { name: bundle.chat.reaction.quick })).toBeInTheDocument();
    expect(screen.getByLabelText(fill(bundle.chat.reaction.react, { emoji: '👍' }))).toBeInTheDocument();
    expect(screen.getByLabelText(bundle.chat.reaction.more)).toBeInTheDocument();
    expect(screen.getByLabelText(bundle.common.button.close)).toBeInTheDocument();
    const one = language === 'en' ? en.chat.reaction.label_one : ja.chat.reaction.label_other;
    expect(screen.getByLabelText(fill(one, { emoji: '👍', count: 1 }))).toBeInTheDocument();
    expect(screen.getByLabelText(fill(bundle.chat.reaction.label_other, { emoji: '❤️', count: 2 }))).toBeInTheDocument();
    if (language === 'ja') {
      expect(latinWords(container, ['Panel', 'body'])).toEqual([]);
    }
  });

  // Given the mixer shows a channel
  // When the language is English or Japanese
  // Then the pan and volume sliders are named in that language
  //
  // Verifies: REQ-I18N-105
  it.each(languages)('names the mixer sliders in %s', async (language, bundle: Bundle) => {
    await i18n.changeLanguage(language);
    render(<MixerPanel channels={channels} onChannelMonitorToggle={() => {}} />);

    expect(screen.getByLabelText(fill(bundle.mixer.channel.panLabel, { name: 'Alice' }))).toBeInTheDocument();
    expect(screen.getByLabelText(fill(bundle.mixer.channel.volumeLabel, { name: 'Alice' }))).toBeInTheDocument();
  });

  // Given a diagnostics result is shown
  // When the language is English or Japanese
  // Then the usage reporting switch below it, with its description, reads in that language
  //
  // Verifies: REQ-TEL-011
  it.each(languages)('shows the usage reporting section under a diagnostics result in %s', async (language, bundle: Bundle) => {
    await i18n.changeLanguage(language);
    const { container } = render(
      <DiagnosticsTab
        state="complete"
        result={diagnosticsResult}
        onUsageReportingChange={() => {}}
        usagePreview=""
      />
    );

    expect(screen.getByRole('switch', { name: bundle.settings.diagnostics.usageToggle })).not.toBeChecked();
    expect(screen.getByText(bundle.settings.diagnostics.usageDescription)).toBeInTheDocument();
    expect(screen.getByText(bundle.settings.diagnostics.usagePreviewOff)).toBeInTheDocument();
    if (language === 'ja') {
      const section = container.querySelector<HTMLElement>('[data-testid="diagnostics-usage"]')!;
      expect(latinWords(section, ['OS', 'CPU', 'ID', 'IP', 'URL', 'AirPods', 'jamjam'])).toEqual([]);
    }
  });

  // Given the diagnostics result recommends a preset and reports a NAT type
  // When the language is English or Japanese
  // Then the preset, the NAT type and the missing values read in that language
  //
  // Verifies: REQ-I18N-105
  it.each(languages)('shows the diagnostics result in %s', async (language, bundle: Bundle) => {
    await i18n.changeLanguage(language);
    const { container } = render(<DiagnosticsTab state="complete" result={diagnosticsResult} />);
    const codes = bundle.settings.diagnostics.problemCodes;

    expect(screen.getByText(bundle.diagnostics.presets.ultraLowLatency)).toBeInTheDocument();
    expect(screen.getByText(bundle.diagnostics.natTypes.portRestrictedCone)).toBeInTheDocument();
    expect(screen.getAllByText(bundle.settings.diagnostics.notAvailable).length).toBeGreaterThan(0);
    expect(screen.getByText(fill(bundle.settings.diagnostics.samples, { count: 32 }))).toBeInTheDocument();
    // Backend-supplied problem codes (rm:887): a fixed-text one and one with
    // interpolated data, to prove the code -> locale lookup actually ran.
    expect(screen.getByText(codes.symmetricNat.message)).toBeInTheDocument();
    expect(screen.getByText(fill(codes.highJitter.message, { jitter: '12.3' }))).toBeInTheDocument();
    expect(screen.getByText(fill(codes.lowMemory.message, { mb: 256 }))).toBeInTheDocument();
    if (language === 'ja') {
      // ENODEV/wss/signal/test are raw backend error text and a URL, kept
      // as-is by design (same as MainScreen's connectionError). Mic/Speaker
      // are the fixture's device names, not translated content.
      expect(
        latinWords(container, [
          'RTT', 'CPU', 'NAT', 'IP', 'kHz', 'ms',
          'ASIO', 'ALL', 'Windows', 'WiFi', 'IPv', 'VPN', 'MB',
          'ENODEV', 'signal', 'test', 'Mic', 'Speaker',
        ])
      ).toEqual([]);
    }
  });

  // Given the session statistics and a device selector
  // When the language is English or Japanese
  // Then their labels read in that language
  //
  // Verifies: REQ-I18N-105
  it.each(languages)('shows the session statistics and device selector in %s', async (language, bundle: Bundle) => {
    await i18n.changeLanguage(language);
    const { container } = render(
      <>
        <SessionStats network={network} latency={latency} />
        <DeviceSelector
          type="input"
          devices={[{ id: 'a', name: 'Mic', is_default: true, is_asio: false, supported_sample_rates: [], supported_channels: [] }]}
          selectedDeviceId="a"
          onDeviceChange={() => {}}
        />
      </>
    );

    expect(screen.getByText(bundle.sessionStats.packetLoss)).toBeInTheDocument();
    expect(screen.getByText(bundle.sessionStats.latencyBreakdown)).toBeInTheDocument();
    expect(screen.getByText(bundle.sessionStats.roundTrip)).toBeInTheDocument();
    expect(screen.getByText(bundle.deviceSelector.input)).toBeInTheDocument();
    expect(screen.getByText(`Mic (${bundle.common.default})`)).toBeInTheDocument();
    if (language === 'ja') {
      expect(latinWords(container, ['Mic', 'RTT', 'ms', 'KB', 'kHz'])).toEqual([]);
    }
  });

  // Given the settings panel has loaded the devices and sample rates
  // When the language is English or Japanese
  // Then the buffer, channel and sample rate options read in that language
  //
  // Verifies: REQ-I18N-105
  it.each(languages)('labels the device options of the settings panel in %s', async (language, bundle: Bundle) => {
    await i18n.changeLanguage(language);
    const { container } = render(<SettingsPanelAdapter initialTab="devices" />);

    expect(await screen.findAllByText(fill(bundle.settings.devices.channelOption, { channel: 3 }))).not.toHaveLength(0);
    expect(screen.getAllByText(fill(bundle.settings.devices.bufferOption, { samples: 64, ms: '1.33' })).length).toBeGreaterThan(0);
    expect(screen.getAllByText(`48 kHz (${bundle.preset.recommended})`).length).toBeGreaterThan(0);
    if (language === 'ja') {
      expect(latinWords(container, ['Mic', 'Speaker', 'kHz', 'ms', 'Ch', 'ASIO', 'MONO', 'CPU'])).toEqual([]);
    }
  });

  // Given the display name field in the profile tab
  // When the name is emptied or too long
  // Then the error reads in the current language, and the placeholder does too
  //
  // Verifies: REQ-I18N-105
  it.each(languages)('validates the display name in %s', async (language, bundle: Bundle) => {
    await i18n.changeLanguage(language);
    render(<SettingsPanelAdapter initialTab="profile" />);

    const input = await screen.findByPlaceholderText(bundle.settings.profile.namePlaceholder);
    await waitFor(() => expect(input).toHaveValue('Taro'));

    fireEvent.change(input, { target: { value: '' } });
    expect(await screen.findByText(bundle.settings.profile.nameRequired)).toBeInTheDocument();

    fireEvent.change(input, { target: { value: 'x'.repeat(33) } });
    expect(await screen.findByText(bundle.settings.profile.nameTooLong)).toBeInTheDocument();
  });

  // Given the device options were loaded in English
  // When the language changes to Japanese while the panel is open
  // Then the options are re-labelled without reloading the devices
  //
  // Verifies: REQ-I18N-105
  it('re-labels the device options when the language changes', async () => {
    await i18n.changeLanguage('en');
    render(<SettingsPanelAdapter initialTab="devices" />);
    expect((await screen.findAllByText(fill(en.settings.devices.channelOption, { channel: 3 }))).length).toBeGreaterThan(0);

    await i18n.changeLanguage('ja');

    expect(await screen.findAllByText(fill(ja.settings.devices.channelOption, { channel: 3 }))).not.toHaveLength(0);
    expect(screen.queryByText(fill(en.settings.devices.channelOption, { channel: 3 }))).not.toBeInTheDocument();
    expect(screen.getAllByText(`48 kHz (${ja.preset.recommended})`).length).toBeGreaterThan(0);
  });
});
