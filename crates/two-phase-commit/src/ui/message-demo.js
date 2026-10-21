// Playback time is independent of audit-log time. Receipts advance state on arrival.
function messageLeg(event, prefix = []) {
  if (!event?.message_id || !event.message || !event.peer) return null;
  const type =
    event.message === "Prepare"
      ? "Prepare"
      : ["Yes", "No"].includes(event.message.Vote)
        ? "Vote"
        : ["Commit", "Abort"].includes(event.message.Decision)
          ? "Decision"
          : null;
  const received = ["Prepared", "VoteReceived", "DecisionApplied"].includes(
    event.kind,
  );
  const sent = [
    "PrepareSent",
    "VoteSent",
    "DecisionSent",
    "DuplicateDelivery",
  ].includes(event.kind);
  const withheld = event.kind === "FaultInjected";
  if (!received && !sent && !withheld) return null;
  const expected = {
    PrepareSent: "Prepare",
    Prepared: "Prepare",
    VoteSent: "Vote",
    VoteReceived: "Vote",
    DecisionSent: "Decision",
    DecisionApplied: "Decision",
    DuplicateDelivery: "Decision",
  }[event.kind];
  if (!type || (!withheld && expected !== type)) return null;
  // Vote withholding is observed by the coordinator, with the sender as its peer.
  const reverse = received || (withheld && type === "Vote");
  const leg = {
    from: reverse ? event.peer : event.actor,
    to: reverse ? event.actor : event.peer,
    label: type === "Prepare" ? "PREPARE" : event.message[type].toUpperCase(),
    received,
    withheld,
    retry: event.kind === "DuplicateDelivery",
  };
  if (
    !validActor(leg.from, event.snapshot) ||
    !validActor(leg.to, event.snapshot) ||
    (type === "Vote"
      ? leg.to !== "coordinator" || leg.from === "coordinator"
      : leg.from !== "coordinator" || leg.to === "coordinator")
  )
    return null;
  if (prefix.length && !withheld) {
    // Use the same FIFO correlation as the waterfall, including overlapping retries.
    const attempt = deliveryAttempts([...prefix, event]).find((a) =>
      received ? a.receipt === prefix.length : a.sent === prefix.length,
    );
    if (received && !attempt) return null;
    leg.retry = attempt?.retry || false;
  }
  return leg;
}

