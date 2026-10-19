import type { StorybookConfig } from "@storybook/react-vite";
import { fileURLToPath } from "node:url";

const config: StorybookConfig = {
  stories: ["../storybook/**/*.stories.tsx"],
  staticDirs: ["../storybook-public"],
  framework: "@storybook/react-vite",
  core: { disableTelemetry: true },
  async viteFinal(config) {
    // Storybook never proxies API calls to an operator's local service.
    config.server = { ...config.server, proxy: {} };
    config.resolve ??= {};
    config.resolve.alias = [
      { find: "@/host", replacement: fileURLToPath(new URL("../storybook/host.ts", import.meta.url)) },
      { find: "@", replacement: fileURLToPath(new URL("../src", import.meta.url)) },
      { find: "@bindings", replacement: fileURLToPath(new URL("../../bindings", import.meta.url)) },
    ];
    return config;
  },
};
export default config;
