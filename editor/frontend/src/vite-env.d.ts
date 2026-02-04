/// <reference types="vite/client" />

interface ImportMetaEnv {
  readonly VITE_EDITOR_API?: string;
}

interface ImportMeta {
  readonly env: ImportMetaEnv;
}