// Teaching time includes room to follow a payload and read its recorded effect.
const TEACHING_FLIGHT_MS = 1800;
function readingPause(text) {
  return Math.min(
    7000,
    Math.max(3200, text.trim().split(/\s+/).length * 200 + 700),
  );
}
// Automatic teaching steps are derived from the full, unmodified audit prefix.
function teachingStep(event, prefix = []) {
  const leg = messageLeg(event, prefix);
  const local = {
    Started: "Start",
    DecisionMade: "Decide",
    FaultInjected: "Fault",
    TimeoutObserved: "Wait",
    Finished: "Complete",
    Failed: "Error",
  }[event.kind];
  if (!local && !leg?.received) return null;
  const timing = {
    hold: readingPause(observationExplanation(event, prefix)),
    flightMs: TEACHING_FLIGHT_MS,
  };
  if (local) return { phase: local, ...timing, animate: false };
  if (event.kind === "DecisionApplied" && leg.retry) {
    const before = prefix.at(-1)?.snapshot;
    const changed =
      before && snapshotChanges(before, event.snapshot).length > 0;
    const demonstrated = prefix.some(
      (prior, index) =>
        prior.kind === "DecisionApplied" &&
        messageLeg(prior, prefix.slice(0, index))?.retry &&
        !snapshotChanges(
          prefix[index - 1]?.snapshot || prior.snapshot,
          prior.snapshot,
        ).length,
    );
    if (!changed && demonstrated) return null;
    return {
      phase: changed ? "Retransmit" : "Idempotent retry",
      ...timing,
      animate: true,
    };
  }
  return {
    phase: {
      Prepared: "Prepare",
      VoteReceived: "Vote",
      DecisionApplied: "Deliver",
    }[event.kind],
    ...timing,
    animate: true,
  };
}
// Describe recorded effects separately from the information in transit.
function messagePayload(leg) {
  return {
    title: `${leg.label}${leg.retry ? " · retry" : ""}`,
    detail: {
      PREPARE: "Prepare update",
      YES: "Ready to commit",
      NO: "Cannot prepare",
      COMMIT: "Apply update",
      ABORT: "Discard update",
    }[leg.label],
  };
}
function observationExplanation(event, prefix = [], inFlight = false) {
  if (!event)
    return "Choose a scenario, then run it to follow the transaction.";
  const snapshot = event.snapshot,
    leg = messageLeg(event, prefix);
  const node = snapshot.nodes.find((n) => n.id === event.actor);
  const name = actorName(event.actor),
    count = snapshot.nodes.length;
  const decision = machineState(snapshot, "coordinator").toUpperCase();
  if (inFlight && leg) {
    const purpose = {
      PREPARE: "Ask whether the local update can be prepared",
      YES: "Report readiness to commit",
      NO: "Report that preparation failed",
      COMMIT: "Deliver the fixed decision to apply the update",
      ABORT: "Deliver the fixed decision to discard the update",
    }[leg.label];
    return `${leg.label}${leg.retry ? " retry" : ""}: ${actorName(leg.from)} → ${actorName(leg.to)}. ${purpose}; receiving state changes wait for arrival.`;
  }
  switch (event.kind) {
    case "Started":
      return `${count} participants begin a transaction. COMMIT requires YES from every participant; each applies its own local update.`;
    case "Prepared":
      return node.state === "Prepared"
        ? `${name} prepared its update and chose YES. It must now wait for the coordinator's decision; its balance is still ${node.balance}.`
        : `${name} could not prepare and chose NO. Its balance stays ${node.balance}; a single NO rules out COMMIT.`;
    case "VoteReceived":
      return leg.label === "NO"
        ? `The coordinator received NO from ${actorName(event.peer)}. One rejection fixes ABORT, even if other participants voted YES.`
        : `The coordinator received YES from ${actorName(event.peer)} (${snapshot.votes_received}/${count} votes). COMMIT requires YES from every distinct participant.`;
    case "DecisionMade":
      return decision === "COMMIT"
        ? `The coordinator fixed COMMIT after all ${count} YES votes. Participants still need the decision before applying their updates.`
        : `The coordinator fixed ABORT. Delivering it releases prepared participants without changing their balances.`;
    case "DecisionApplied": {
      const previous = prefix
        .at(-1)
        ?.snapshot.nodes.find((n) => n.id === event.actor);
      if (
        leg.retry &&
        previous?.balance === node.balance &&
        previous?.state === node.state
      )
        return `${name} received ${leg.label} again; its balance stays ${node.balance}. Duplicate delivery is safe because the update is applied once.`;
      if (leg.label === "COMMIT")
        return `${name} ${leg.retry ? "finally " : ""}received COMMIT; balance ${previous?.balance ?? node.balance} → ${node.balance}. The prepared update is applied only when the decision arrives.`;
      return `${name} received ABORT; its balance stays ${node.balance}. It can stop waiting and discard the pending update.`;
    }
    case "FaultInjected": {
      if (
        !snapshot.coordinator_available &&
        prefix.at(-1)?.snapshot.coordinator_available !== false
      )
        return "The coordinator became unavailable (simulated). Prepared participants cannot choose a decision on their own, so they remain blocked.";
      if (leg?.label === "COMMIT" || leg?.label === "ABORT")
        return `${leg.label} was withheld from ${actorName(leg.to)}. A coordinator decision does not guarantee delivery; this participant still needs it.`;
      return `${leg?.label || "A message"} from ${actorName(event.peer)} was withheld. The missing vote is not a NO, and does not let prepared participants decide.`;
    }
    case "TimeoutObserved":
      return "A simulated wait expired; prepared participants remain blocked. A timeout does not reveal whether the coordinator decided COMMIT or ABORT.";
    case "Finished": {
      const blocked = snapshot.nodes.filter((n) => n.blocked).length;
      return blocked
        ? `Recording ended with ${blocked} participants still Prepared and blocked. Finishing the demo does not resolve the transaction.`
        : `Recording ended with ${decision} delivered to all ${count} participants. ${decision === "COMMIT" ? "Each balance reflects one application, even when deliveries were repeated." : "Balances are unchanged; no update was applied."}`;
    }
    case "Failed":
      return "Scenario execution failed. Earlier observations remain inspectable; an execution error is not a transaction decision.";
    default:
      return leg
        ? `${messagePayload(leg).title} was sent from ${actorName(leg.from)} to ${actorName(leg.to)}. Sending does not prove receipt or application.`
        : event.description;
  }
}

