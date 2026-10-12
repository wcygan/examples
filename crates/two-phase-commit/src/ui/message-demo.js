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
    leg.retry = attempt?.retry || false;
  }
  return leg;
}

class DemoPlayback {
  constructor({
    count,
    eventAt,
    active,
    animate,
    change,
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
      !this.active()
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
    if (leg?.received) {
      this.pending = event;
      this.change();
      this.animation = this.animate(event);
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
    this.remaining = 650;
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
  const cardHeight = compact ? 108 : 88;
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
    const y = (compact ? 200 : 152) + row * (compact ? 160 : 128),
      end = [x + card / 2, y];
    boxes.push([actor, x, y, card]);
    if (row === 0) {
      const start = [
          width / 2 + (column - (inRow - 1) / 2) * 50,
          16 + cardHeight,
        ],
        bend = y - 40 + column * 6;
      routes.set(actor, [start, [start[0], bend], [end[0], bend], end]);
    } else {
      const left = column % 2 === 0,
        side = left ? 6 + row * 2 : width - 6 - row * 2;
      const start = [width / 2 + (left ? -95 : 95), 36 + row * 25];
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
    height: compact
      ? Math.max(360, 324 + (rows - 1) * 160)
      : Math.max(330, 256 + (rows - 1) * 128),
    cardHeight,
    boxes,
    routes,
  };
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
      svgElement("rect", { x: -56, y: -13, width: 112, height: 26, rx: 6 }),
    );
    this.packetText = svgElement("text", { "text-anchor": "middle", y: 4 });
    this.packet.append(this.packetText);
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
        line.setAttribute(
          "y",
          y + (this.compact.matches ? [24, 49, 74, 96] : [20, 41, 62, 80])[i],
        );
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
    return points.map((p, i) => ({
      transform: `translate(${p[0]}px,${p[1]}px)`,
      offset: distances[i] / distances.at(-1),
    }));
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
        const p = this.points(leg)[0];
        this.packet.style.transform = `translate(${p[0]}px,${p[1]}px)`;
        this.packet.style.setProperty("--packet-color", color);
        this.packetText.textContent = `${leg.label}${leg.retry ? " · retry" : ""}`;
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
              ? "Awaiting decision"
              : "";
      const meta = participant
        ? `Vote ${participant.vote?.toUpperCase() || "—"} · Balance ${participant.balance}`
        : `${snapshot.votes_received} / ${snapshot.nodes.length} votes received`;
      const label =
        actor === "coordinator"
          ? `Decision: ${state === "Collecting" ? "Undecided" : state.toUpperCase()}`
          : state;
      [actorName(actor), label, meta, note].forEach(
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
      this.caption.textContent = `${leg.label}${leg.retry ? " retry" : ""}: ${actorName(leg.from)} → ${actorName(leg.to)}. State updates on arrival.`;
      this.key = null;
      return;
    }
    const key = event ? `${event.run_id}:${event.sequence}` : "initial";
    if (key === this.key) return;
    this.key = key;
    this.showLeg(messageLeg(event, prefix));
    this.caption.textContent =
      event?.description ||
      "Choose a scenario, then run it to follow the transaction.";
  }
  flight(event, prefix, reduced) {
    const leg = messageLeg(event, prefix);
    this.showLeg(leg, true);
    if (reduced) {
      this.packet.setAttribute("visibility", "hidden");
      return null;
    }
    const animation = this.packet.animate(this.frames(leg), {
      duration: 900,
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
    this.rows.replaceChildren();
    this.links.replaceChildren();
    this.links.toggleAttribute("hidden", !events.length);
    this.root.closest(".audit").style.setProperty("--lanes", actors.length);
    const lanes = document.getElementById("audit-lanes");
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
