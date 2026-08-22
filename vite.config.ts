import { sveltekit } from '@sveltejs/kit/vite';
import { defineConfig } from 'vite';

export default defineConfig({
  plugins: [sveltekit()],
  server: {
    fs: {
      // Tauri's persistent development webview can briefly request a module
      // from the previous Vite graph after a hot reload. The repository root
      // is trusted development source and includes package metadata.
      allow: ['.']
    }
  }
});
