import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";
import { canonicalJson, validateDocument } from "../generated/typescript/runtime.mjs";

const validOrder = new URL("../../../schemas/v1/examples/valid/order.json", import.meta.url);
const invalidFlags = new URL("../../../schemas/v1/examples/invalid/incompatible-flags.json", import.meta.url);
const unsafeInteger = new URL("../../../schemas/v1/examples/invalid/unsafe-json-integer.json", import.meta.url);

test("valid wire document canonicalizes deterministically", async () => {
  const value = JSON.parse(await readFile(validOrder, "utf8"));
  assert.deepEqual(JSON.parse(canonicalJson(value)), value);
});

test("incompatible order flags fail closed", async () => {
  const value = JSON.parse(await readFile(invalidFlags, "utf8"));
  assert.throws(() => validateDocument(value));
});

test("unsafe JSON integer fails closed", async () => {
  const value = JSON.parse(await readFile(unsafeInteger, "utf8"));
  assert.throws(() => validateDocument(value));
});
