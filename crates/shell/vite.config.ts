import { defineConfig } from "vite";
import vue from "@vitejs/plugin-vue";
import tailwindcss from "@tailwindcss/vite";

// Tauri v2 expects the dev server on a fixed port and the build to emit to dist/.
// The host shell (src-tauri) is ignored by the watcher so Rust changes don't reload the UI.
export default defineConfig({
  plugins: [vue(), tailwindcss()],
  clearScreen: false,
  server: {
    port: 1420,
    strictPort: true,
    watch: {
      ignored: ["**/src-tauri/**"],
    },
  },
  build: {
    target: "es2022",
    outDir: "dist",
  },
});
