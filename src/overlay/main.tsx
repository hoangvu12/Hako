// Entry for the in-game overlay window (see `overlay.html` + the `overlay`
// window in `tauri.conf.json`). Like the updater splash, it's its own tiny
// bundle — it must NOT import the router/query/app shell or the app's
// `styles.css` (which paints an opaque body), so the window stays transparent
// and paints instantly.
import React from "react";
import ReactDOM from "react-dom/client";
import { OverlayApp } from "./overlay-app";

ReactDOM.createRoot(document.getElementById("overlay-root") as HTMLElement).render(
  <React.StrictMode>
    <OverlayApp />
  </React.StrictMode>
);
