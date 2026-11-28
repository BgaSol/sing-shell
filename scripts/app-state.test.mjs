import { test } from "node:test";
import assert from "node:assert/strict";
import { ConfigurationReader } from "../src/app-state.ts";
const deferred = () => {
  let resolve, reject;
  const promise = new Promise((a, b) => {
    resolve = a;
    reject = b;
  });
  return { promise, resolve, reject };
};
test("a write during an old read forces another read and never renders the old response", async () => {
  const a = deferred(),
    b = deferred(),
    calls = [],
    renders = [];
  const reader = new ConfigurationReader(
    () => {
      calls.push(1);
      return calls.length === 1 ? a.promise : b.promise;
    },
    (v) => renders.push(v),
  );
  const first = reader.refresh(),
    second = reader.refresh();
  assert.equal(first, second);
  a.resolve({ revision: 1 });
  await new Promise((resolve) => setImmediate(resolve));
  assert.equal(calls.length, 2);
  assert.equal(renders.length, 0);
  b.resolve({ revision: 2 });
  await first;
  assert.deepEqual(renders, [{ revision: 2 }]);
});
test("failed reads release the reader and explicit reset accepts lower native revisions", async () => {
  let failure = true;
  const reader = new ConfigurationReader(
    async () => {
      if (failure) throw Error("read failed");
      return { revision: 0 };
    },
    () => {},
  );
  await assert.rejects(reader.refresh(), /read failed/);
  failure = false;
  reader.value = { revision: 10 };
  await reader.refresh();
  assert.equal(reader.value.revision, 0);
});
test("session reads cannot replace configuration with another domain revision", async () => {
  const settings = {
    nodesRevision: 1,
    routingRevision: 2,
    coreRevision: 3,
    preferencesRevision: 4,
    connectionRevision: 5,
    startup: {},
  };
  const reader = new ConfigurationReader(
    async () => ({ settings }),
    () => {},
  );
  await reader.refresh();
  assert.equal(
    reader.updateSession({ revisions: [1, 3, 3, 4, 5], startup: {} }),
    false,
  );
  assert.equal(
    reader.updateSession({
      revisions: [1, 2, 3, 4, 5],
      startup: { restoreCoreOnLaunch: true },
    }),
    true,
  );
  assert.equal(reader.value.settings.startup.restoreCoreOnLaunch, true);
});
