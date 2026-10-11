// Observations select the visual state; this module never executes the protocol.
const machineNamespace = "http://www.w3.org/2000/svg";
const machineViews = new Map();
function machineState(snapshot, actor) {
  if (actor === "coordinator") {
    return typeof snapshot.coordinator === "object"
      ? snapshot.coordinator.Decided
      : "Collecting";
  }
  return snapshot.nodes.find((node) => node.id === actor).state;
}
function machineTransition(actor, previous, snapshot, event) {
  if (!event) return null;
  const from = machineState(previous, actor),
    to = machineState(snapshot, actor);
  if (from !== to) return `${from}-${to}`;
  if (actor === "coordinator" && event.actor === actor) {
    if (event.kind === "TimeoutObserved" && to === "Collecting")
      return "Collecting-Collecting";
    if (event.kind === "VoteReceived" && to !== "Collecting")
      return `${to}-${to}`;
  }
  if (actor !== "coordinator") {
    if (event.kind === "TimeoutObserved" && to === "Prepared")
      return "Prepared-Prepared";
    if (
      event.actor === actor &&
      event.kind === "DecisionApplied" &&
      ["Committed", "Aborted"].includes(to)
    )
      return `${to}-${to}`;
  }
  return null;
}
function machineElement(tag, attributes = {}, text) {
  const element = document.createElementNS(machineNamespace, tag);
  for (const [key, value] of Object.entries(attributes))
    element.setAttribute(key, value);
  if (text) element.textContent = text;
  return element;
}
function machineModel(coordinator) {
  const states = coordinator
    ? [
        { name: "Collecting", x: 20, y: 105, w: 120, h: 50 },
        { name: "Commit", x: 326, y: 55, w: 112, h: 50 },
        { name: "Abort", x: 326, y: 193, w: 112, h: 50 },
      ]
    : [
        { name: "Working", x: 20, y: 105, w: 100, h: 50 },
        { name: "Prepared", x: 170, y: 105, w: 106, h: 50 },
        { name: "Committed", x: 326, y: 55, w: 112, h: 50 },
        { name: "Aborted", x: 326, y: 193, w: 112, h: 50 },
      ];
  const bounds = Object.fromEntries(states.map((state) => [state.name, state]));
  const port = (name, face, offset = 0.5) => {
    const b = bounds[name];
    if (face === "left") return [b.x, b.y + b.h * offset];
    if (face === "right") return [b.x + b.w, b.y + b.h * offset];
    if (face === "top") return [b.x + b.w * offset, b.y];
    return [b.x + b.w * offset, b.y + b.h];
  };
  const edge = (from, to, points, label, x, y) => ({
    key: `${from}-${to}`,
    to,
    path: points
      .map((point, i) => `${i ? "L" : "M"}${point.join(" ")}`)
      .join(" "),
    label,
    x,
    y,
  });
  const terminal = coordinator ? ["Commit", "Abort"] : ["Committed", "Aborted"];
  const edges = coordinator
    ? [
        edge(
          "Collecting",
          "Commit",
          [
            port("Collecting", "right"),
            [234, 130],
            [234, 80],
            port("Commit", "left"),
          ],
          "All YES",
          277,
          70,
        ),
        edge(
          "Collecting",
          "Abort",
          [port("Collecting", "bottom"), [80, 218], port("Abort", "left")],
          "Any NO",
          198,
          208,
        ),
        edge(
          "Collecting",
          "Collecting",
          [
            port("Collecting", "top", 0.2),
            [44, 70],
            [116, 70],
            port("Collecting", "top", 0.8),
          ],
          "Timeout: wait",
          80,
          60,
        ),
      ]
    : [
        edge(
          "Working",
          "Prepared",
          [port("Working", "right"), port("Prepared", "left")],
          "YES",
          145,
          120,
        ),
        edge(
          "Working",
          "Aborted",
          [port("Working", "bottom"), [70, 218], port("Aborted", "left")],
          "NO",
          140,
          208,
        ),
        edge(
          "Prepared",
          "Committed",
          [port("Prepared", "top"), [223, 80], port("Committed", "left")],
          "COMMIT",
          274,
          70,
        ),
        edge(
          "Prepared",
          "Aborted",
          [
            port("Prepared", "bottom"),
            [223, 181],
            [382, 181],
            port("Aborted", "top"),
          ],
          "ABORT",
          291,
          174,
        ),
        edge(
          "Prepared",
          "Prepared",
          [
            port("Prepared", "right", 0.2),
            [303, 115],
            [303, 145],
            port("Prepared", "right", 0.8),
          ],
          "Timeout",
          350,
          135,
        ),
      ];
  terminal.forEach((name, index) => {
    const face = index ? "bottom" : "top",
      y = index ? 269 : 26;
    const start = port(name, face, 0.2),
      end = port(name, face, 0.8);
    edges.push(
      edge(
        name,
        name,
        [port(name, face, 0.2), [348.4, y], [415.6, y], port(name, face, 0.8)],
        coordinator ? "Late vote: fixed" : "Same outcome",
        382,
        index ? 283 : 16,
      ),
    );
  });
  return { states, edges };
}
function createMachine(actor, title) {
  const figure = document.createElement("figure");
  figure.className = "machine";
  const heading = document.createElement("h3");
  heading.textContent = title;
  figure.append(heading);
  const svg = machineElement("svg", {
    viewBox: "0 0 480 296",
    role: "img",
    "data-actor": actor,
  });
  const defs = machineElement("defs");
  svg.append(defs);
  const model = machineModel(actor === "coordinator");
  const edges = model.edges.map((edge) => {
    const markerId = `arrow-${actor}-${edge.key}`;
    const marker = machineElement("marker", {
      id: markerId,
      viewBox: "0 0 8 8",
      refX: 8,
      refY: 4,
      markerWidth: 6,
      markerHeight: 6,
      orient: "auto",
      markerUnits: "userSpaceOnUse",
    });
    const arrow = machineElement("path", { d: "M0 0 L8 4 L0 8 Z" });
    marker.append(arrow);
    defs.append(marker);
    const path = machineElement("path", {
      d: edge.path,
      class: "machine-edge",
      "data-transition": edge.key,
      "marker-end": `url(#${markerId})`,
    });
    const label = machineElement(
      "text",
      {
        x: edge.x,
        y: edge.y,
        class: "machine-edge-label",
        "text-anchor": "middle",
      },
      edge.label,
    );
    svg.append(path, label);
    return { ...edge, pathElement: path, labelElement: label, arrow };
  });
  const nodes = model.states.map((state) => {
    const group = machineElement("g", {
      "data-state": state.name,
      class: "machine-state",
    });
    group.append(
      machineElement("rect", {
        x: state.x,
        y: state.y,
        width: state.w,
        height: state.h,
        rx: 9,
      }),
    );
    group.append(
      machineElement(
        "text",
        {
          x: state.x + state.w / 2,
          y: state.y + 24,
          "text-anchor": "middle",
          class: "machine-state-label",
        },
        state.name,
      ),
    );
    group.append(
      machineElement(
        "text",
        {
          x: state.x + state.w / 2,
          y: state.y + 40,
          "text-anchor": "middle",
          class: "machine-current-label",
        },
        "Current",
      ),
    );
    svg.append(group);
    return { ...state, group };
  });
  const note = document.createElement("figcaption");
  note.className = "machine-note";
  figure.append(svg, note);
  document.getElementById("machines").append(figure);
  return { svg, nodes, edges, note, title };
}
function renderStateMachines(snapshot, previous, event) {
  for (const [actor, title] of [
    ["coordinator", "Coordinator"],
    ["source-account", "Source account"],
    ["destination-account", "Destination account"],
  ]) {
    if (!machineViews.has(actor))
      machineViews.set(actor, createMachine(actor, title));
    const view = machineViews.get(actor),
      state = machineState(snapshot, actor),
      transition = machineTransition(actor, previous, snapshot, event);
    const node = snapshot.nodes.find((node) => node.id === actor);
    view.nodes.forEach((node) => {
      node.group.setAttribute("data-current", String(node.name === state));
      node.group.style.setProperty("--state-ink", `var(--${tone(node.name)})`);
      node.group.style.setProperty(
        "--state-fill",
        `var(--${tone(node.name)}-soft)`,
      );
    });
    view.edges.forEach((edge) => {
      const active = edge.key === transition,
        color = active ? `var(--${tone(edge.to)})` : "var(--muted)";
      edge.pathElement.setAttribute("data-current", String(active));
      edge.pathElement.style.stroke = color;
      edge.arrow.style.fill = color;
      edge.labelElement.setAttribute("data-current", String(active));
    });
    const activeEdge = view.edges.find((edge) => edge.key === transition);
    view.note.textContent = node?.blocked
      ? "Blocked: a timeout cannot imply ABORT."
      : actor === "coordinator" && !snapshot.coordinator_available
        ? "Unavailable · simulated coordinator loss."
        : activeEdge
          ? `${activeEdge.key.replace("-", " → ")} · ${activeEdge.label}`
          : event
            ? "No state change at this event."
            : "Initial state.";
    view.svg.setAttribute(
      "aria-label",
      `${title}: current state ${state}. ${view.note.textContent}`,
    );
  }
  document.getElementById("machine-event").textContent = event
    ? `Event #${event.sequence}: ${event.description}`
    : "Initial state · advance the replay to follow transitions.";
}
