const $ = (id) => document.getElementById(id);
let events = [],
  runs = [],
  selected = null,
  source = null,
  historySignature = "",
  detailKey = "",
  visualView = "scene";
const reducedMotion = matchMedia("(prefers-reduced-motion: reduce)");
const board = new MessageBoard(
  $("message-board"),
  $("message-caption"),
  (actor) => {
    $("machine-node").value = actor;
    setView("machine");
    $("machine-node").focus();
  },
);
const playback = new DemoPlayback({
  count: () => events.length,
  eventAt: (index) => events[index],
  active: () => runs.find((run) => run.id === selected)?.status === "running",
  animate: (event) =>
    board.flight(
      event,
      events.slice(0, playback.position),
      reducedMotion.matches,
    ),
  change: () => render(),
});
const waterfall = new AuditWaterfall(
  $("audit-scroll"),
  $("timeline"),
  $("audit-links"),
  (sequence) =>
    playback.seek(events.findIndex((e) => e.sequence === sequence) + 1),
  () => {
    if (playback.playing) playback.pause();
  },
);
reducedMotion.addEventListener("change", (event) => {
  if (event.matches) playback.reduceMotion();
});
document.addEventListener("visibilitychange", () =>
  playback.visibility(document.hidden),
);
playback.visibility(document.hidden);
function setView(view) {
  playback.pause();
  visualView = view;
  for (const name of ["scene", "machine"]) {
    const active = name === view,
      tab = $(`${name}-tab`);
    tab.setAttribute("aria-selected", String(active));
    tab.tabIndex = active ? 0 : -1;
    $(`${name}-view`).hidden = !active;
  }
  render();
}
function showDetails(event, before) {
  const key = event ? `${event.run_id}:${event.sequence}` : "initial";
  if (key === detailKey) return;
  detailKey = key;
  const root = $("detail-content");
  root.replaceChildren();
  if (!event) {
    root.textContent = "Select an observation to inspect its recorded data.";
    return;
  }
  const fields = {
    Run: event.run_id,
    Sequence: event.sequence,
    Timestamp: `${event.elapsed_ms} ms since run start`,
    Kind: event.kind,
    Actor: event.actor,
    Peer: event.peer || "—",
    "Message ID": event.message_id || "—",
    Payload: JSON.stringify(event.message),
    Description: event.description,
  };
  const dl = document.createElement("dl");
  for (const [name, value] of Object.entries(fields)) {
    const dt = document.createElement("dt"),
      dd = document.createElement("dd");
    dt.textContent = name;
    dd.textContent = value;
    dl.append(dt, dd);
  }
  root.append(dl);
  const changes = snapshotChanges(before, event.snapshot),
    list = document.createElement("ul");
  (changes.length ? changes : ["No recorded state or balance change."]).forEach(
    (change) => {
      const li = document.createElement("li");
      li.textContent = change;
      list.append(li);
    },
  );
  root.append(list);
  const raw = document.createElement("details"),
    summary = document.createElement("summary"),
    pre = document.createElement("pre");
  summary.textContent = "Full recorded event";
  pre.textContent = JSON.stringify(event, null, 2);
  raw.append(summary, pre);
  root.append(raw);
}
function render() {
  const run = runs.find((r) => r.id === selected);
  const initial =
    events[0]?.snapshot ||
    initialSnapshot(
      run?.participant_count ?? Number($("participant-count").value),
    );
  const position = playback.position,
    event = events[position - 1],
    snapshot = event?.snapshot || initial,
    before = events[position - 2]?.snapshot || initial;
  board.render(
    snapshot,
    event,
    events.slice(0, playback.pending ? position : Math.max(0, position - 1)),
    playback.pending,
  );
  waterfall.render(events.slice(0, position), snapshot);
  const actorSignature = actorIds(snapshot).join(",");
  if ($("machine-node").dataset.actors !== actorSignature) {
    const picker = $("machine-node"),
      selectedActor = picker.value;
    picker.dataset.actors = actorSignature;
    picker.replaceChildren();
    actorIds(snapshot).forEach((actor) =>
      picker.add(new Option(actorName(actor), actor)),
    );
    picker.value = actorIds(snapshot).includes(selectedActor)
      ? selectedActor
      : "coordinator";
  }
  if (visualView === "machine")
    renderStateMachines(snapshot, before, event, $("machine-node").value);
  showDetails(event, before);
  $("event-label").textContent = event
    ? `Observation ${event.sequence} · ${observationLabel(event, events.slice(0, position - 1))}`
    : "Initial state";
  $("demo-backlog").textContent = `${events.length - position} waiting to play`;
  $("mode").textContent = playback.playing
    ? playback.following
      ? "Following"
      : "Playing"
    : playback.following
      ? "Caught up"
      : "Paused";
  $("play").textContent = playback.playing ? "Pause" : "Play";
  $("position").textContent = `${position} / ${events.length}`;
  $("scrubber").max = events.length;
  $("scrubber").value = position;
  $("previous").disabled = position === 0;
  $("next").disabled = position === events.length;
  $("play").disabled = !events.length;
  $("live").disabled = !selected;
  $("scrubber").disabled = !events.length;
  $("empty").hidden = position > 0;
  $("summary").textContent = selected
    ? `Run ${selected} · ${events.length} collected`
    : "No run selected";
  if (run) {
    $("connection").textContent = {
      running: "Recording",
      finished: "Recording complete",
      failed: "Recording failed",
    }[run.status];
    $("connection").className =
      `badge ${run.status === "failed" ? "red" : "neutral"}`;
  } else $("connection").textContent = "Ready";
}
async function request(url, options) {
  const response = await fetch(url, options),
    body = await response.json();
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
  $("run").title = $("run").disabled
    ? "Another scenario is recording"
    : "Run the selected scenario";
  const signature = JSON.stringify(runs.map((r) => [r.id, r.status]));
  if (signature === historySignature) return;
  historySignature = signature;
  const history = $("history");
  history.replaceChildren();
  history.add(
    new Option(runs.length ? "Choose a recorded run" : "No runs yet", ""),
  );
  for (const run of runs)
    history.add(
      new Option(
        `Run ${run.id} · ${run.scenario} · ${run.participant_count} participants`,
        run.id,
      ),
    );
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
  source?.close();
  source = null;
  selected = id;
  events = [];
  detailKey = "";
  playback.seek(0);
  setView("scene");
  $("history").value = id || "";
  error("");
  $("event-details").open = false;
  if (!id) return;
  $("connection").textContent = "Loading…";
  try {
    const run = await request(`/api/runs/${id}`);
    if (selected !== id) return;
    events = run.events;
    runs = runs.map((r) => (r.id === id ? { ...run, events: [] } : r));
    render();
    if (live) playback.play(true);
    if (run.status === "running") {
      const stream = new EventSource(`/api/runs/${id}/events`);
      source = stream;
      stream.onmessage = (message) => {
        if (selected === id) accept(JSON.parse(message.data));
      };
      stream.onerror = async () => {
        if (selected !== id) return;
        try {
          const current = await request(`/api/runs/${id}`);
          if (selected !== id) return;
          current.events.forEach(accept);
          if (current.status !== "running") {
            stream.close();
            await refreshRuns();
          } else $("connection").textContent = "Reconnecting…";
        } catch (e) {
          error(e.message);
        }
      };
    }
  } catch (e) {
    if (selected === id) error(e.message);
  }
}
$("run").onclick = async () => {
  error("");
  $("run").disabled = true;
  try {
    const run = await request("/api/runs", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({
        scenario: $("scenario").value,
        participant_count: Number($("participant-count").value),
      }),
    });
    await refreshRuns();
    await selectRun(run.id, true);
  } catch (e) {
    error(e.message);
    await refreshRuns().catch(() => {});
  }
};
$("history").onchange = () => selectRun(Number($("history").value) || null);
$("participant-count").oninput = () => {
  $("participant-count-value").value = $("participant-count").value;
  if (!selected) render();
};
function play(follow = false) {
  if (visualView !== "scene") setView("scene");
  playback.play(follow);
}
$("play").onclick = () => (playback.playing ? playback.pause() : play());
$("previous").onclick = () => playback.seek(playback.position - 1);
$("next").onclick = () => playback.seek(playback.position + 1);
$("scrubber").oninput = () => playback.seek(Number($("scrubber").value));
$("live").onclick = () => play(true);
$("speed").onchange = () => playback.speed(Number($("speed").value));
$("machine-node").onchange = () => {
  playback.pause();
  render();
};
for (const name of ["scene", "machine"]) {
  $(`${name}-tab`).onclick = () => setView(name);
  $(`${name}-tab`).onkeydown = (e) => {
    if (["ArrowLeft", "ArrowRight", "Home", "End"].includes(e.key)) {
      e.preventDefault();
      const next =
        e.key === "Home"
          ? "scene"
          : e.key === "End"
            ? "machine"
            : name === "scene"
              ? "machine"
              : "scene";
      setView(next);
      $(`${next}-tab`).focus();
    }
  };
}
render();
refreshRuns().catch((e) => error(e.message));
setInterval(() => refreshRuns().catch((e) => error(e.message)), 1000);
