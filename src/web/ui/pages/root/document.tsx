/** web/ui/pages/root/document.tsx — the root chooser's prerendered document. See ../../../http-router.md. */
import { Document } from '../../lib/Document.tsx';

export const RootDocument = () => (
  <Document page={{ name: 'root', title: '%%HOUSEHOLD%% — shared albums', hasScript: true }}>
    <div id="app"></div>
    <noscript>
      <main>This page needs JavaScript. It only decides which panel to open.</main>
    </noscript>
  </Document>
);
