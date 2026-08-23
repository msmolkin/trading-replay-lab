import assert from "node:assert/strict";
import test from "node:test";
import { renderToStaticMarkup } from "react-dom/server";
import { Button, DataTable, Dialog } from "./components";

test("button defaults to non-submit and preserves accessible name", () => {
  const html = renderToStaticMarkup(<Button aria-label="Place order">Buy</Button>);
  assert.match(html, /type="button"/);
  assert.match(html, /aria-label="Place order"/);
});

test("dialog wires its visible title to aria-labelledby and has a close control", () => {
  const html = renderToStaticMarkup(
    <Dialog open title="Confirm order" titleId="confirm-title">
      Body
    </Dialog>,
  );
  assert.match(html, /aria-labelledby="confirm-title"/);
  assert.match(html, /id="confirm-title"/);
  assert.match(html, /aria-label="Close"/);
});

test("data table exposes caption and scoped column headers", () => {
  const html = renderToStaticMarkup(
    <DataTable
      caption="Working orders"
      columns={[{ key: "id", header: "Order", render: (row: { id: string }) => row.id }]}
      rows={[{ id: "one" }]}
      rowKey={(row) => row.id}
    />,
  );
  assert.match(html, /<caption>Working orders<\/caption>/);
  assert.match(html, /scope="col"/);
  assert.match(html, /tabindex="0"/i);
});
