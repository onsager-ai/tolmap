import type { Preview } from "@storybook/react-vite";
import "../src/index.css";

const preview: Preview = {
  parameters: { layout: "fullscreen" },
  globalTypes: {
    theme: { toolbar: { icon: "circlehollow", items: ["system", "light", "dark"], dynamicTitle: true } },
    host: { toolbar: { icon: "globe", items: ["oss", "hub"], dynamicTitle: true } },
  },
  initialGlobals: { theme: "light", host: "oss" },
};
export default preview;
