import React from "react";
import ReactDOM from "react-dom/client";

import "@fontsource/inter/400.css";
import "@fontsource/inter/500.css";
import "@fontsource/inter/600.css";
import "@fontsource/jetbrains-mono/400.css";

import App from "./App";
import { AppProviders } from "./app/providers/AppProviders";
import { applyInitialTheme } from "./lib/theme";
import "./styles/globals.css";

// Apply the persisted (or system) theme synchronously so the page
// never flashes the wrong mode before React mounts.
applyInitialTheme();

ReactDOM.createRoot(document.getElementById("root")!).render(
  <React.StrictMode>
    <AppProviders>
      <App />
    </AppProviders>
  </React.StrictMode>
);
