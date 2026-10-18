export interface Protocol {
  name: string;
  input?: boolean;
  output?: boolean;
  platforms: ('macos' | 'windows' | 'linux')[];
  /** General description, used on the landing matrix. */
  note?: string;
  /** Description when listed as an input; falls back to `note`. */
  inputNote?: string;
  /** Description when listed as an output; falls back to `note`. */
  outputNote?: string;
}

/** Inputs and outputs, shared by the landing matrix and the docs pages. */
export const protocols: Protocol[] = [
  {
    name: 'NDI',
    input: true,
    output: true,
    platforms: ['macos', 'windows', 'linux'],
    note: 'Network video',
    inputNote: 'Proprietary network video protocol',
    outputNote: 'Proprietary network video protocol',
  },
  {
    name: 'OMT',
    input: true,
    output: true,
    platforms: ['macos', 'windows', 'linux'],
    note: 'Open Media Transport',
    inputNote: 'Open Media Transport, open-source network video protocol',
    outputNote: 'Open Media Transport, open-source network video protocol',
  },
  {
    name: 'DeckLink',
    input: true,
    output: true,
    platforms: ['macos', 'windows', 'linux'],
    note: 'SDI and HDMI',
    inputNote: 'Blackmagic Design capture devices (SDI, HDMI, ...)',
    outputNote: 'Blackmagic Design playback devices (SDI, HDMI, ...)',
  },
  {
    name: 'Syphon',
    input: true,
    output: true,
    platforms: ['macos'],
    note: 'Zero-copy GPU texture sharing',
    inputNote: 'Zero-copy GPU texture sharing',
    outputNote: 'Zero-copy GPU texture sharing',
  },
  {
    name: 'Spout',
    input: true,
    output: true,
    platforms: ['windows'],
    note: 'Zero-copy GPU texture sharing',
    inputNote: 'Zero-copy GPU texture sharing',
    outputNote: 'Zero-copy GPU texture sharing',
  },
  {
    name: 'AVFoundation',
    input: true,
    platforms: ['macos'],
    inputNote: 'Webcams and capture devices',
  },
  {
    name: 'DirectShow',
    input: true,
    platforms: ['windows'],
    inputNote: 'Webcams and WDM capture devices',
  },
  {
    name: 'MediaFoundation',
    input: true,
    platforms: ['windows'],
    inputNote: 'Webcams and capture devices',
  },
  {
    name: 'Screen capture',
    input: true,
    platforms: ['macos', 'windows'],
    inputNote: 'Displays and windows',
  },
  {
    name: 'VNC',
    input: true,
    platforms: ['macos', 'windows', 'linux'],
    note: 'Remote desktop',
    inputNote: 'Remote desktop frames over RFB',
  },
  {
    name: 'Test patterns',
    input: true,
    platforms: ['macos', 'windows', 'linux'],
    inputNote: 'SMPTE bars and generative patterns',
  },
];
