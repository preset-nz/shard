import path from 'node:path';
import tailwindcss from '@tailwindcss/vite';
import react from '@vitejs/plugin-react';
import { defineConfig, searchForWorkspaceRoot } from 'vite';

const host = process.env.TAURI_DEV_HOST;

// https://vite.dev/config/
export default defineConfig(async () => ({
  plugins: [tailwindcss(), react()],

  resolve: {
    alias: {
      '@': path.resolve(__dirname, './src'),
    },
    // @preset.nz packages ship unbuilt TypeScript. Pin the shared runtime to
    // this app's copy; facets and the kit are in the list because two copies would mean
    // two registries (a temporary `pnpm link` makes that easy to hit).
    dedupe: [
      'react',
      'react-dom',
      '@tauri-apps/api',
      '@preset.nz/ux-kit',
      '@preset.nz/facets',
      '@preset.nz/app-kit',
    ],
  },

  // Vite options tailored for Tauri development and only applied in `tauri dev` or `tauri build`
  //
  // 1. prevent Vite from obscuring rust errors
  clearScreen: false,
  // 2. tauri expects a fixed port, fail if that port is not available
  server: {
    port: 1460,
    strictPort: true,
    host: host || false,
    hmr: host
      ? {
          protocol: 'ws',
          host,
          port: 1461,
        }
      : undefined,
    // ux-kit is linked from ../../packages while it waits for a release
    // (pending-releases); let Vite serve its fonts from there.
    fs: {
      allow: [
        searchForWorkspaceRoot(process.cwd()),
        path.resolve(__dirname, '../../packages/ux-kit'),
      ],
    },
    watch: {
      // 3. tell Vite to ignore watching `src-tauri`
      ignored: ['**/src-tauri/**'],
    },
  },
}));
