import '@fontsource-variable/inter';
import { registerBuiltinRenderers } from '@preset.nz/facets';
import React from 'react';
import ReactDOM from 'react-dom/client';
import App from '@/App';
import '@/index.css';

// facets looks renderers up by string kind, so they must be registered before
// the first panel renders.
registerBuiltinRenderers();

ReactDOM.createRoot(document.getElementById('root') as HTMLElement).render(
  <React.StrictMode>
    <App />
  </React.StrictMode>,
);
