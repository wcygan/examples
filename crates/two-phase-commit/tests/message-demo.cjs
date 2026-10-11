const assert = require("node:assert/strict"),
  fs = require("node:fs"),
  vm = require("node:vm");
const context = vm.createContext({ setTimeout, clearTimeout });
vm.runInContext(
  fs.readFileSync(`${__dirname}/../src/ui/message-demo.js`, "utf8"),
  context,
);
const Player = vm.runInContext("DemoPlayback", context),
  leg = context.messageLeg;
const sent = {
  kind: "PrepareSent",
  actor: "coordinator",
  peer: "source-account",
  message: "Prepare",
  message_id: "p",
};
const received = {
  ...sent,
  kind: "Prepared",
  actor: "source-account",
  peer: "coordinator",
};
assert.equal(leg(received).from, "coordinator");
assert.equal(leg(received).to, "source-account");
assert.equal(leg(received).label, "PREPARE");
const vote = {
  kind: "VoteReceived",
  actor: "coordinator",
  peer: "source-account",
  message: { Vote: "No" },
  message_id: "v",
};
assert.equal(leg(vote).from, "source-account");
assert.equal(leg(vote).label, "NO");
const duplicate = {
  kind: "DuplicateDelivery",
  actor: "coordinator",
  peer: "source-account",
  message: { Decision: "Commit" },
  message_id: "d",
};
const applied = {
  ...duplicate,
  kind: "DecisionApplied",
  actor: "source-account",
  peer: "coordinator",
};
assert.equal(leg(applied, [duplicate]).retry, true);
assert.equal(leg(applied).label, "COMMIT");
assert.equal(leg({ ...duplicate, kind: "FaultInjected" }).withheld, true);
assert.equal(leg({ kind: "Finished" }), null);

function harness(log = [sent, received, sent, received]) {
  let time = 0,
    serial = 0,
    flights = [];
  const jobs = new Map();
  const animation = () => {
    let resolve;
    const a = {
      finished: new Promise((r) => (resolve = r)),
      pause() {
        this.paused = true;
      },
      play() {
        this.paused = false;
      },
      cancel() {
        this.cancelled = true;
      },
      finish() {
        resolve();
      },
    };
    flights.push(a);
    return a;
  };
  const player = new Player({
    count: () => log.length,
    eventAt: (i) => log[i],
    active: () => false,
    animate: animation,
    change: () => {},
    now: () => time,
    schedule: (fn, ms) => {
      jobs.set(++serial, { fn, at: time + ms });
      return serial;
    },
    unschedule: (id) => jobs.delete(id),
  });
  const advance = (ms) => {
    const end = time + ms;
    while (true) {
      const due = [...jobs]
        .filter(([, j]) => j.at <= end)
        .sort((a, b) => a[1].at - b[1].at)[0];
      if (!due) break;
      time = due[1].at;
      jobs.delete(due[0]);
      due[1].fn();
    }
    time = end;
  };
  return { player, log, advance, flights, jobs };
}
(async () => {
  const h = harness();
  h.player.play(true);
  assert.equal(h.player.position, 1);
  h.advance(650);
  assert.equal(h.player.position, 1);
  assert.equal(h.player.pending.kind, "Prepared");
  h.player.pause();
  assert.equal(h.flights[0].paused, true);
  h.log.push(sent);
  h.advance(10000);
  h.player.wake();
  assert.equal(h.player.position, 1);
  h.player.play(false);
  h.flights[0].finish();
  await Promise.resolve();
  assert.equal(h.player.position, 2);
  h.advance(100);
  h.player.pause();
  h.player.speed(2);
  h.player.play(false);
  h.advance(274);
  assert.equal(h.player.position, 2);
  h.advance(1);
  assert.equal(h.player.position, 3);
  h.advance(325);
  assert.equal(h.player.pending.kind, "Prepared");
  const stale = h.flights[1];
  h.player.seek(0);
  stale.finish();
  await Promise.resolve();
  assert.equal(h.player.position, 0);
  assert.equal(h.player.pending, null);
  const hidden = harness();
  hidden.player.play();
  hidden.advance(650);
  hidden.player.visibility(true);
  assert.equal(hidden.flights[0].paused, true);
  hidden.advance(10000);
  assert.equal(hidden.player.position, 1);
  hidden.player.visibility(false);
  assert.equal(hidden.flights[0].paused, false);
  hidden.flights[0].finish();
  await Promise.resolve();
  assert.equal(hidden.player.position, 2);
  const reduced = harness();
  reduced.player.play();
  reduced.advance(650);
  reduced.player.pause();
  reduced.player.reduceMotion();
  await Promise.resolve();
  assert.equal(reduced.player.position, 1);
  reduced.player.play();
  await Promise.resolve();
  assert.equal(reduced.player.position, 2);
  const repeated = harness([sent, sent, sent]);
  repeated.player.play(true);
  repeated.advance(200);
  repeated.player.play(true);
  assert.equal(repeated.jobs.size, 1);
  repeated.advance(450);
  assert.equal(repeated.player.position, 2);
  console.log(
    "Message direction, pause/resume, buffered logs, speed, cancellation, visibility, and reduced motion passed.",
  );
})().catch((e) => {
  console.error(e);
  process.exitCode = 1;
});