function transactionProgress(snapshot, prefix) {
  const decision = machineState(snapshot, "coordinator"),
    count = snapshot.nodes.length;
  const delivered = new Set(
    prefix
      .filter(
        (e) =>
          e.kind === "DecisionApplied" &&
          e.message?.Decision?.toUpperCase() === decision.toUpperCase(),
      )
      .map((e) => e.actor),
  );
  const blocked = snapshot.nodes.filter((n) => n.blocked).length;
  const detail =
    decision === "Collecting"
      ? `${snapshot.nodes.filter((n) => n.state === "Prepared").length}/${count} prepared · ${snapshot.votes_received}/${count} votes received`
      : `${decision.toUpperCase()} decided · ${delivered.size}/${count} delivered · ${count - delivered.size} awaiting delivery`;
  return `${detail}${blocked ? ` · ${blocked} blocked` : ""}${snapshot.coordinator_available ? "" : " · Coordinator unavailable (simulated)"}`;
}

class DemoPlayback {
  constructor({
    count,
    eventAt,
    active,
    animate,
    change,
    stepAt = null,
    now = () => performance.now(),
    schedule = (callback, delay) => setTimeout(callback, delay),
    unschedule = (timer) => clearTimeout(timer),
  }) {
    Object.assign(this, {
      count,
      eventAt,
      active,
      animate,
      change,
      stepAt,
      now,
      schedule,
      unschedule,
    });
    this.position = 0;
    this.playing = false;
    this.following = false;
    this.pending = null;
    this.animation = null;
    this.timer = null;
    this.remaining = 0;
    this.rate = 1;
    this.suspended = false;
    this.generation = 0;
    this.reducePending = false;
    this.mode = "all";
    this.step = null;
  }
  pauseClock() {
    if (this.timer !== null) {
      this.remaining = Math.max(
        0,
        this.remaining - (this.now() - this.started) * this.rate,
      );
      this.unschedule(this.timer);
      this.timer = null;
    }
    this.animation?.pause();
  }
  pause() {
    this.pauseClock();
    this.playing = false;
    this.following = false;
    this.change();
  }
  seek(position) {
    this.pauseClock();
    this.generation++;
    this.animation?.cancel();
    this.animation = null;
    this.pending = null;
    this.step = null;
    this.reducePending = false;
    this.remaining = 0;
    this.position = Math.max(0, Math.min(position, this.count()));
    this.playing = false;
    this.following = false;
    this.change();
  }
  play(follow = false) {
    this.pauseClock();
    this.following = follow;
    if (
      !follow &&
      this.position === this.count() &&
      this.count() > 0 &&
      !this.active() &&
      this.remaining === 0 &&
      !this.animation
    )
      this.seek(0);
    this.playing = true;
    this.following = follow;
    this.change();
    this.resume();
  }
  speed(rate) {
    this.pauseClock();
    this.rate = rate;
    if (this.animation) this.animation.playbackRate = rate;
    if (this.playing) this.resume();
    this.change();
  }
  visibility(hidden) {
    this.suspended = hidden;
    if (hidden) this.pauseClock();
    else if (this.playing) this.resume();
  }
  resume() {
    if (!this.playing || this.suspended) return;
    if (this.animation) {
      if (this.reducePending) {
        this.reducePending = false;
        this.animation.finish();
        return;
      }
      this.animation.play();
      return;
    }
    if (this.timer !== null) return;
    if (this.remaining > 0) {
      this.started = this.now();
      this.timer = this.schedule(() => {
        this.timer = null;
        this.remaining = 0;
        this.next();
      }, this.remaining / this.rate);
      return;
    }
    this.next();
  }
  wake() {
    if (
      this.playing &&
      !this.suspended &&
      !this.animation &&
      this.timer === null
    )
      this.resume();
  }
  next() {
    if (!this.playing || this.suspended) return;
    if (this.mode === "teaching" && this.stepAt) {
      const before = this.position;
      while (this.position < this.count() && !this.stepAt(this.position))
        this.position++;
      if (this.position !== before) this.change();
    }
    if (this.position >= this.count()) {
      if (!this.following || !this.active()) {
        this.playing = false;
        this.change();
      }
      return;
    }
    const event = this.eventAt(this.position),
      generation = this.generation;
    const leg = messageLeg(event);
    this.step =
      this.mode === "teaching" && this.stepAt
        ? this.stepAt(this.position)
        : { phase: "Observation", hold: 650, animate: !!leg?.received };
    if (leg?.received && this.step?.animate) {
      this.pending = event;
      this.change();
      this.animation = this.animate(event, this.step.flightMs ?? 900);
      if (this.animation) {
        this.animation.playbackRate = this.rate;
        this.animation.finished.then(
          () => {
            if (generation !== this.generation) return;
            this.animation = null;
            this.commit();
          },
          () => {},
        );
        return;
      }
    }
    this.commit();
  }
  commit() {
    this.pending = null;
    this.position++;
    this.remaining = this.step?.hold ?? 650;
    this.change();
    this.resume();
  }
  reduceMotion() {
    if (this.animation) {
      this.reducePending = true;
      if (this.playing && !this.suspended) {
        this.reducePending = false;
        this.animation.finish();
      }
    }
  }
}

