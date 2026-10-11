const $ = (id) => document.getElementById(id);
let events = [],
  position = 0,
  source = null,
  selected = null,
  runs = [],
  renderedTrace = "",
  historySignature = "",
  viewedPosition = null;
const initial = {
  coordinator: "Collecting",
  coordinator_available: true,
  votes_received: 0,
  nodes: [
    {
      id: "source-account",
      state: "Working",
      vote: null,
      balance: 100,
      blocked: false,
    },
    {
      id: "destination-account",
      state: "Working",
      vote: null,
      balance: 20,
      blocked: false,
    },
  ],
};
const tone = (state) =>
  ({
    Prepared: "amber",
    Committed: "green",
    Commit: "green",
    Aborted: "red",
    Abort: "red",
  })[state] || "blue";
const reducedMotion = window.matchMedia("(prefers-reduced-motion: reduce)");
const messageBoard = new MessageBoard($("message-board"), $("message-caption"));
const playback = new DemoPlayback({
  count: () => events.length,
  eventAt: (index) => events[index],
  active: () => runs.find((run) => run.id === selected)?.status === "running",
  animate: (event) =>
    messageBoard.flight(
      event,
      events.slice(0, position),
      reducedMotion.matches,
    ),
  change: () => {
    position = playback.position;
    render();
  },
});
reducedMotion.addEventListener("change", (event) => {
  if (event.matches) playback.reduceMotion();
});
let demoVisible = false;
const updateVisibility = () =>
  playback.visibility(document.hidden || !demoVisible);
