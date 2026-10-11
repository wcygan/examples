// Run with node; no frontend tooling or dependencies are required.
const assert = require("node:assert/strict");
const fs = require("node:fs");
const vm = require("node:vm");
const context = vm.createContext({});
vm.runInContext(
  fs.readFileSync(`${__dirname}/../src/ui/state-machines.js`, "utf8"),
  context,
);
const snapshot = (
  coordinator = "Collecting",
  source = "Working",
  destination = "Working",
) => ({
  coordinator,
  nodes: [
    { id: "source-account", state: source },
    { id: "destination-account", state: destination },
  ],
});
const event = (kind, actor = "coordinator") => ({ kind, actor });
const transition = context.machineTransition;
const working = snapshot(),
  prepared = snapshot("Collecting", "Prepared", "Prepared");
const commit = snapshot({ Decided: "Commit" }, "Committed", "Prepared");
assert.equal(
  transition(
    "source-account",
    working,
    prepared,
    event("Prepared", "source-account"),
  ),
  "Working-Prepared",
);
assert.equal(
  transition(
    "destination-account",
    working,
    snapshot("Collecting", "Working", "Aborted"),
    event("Prepared", "destination-account"),
  ),
  "Working-Aborted",
);
assert.equal(
  transition("coordinator", prepared, commit, event("VoteReceived")),
  "Collecting-Commit",
);
assert.equal(
  transition(
    "coordinator",
    working,
    snapshot({ Decided: "Abort" }),
    event("VoteReceived"),
  ),
  "Collecting-Abort",
);
assert.equal(
  transition("source-account", prepared, prepared, event("TimeoutObserved")),
  "Prepared-Prepared",
);
assert.equal(
  transition("coordinator", prepared, prepared, event("TimeoutObserved")),
  "Collecting-Collecting",
);
assert.equal(
  transition("destination-account", commit, commit, event("FaultInjected")),
  null,
);
assert.equal(
  transition(
    "source-account",
    prepared,
    commit,
    event("DecisionApplied", "source-account"),
  ),
  "Prepared-Committed",
);
assert.equal(
  transition("source-account", commit, commit, event("DuplicateDelivery")),
  null,
);
assert.equal(
  transition(
    "source-account",
    commit,
    commit,
    event("DecisionApplied", "source-account"),
  ),
  "Committed-Committed",
);
assert.equal(
  transition("coordinator", commit, commit, event("VoteReceived")),
  "Commit-Commit",
);
assert.equal(
  transition(
    "destination-account",
    commit,
    commit,
    event("DecisionApplied", "source-account"),
  ),
  null,
);
assert.equal(transition("source-account", working, commit, null), null);
const abort = snapshot({ Decided: "Abort" }, "Aborted", "Aborted");
assert.equal(
  transition(
    "source-account",
    prepared,
    abort,
    event("DecisionApplied", "source-account"),
  ),
  "Prepared-Aborted",
);
assert.equal(
  transition(
    "destination-account",
    abort,
    abort,
    event("DecisionApplied", "destination-account"),
  ),
  "Aborted-Aborted",
);
// Every transition arrow terminates on the target outline, including self loops.
for (const coordinator of [true, false]) {
  const model = context.machineModel(coordinator);
  for (const edge of model.edges) {
    const points = edge.path.match(/[ML][\d.]+ [\d.]+/g);
    const [x, y] = points.at(-1).slice(1).split(" ").map(Number);
    const node = model.states.find((node) => node.name === edge.to);
    assert.ok(
      ((x === node.x || x === node.x + node.w) &&
        y >= node.y &&
        y <= node.y + node.h) ||
        ((y === node.y || y === node.y + node.h) &&
          x >= node.x &&
          x <= node.x + node.w),
      edge.key,
    );
  }
}
console.log("State-machine transitions and arrow endpoints passed.");
