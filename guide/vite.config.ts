// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

import mdx from "@mdx-js/rollup";
import react from "@vitejs/plugin-react";
import { viteSingleFile } from "vite-plugin-singlefile";
import { defineConfig } from "vitest/config";

// Relative asset paths, so dist/ works wherever it is served from: `vite preview`, a file
// server, or a published artifact. `--mode single` builds one self-contained HTML file instead,
// dist-single/index.html: every script, style, font, trace and the local source inlined.
export default defineConfig(({ mode }) => {
  const single = mode === "single";
  return {
    base: "./",
    // The local source the guide shows (tools/sources.ts), served and copied as is; inlined in
    // the single file, so not copied there.
    publicDir: single ? false : "src/generated/public",
    plugins: [
      { enforce: "pre", ...mdx() },
      react({ include: /\.(mdx|ts|tsx)$/ }),
      ...(single ? [viteSingleFile({ removeViteModuleLoader: true })] : []),
    ],
    build: {
      target: "es2023",
      chunkSizeWarningLimit: single ? 32768 : 4096,
      outDir: single ? "dist-single" : "dist",
    },
    test: { include: ["tests/**/*.test.ts"] },
  };
});
