// SPDX-License-Identifier: GPL-3.0-or-later
import { render } from "preact";
import { App } from "./app";
import { AboutPage, CompatPage } from "./pages";
import "./style.css";

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

render(route(location.pathname), document.getElementById("app")!);
