// Extra downloadable templates shown on the download page. Add an entry (and
// the file under website/public/downloads) for each new control surface.
/**
 * @typedef {object} TemplateDownload
 * @property {string} title
 * @property {string} description
 * @property {string} href
 * @property {string} action
 * @property {string} [docHref]
 */

/** @type {TemplateDownload[]} */
export const templateDownloads = [
  {
    title: 'Bitfocus Companion',
    description: 'Control Multiviewer using a Streamdeck via OSC',
    href: '/downloads/Multiviewer-companion.companionconfig',
    action: 'Download Companion config',
    docHref: '/docs/osc/#bitfocus-companion',
  },
];
