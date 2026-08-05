import assert from "node:assert/strict";
import test from "node:test";

import { defaultResponseError, parseStructuredErrorBody } from "./http.ts";

test("parses V2 error envelopes without dropping the code", () => {
  assert.deepEqual(
    parseStructuredErrorBody(
      JSON.stringify({ error: { code: "invalid_input", message: "Invalid" } })
    ),
    { code: "invalid_input", message: "Invalid" }
  );
});

test("parses detail envelopes and leaves plain text unstructured", () => {
  assert.deepEqual(parseStructuredErrorBody('{"detail":"Missing"}'), {
    code: undefined,
    message: "Missing",
  });
  assert.equal(parseStructuredErrorBody("plain text"), null);
});

test("default HTTP errors retain status, code, and raw body", async () => {
  const body = JSON.stringify({
    error: { code: "not_found", message: "Not found" },
  });
  const error = await defaultResponseError(
    new Response(body, { status: 404, statusText: "Not Found" })
  );

  assert.equal(error.status, 404);
  assert.equal(error.code, "not_found");
  assert.equal(error.message, "Not found");
  assert.equal(error.body, body);
});
