export default {
  clearScreen: false,
  server: {
    port: 1420,
    strictPort: true,
    // The Rust build output lives under src-tauri/target; reloading the page for it is noise.
    watch: { ignored: ["**/src-tauri/**"] },
  },
};