const visibilityObserver = new IntersectionObserver((entries) => {
  demoVisible = entries[0].isIntersecting;
  updateVisibility();
});
visibilityObserver.observe($("message-board"));
document.addEventListener("visibilitychange", updateVisibility);
updateVisibility();
function badge(text, color) {
  const el = document.createElement("span");
  el.className = `badge ${color}`;
  el.textContent = text;
  return el;
}
function card(id, title, state, fields, note) {
  const root = $(id);
  root.replaceChildren();
  const heading = document.createElement("h2");
  heading.textContent = title;
  root.append(heading, badge(state, tone(state)));
  const dl = document.createElement("dl");
  for (const [label, value] of fields) {
    const dt = document.createElement("dt"),
      dd = document.createElement("dd");
    dt.textContent = label;
    dd.textContent = value;
    dl.append(dt, dd);
  }
  root.append(dl);
  if (note) {
    const p = document.createElement("p");
    p.className = "note";
    p.textContent = note;
    root.append(p);
  }
}
function render() {
  const snapshot = position ? events[position - 1].snapshot : initial;
  messageBoard.render(
    snapshot,
    position ? events[position - 1] : null,
    events.slice(0, Math.max(0, position - 1)),
    playback.pending,
  );
  $("demo-backlog").textContent = `${events.length - position} buffered`;
  renderStateMachines(
    snapshot,
    position > 1 ? events[position - 2].snapshot : initial,
    position ? events[position - 1] : null,
  );
  const decision =
    typeof snapshot.coordinator === "object"
      ? snapshot.coordinator.Decided
      : "Undecided";
  card(
    "coordinator",
    "Coordinator",
    decision === "Undecided" ? "Collecting" : decision,
    [
      ["Decision", decision],
      ["Votes received", `${snapshot.votes_received} / 2`],
    ],
    snapshot.coordinator_available
      ? "Decision is separate from delivery."
      : "Coordinator unavailable · simulated",
  );
  snapshot.nodes.forEach((node, i) =>
    card(
      i ? "destination" : "source",
      i ? "Destination account" : "Source account",
      node.state,
      [
        ["Vote", node.vote || "—"],
        ["Balance", node.balance],
      ],
      node.blocked
        ? "Blocked · waiting for a decision"
        : node.state === "Prepared"
          ? "Prepared · awaiting decision"
          : null,
    ),
  );
  $("mode").textContent = playback.following
    ? playback.playing
      ? "Following · paced"
      : "Caught up"
    : playback.playing
      ? "Playing replay"
      : "Replay paused";
  $("play").textContent = playback.playing ? "Pause" : "Play";
  $("position").textContent = `${position} / ${events.length}`;
  $("scrubber").max = events.length;
  $("scrubber").value = position;
  $("previous").disabled = position === 0;
  $("next").disabled = position === events.length;
  $("play").disabled = !events.length;
  $("live").disabled = !selected;
  const run = runs.find((r) => r.id === selected);
  $("summary").textContent = run
    ? `${run.scenario} · ${run.status} · ${events.length} observations`
    : "Choose a scenario to see the protocol unfold.";
  $("empty").hidden = position > 0;
  const traceKey = `${selected}:${position}:${events.length}`;
  if (traceKey === renderedTrace) return;
  renderedTrace = traceKey;
  const moved = position !== viewedPosition;
  viewedPosition = position;
  const timeline = $("timeline"),
    oldScroll = timeline.scrollTop;
  timeline.replaceChildren();
  events.slice(0, position).forEach((event, index) => {
    const li = document.createElement("li"),
      box = document.createElement("div");
    const state = event.snapshot.nodes.find((n) => n.id === event.actor)?.state;
    const color =
      event.kind === "FaultInjected" || event.kind === "TimeoutObserved"
        ? "amber"
        : event.kind === "Failed"
          ? "red"
          : tone(state);
    box.className = `event ${color}${index === position - 1 ? " current" : ""}`;
    box.style.gridColumn =
      event.actor === "source-account"
        ? "2"
        : event.actor === "destination-account"
          ? "3"
          : "1";
    if (event.peer) {
      const message = document.createElement("div");
      message.className = "message";
      message.textContent = `${event.actor === "coordinator" ? "Coordinator" : event.actor === "source-account" ? "Source" : "Destination"} ↔ ${event.peer === "coordinator" ? "Coordinator" : event.peer === "source-account" ? "Source" : "Destination"}`;
      box.append(message);
    }
    const description = document.createElement("div");
    description.textContent = event.description;
    const metadata = document.createElement("small");
    metadata.textContent = `#${event.sequence} · ${(event.elapsed_ms / 1000).toFixed(2)}s · ${event.kind}${event.message_id ? " · " + event.message_id : ""}`;
    box.append(description, metadata);
    li.append(box);
    timeline.append(li);
  });
  timeline.scrollTop =
    playback.following || playback.playing || moved
      ? timeline.scrollHeight
      : oldScroll;
}
async function request(url, options) {
  const response = await fetch(url, options);
  const body = await response.json();
  if (!response.ok)
    throw new Error(body?.error || `Request failed (${response.status})`);
  return body;
}
function error(message) {
  $("error").textContent = message;
  $("error").hidden = !message;
}
async function refreshRuns() {
  runs = await request("/api/runs");
  $("run").disabled = runs.some((r) => r.status === "running");
  const history = $("history"),
    signature = JSON.stringify(runs.map((r) => [r.id, r.status]));
  if (signature !== historySignature) {
    historySignature = signature;
    history.replaceChildren();
    if (!runs.length) {
      const option = new Option("No runs yet", "");
      history.add(option);
    }
    for (const run of runs)
      history.add(
        new Option(`#${run.id} · ${run.scenario} · ${run.status}`, run.id),
      );
  }
  history.value = selected || "";
  render();
  playback.wake();
}
function accept(event) {
  if (
    event.run_id !== selected ||
    events.some((e) => e.sequence === event.sequence)
  )
    return;
  events.push(event);
  events.sort((a, b) => a.sequence - b.sequence);
  render();
  playback.wake();
}
async function selectRun(id, live = false) {
  if (source) source.close();
  source = null;
  selected = id;
  $("history").value = id;
  events = [];
  playback.seek(0);
  error("");
  render();
  try {
    const run = await request(`/api/runs/${id}`);
    if (selected !== id) return;
    events = run.events;
    render();
    if (live) {
      $("message-board").scrollIntoView({
        block: "center",
        behavior: "instant",
      });
      playback.play(true);
    }
    if (run.status === "running") {
      source = new EventSource(`/api/runs/${id}/events`);
      $("connection").textContent = "Connected";
      source.onmessage = (message) => {
        accept(JSON.parse(message.data));
      };
      source.onerror = async () => {
        if (selected !== id) return;
        try {
          const current = await request(`/api/runs/${id}`);
          if (selected !== id) return;
          current.events.forEach(accept);
          if (current.status !== "running") {
            source?.close();
            $("connection").textContent = "Run complete";
            await refreshRuns();
          } else $("connection").textContent = "Reconnecting…";
        } catch (e) {
          error(e.message);
        }
      };
    } else $("connection").textContent = "Recorded run";
  } catch (e) {
    error(e.message);
  }
}
$("run").onclick = async () => {
  error("");
  $("run").disabled = true;
  try {
    const result = await request("/api/runs", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ scenario: $("scenario").value }),
    });
    await refreshRuns();
    await selectRun(result.id, true);
  } catch (e) {
    error(e.message);
    await refreshRuns();
  }
};
$("history").onchange = () => {
  if ($("history").value) selectRun(Number($("history").value));
};
$("play").onclick = () => {
  if (playback.playing) playback.pause();
  else {
    $("message-board").scrollIntoView({ block: "center", behavior: "instant" });
    playback.play(false);
  }
};
$("previous").onclick = () => playback.seek(position - 1);
$("next").onclick = () => playback.seek(position + 1);
$("scrubber").oninput = () => playback.seek(Number($("scrubber").value));
$("live").onclick = () => {
  $("message-board").scrollIntoView({ block: "center", behavior: "instant" });
  playback.play(true);
};
$("speed").onchange = () => playback.speed(Number($("speed").value));
render();
refreshRuns().catch((e) => error(e.message));
setInterval(() => refreshRuns().catch((e) => error(e.message)), 1000);
