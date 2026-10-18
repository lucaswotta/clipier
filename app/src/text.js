export const escapeHtml = (text) =>
  String(text ?? "").replace(/[&<>"]/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;" })[c]);

/** Escaped text with `spans` between backticks rendered as code. */
export const rich = (text) => escapeHtml(text).replace(/`([^`]+)`/g, "<code>$1</code>");
