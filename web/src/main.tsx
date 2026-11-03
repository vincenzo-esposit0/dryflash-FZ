// SPDX-License-Identifier: GPL-3.0-or-later
import { render } from "preact";
import { App } from "./app";
import { getLocale, whenLocaleChanges } from "./i18n";
import { AboutPage, CompatPage } from "./pages";
// Served from here: the CSP allows no other host, and a visitor's request reaches no font service (ADR-0018).
import "@fontsource/jetbrains-mono/latin-400.css";
import "@fontsource/jetbrains-mono/latin-600.css";
import "@fontsource/share-tech-mono/latin-400.css";
import "@fontsource/ibm-plex-sans/latin-400.css";
import "./style.css";

document.documentElement.lang = getLocale();

function route(path: string) {
  switch (path.replace(/\/+$/, "")) {
    case "/embed":
      return <App embed />;
    case "/compat":
      return <CompatPage />;
    case "/about":
      return <AboutPage />;
    default:
      return <App />;
  }
}

// Drawn again rather than reloaded: the same components stay, so a running firmware does not restart.
const draw = () => render(route(location.pathname), document.getElementById("app")!);
whenLocaleChanges(draw);
draw();
