import assert from "node:assert/strict";
import test from "node:test";
import { formatFixedPoint, formatSessionTime, formatSignedPpb } from "./format";

test("fixed-point formatting never converts bigint through Number", () => {
  assert.equal(formatFixedPoint(9_007_199_254_740_993n, 2), "90071992547409.93");
  assert.equal(formatFixedPoint(-5n, 3), "-0.005");
  assert.equal(formatFixedPoint(42n, 0), "42");
});

test("rate formatting is deterministic", () => {
  assert.equal(formatSignedPpb(123_456_789n, 3), "0.123");
  assert.equal(formatSignedPpb(-50_000_000n, 2), "-0.05");
});

test("hidden calendar modes never require an absolute timestamp", () => {
  assert.equal(formatSessionTime("RELATIVE", 3_661_000_000_000n), "+01:01:01");
  assert.equal(
    formatSessionTime("HIDDEN_UNTIL_COMPLETE", 3_661_000_000_000n),
    "Sealed time +01:01:01",
  );
  assert.throws(() => formatSessionTime("ABSOLUTE", 0n));
});