// Shared presentation rules serve the scene, waterfall, and recorded details.
function actorIds(snapshot) {
  return ["coordinator", ...snapshot.nodes.map((node) => node.id)];
}
function actorName(actor) {
  if (actor === "coordinator") return "Coordinator";
  const legacy = { "source-account": 1, "destination-account": 2 };
  return `Participant ${legacy[actor] || actor.split("-").at(-1)}`;
}
function validActor(actor, snapshot) {
  return (
    actor === "coordinator" ||
    (snapshot
      ? snapshot.nodes.some((node) => node.id === actor)
      : /^(participant-[1-5]|source-account|destination-account)$/.test(actor))
  );
}
const tone = (state) =>
  ({
    Prepared: "amber",
    Committed: "green",
    Commit: "green",
    COMMIT: "green",
    YES: "green",
    Aborted: "red",
    Abort: "red",
    ABORT: "red",
    NO: "red",
  })[state] || "blue";
function initialSnapshot(count = 2) {
  return {
    coordinator: "Collecting",
    coordinator_available: true,
    votes_received: 0,
    nodes: Array.from({ length: count }, (_, i) => ({
      id: `participant-${i + 1}`,
      state: "Working",
      vote: null,
      balance: i ? 20 : 100,
      blocked: false,
    })),
  };
}
function observationLabel(event, prefix = []) {
  if (
    event.kind === "FaultInjected" &&
    !event.snapshot.coordinator_available &&
    prefix.at(-1)?.snapshot.coordinator_available !== false
  )
    return "Simulated loss";
  const leg = messageLeg(event, prefix);
  if (leg) {
    if (leg.withheld) return `${leg.label} withheld`;
    if (event.kind === "Prepared")
      return event.snapshot.nodes.find((n) => n.id === event.actor).state ===
        "Prepared"
        ? "PREPARE received"
        : "PREPARE rejected";
    if (leg.received)
      return `${leg.label}${leg.retry ? " retry" : ""} received`;
    return `${leg.label} ${leg.retry ? "retry" : "sent"}`;
  }
  return (
    {
      Started: "Start",
      DecisionMade: `Decide ${machineState(event.snapshot, "coordinator").toUpperCase()}`,
      TimeoutObserved: "Simulated timeout",
      FaultInjected: "Injected fault",
      Finished: "Recorded",
      Failed: "Run failed",
    }[event.kind] || event.kind
  );
}
function observationTone(event) {
  if (["FaultInjected", "TimeoutObserved"].includes(event.kind)) return "amber";
  if (event.kind === "Failed") return "red";
  const leg = messageLeg(event);
  if (event.kind === "Prepared")
    return tone(machineState(event.snapshot, event.actor));
  return tone(leg?.label || machineState(event.snapshot, event.actor));
}
function snapshotChanges(before, after) {
  const changes = [];
  const add = (label, from, to) => {
    if (JSON.stringify(from) !== JSON.stringify(to))
      changes.push(`${label}: ${from ?? "—"} → ${to ?? "—"}`);
  };
  add(
    "Coordinator decision",
    machineState(before, "coordinator"),
    machineState(after, "coordinator"),
  );
  add("Votes received", before.votes_received, after.votes_received);
  add(
    "Coordinator available",
    before.coordinator_available,
    after.coordinator_available,
  );
  after.nodes.forEach((node) => {
    const previous = before.nodes.find((n) => n.id === node.id);
    for (const key of ["state", "vote", "balance", "blocked"])
      add(`${actorName(node.id)} ${key}`, previous?.[key], node[key]);
  });
  return changes;
}
// Each send is a delivery attempt. A withheld observation is never a send or receipt.
function deliveryAttempts(events) {
  const attempts = [],
    pending = new Map(),
    seen = new Set();
  events.forEach((event, index) => {
    const leg = messageLeg(event);
    if (!leg) return;
    const identity = JSON.stringify([
      event.message_id,
      leg.from,
      leg.to,
      leg.label,
    ]);
    if (leg.withheld) {
      seen.add(identity);
      const queue = pending.get(event.message_id) || [];
      const match = queue.findIndex(
        (a) => a.from === leg.from && a.to === leg.to && a.label === leg.label,
      );
      if (match >= 0) queue.splice(match, 1)[0].withheld = true;
      return;
    }
    if (leg.received) {
      const queue = pending.get(event.message_id) || [];
      const match = queue.findIndex(
        (a) => a.from === leg.from && a.to === leg.to && a.label === leg.label,
      );
      if (match >= 0) {
        const attempt = queue.splice(match, 1)[0];
        attempt.receipt = index;
        attempt.receiptSequence = event.sequence;
      }
    } else {
      const attempt = {
        ...leg,
        retry: leg.retry || seen.has(identity),
        sent: index,
        sequence: event.sequence,
        receipt: null,
      };
      attempts.push(attempt);
      seen.add(identity);
      if (!pending.has(event.message_id)) pending.set(event.message_id, []);
      pending.get(event.message_id).push(attempt);
    }
  });
  return attempts;
}
function svgElement(tag, attributes = {}, text) {
  const el = document.createElementNS("http://www.w3.org/2000/svg", tag);
  for (const [key, value] of Object.entries(attributes))
    el.setAttribute(key, value);
  if (text) el.textContent = text;
  return el;
}
function sceneGeometry(participants, compact) {
  const cardHeight = 88;
  const width = compact ? 360 : 560,
    columns = compact ? 2 : Math.min(3, participants.length);
  const gap = compact ? 12 : 16,
    card = (width - 24 - (columns - 1) * gap) / columns;
  const rows = Math.ceil(participants.length / columns),
    boxes = [["coordinator", (width - 190) / 2, 16, 190]],
    routes = new Map();
  participants.forEach((actor, index) => {
    const row = Math.floor(index / columns),
      column = index % columns;
    const inRow = Math.min(columns, participants.length - row * columns);
    const x =
      (width - (inRow * card + (inRow - 1) * gap)) / 2 + column * (card + gap);
    const y = 152 + row * 128,
      end = [x + card / 2, y - 9];
    boxes.push([actor, x, y, card]);
    if (row === 0) {
      const start = [
          width / 2 + (column - (inRow - 1) / 2) * 50,
          16 + cardHeight + 9,
        ],
        bend = y - 40 + column * 6;
      routes.set(actor, [start, [start[0], bend], [end[0], bend], end]);
    } else {
      const left = column % 2 === 0,
        side = left ? 6 + row * 2 : width - 6 - row * 2;
      const start = [width / 2 + (left ? -104 : 104), 36 + row * 25];
      routes.set(actor, [
        start,
        [side, start[1]],
        [side, y - 24],
        [end[0], y - 24],
        end,
      ]);
    }
  });
  return {
    width,
    height: Math.max(330, 256 + (rows - 1) * 128),
    cardHeight,
    boxes,
    routes,
  };
}

