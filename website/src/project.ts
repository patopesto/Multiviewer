import type { ProjectConfig } from '@patopest/astro-theme/config';
import releasesFallback from './data/releases.fallback.json';
import icon from './assets/icon.svg?raw';

const repository = 'https://gitlab.com/patopest/multiviewer';

export const project: ProjectConfig = {
  name: 'Multiviewer',
  tagline: 'Hardware-accelerated multi-protocol video multiviewer.',
  description:
    'Compose NDI, DeckLink, Syphon, Spout, capture-device and screen sources on a real-time canvas, then broadcast the result. Native apps for macOS, Windows and Linux.',
  domain: 'https://multiviewer.example.com',
  accent: '#ff3b30',
  icon: { svg: icon, size: '30px' },
  gitlabProject: 'patopest/multiviewer',
  repoUrl: repository,
  nav: [
    { label: 'Features', href: '/#features' },
    { label: 'Docs', href: '/docs/getting-started/' },
    { label: 'Release notes', href: '/release-notes/' },
  ],
  platforms: [
    { os: 'macos', label: 'macOS', note: 'Apple Silicon. Requires macOS 12.3 or later.' },
    { os: 'windows', label: 'Windows', note: 'Windows 10 or later, 64-bit.' },
    { os: 'linux', label: 'Linux', note: 'x86_64 and arm64, as AppImage or .deb.' },
  ],
  footer: [
    {
      title: 'Project',
      links: [
        { label: 'Features', href: '/#features' },
        { label: 'Download', href: '/download/' },
        { label: 'Release notes', href: '/release-notes/' },
      ],
    },
    {
      title: 'Docs',
      links: [
        { label: 'Getting started', href: '/docs/getting-started/' },
        { label: 'Sources', href: '/docs/sources/' },
        { label: 'Outputs', href: '/docs/outputs/' },
        { label: 'Keyboard shortcuts', href: '/docs/shortcuts/' },
      ],
    },
    {
      title: 'Resources',
      links: [
        { label: 'Source code', href: repository },
        { label: 'Report an issue', href: `${repository}/-/issues` },
      ],
    },
  ],
  releasesFallback: releasesFallback as ProjectConfig['releasesFallback'],
  projects: [
    {
      name: 'sACN Monitor',
      description: 'A tool to view sACN and ArtNet DMX data on the network',
      href: "https://sacn-monitor.bambinito.net",
      logo: "https://sacn-monitor.bambinito.net/appicon.png"
    },
    {
      name: 'Timecoder',
      description: 'View, edit, convert and route Timecode in realtime',
      href: "https://gitlab.com/patopest/timecoder",
    },
    {
      name: 'GrandMA3 plugins and LUA docs',
      description: 'A collection of grandMA3 plugins and comprehensive LUA documentation',
      href: "https://grandma3.bambinito.net",
    },
  ],
};
