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
    inputNote: 'Network video, bundled runtime',
    outputNote: 'Advertise the canvas on the network',
  },
  {
    name: 'DeckLink',
    input: true,
    output: true,
    platforms: ['macos', 'windows', 'linux'],
    note: 'SDI and HDMI',
    inputNote: 'SDI and HDMI capture and playback',
    outputNote: 'Play out to SDI or HDMI',
  },
  {
    name: 'Syphon',
    input: true,
    output: true,
    platforms: ['macos'],
    note: 'Zero-copy GPU texture sharing',
    inputNote: 'Zero-copy GPU texture sharing',
    outputNote: 'Share the canvas as a GPU texture',
  },
  {
    name: 'Spout',
    input: true,
    output: true,
    platforms: ['windows'],
    note: 'Zero-copy GPU texture sharing',
    inputNote: 'Zero-copy GPU texture sharing',
    outputNote: 'Share the canvas as a GPU texture',
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
    name: 'Test patterns',
    input: true,
    platforms: ['macos', 'windows', 'linux'],
    inputNote: 'SMPTE bars and generative patterns',
  },
];
