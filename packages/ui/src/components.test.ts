import assert from "node:assert/strict";
import test from "node:test";
import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { Button, DataTable, Dialog } from "./components.ts";

test("button defaults to non-submit and preserves accessible name", () => {
  const html = renderToStaticMarkup(
    createElement(Button, { "aria-label": "Place order" }, "Buy"),
  );
  assert.match(html, /type="button"/);
  assert.match(html, /aria-label="Place order"/);
});

test("dialog wires its visible title to aria-labelledby and has a close control", () => {
  const html = renderToStaticMarkup(
    createElement(Dialog, { open: true, title: "Confirm order", titleId: "confirm-title" }, "Body"),
  );
  assert.match(html, /aria-labelledby="confirm-title"/);
  assert.match(html, /id="confirm-title"/);
  assert.match(html, /aria-label="Close"/);
});

test("data table exposes caption and scoped column headers", () => {
  const rows = [{ id: "one" }];
  const html = renderToStaticMarkup(
    createElement(DataTable<{ id: string }>, {
      caption: "Working orders",
      columns: [{ key: "id", header: "Order", render: (row) => row.id }],
      rows,
      rowKey: (row) => row.id,
    }),
  );
  assert.match(html, /<caption>Working orders<\/caption>/);
  assert.match(html, /scope="col"/);
  assert.match(html, /tabindex="0"/i);
});
