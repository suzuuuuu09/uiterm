import babel from "@rolldown/plugin-babel";
import tailwindcss from "@tailwindcss/vite";
import react, { reactCompilerPreset } from "@vitejs/plugin-react";
import { defineConfig, type PluginOption } from "vite-plus";

export default defineConfig({
  clearScreen: false,
  plugins: [
    react(),
    babel({
      presets: [reactCompilerPreset()],
    }),
    tailwindcss(),
  ] as PluginOption[],
  server: {
    port: 1420,
    strictPort: true,
    watch: {
      ignored: ["**/src-tauri/**"],
    },
  },
  optimizeDeps: {
    exclude: ["@wterm/ghostty", "@wterm/core", "@gespenst/core", "ghostty-web"],
  },
  assetsInclude: ["**/*.wasm"],
});
