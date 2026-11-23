import React from "react";
import ReactDOM from "react-dom/client";
import App from "./App";
import "./styles.css";

if (/Mac/i.test(navigator.platform || navigator.userAgent)) {
  document.documentElement.dataset.platform = "macos";
}

ReactDOM.createRoot(document.getElementById("root")!).render(
  <React.StrictMode>
    <App />
  </React.StrictMode>
);
