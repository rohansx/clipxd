import React from "react";
import ReactDOM from "react-dom/client";
import { HelmetProvider } from "react-helmet-async";
import App from "./App";
import "./styles.css";

// Mark root as "booted" so the static prerendered hero shell is hidden
// once React is about to render. The data-theme attribute is set on
// <html> directly in index.html so the inlined critical CSS resolves
// the theme variables on first paint — without waiting for this script.
const root = document.getElementById("root")!;
root.classList.add("lx-booted");

ReactDOM.createRoot(root).render(
  <React.StrictMode>
    <HelmetProvider>
      <App />
    </HelmetProvider>
  </React.StrictMode>,
);