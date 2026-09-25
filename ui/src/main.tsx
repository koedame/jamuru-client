// Must be first: logging is in place before any other module runs.
import "./lib/installLogging";

import React from "react";
import ReactDOM from "react-dom/client";
import App from "./App";
import Catalog from "./Catalog";
import { setBackend } from "./lib/backend";
import { HELP_HASH, helperBackend } from "./lib/helperBackend";

// Import i18n (must be imported before App)
import "./i18n";

// Import brand fonts (must be imported before tokens.css uses them)
import "@fontsource/inter/400.css";
import "@fontsource/inter/600.css";
import "@fontsource/inter/700.css";
import "@fontsource/roboto-mono/400.css";
import "@fontsource/roboto-mono/700.css";

// Import design tokens
import "./styles/tokens.css";

// The window someone helping works in draws the helped app's screen from the
// helped app's state, so it is connected to that app's backend before anything mounts.
if (window.location.hash === HELP_HASH) setBackend(helperBackend);

// Check if catalog mode is enabled via environment variable
const isCatalogMode = import.meta.env.VITE_CATALOG_MODE === "true";

ReactDOM.createRoot(document.getElementById("root")!).render(
  <React.StrictMode>
    {isCatalogMode ? <Catalog /> : <App />}
  </React.StrictMode>
);
