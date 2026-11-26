/// <reference types="vite-plus/client" />

declare module "*.wasm?url" {
  const src: string;
  export default src;
}
