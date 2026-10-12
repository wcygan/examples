// Exercise presentation against real protocol traces from an ephemeral console.
const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");
const vm = require("node:vm");
const { spawn } = require("node:child_process");
const context = vm.createContext({ setTimeout, clearTimeout });
for (const file of ["state-machines.js", "message-demo.js"]) {
  vm.runInContext(
    fs.readFileSync(`${__dirname}/../src/ui/${file}`, "utf8"),
    context,
  );
}
const {
  deliveryAttempts,
  snapshotChanges,
  observationLabel,
  observationTone,
  messageLeg,
  sceneGeometry,
  initialSnapshot,
} = context;
const binary = path.resolve(
  __dirname,
  "../../../target/debug/two-phase-commit",
);
const server = spawn(binary, ["--port", "0"], {
  stdio: ["ignore", "pipe", "ignore"],
});
async function checkScenario(base, scenario, count) {
  const start = await fetch(`${base}/api/runs`, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ scenario, participant_count: count }),
  });
  assert.equal(start.status, 201);
  const { id } = await start.json();
  let run;
  for (let i = 0; i < 100; i++) {
    run = await (await fetch(`${base}/api/runs/${id}`)).json();
    if (run.status !== "running") break;
    await new Promise((resolve) => setTimeout(resolve, 10));
  }
  assert.equal(run.status, "finished");
  const attempts = deliveryAttempts(run.events);
  const sends = run.events.filter((e) =>
    ["PrepareSent", "VoteSent", "DecisionSent", "DuplicateDelivery"].includes(
      e.kind,
    ),
  );
  assert.equal(
    attempts.length,
    sends.length,
    "Only actual sends create attempts",
  );
  assert.ok(
    attempts.every((a) => sends.some((e) => e.sequence === a.sequence)),
  );
  const voteAttempts = attempts.filter((a) => ["YES", "NO"].includes(a.label));
  assert.equal(voteAttempts.length, count);
  for (const actor of run.events[0].snapshot.nodes.map((n) => n.id)) {
    const votes = voteAttempts.filter((a) => a.from === actor);
    assert.equal(votes.length, 1, `${scenario}: ${actor} sends one vote`);
    const sent = run.events[votes[0].sent];
    assert.equal(
      votes[0].label,
      sent.snapshot.nodes.find((n) => n.id === actor).vote.toUpperCase(),
    );
  }
  if (scenario !== "no-vote")
    assert.ok(voteAttempts.every((a) => a.label === "YES"));
  const receipts = run.events.filter((e) =>
    ["Prepared", "VoteReceived", "DecisionApplied"].includes(e.kind),
  );
  assert.equal(
    attempts.filter((a) => a.receipt !== null).length,
    receipts.length,
  );
  for (const a of attempts) {
    const sent = run.events[a.sent];
    assert.equal(sent.sequence, a.sequence);
    if (a.withheld) {
      assert.equal(a.receipt, null);
      continue;
    }
    if (a.receipt !== null) {
      const receipt = run.events[a.receipt];
      assert.equal(receipt.message_id, sent.message_id);
      assert.ok(receipt.sequence > sent.sequence);
      assert.equal(receipt.actor, a.to);
      assert.equal(receipt.peer, a.from);
      // Revealing the send alone must never expose its future receipt.
      const prefix = deliveryAttempts(run.events.slice(0, a.sent + 1));
      assert.equal(prefix.find((p) => p.sequence === a.sequence).receipt, null);
    }
  }
  const final = run.events.at(-1).snapshot;
  assert.equal(observationLabel(run.events.at(-1)), "Recorded");
  if (["commit", "partial-delivery", "recovery"].includes(scenario)) {
    assert.deepEqual(
      final.nodes.map((n) => n.balance),
      Array.from({ length: count }, (_, i) =>
        i === 0 ? 100 - 10 * (count - 1) : 30,
      ),
    );
    const decided = run.events.find((e) => e.kind === "DecisionMade");
    assert.ok(decided.snapshot.nodes.some((n) => n.state === "Prepared"));
  } else
    assert.deepEqual(
      final.nodes.map((n) => n.balance),
      Array.from({ length: count }, (_, i) => (i === 0 ? 100 : 20)),
    );
  if (scenario === "no-vote") {
    assert.equal(attempts.filter((a) => a.label === "NO").length, 1);
    assert.equal(
      attempts.find((a) => a.label === "NO").from,
      `participant-${count}`,
    );
    assert.equal(attempts.filter((a) => a.label === "ABORT").length, count);
    assert.ok(!attempts.some((a) => a.label === "COMMIT"));
    const rejection = run.events.find(
      (e) => e.kind === "Prepared" && e.actor === `participant-${count}`,
    );
    assert.equal(observationLabel(rejection), "PREPARE rejected");
    assert.ok(final.nodes.every((n) => n.state === "Aborted"));
  }
  if (["missing-vote", "coordinator-loss"].includes(scenario)) {
    assert.ok(final.nodes.every((n) => n.blocked && n.state === "Prepared"));
    assert.ok(run.events.some((e) => e.kind === "TimeoutObserved"));
    assert.equal(final.coordinator_available, scenario !== "coordinator-loss");
    const withheld = attempts.filter((a) => a.withheld);
    assert.equal(withheld.length, scenario === "missing-vote" ? 1 : count - 1);
    assert.equal(withheld[0].label, "YES");
    assert.equal(withheld[0].to, "coordinator");
    assert.equal(withheld[0].receipt, null);
    assert.equal(
      deliveryAttempts(run.events.slice(0, withheld[0].sent + 1)).find(
        (a) => a.sequence === withheld[0].sequence,
      ).withheld,
      false,
    );
    const fault = run.events.find((e) => e.kind === "FaultInjected");
    assert.equal(
      observationLabel(fault),
      scenario === "missing-vote" ? "YES withheld" : "Simulated loss",
    );
    assert.equal(
      observationLabel(run.events.find((e) => e.kind === "TimeoutObserved")),
      "Simulated timeout",
    );
    assert.ok(!attempts.some((a) => ["COMMIT", "ABORT"].includes(a.label)));
  }
  if (scenario === "partial-delivery") {
    const fault = run.events.find((e) => e.kind === "FaultInjected");
    assert.equal(messageLeg(fault).withheld, true);
    assert.ok(!attempts.some((a) => a.sequence === fault.sequence));
    const destination = attempts.filter(
      (a) => a.label === "COMMIT" && a.to === fault.peer,
    );
    assert.equal(destination.length, 1);
    assert.equal(destination[0].retry, true);
    assert.ok(destination[0].receipt !== null);
    const retryIndex = destination[0].sent;
    assert.equal(
      observationLabel(run.events[retryIndex], run.events.slice(0, retryIndex)),
      "COMMIT retry",
    );
    assert.equal(
      messageLeg(
        run.events[destination[0].receipt],
        run.events.slice(0, destination[0].receipt),
      ).retry,
      true,
    );
    const snap = fault.snapshot;
    assert.equal(snap.nodes[count - 1].state, "Prepared");
    assert.equal(snap.nodes[count - 1].balance, 20);
  }
  assert.equal(
    attempts.filter((a) => a.retry).length,
    ["partial-delivery", "recovery"].includes(scenario) ? count : 0,
  );
  assert.equal(
    run.events.filter((e) => e.kind === "DuplicateDelivery").length,
    scenario === "recovery"
      ? count
      : scenario === "partial-delivery"
        ? count - 1
        : 0,
  );
  for (let i = 1; i < run.events.length; i++) {
    const before = run.events[i - 1].snapshot;
    const event = run.events[i];
    // Balance mutations must be a real COMMIT receipt on that account, once only.
    for (const node of event.snapshot.nodes) {
      const prior = before.nodes.find((n) => n.id === node.id);
      if (node.balance !== prior.balance) {
        assert.equal(event.kind, "DecisionApplied");
        assert.equal(event.actor, node.id);
        assert.equal(event.message.Decision, "Commit");
        assert.equal(prior.state, "Prepared");
        assert.equal(node.state, "Committed");
      }
    }
    if (event.kind === "FaultInjected") {
      assert.equal(event.snapshot.votes_received, before.votes_received);
      assert.deepEqual(event.snapshot.coordinator, before.coordinator);
    }
  }
  for (const a of attempts.filter(
    (a) => run.events[a.sent].kind === "DuplicateDelivery",
  )) {
    assert.ok(a.receipt !== null);
    assert.equal(
      snapshotChanges(
        run.events[a.receipt - 1].snapshot,
        run.events[a.receipt].snapshot,
      ).length,
      0,
    );
  }
  const prepared = run.events.find((e) => e.kind === "Prepared");
  assert.equal(
    observationTone(prepared),
    prepared.snapshot.nodes.find((n) => n.id === prepared.actor).state ===
      "Prepared"
      ? "amber"
      : "red",
  );
  const failure = { ...run.events.at(-1), kind: "Failed" };
  assert.equal(observationLabel(failure), "Run failed");
  assert.equal(observationTone(failure), "red");
  assert.equal(snapshotChanges(final, structuredClone(final)).length, 0);
}
(async () => {
  for (const count of [2, 3, 4, 5]) {
    const snapshot = initialSnapshot(count);
    assert.equal(snapshot.nodes.length, count);
    for (const compact of [false, true]) {
      const geometry = sceneGeometry(
        snapshot.nodes.map((n) => n.id),
        compact,
      );
      assert.equal(geometry.boxes.length, count + 1);
      for (const [actor, x, y, width] of geometry.boxes) {
        assert.ok(x >= 0 && x + width <= geometry.width);
        assert.ok(y >= 0 && y + geometry.cardHeight <= geometry.height);
        for (const [other, ox, oy, ow] of geometry.boxes) {
          if (actor === other) continue;
          assert.ok(
            x + width <= ox ||
              ox + ow <= x ||
              y + geometry.cardHeight <= oy ||
              oy + geometry.cardHeight <= y,
            "Nodes overlap",
          );
        }
      }
      for (const points of geometry.routes.values()) {
        for (let i = 1; i < points.length; i++) {
          const [ax, ay] = points[i - 1],
            [bx, by] = points[i];
          assert.ok(ax === bx || ay === by);
          for (const [, x, y, w] of geometry.boxes) {
            const horizontal =
              ay === by &&
              ay > y &&
              ay < y + geometry.cardHeight &&
              Math.max(ax, bx) > x &&
              Math.min(ax, bx) < x + w;
            const vertical =
              ax === bx &&
              ax > x &&
              ax < x + w &&
              Math.max(ay, by) > y &&
              Math.min(ay, by) < y + geometry.cardHeight;
            assert.ok(
              !horizontal && !vertical,
              "A message route crosses a node",
            );
          }
        }
      }
    }
  }
  const base = await new Promise((resolve, reject) => {
    const timeout = setTimeout(
      () =>
        reject(new Error("Console startup timed out; build its binary first")),
      10000,
    );
    server.on("error", (error) => {
      clearTimeout(timeout);
      reject(error);
    });
    server.stdout.on("data", (chunk) => {
      const match = chunk
        .toString()
        .match(/Console: (http:\/\/127\.0\.0\.1:\d+)/);
      if (match) {
        clearTimeout(timeout);
        resolve(match[1]);
      }
    });
  });
  for (const scenario of [
    "commit",
    "no-vote",
    "missing-vote",
    "coordinator-loss",
    "partial-delivery",
    "recovery",
  ])
    for (const count of [2, 3, 4, 5])
      await checkScenario(base, scenario, count);
  // Reused IDs still pair each receipt with a distinct pending attempt.
  const send = {
    kind: "DecisionSent",
    actor: "coordinator",
    peer: "source-account",
    message_id: "same",
    message: { Decision: "Commit" },
    sequence: 1,
  };
  const receipt = {
    ...send,
    kind: "DecisionApplied",
    actor: send.peer,
    peer: send.actor,
    sequence: 3,
  };
  const paired = deliveryAttempts([
    send,
    { ...send, sequence: 2, kind: "DuplicateDelivery" },
    receipt,
    { ...receipt, sequence: 4 },
  ]);
  assert.equal(paired[0].receiptSequence, 3);
  assert.equal(paired[1].receiptSequence, 4);
  assert.equal(
    messageLeg(receipt, [
      send,
      { ...send, sequence: 2, kind: "DuplicateDelivery" },
    ]).retry,
    false,
  );
  assert.equal(
    messageLeg({ ...receipt, sequence: 4 }, [
      send,
      { ...send, sequence: 2, kind: "DuplicateDelivery" },
      receipt,
    ]).retry,
    true,
  );
  // A local observation with a payload must never masquerade as a protocol send.
  assert.equal(messageLeg({ ...send, kind: "DecisionMade" }), null);
  assert.equal(messageLeg({ ...send, kind: "VoteSent" }), null);
  assert.equal(
    messageLeg({ ...send, actor: "source-account", peer: "coordinator" }),
    null,
  );
  const blocked = { ...send, kind: "FaultInjected", sequence: 2 };
  assert.equal(deliveryAttempts([blocked]).length, 0);
  const later = deliveryAttempts([
    blocked,
    { ...send, sequence: 3 },
    { ...receipt, sequence: 4 },
  ]);
  assert.equal(later.length, 1);
  assert.equal(later[0].sequence, 3);
  assert.equal(later[0].receiptSequence, 4);
  assert.equal(later[0].retry, true);
  const voteSend = {
    ...send,
    kind: "VoteSent",
    actor: "source-account",
    peer: "coordinator",
    message: { Vote: "Yes" },
  };
  const voteFault = {
    ...voteSend,
    kind: "FaultInjected",
    actor: "coordinator",
    peer: "source-account",
    sequence: 2,
  };
  const voteReceipt = {
    ...voteFault,
    kind: "VoteReceived",
    sequence: 4,
  };
  const intercepted = deliveryAttempts([
    voteSend,
    voteFault,
    { ...voteSend, sequence: 3 },
    voteReceipt,
  ]);
  assert.equal(intercepted.length, 2);
  assert.equal(intercepted[0].withheld, true);
  assert.equal(intercepted[0].receipt, null);
  assert.equal(intercepted[1].receiptSequence, 4);
  console.log(
    "All 24 real traces (six scenarios × 2–5 participants): ordered attempts, withheld delivery, retries, exact snapshots, and no look-ahead passed.",
  );
})()
  .catch((error) => {
    console.error(error);
    process.exitCode = 1;
  })
  .finally(() => server.kill());
