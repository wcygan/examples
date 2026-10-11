// Playback time is independent of audit-log time. Receipts advance state on arrival.
function messageLeg(event, prefix = []) {
  if (!event?.message || !event.peer) return null;
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
  const label =
    event.message === "Prepare"
      ? "PREPARE"
      : event.message.Vote?.toUpperCase() ||
        event.message.Decision?.toUpperCase();
  if (!label) return null;
  const prior = [...prefix]
    .reverse()
    .find(
      (e) =>
        e.message_id === event.message_id &&
        [
          "PrepareSent",
          "VoteSent",
          "DecisionSent",
          "DuplicateDelivery",
        ].includes(e.kind),
    );
  return {
    from: received ? event.peer : event.actor,
    to: received ? event.actor : event.peer,
    label,
    received,
    withheld,
    retry:
      event.kind === "DuplicateDelivery" ||
      (received && prior?.kind === "DuplicateDelivery"),
  };
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

class MessageBoard {
  constructor(root, caption) {
    this.caption = caption;
    this.key = "";
    const ns = "http://www.w3.org/2000/svg";
    const el = (tag, attrs = {}, text) => {
      const node = document.createElementNS(ns, tag);
      for (const [key, value] of Object.entries(attrs))
        node.setAttribute(key, value);
      if (text) node.textContent = text;
      return node;
    };
    this.svg = el("svg", {
      viewBox: "0 0 720 275",
      role: "img",
      "aria-label": "Protocol messages between coordinator and participants",
    });
    this.compact = window.matchMedia("(max-width: 650px)");
    this.nodes = new Map();
    this.routes = new Map();
    const boxes = [
      ["coordinator", "Coordinator", 24, 95, 150],
      ["source-account", "Source account", 526, 20, 170],
      ["destination-account", "Destination account", 526, 184, 170],
    ];
    const paths = [
      [
        "source-account",
        [
          [174, 128],
          [350, 128],
          [350, 53],
          [526, 53],
        ],
      ],
      [
        "destination-account",
        [
          [174, 128],
          [350, 128],
          [350, 217],
          [526, 217],
        ],
      ],
    ];
    const defs = el("defs");
    this.svg.append(defs);
    for (const [actor, points] of paths) {
      const path = el("path", {
        d: points.map((p, i) => `${i ? "L" : "M"}${p.join(" ")}`).join(" "),
        class: "message-wire",
      });
      const markerId = `message-arrow-${actor}`;
      const marker = el("marker", {
        id: markerId,
        viewBox: "0 0 8 8",
        refX: 8,
        refY: 4,
        markerWidth: 7,
        markerHeight: 7,
        orient: "auto-start-reverse",
        markerUnits: "userSpaceOnUse",
      });
      const arrow = el("path", { d: "M0 0 L8 4 L0 8 Z" });
      marker.append(arrow);
      defs.append(marker);
      this.svg.append(path);
      this.routes.set(actor, { points, path, arrow, markerId });
    }
    for (const [actor, title, x, y, w] of boxes) {
      const group = el("g", { "data-actor": actor, class: "message-node" });
      group.append(el("rect", { x, y, width: w, height: 66, rx: 10 }));
      group.append(
        el(
          "text",
          {
            x: x + w / 2,
            y: y + 25,
            "text-anchor": "middle",
            class: "message-node-title",
          },
          title,
        ),
      );
      const state = el("text", {
        x: x + w / 2,
        y: y + 47,
        "text-anchor": "middle",
        class: "message-node-state",
      });
      group.append(state);
      this.svg.append(group);
      this.nodes.set(actor, { group, state, title });
    }
    this.packet = el("g", {
      class: "message-packet",
      visibility: "hidden",
      "aria-hidden": "true",
    });
    this.packet.append(
      el("rect", { x: -11, y: -11, width: 22, height: 22, rx: 6 }),
    );
    this.glyph = el("text", { "text-anchor": "middle", y: 4 });
    this.packet.append(this.glyph);
    this.svg.append(this.packet);
    this.block = el(
      "text",
      {
        x: 350,
        y: 184,
        "text-anchor": "middle",
        class: "message-block",
        visibility: "hidden",
      },
      "×",
    );
    this.svg.append(this.block);
    root.append(this.svg);
    this.layout();
    this.compact.addEventListener("change", () => this.layout());
  }
  color(label) {
    return {
      YES: "green",
      NO: "red",
      COMMIT: "green",
      ABORT: "red",
      PREPARE: "blue",
    }[label];
  }
  points(leg) {
    const actor = leg.from === "coordinator" ? leg.to : leg.from;
    const points = this.routes.get(actor).points;
    return leg.from === "coordinator" ? points : [...points].reverse();
  }
  setPacket(leg, point) {
    this.packet.setAttribute("visibility", "visible");
    this.packet.style.setProperty(
      "--packet-color",
      `var(--${this.color(leg.label)})`,
    );
    this.glyph.textContent = {
      PREPARE: "P",
      YES: "Y",
      NO: "N",
      COMMIT: "C",
      ABORT: "A",
    }[leg.label];
    this.packet.style.transform = `translate(${point[0]}px,${point[1]}px)`;
  }
  render(snapshot, event, prefix, pending) {
    for (const [actor, node] of this.nodes) {
      const state = machineState(snapshot, actor);
      node.state.textContent = state;
      node.group.style.setProperty(
        "--state-fill",
        `var(--${tone(state)}-soft)`,
      );
      node.group.style.setProperty("--state-ink", `var(--${tone(state)})`);
    }
    if (pending) {
      this.svg.setAttribute(
        "aria-label",
        `Receiving ${messageLeg(pending).label}; state updates on arrival`,
      );
      this.key = "";
      this.caption.textContent = `Receiving ${messageLeg(pending).label} · state updates on arrival`;
      return;
    }
    const key = event ? `${event.run_id}:${event.sequence}` : "initial";
    if (this.key === key) return;
    this.key = key;
    const leg = messageLeg(event, prefix);
    this.leg = leg;
    this.packet.setAttribute("visibility", "hidden");
    this.block.setAttribute("visibility", "hidden");
    for (const { path } of this.routes.values()) {
      path.removeAttribute("data-active");
      path.removeAttribute("data-withheld");
      path.removeAttribute("marker-start");
      path.removeAttribute("marker-end");
    }
    if (!leg) {
      this.caption.textContent = event
        ? event.description
        : "Start a scenario to follow PREPARE, votes, and decisions.";
      return;
    }
    const actor = leg.from === "coordinator" ? leg.to : leg.from,
      route = this.routes.get(actor);
    route.path.setAttribute("data-active", "true");
    route.path.setAttribute(
      leg.from === "coordinator" ? "marker-end" : "marker-start",
      `url(#${route.markerId})`,
    );
    route.arrow.style.fill = `var(--${this.color(leg.label)})`;
    route.path.style.setProperty(
      "--wire-color",
      `var(--${this.color(leg.label)})`,
    );
    const name = (actor) => this.nodes.get(actor).title;
    this.caption.textContent = `${leg.label}${leg.retry ? " · retry" : ""}: ${name(leg.from)} → ${name(leg.to)} · ${leg.withheld ? "withheld" : leg.received ? "observed at receiver" : "sent; waiting for receipt"}`;
    this.svg.setAttribute("aria-label", this.caption.textContent);
    if (leg.withheld) {
      route.path.setAttribute("data-withheld", "true");
      this.positionBlock(leg);
      this.block.setAttribute("visibility", "visible");
    } else if (!leg.received) this.setPacket(leg, this.points(leg)[0]);
  }
  frames(leg) {
    const points = this.points(leg);
    let total = 0;
    const distances = [0];
    for (let i = 1; i < points.length; i++) {
      total += Math.hypot(
        points[i][0] - points[i - 1][0],
        points[i][1] - points[i - 1][1],
      );
      distances.push(total);
    }
    return points.map((point, i) => ({
      transform: `translate(${point[0]}px,${point[1]}px)`,
      offset: distances[i] / total,
    }));
  }
  layout() {
    const compact = this.compact.matches;
    this.svg.setAttribute("viewBox", compact ? "0 0 360 350" : "0 0 720 275");
    const boxes = compact
      ? [
          ["coordinator", 100, 24, 160],
          ["source-account", 10, 248, 160],
          ["destination-account", 190, 248, 160],
        ]
      : [
          ["coordinator", 24, 95, 150],
          ["source-account", 526, 20, 170],
          ["destination-account", 526, 184, 170],
        ];
    const bounds = new Map(
      boxes.map(([actor, x, y, w]) => [actor, { x, y, w }]),
    );
    for (const [actor, b] of bounds) {
      const node = this.nodes.get(actor);
      const rect = node.group.querySelector("rect");
      rect.setAttribute("x", b.x);
      rect.setAttribute("y", b.y);
      rect.setAttribute("width", b.w);
      const title = node.group.querySelector(".message-node-title");
      title.setAttribute("x", b.x + b.w / 2);
      title.setAttribute("y", b.y + 25);
      node.state.setAttribute("x", b.x + b.w / 2);
      node.state.setAttribute("y", b.y + 47);
    }
    const coordinator = bounds.get("coordinator");
    for (const [actor, route] of this.routes) {
      const b = bounds.get(actor);
      const start = compact
        ? [coordinator.x + coordinator.w / 2, coordinator.y + 66]
        : [coordinator.x + coordinator.w, coordinator.y + 33];
      const end = compact ? [b.x + b.w / 2, b.y] : [b.x, b.y + 33];
      route.points = compact
        ? [start, [start[0], 154], [end[0], 154], end]
        : [start, [350, start[1]], [350, end[1]], end];
      route.path.setAttribute(
        "d",
        route.points.map((p, i) => `${i ? "L" : "M"}${p.join(" ")}`).join(" "),
      );
    }
    if (this.activeAnimation)
      this.activeAnimation.effect.setKeyframes(this.frames(this.leg));
    else if (this.leg && !this.leg.received && !this.leg.withheld)
      this.setPacket(this.leg, this.points(this.leg)[0]);
    if (this.leg?.withheld) this.positionBlock(this.leg);
  }
  positionBlock(leg) {
    const points = this.points(leg);
    this.block.setAttribute("x", points[2][0]);
    this.block.setAttribute("y", (points[1][1] + points[3][1]) / 2 + 10);
  }
  flight(event, prefix, reduced) {
    const leg = messageLeg(event, prefix),
      points = this.points(leg);
    this.leg = leg;
    for (const { path } of this.routes.values()) {
      path.removeAttribute("data-active");
      path.removeAttribute("data-withheld");
      path.removeAttribute("marker-start");
      path.removeAttribute("marker-end");
    }
    const actor = leg.from === "coordinator" ? leg.to : leg.from,
      route = this.routes.get(actor);
    route.path.setAttribute("data-active", "true");
    route.path.setAttribute(
      leg.from === "coordinator" ? "marker-end" : "marker-start",
      `url(#${route.markerId})`,
    );
    route.arrow.style.fill = `var(--${this.color(leg.label)})`;
    route.path.style.setProperty(
      "--wire-color",
      `var(--${this.color(leg.label)})`,
    );
    route.path.removeAttribute("data-withheld");
    this.block.setAttribute("visibility", "hidden");
    this.setPacket(leg, points[0]);
    if (reduced) {
      this.packet.setAttribute("visibility", "hidden");
      return null;
    }
    const animation = this.packet.animate(this.frames(leg), {
      duration: 900,
      easing: "linear",
      fill: "none",
    });
    this.activeAnimation = animation;
    const clear = () => {
      if (this.activeAnimation === animation) this.activeAnimation = null;
    };
    animation.onfinish = clear;
    animation.oncancel = clear;
    return animation;
  }
}
