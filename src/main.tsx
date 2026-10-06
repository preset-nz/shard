import { registerBuiltinRenderers } from '@preset.nz/facets';
import React from 'react';
import ReactDOM from 'react-dom/client';
import App from '@/App';
import '@/index.css';

// facets looks renderers up by string kind, so they must be registered before
// the first panel renders.
registerBuiltinRenderers();

// No WebKit context menu (Reload, Inspect Element): right-click opens an app-drawn menu or
// nothing. Text fields keep WebKit's for Cut, Copy and Paste. Stopgap until app-kit's
// nativeContextMenu() and its native text menu arrive with epic 32 story 4; delete it then.
document.addEventListener('contextmenu', (e) => {
  if (e.defaultPrevented) return;
  const el = e.target as HTMLElement | null;
  if (el?.closest('input, textarea, [contenteditable="true"]')) return;
  e.preventDefault();
});

ReactDOM.createRoot(document.getElementById('root') as HTMLElement).render(
  <React.StrictMode>
    <App />
  </React.StrictMode>,
);
