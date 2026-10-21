const assert = require("node:assert/strict"),
  fs = require("node:fs"),
  vm = require("node:vm");
const context = vm.createContext({ setTimeout, clearTimeout });
for (const file of ["state-machines.js", "message-demo.js"])
  vm.runInContext(
    fs.readFileSync(`${__dirname}/../src/ui/${file}`, "utf8"),
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
const initial = context.initialSnapshot();
initial.nodes[0].id = "source-account";
initial.nodes[1].id = "destination-account";
sent.snapshot = initial;
received.snapshot = structuredClone(initial);
received.snapshot.nodes[0].state = "Prepared";
received.snapshot.nodes[0].vote = "Yes";
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

function harness(log = [sent, received, sent, received], teaching = false) {
  let time = 0,
    serial = 0,
    flights = [];
  const jobs = new Map();
  const animation = (event, duration) => {
    let resolve;
    const a = {
      duration,
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
    stepAt: (i) => context.teachingStep(log[i], log.slice(0, i)),
    now: () => time,
    schedule: (fn, ms) => {
      jobs.set(++serial, { fn, at: time + ms });
      return serial;
    },
    unschedule: (id) => jobs.delete(id),
  });
  if (teaching) {
    log.forEach((event) => (event.snapshot ??= initial));
    player.mode = "teaching";
  }
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
  assert.equal(h.flights[0].duration, 900);
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
  const started = { kind: "Started", actor: "coordinator" },
    finished = { kind: "Finished", actor: "coordinator" };
  const teaching = harness(
    [started, { ...sent, sequence: 2 }, { ...received, sequence: 3 }, finished],
    true,
  );
  teaching.player.play();
  assert.equal(teaching.player.position, 1);
  teaching.advance(context.teachingStep(started).hold);
  assert.equal(
    teaching.player.position,
    2,
    "Quiet send revealed without an extra pause",
  );
  assert.equal(teaching.player.pending.kind, "Prepared");
  assert.equal(teaching.flights[0].duration, 1800);
  teaching.player.pause();
  teaching.advance(10000);
  assert.equal(
    teaching.player.position,
    2,
    "Receipt snapshot waits for arrival",
  );
  teaching.player.speed(0.5);
  assert.equal(teaching.flights[0].playbackRate, 0.5);
  teaching.player.play();
  teaching.flights[0].finish();
  await Promise.resolve();
  assert.equal(teaching.player.position, 3);
  teaching.advance(teaching.player.remaining / teaching.player.rate);
  assert.equal(teaching.player.position, 4);
  teaching.advance(200);
  teaching.player.pause();
  teaching.player.play();
  assert.equal(
    teaching.player.position,
    4,
    "Resuming the final reading pause must not rewind",
  );
  teaching.advance(teaching.player.remaining / teaching.player.rate);
  assert.equal(teaching.player.playing, false);
  teaching.player.seek(2);
  assert.equal(
    teaching.player.position,
    2,
    "Manual seek includes quiet observations",
  );
  const continuous = harness([started, sent, received, finished], true);
  continuous.player.play();
  const hold = continuous.player.remaining;
  continuous.advance(100);
  continuous.player.speed(0.73);
  assert.equal(continuous.player.remaining, hold - 100);
  continuous.advance(100);
  continuous.player.speed(1.37);
  assert.equal(continuous.player.remaining, hold - 173);
  assert.equal(
    continuous.jobs.size,
    1,
    "Speed adjustments retain one hold timer",
  );
  continuous.advance(continuous.player.remaining / 1.37);
  const flight = continuous.player.animation;
  continuous.player.speed(0.83);
  assert.equal(continuous.player.animation, flight);
  assert.equal(flight.playbackRate, 0.83);
  continuous.player.pause();
  continuous.player.speed(1.23);
  assert.equal(continuous.player.playing, false);
  assert.equal(flight.paused, true);
  continuous.player.play();
  flight.finish();
  await Promise.resolve();
  const reading = continuous.player.remaining;
  continuous.advance(100);
  continuous.player.speed(0.41);
  assert.equal(continuous.player.remaining, reading - 123);
  assert.equal(continuous.jobs.size, 1);
  assert.equal(context.readingPause("Short explanation."), 3200);
  assert.equal(context.readingPause("word ".repeat(100)), 7000);
  assert(
    context.readingPause("word ".repeat(25)) >
      context.readingPause("word ".repeat(15)),
  );
  const streaming = harness([sent], true);
  streaming.player.active = () => true;
  streaming.player.play(true);
  assert.equal(streaming.player.position, 1);
  assert.equal(streaming.jobs.size, 0);
  streaming.log.push(received);
  streaming.player.wake();
  assert.equal(streaming.player.pending.kind, "Prepared");
  streaming.player.seek(0);
  streaming.flights[0].finish();
  await Promise.resolve();
  assert.equal(
    streaming.player.position,
    0,
    "Seeking cancels teaching flights too",
  );
  console.log(
    "Message direction, pause/resume, buffered logs, speed, cancellation, visibility, and reduced motion passed.",
  );
})().catch((e) => {
  console.error(e);
  process.exitCode = 1;
});
