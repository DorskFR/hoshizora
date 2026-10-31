import assert from "node:assert/strict";
import test from "node:test";

import { escapeRegExp } from "../src/regexp.ts";

test("API URL regexp only matches the configured origin", () => {
  const re = new RegExp(`^${escapeRegExp("http://localhost:8081")}`);
  assert.ok(re.test("http://localhost:8081/stars"));
  assert.ok(!re.test("http://localhostX8081/stars"));
  assert.ok(!re.test("http://evil.example/http://localhost:8081"));
});
