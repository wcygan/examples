'use strict';
const $=id=>document.getElementById(id);
const {Trace,Player,initial,label,normal,edges,route,frames,short,payloadPoint,explanation,FLIGHT_MS}=RaftMotion;
const session={trace:new Trace(3),id:null,status:'ready',selection:0,stream:null,historyKey:''};
const reduced=matchMedia('(prefers-reduced-motion: reduce)');
let flightRoutes=new Map(), geometry=new Map(), nodeViews=new Map(), renderedCount=0, selectedRow=null, selectedSequence=null;
let links=new Map(), seenNodes=new Map(), positioned=0, layoutKey='';
const escape=value=>String(value).replace(/[&<>"']/g,c=>({'&':'&amp;','<':'&lt;','>':'&gt;','"':'&quot;',"'":'&#39;'}[c]));
const same=(a,b)=>JSON.stringify(a)===JSON.stringify(b);
const player=new Player({trace:session.trace,active:()=>session.status==='running',animate:animateFlight,retarget:retargetFlight,change:render});
player.reduced=reduced.matches;
function svg(tag,attributes) {
  const element=document.createElementNS('http://www.w3.org/2000/svg',tag);
  for(const [key,value] of Object.entries(attributes))element.setAttribute(key,value);
  return element;
}
function layout() {
  const count=session.trace.count, narrow=matchMedia('(max-width:600px)').matches;
  const positions=count===3?[[50,25],[23,75],[77,75]]:count===4?[[25,25],[75,25],[25,75],[75,75]]
    :narrow?[[50,16],[25,50],[75,50],[25,84],[75,84]]:[[25,25],[75,25],[16.5,75],[50,75],[83.5,75]];
  const key=`${count}:${narrow}`;
  if(key!==layoutKey) {
    layoutKey=key; $('node-cards').replaceChildren();nodeViews.clear();seenNodes.clear();
    positions.forEach(([x,y],i)=>{
      const card=document.createElement('article');card.id=`node${i+1}`;card.className='node';
      Object.assign(card.style,{left:`${x}%`,top:`${y}%`,width:`${count===5&&!narrow?31:42}%`});
      card.innerHTML=`<h3>Node ${i+1}</h3><p class="role"></p><p class="term"></p><p class="indexes"></p><p class="value"></p><div class="log-strip" aria-label="Observed log entries"></div>`;
      $('node-cards').append(card);
      nodeViews.set(i+1,{card,role:card.querySelector('.role'),term:card.querySelector('.term'),
        indexes:card.querySelector('.indexes'),value:card.querySelector('.value'),log:card.querySelector('.log-strip'),entries:new Map()});
    });
    $('lanes').style.gridTemplateColumns=`repeat(${count},1fr)`;
    $('lanes').innerHTML=positions.map((_,i)=>`<span>Node ${i+1}</span>`).join('');
  }
  const graphic=document.querySelector('.graphic');graphic.dataset.count=count;
}
function measure() {
  const root=document.querySelector('.graphic'), bounds=root.getBoundingClientRect();geometry.clear();flightRoutes.clear();
  for(const [id,{card}] of nodeViews) {
    const box=card.getBoundingClientRect();geometry.set(id,{x:box.left-bounds.left,y:box.top-bounds.top,width:box.width,height:box.height});
  }
  root.querySelector('svg').setAttribute('viewBox',`0 0 ${bounds.width} ${bounds.height}`);
  const wires=[];
  for(const [id,a] of geometry)for(const [other,b] of geometry)if(other>id) {
    const [start,end]=edges(a,b);wires.push(`M${start.join(' ')} L${end.join(' ')}`);
  }
  $('routes').setAttribute('d',wires.join(' '));
}
function updateNodes(snapshot,highlight=false) {
  layout();let resized=false;
  for(const [key,state] of Object.entries(snapshot)) {
    const id=Number(key), view=nodeViews.get(id), before=seenNodes.get(id);
    if(same(before,state))continue;
    view.card.className=`node${state.role==='Leader'?' leader':''}${state.isolated?' blocked':''}${!state.available?' stopped':''}`;
    view.role.textContent=`${!state.available?'Stopped':state.role}${state.isolated?' · Isolated':''}`;
    view.term.textContent=`Term ${state.term} · Leader ${state.leader??'—'}`;
    view.indexes.textContent=`Commit ${state.committed??'—'} · Applied ${state.applied??'—'}`;
    const value=state.values.lesson??'—';
    if(view.value.textContent!==`lesson = ${value}`) {
      view.value.textContent=`lesson = ${value}`;
      if(highlight===true && before && !player.reduced)view.value.animate([{background:'#dcefe1'},{background:'transparent'}],{duration:350});
    }
    const entries=Object.values(state.log||{}).filter(normal).slice(-3), present=new Set();
    for(const entry of entries) {
      const index=entry.log_id.index;present.add(index);
      let chip=view.entries.get(index);
      if(!chip){chip=document.createElement('span');view.log.append(chip);view.entries.set(index,chip);}
      const status=state.applied!==null&&state.applied>=index?'Applied':state.committed!==null&&state.committed>=index?'Committed':'Recorded';
      chip.className=`log-entry ${status.toLowerCase()}`;chip.textContent=`${index} · ${status}`;
      chip.title=`Entry ${index}: ${entry.payload.Normal.key} = ${entry.payload.Normal.value}`;
    }
    for(const [index,chip] of view.entries)if(!present.has(index)){chip.remove();view.entries.delete(index);}
    if(!entries.length && !view.entries.size)view.log.dataset.empty='No application entries';else delete view.log.dataset.empty;
    seenNodes.set(id,structuredClone(state));resized=true;
  }
  if(resized || !geometry.size || highlight==='layout')measure();
}
function flightRoute(leg) {
  const root=document.querySelector('.graphic');
  const key=`${leg.from}:${leg.to}`;
  if(!flightRoutes.has(key))flightRoutes.set(key,route(geometry.get(leg.from),geometry.get(leg.to),[...geometry.values()],root.clientWidth,root.clientHeight));
  return flightRoutes.get(key);
}
function showFlight(leg) {
  if(!leg){$('active-route').setAttribute('hidden','');$('flight-label').hidden=true;$('packet').setAttribute('visibility','hidden');return;}
  const points=flightRoute(leg),root=document.querySelector('.graphic'),a=payloadPoint(points[0],root.clientWidth,root.clientHeight);
  $('active-route').removeAttribute('hidden');$('active-route').setAttribute('d',points.map((p,i)=>`${i?'L':'M'}${p.join(' ')}`).join(' '));
  $('flight-label').textContent=`${leg.text} · Node ${leg.from} → Node ${leg.to}`;$('flight-label').hidden=false;
  $('payload-title').textContent=short(leg.payload.title);$('payload-detail').textContent=short(leg.payload.detail);
  $('payload-full').textContent=`${leg.payload.title}: ${leg.payload.detail}`;
  $('packet').style.transform=`translate(${a[0]}px,${a[1]}px)`;
}
function payloadFrames(leg) {
  const root=document.querySelector('.graphic');
  return frames(flightRoute(leg),{width:root.clientWidth,height:root.clientHeight});
}
function animateFlight(leg,duration=FLIGHT_MS) {
  showFlight(leg);
  $('packet').setAttribute('visibility','visible');
  return $('packet').animate(payloadFrames(leg),
    {duration,easing:'linear',fill:'both'});
}
function retargetFlight(leg,animation) {
  showFlight(leg);
  animation.effect.setKeyframes(payloadFrames(leg));
}
function progress(snapshot) {
  const step=player.pending??player.current;
  $('phase').textContent=step?.phase??(player.cursor?'Inspect':session.id?'Initial':'Ready');
  const entries=Object.values(snapshot).flatMap(n=>Object.values(n.log||{})).filter(normal);
  const latest=entries.sort((a,b)=>b.log_id.index-a.log_id.index)[0], majority=Math.floor(session.trace.count/2)+1;
  if(latest) {
    const replicated=Object.values(snapshot).filter(n=>same(n.log?.[latest.log_id.index],latest)).length;
    const committed=Object.values(snapshot).some(n=>n.committed!==null&&n.committed>=latest.log_id.index&&same(n.log[latest.log_id.index],latest));
    $('quorum').textContent=`Entry ${latest.log_id.index} · ${replicated}/${session.trace.count} recorded · Majority ${majority}${committed?' · Committed':''}`;
  } else $('quorum').textContent=`Majority: ${majority} of ${session.trace.count}`;
}
function lane(actor) {return (actor-.5)*100/session.trace.count;}
function resetWaterfall() {
  $('rows').innerHTML='<div class="entries"><svg class="connection" aria-hidden="true"></svg></div>';
  renderedCount=0;selectedRow=null;selectedSequence=null;positioned=0;links.clear();
}
function waterfall(reason) {
  if(player.cursor<renderedCount)resetWaterfall();
  const entries=$('rows').firstElementChild, drawing=entries.querySelector('svg');
  for(let index=renderedCount;index<player.cursor;index++) {
    const event=session.trace.events[index], button=document.createElement('button');
    button.dataset.sequence=event.sequence;
    button.setAttribute('aria-label',`Observation ${event.sequence}: Node ${event.actor}, ${label(event)}`);
    button.innerHTML=`<span class="number">${event.sequence}</span><span class="marker ${event.kind}" style="left:${lane(event.actor)}%;max-width:${100/session.trace.count-1}%">${escape(label(event))}</span>`;
    entries.append(button);
    const leg=session.trace.leg(event);
    if(leg) {
      const line=svg('line',{});drawing.append(line);links.set(event.sequence,{line,leg});
    }
  }
  const previous=renderedCount;renderedCount=player.cursor;
  if(previous!==renderedCount || reason==='layout')positionLinks();
  if(selectedSequence!==player.cursor) {
    selectedRow?.classList.remove('selected');selectedRow?.removeAttribute('aria-current');
    selectedRow=entries.querySelector(`[data-sequence="${player.cursor}"]`);selectedSequence=player.cursor;
    selectedRow?.classList.add('selected');selectedRow?.setAttribute('aria-current','step');
    for(const [seq,{line,leg}] of links)line.classList.toggle('selected',seq===player.cursor || leg.send.sequence===player.cursor);
    if(selectedRow) {
      const panel=$('rows'), bottom=selectedRow.offsetTop+selectedRow.offsetHeight;
      if(selectedRow.offsetTop<panel.scrollTop)panel.scrollTop=selectedRow.offsetTop;
      else if(bottom>panel.scrollTop+panel.clientHeight)panel.scrollTop=bottom-panel.clientHeight;
    }
  }
}
function positionLinks() {
  const entries=$('rows').firstElementChild, drawing=entries.querySelector('svg'), width=entries.clientWidth;
  drawing.setAttribute('viewBox',`0 0 ${width} ${Math.max(entries.offsetHeight,1)}`);drawing.style.height=`${entries.offsetHeight}px`;
  drawing.querySelectorAll('.lifeline').forEach(e=>e.remove());
  for(let id=1;id<=session.trace.count;id++)drawing.prepend(svg('line',{class:'lifeline',x1:width*lane(id)/100,x2:width*lane(id)/100,y1:0,y2:entries.offsetHeight}));
  for(const [seq,{line,leg}] of links) {
    if(seq<=positioned && width===Number(drawing.dataset.width))continue;
    const a=entries.querySelector(`[data-sequence="${leg.send.sequence}"]`),b=entries.querySelector(`[data-sequence="${seq}"]`);
    const attributes={x1:width*lane(leg.from)/100,x2:width*lane(leg.to)/100,y1:a.offsetTop+a.offsetHeight/2,y2:b.offsetTop+b.offsetHeight/2};
    for(const [key,value] of Object.entries(attributes))line.setAttribute(key,value);
  }
  positioned=renderedCount;drawing.dataset.width=width;
}
function render(reason='controls') {
  const trace=session.trace, event=trace.events[player.cursor-1], snapshot=trace.snapshot(player.cursor);
  updateNodes(snapshot,reason==='layout'?'layout':reason==='arrive');progress(snapshot);
  const step=player.pending??player.current;
  if(!player.pending?.flight)showFlight(null);
  $('explanation').hidden=false;
  $('observation').textContent=player.pending?.flight?`${step.phase} · in flight${!player.playing?' · paused':''}`
    :event?`Observation ${event.sequence} · Node ${event.actor}`:'Initial cluster';
  $('explanation').textContent=player.pending?.flight
    ?explanation(player.pending.event,trace.snapshot(player.pending.sequence-1),player.pending.flight,player.pending.phase,true)
    :(player.current?.sequence===player.cursor?player.current.explanation:null)??trace.explain(player.cursor);
  if(reason!=='collected')$('details').textContent=event?JSON.stringify(event,null,2):'No observation selected';
  $('speed-value').textContent=`${player.rate.toFixed(2)}×`;
  $('speed').setAttribute('aria-valuetext',`${player.rate.toFixed(2)} times speed`);
  $('position').textContent=`${player.cursor} / ${trace.events.length}`;$('scrubber').max=trace.events.length;$('scrubber').value=player.cursor;
  $('waiting').textContent=`${trace.events.length-player.cursor} waiting`;
  $('recording').textContent=session.status==='running'?'Recording':session.status==='failed'?'Recording failed':session.id?'Recording complete':'Ready';
  $('play').textContent=player.playing?'Pause':'Play';$('previous').disabled=player.cursor===0;$('next').disabled=player.cursor===trace.events.length;
  waterfall(reason);
}
async function history() {
  const response=await fetch('/api/runs');if(!response.ok)throw Error('Could not load history');const runs=await response.json();
  const key=JSON.stringify(runs.map(r=>[r.id,r.scenario,r.node_count,r.status]));
  if(key!==session.historyKey)$('history').innerHTML='<option value="">Choose a recording</option>'+runs.map(r=>`<option value="${r.id}">Run ${r.id} · ${r.node_count} nodes · ${escape(r.scenario)}</option>`).join('');
  session.historyKey=key;$('history').value=session.id??'';$('run').disabled=runs.some(r=>r.status==='running');
}
function ingest(events) {
  for(const event of events)session.trace.append(event);
  const last=session.trace.events.at(-1);
  if(['finished','failed'].includes(last?.kind)){session.status=last.kind;session.stream?.close();history().catch(showError);}
  render('collected');player.wake();
}
async function select(id,follow=false) {
  $('error').textContent='';
  const selection=++session.selection;player.seek(0);session.stream?.close();session.stream=null;session.id=Number(id);
  const response=await fetch(`/api/runs/${id}`);if(!response.ok)throw Error('Recording unavailable');const run=await response.json();
  if(selection!==session.selection)return;
  session.trace=new Trace(run.node_count);session.status=run.status;
  layoutKey='';resetWaterfall();player.load(session.trace);ingest(run.events);
  if(run.status==='running') {
    const stream=new EventSource(`/api/runs/${id}/events`);session.stream=stream;
    stream.onmessage=({data})=>{if(session.stream===stream)try{ingest([JSON.parse(data)]);}catch(e){showError(e);player.pause();}};
    stream.onerror=async()=>{
      try {
        const response=await fetch(`/api/runs/${id}`),latest=await response.json();
        if(session.stream!==stream || selection!==session.selection)return;
        ingest(latest.events);if(latest.status!=='running'){session.status=latest.status;stream.close();render();}
      } catch(e){showError(e);}
    };
  }
  if(follow)player.play(true);
}
function showError(error) {$('error').textContent=error.message;}
$('run').onclick=async()=>{
  $('error').textContent='';$('run').disabled=true;
  try {
    const response=await fetch('/api/runs',{method:'POST',headers:{'content-type':'application/json'},body:JSON.stringify({scenario:$('scenario').value,node_count:Number($('node-count').value)})});
    const result=await response.json();if(!response.ok)throw Error(result.error);await select(result.id,true);await history();
  }catch(error){showError(error);$('run').disabled=false;}
};
$('history').onchange=()=>{if($('history').value)select($('history').value).catch(showError);};
$('node-count').oninput=()=>{
  $('node-count-value').textContent=$('node-count').value;
  if(!session.id){session.trace=new Trace(Number($('node-count').value));player.trace=session.trace;layoutKey='';resetWaterfall();render();}
};
$('play').onclick=()=>player.playing?player.pause():player.play();$('follow').onclick=()=>player.play(true);
$('previous').onclick=()=>player.seek(player.cursor-1);$('next').onclick=()=>player.seek(player.cursor+1);
$('scrubber').oninput=()=>player.seek(Number($('scrubber').value));$('speed').oninput=()=>player.speed(Number($('speed').value));
$('mode').onchange=()=>{const playing=player.playing,following=player.follow;player.seek(player.cursor);player.mode=$('mode').value;if(playing)player.play(following);};
$('rows').onclick=event=>{
  const row=event.target.closest('button[data-sequence]');if(!row)return;
  const sequence=Number(row.dataset.sequence);player.seek(sequence);$('rows').querySelector(`[data-sequence="${sequence}"]`)?.focus({preventScroll:true});
};
for(const kind of ['wheel','touchstart'])$('rows').addEventListener(kind,()=>player.pause(),{passive:true});
$('rows').addEventListener('keydown',e=>{if(['ArrowUp','ArrowDown','PageUp','PageDown','Home','End'].includes(e.key))player.pause();});
document.querySelector('details').addEventListener('toggle',e=>{if(e.target.open)player.pause();});
reduced.addEventListener('change',e=>player.reduce(e.matches));
document.addEventListener('visibilitychange',()=>player.visibility(document.hidden));
window.addEventListener('resize',()=>{layoutKey='';positioned=0;player.relayout();});
resetWaterfall();render();history().catch(showError);setInterval(()=>history().catch(showError),1000);