// Keep the card readable at the edge of a route; the wire stays exact.
function payloadPoint(point, width, height) {
  return [
    Math.max(70, Math.min(width - 70, point[0])),
    Math.max(22, Math.min(height - 22, point[1])),
  ];
}
class MessageBoard {
  constructor(root, caption, inspect) {
    this.caption = caption;
    this.svg = svgElement("svg", {
      viewBox: "0 0 560 360",
      "aria-label": "Transaction nodes and messages",
    });
    this.nodes = new Map();
    this.routes = new Map();
    this.root = root;
    this.inspect = inspect;
    this.topology = svgElement("g");
    this.svg.append(this.topology);
    this.packet = svgElement("g", {
      class: "message-packet",
      visibility: "hidden",
      "aria-hidden": "true",
    });
    this.packet.append(
      svgElement("rect", { x: -70, y: -22, width: 140, height: 44, rx: 5 }),
    );
    this.packetTitle = svgElement("text", {
      "text-anchor": "middle",
      y: -3,
      class: "payload-title",
    });
    this.packetDetail = svgElement("text", {
      "text-anchor": "middle",
      y: 13,
      class: "payload-detail",
    });
    this.packet.append(this.packetTitle, this.packetDetail);
    this.svg.append(this.packet);
    this.block = svgElement(
      "text",
      { class: "message-block", "text-anchor": "middle", visibility: "hidden" },
      "× Withheld",
    );
    this.svg.append(this.block);
    root.append(this.svg);
    this.compact = matchMedia("(max-width: 500px)");
    this.configure(initialSnapshot());
    this.compact.addEventListener("change", () => this.layout());
  }
  configure(snapshot) {
    const actors = actorIds(snapshot),
      signature = actors.join(",");
    if (signature === this.topologyKey) return;
    this.animation?.cancel();
    this.animation = null;
    this.topologyKey = signature;
    this.participants = snapshot.nodes.map((node) => node.id);
    this.root.dataset.count = this.participants.length;
    this.key = null;
    this.leg = null;
    this.nodes.clear();
    this.routes.clear();
    this.topology.replaceChildren();
    const defs = svgElement("defs");
    this.topology.append(defs);
    for (const actor of actors.filter((a) => a !== "coordinator")) {
      const markerId = `scene-arrow-${actor}`;
      const marker = svgElement("marker", {
        id: markerId,
        viewBox: "0 0 8 8",
        refX: 8,
        refY: 4,
        markerWidth: 7,
        markerHeight: 7,
        orient: "auto-start-reverse",
        markerUnits: "userSpaceOnUse",
      });
      const arrow = svgElement("path", { d: "M0 0 L8 4 L0 8 Z" });
      marker.append(arrow);
      defs.append(marker);
      const path = svgElement("path", { class: "message-wire" });
      this.topology.append(path);
      this.routes.set(actor, { path, arrow, markerId });
    }
    for (const actor of actors) {
      const group = svgElement("g", {
        "data-actor": actor,
        class: "message-node",
        role: "button",
        tabindex: 0,
      });
      const rect = svgElement("rect", { height: 108, rx: 8 });
      group.append(rect);
      const lines = ["title", "state", "meta", "note"].map((kind, i) => {
        const text = svgElement("text", {
          "text-anchor": "middle",
          class: `message-node-${kind}`,
        });
        group.append(text);
        return text;
      });
      group.onclick = () => this.inspect(actor);
      group.onkeydown = (e) => {
        if (["Enter", " "].includes(e.key)) {
          e.preventDefault();
          this.inspect(actor);
        }
      };
      this.topology.append(group);
      this.nodes.set(actor, { group, rect, lines });
    }
    this.layout();
  }
  layout() {
    const geometry = sceneGeometry(this.participants, this.compact.matches);
    this.bounds = geometry;
    this.svg.setAttribute(
      "viewBox",
      `0 0 ${geometry.width} ${geometry.height}`,
    );
    for (const [actor, x, y, width] of geometry.boxes) {
      const node = this.nodes.get(actor);
      node.rect.setAttribute("height", geometry.cardHeight);
      for (const [key, value] of Object.entries({ x, y, width }))
        node.rect.setAttribute(key, value);
      node.lines.forEach((line, i) => {
        line.setAttribute("x", x + width / 2);
        line.setAttribute("y", y + [20, 41, 62, 80][i]);
      });
    }
    for (const [actor, points] of geometry.routes) {
      const route = this.routes.get(actor);
      route.points = points;
      route.path.setAttribute(
        "d",
        points.map((p, i) => `${i ? "L" : "M"}${p.join(" ")}`).join(" "),
      );
    }
    if (this.animation)
      this.animation.effect.setKeyframes(this.frames(this.leg));
    else if (this.leg) this.showLeg(this.leg);
  }
  points(leg) {
    const points = this.routes.get(
      leg.from === "coordinator" ? leg.to : leg.from,
    ).points;
    return leg.from === "coordinator" ? points : [...points].reverse();
  }
  frames(leg) {
    const points = this.points(leg),
      distances = [0];
    for (let i = 1; i < points.length; i++)
      distances.push(
        distances[i - 1] +
          Math.hypot(
            points[i][0] - points[i - 1][0],
            points[i][1] - points[i - 1][1],
          ),
      );
    return points.map((point, i) => {
      const p = payloadPoint(point, this.bounds.width, this.bounds.height);
      return {
        transform: `translate(${p[0]}px,${p[1]}px)`,
        offset: distances[i] / distances.at(-1),
      };
    });
  }
  showLeg(leg, flying = false) {
    this.leg = leg;
    this.packet.setAttribute("visibility", "hidden");
    this.block.setAttribute("visibility", "hidden");
    for (const { path } of this.routes.values()) {
      path.removeAttribute("data-active");
      path.removeAttribute("data-withheld");
      path.removeAttribute("marker-start");
      path.removeAttribute("marker-end");
    }
    if (!leg) return;
    const route = this.routes.get(
        leg.from === "coordinator" ? leg.to : leg.from,
      ),
      color = `var(--${tone(leg.label)})`;
    route.path.setAttribute("data-active", "true");
    route.path.style.setProperty("--wire-color", color);
    route.arrow.style.fill = color;
    if (leg.withheld) {
      route.path.setAttribute("data-withheld", "true");
      this.block.setAttribute("x", route.points.at(-1)[0]);
      this.block.setAttribute("y", route.points.at(-1)[1] - 14);
      this.block.setAttribute("visibility", "visible");
    } else {
      route.path.setAttribute(
        leg.from === "coordinator" ? "marker-end" : "marker-start",
        `url(#${route.markerId})`,
      );
      if (!leg.received || flying) {
        const p = payloadPoint(
          this.points(leg)[0],
          this.bounds.width,
          this.bounds.height,
        );
        this.packet.style.transform = `translate(${p[0]}px,${p[1]}px)`;
        this.packet.style.setProperty("--packet-color", color);
        const payload = messagePayload(leg);
        this.packetTitle.textContent = payload.title;
        this.packetDetail.textContent = payload.detail;
        this.packet.setAttribute("visibility", "visible");
      }
    }
  }
  render(snapshot, event, prefix, pending) {
    this.configure(snapshot);
    for (const [actor, node] of this.nodes) {
      const state = machineState(snapshot, actor),
        participant = snapshot.nodes.find((n) => n.id === actor);
      const note =
        actor === "coordinator"
          ? snapshot.coordinator_available
            ? ""
            : "Unavailable · simulated"
          : participant.blocked
            ? "Blocked · waiting"
            : state === "Prepared"
              ? "Update pending"
              : state === "Committed"
                ? "Update applied once"
                : state === "Aborted"
                  ? "No update applied"
                  : "";
      const meta = participant
        ? `Vote ${participant.vote?.toUpperCase() || "—"} · Balance ${participant.balance}`
        : `${snapshot.votes_received} / ${snapshot.nodes.length} votes received`;
      const label =
        actor === "coordinator"
          ? `Decision: ${state === "Collecting" ? "Undecided" : state.toUpperCase()}`
          : state;
      const visibleMeta = participant ? meta.replace("Vote ", "") : meta;
      [actorName(actor), label, visibleMeta, note].forEach(
        (text, i) => (node.lines[i].textContent = text),
      );
      node.group.style.setProperty("--state-ink", `var(--${tone(state)})`);
      node.group.setAttribute(
        "aria-label",
        `${actorName(actor)}. ${label}. ${meta}. ${note}. Inspect state machine`,
      );
    }
    if (pending) {
      const leg = messageLeg(pending, prefix);
      this.caption.textContent = observationExplanation(pending, prefix, true);
      this.key = null;
      return;
    }
    const key = event ? `${event.run_id}:${event.sequence}` : "initial";
    if (key === this.key) return;
    this.key = key;
    this.showLeg(messageLeg(event, prefix));
    this.caption.textContent = observationExplanation(event, prefix);
  }
  flight(event, prefix, reduced, duration = 900) {
    const leg = messageLeg(event, prefix);
    this.showLeg(leg, true);
    if (reduced) {
      this.packet.setAttribute("visibility", "hidden");
      return null;
    }
    const animation = this.packet.animate(this.frames(leg), {
      duration,
      easing: "linear",
    });
    this.animation = animation;
    const clear = () => {
      if (this.animation === animation) this.animation = null;
    };
    animation.onfinish = clear;
    animation.oncancel = clear;
    return animation;
  }
}
class AuditWaterfall {
  constructor(root, rows, links, seek, pause) {
    Object.assign(this, { root, rows, links, seek, pause });
    for (const kind of ["wheel", "touchmove", "pointerdown"])
      root.addEventListener(
        kind,
        () => {
          this.browsing = true;
          pause();
        },
        { passive: true },
      );
    root.addEventListener("keydown", (e) => {
      if (
        [
          "ArrowLeft",
          "ArrowRight",
          "ArrowUp",
          "ArrowDown",
          "PageUp",
          "PageDown",
          "Home",
          "End",
        ].includes(e.key)
      ) {
        this.browsing = true;
        pause();
      }
    });
    new ResizeObserver(() => {
      this.laneHint();
      if (!this.browsing) this.ensureVisible();
    }).observe(root);
    root.addEventListener(
      "scroll",
      () => {
        document.getElementById("audit-lanes").scrollLeft = root.scrollLeft;
      },
      { passive: true },
    );
  }
  render(events, snapshot) {
    const actors = actorIds(snapshot);
    const key =
      actors.join(",") +
      events.map((e) => `${e.run_id}:${e.sequence}`).join(",");
    if (key === this.key) return;
    this.key = key;
    this.browsing = false;
    const focusSequence = this.rows.querySelector(":focus")?.dataset.sequence;
    const append =
      this.actorsKey === actors.join(",") &&
      (this.renderedEvents || []).every((event, i) => event === events[i]);
    const offset = append ? this.renderedEvents.length : 0;
    this.rows
      .querySelector('[aria-current="step"]')
      ?.removeAttribute("aria-current");
    if (!append) this.rows.replaceChildren();
    this.renderedEvents = events;
    this.links.replaceChildren();
    this.links.toggleAttribute("hidden", !events.length);
    this.root.closest(".audit").style.setProperty("--lanes", actors.length);
    const lanes = document.getElementById("audit-lanes");
    if (this.actorsKey !== actors.join(",")) {
      this.actorsKey = actors.join(",");
      lanes.replaceChildren(document.createElement("span"));
      actors.forEach((actor) => {
        const label = document.createElement("span");
        label.textContent =
          actor === "coordinator"
            ? "Coord."
            : actorName(actor).replace("Participant ", "P");
        label.title = actorName(actor);
        lanes.append(label);
      });
    }
    const height = Math.max(48, events.length * 48);
    this.links.setAttribute("viewBox", `0 0 ${actors.length * 100} ${height}`);
    this.links.setAttribute("preserveAspectRatio", "none");
    this.links.style.height = `${height}px`;
    for (const x of actors.map((_, i) => i * 100 + 50))
      this.links.append(
        svgElement("line", {
          x1: x,
          x2: x,
          y1: 0,
          y2: height,
          class: "audit-lifeline",
        }),
      );
    const defs = svgElement("defs"),
      marker = svgElement("marker", {
        id: "audit-arrow",
        viewBox: "0 0 8 8",
        refX: 8,
        refY: 4,
        markerWidth: 5,
        markerHeight: 5,
        orient: "auto",
      });
    marker.append(
      svgElement("path", { d: "M0 0 L8 4 L0 8 Z", fill: "var(--muted)" }),
    );
    defs.append(marker);
    this.links.append(defs);
    for (const attempt of deliveryAttempts(events)) {
      const x1 = actors.indexOf(attempt.from) * 100 + 50,
        x2 = actors.indexOf(attempt.to) * 100 + 50,
        y1 = attempt.sent * 48 + 37;
      const received = attempt.receipt !== null,
        y2 = received ? attempt.receipt * 48 + 11 : y1 + 8;
      const end = received ? x2 : x1 + (x2 - x1) * 0.65;
      this.links.append(
        svgElement("path", {
          d: `M${x1} ${y1} L${end} ${y2}`,
          class: "audit-link",
          "data-selected":
            attempt.sequence === events.at(-1)?.sequence ||
            attempt.receiptSequence === events.at(-1)?.sequence,
          "data-pending": !received,
          "data-withheld": attempt.withheld,
          "data-send": attempt.sequence,
          "data-receipt": attempt.receiptSequence || "",
          "marker-end": attempt.withheld ? "" : "url(#audit-arrow)",
        }),
      );
      if (attempt.withheld)
        this.links.append(
          svgElement(
            "text",
            {
              x: end,
              y: y2 + 4,
              fill: "var(--red)",
              "text-anchor": "middle",
              "font-size": 15,
            },
            "×",
          ),
        );
    }
    events.forEach((event, i) => {
      if (i < offset) return;
      const li = document.createElement("li"),
        button = document.createElement("button"),
        number = document.createElement("span"),
        label = document.createElement("span");
      button.className = "audit-row";
      button.dataset.sequence = event.sequence;
      button.setAttribute(
        "aria-label",
        `Observation ${event.sequence}, ${actorName(event.actor)}, ${event.description}`,
      );
      if (i === events.length - 1) button.setAttribute("aria-current", "step");
      button.onclick = () => this.seek(event.sequence);
      number.className = "audit-number";
      number.textContent = event.sequence;
      label.className = `audit-mark ${observationTone(event)}`;
      label.style.gridColumn = actors.indexOf(event.actor) + 2;
      label.textContent = observationLabel(event, events.slice(0, i));
      button.append(number, label);
      li.append(button);
      this.rows.append(li);
    });
    this.rows.lastElementChild?.firstElementChild.setAttribute(
      "aria-current",
      "step",
    );
    if (focusSequence)
      this.rows
        .querySelector(`[data-sequence="${focusSequence}"]`)
        ?.focus({ preventScroll: true });
    this.ensureVisible();
    this.laneHint();
  }
  laneHint() {
    document.querySelector(".audit-heading p").textContent =
      this.root.scrollWidth > this.root.clientWidth
        ? "Observation order · scroll sideways for all lanes"
        : "Observation order";
  }
  ensureVisible() {
    const row = this.rows.lastElementChild;
    if (row) {
      const top = row.offsetTop;
      if (
        top < this.root.scrollTop ||
        top + 48 > this.root.scrollTop + this.root.clientHeight
      )
        this.root.scrollTop = Math.max(0, top + 48 - this.root.clientHeight);
    }
  }
}
