/* Recorded observations remain authoritative; teaching steps are a projection. */
const RaftMotion = (() => {
  const FLIGHT_MS = 900, HOLD_MS = 650, TEACHING_FLIGHT_MS = 1800;
  function readingPause(text) {
    return Math.min(7000, Math.max(3200, text.trim().split(/\s+/).length * 200 + 700));
  }
  const same = (a, b) => JSON.stringify(a) === JSON.stringify(b);
  const normal = entry => entry?.payload?.Normal;
  const term = payload => payload?.vote?.leader_id?.term;
  const logKey = entry => JSON.stringify(entry?.log_id);
  const valueLabel = entry => {
    const command = normal(entry);
    return command ? `${command.key} = ${command.value}` : 'protocol entry';
  };
  function initial(count) {
    return Object.fromEntries(Array.from({length: count}, (_, i) => [i + 1, {
      role: 'Learner', term: 0, leader: null, available: true, isolated: false,
      committed: null, applied: null, values: {}, log: {},
    }]));
  }
  function label(event) {
    const boundary = {sent:'sent', received:'received', response_sent:'sent', response_received:'received'}[event.kind];
    return event.description + (boundary ? ` · ${boundary}` : '');
  }
  const short = (value, limit=24) => {
    const chars=Array.from(String(value));
    return chars.length>limit?chars.slice(0,limit-1).join('')+'…':chars.join('');
  };
  function payloadSummary(send) {
    const p=send.payload ?? {}, round=term(p);
    switch(send.description) {
      case 'RequestVote': return {title:'Request vote', detail:`Term ${round} · ${p.last_log_id ? `log #${p.last_log_id.index}` : 'empty log'}`};
      case 'RequestVote response': {
        const granted=p.Ok?.vote_granted;
        return {title:typeof granted==='boolean'?`Vote ${granted?'granted':'denied'}`:'Vote response',
          detail:p.Err?'RPC error':`Current term ${term(p.Ok) ?? '—'}`};
      }
      case 'AppendEntries': {
        const entries=p.entries || [], writes=entries.filter(normal);
        if(writes.length) return {title:writes.length===1?'Replicate entry':`Replicate ${writes.length} entries`,
          detail:`#${writes[0].log_id.index} · ${valueLabel(writes[0])}`};
        if(entries.length) return {title:'Replicate metadata',detail:entries.some(e=>e.payload?.Membership)?'Cluster membership':'Election marker'};
        return p.leader_commit ? {title:'Announce commit',detail:`Committed through #${p.leader_commit.index}`}
          : {title:'Heartbeat',detail:`Leader · term ${round}`};
      }
      case 'AppendEntries response': return {title:p.Ok==='Success'?'Append accepted':p.Ok==='Conflict'?'Log mismatch':'Append response',
        detail:p.Err?'RPC error':p.Ok==='Success'?'Log matches leader':p.Ok?.PartialSuccess?`Matched through #${p.Ok.PartialSuccess.index}`:p.Ok?.HigherVote?`Higher term ${p.Ok.HigherVote.leader_id.term}`:'Leader must check reply'};
      case 'InstallSnapshot': return {title:'Install snapshot',detail:p.meta?.last_log_id?`Through entry #${p.meta.last_log_id.index}`:'State snapshot chunk'};
      case 'InstallSnapshot response': return {title:'Snapshot response',detail:p.Err?'RPC error':'Snapshot reply'};
      default: return {title:'Protocol message',detail:send.description};
    }
  }
  function explanation(event,before,leg=null,phase='',inFlight=false) {
    if(!event)return 'Choose a scenario to follow an election and a replicated write.';
    const node=`Node ${event.actor}`, state=event.nodes[event.actor], p=event.payload;
    const count=Object.keys(event.nodes).length, majority=Math.floor(count/2)+1;
    if(leg) {
      const request=leg.send, body=request.payload;
      const movement=inFlight?`Node ${leg.from} → Node ${leg.to}`:`Node ${leg.to} received`;
      if(request.description==='RequestVote')
        return `${movement}: a vote request for term ${term(body)}. A candidate needs ${majority} votes; each node can vote for only one candidate per term.`;
      if(request.description==='RequestVote response')
        return `${movement}: ${body.Ok?.vote_granted===true?'a granted vote':body.Ok?.vote_granted===false?'a denied vote':'a vote response'}. Election votes choose a leader; they do not commit a client write.`;
      if(request.description==='AppendEntries') {
        const writes=(body.entries || []).filter(normal);
        if(writes.length) return `${movement}: entry #${writes[0].log_id.index}, ${short(valueLabel(writes[0]),36)}. Receiving the request does not yet mean the entry is recorded, committed, or applied.`;
        if(body.entries?.length) return `${movement}: cluster metadata. Raft replicates membership and election markers as well as application writes.`;
        if(body.leader_commit) return `${movement}: commit index ${body.leader_commit.index}. This tells the follower which entries are committed; applying their values is a separate step.`;
        return `${movement}: a heartbeat. It maintains contact with the leader without carrying a new application write.`;
      }
      if(request.description==='AppendEntries response')
        return `${movement}: an append response. The leader checks this reply to track replication; a reply alone does not establish majority commitment.`;
      return `${movement}: ${request.description}. This protocol exchange supports recovery; the recorded state changes remain separate observations.`;
    }
    switch(event.kind) {
      case 'started': return `${count} nodes begin a Raft cluster. A majority of ${majority} is required to elect a leader and confirm a new write.`;
      case 'metrics': return state.role==='Candidate'
        ? `${node} ${before[event.actor]?.role!==state.role || before[event.actor]?.term!==state.term?'started an election':'remains a candidate'} in term ${state.term}. A majority of votes is required before it can lead.`
        : state.role==='Leader' ? `${node} ${before[event.actor]?.role!==state.role || before[event.actor]?.term!==state.term?'became':'remains'} leader in term ${state.term}. It still needs majority replication before committing each write.`
        : `${node} is ${state.role} in term ${state.term}. Role changes describe leadership, separately from log commitment and application.`;
      case 'client_write': return `The client proposed ${short(p.key+' = '+p.value,36)} to ${node}. A proposal is not success yet; the leader must replicate it to a majority.`;
      case 'client_acknowledged': return `The client received success for entry #${p.log_id.index}. The write is committed and applied at the leader; followers may still be catching up.`;
      case 'client_timeout': return 'The client deadline expired without a success response. A timeout alone does not prove the write was rejected; its eventual commit status must be checked.';
      case 'fault': return p.stopped===true || !state.available
        ? `${node} stopped (simulated). The remaining nodes need a majority to elect another leader and continue writing.`
        : p.isolated===false ? `${node} rejoined the network. It can now repair missed log entries using the current leader's history.`
        : `${node} was isolated (simulated). Only a connected majority can confirm new writes; an isolated node cannot make progress alone.`;
      case 'dropped': return event.description.startsWith('Response')
        ? `The response from ${node} to Node ${event.peer} was blocked. The request was processed, but its sender did not receive this reply.`
        : `Delivery from ${node} to Node ${event.peer} was blocked by the simulated transport. No request arrived for this attempt.`;
      case 'committed': return `Entry #${state.committed} became committed at ${node}. Raft must preserve this agreed entry; applying it to the local key/value state is separate.`;
      case 'applied': return normal(p) ? `${node} applied entry #${p.log_id.index}: ${short(valueLabel(p),36)}. ${phase==='Catch up'?'The rejoined node is recovering changes it missed.':'Application changes the stored value; receiving or recording the entry alone would not.'}` : `${node} applied protocol entry #${p.log_id.index}. The application cursor advances without changing the client value.`;
      case 'log_truncated': return `${node} removed a conflicting log suffix. Raft repairs uncommitted entries to agree with the leader while preserving committed history.`;
      case 'snapshot_installed': return `${node} installed a state snapshot. Recovery can restore committed state without replaying every old log entry.`;
      case 'finished': return 'Recording is complete. The replay still distinguishes received messages, recorded entries, committed entries, and applied values.';
      case 'failed': return 'Scenario execution failed. Retained observations remain inspectable; an execution error is not a Raft commit decision.';
      case 'log_appended': return `${node} recorded a log entry. Recording is not commitment; the leader still needs agreement from a majority.`;
      case 'vote_saved': return `${node} saved its vote for the election round. A recorded vote prevents conflicting votes in the same term.`;
      default: return `${event.description}. The audit retains this local observation separately from network messages.`;
    }
  }
  function flight(send, receipt) {
    let text = send.description;
    if (text === 'RequestVote') text = `Vote request · term ${term(send.payload)}`;
    if (text === 'RequestVote response') {
      const granted=send.payload?.Ok?.vote_granted;
      text=typeof granted==='boolean'?`Vote ${granted?'granted':'denied'}`:send.payload?.Err?'Vote response · error':'Vote response';
    }
    if (text === 'AppendEntries') {
      const entries = (send.payload.entries || []).filter(normal);
      text = entries.length ? `Entry ${entries.map(e => e.log_id.index).join(', ')} · ${valueLabel(entries[0])}`
        : send.payload.leader_commit ? `Commit index ${send.payload.leader_commit.index}` : 'Heartbeat';
    }
    return {from:send.actor, to:receipt.actor, text, payload:payloadSummary(send), send, receipt};
  }
  class Trace {
    constructor(count=3) {
      this.count=count; this.events=[]; this.steps=[]; this.sends=new Map();
      this.seen=new Set(); this.firstCommits=new Set(); this.firstApplications=new Set();
      this.recovering=new Set();
    }
    once(key) {
      if (this.seen.has(key)) return false;
      this.seen.add(key); return true;
    }
    append(event) {
      if (event.sequence <= this.events.length) return false;
      if (event.sequence !== this.events.length + 1) throw Error('Audit sequence gap');
      const before=this.events.at(-1)?.nodes ?? initial(this.count);
      this.events.push(event);
      if (['sent','response_sent'].includes(event.kind)) this.sends.set(`${event.kind}:${event.message_id}`, event);
      const sendKind=event.kind==='received' ? 'sent' : event.kind==='response_received' ? 'response_sent' : null;
      const send=sendKind && this.sends.get(`${sendKind}:${event.message_id}`);
      const leg=send && send.actor===event.peer && send.peer===event.actor ? flight(send,event) : null;
      const step=this.classify(event,before,leg);
      if (step) {
        const text=explanation(event,before,leg,step.phase);
        this.steps.push({event,sequence:event.sequence,hold:readingPause(text),flightMs:TEACHING_FLIGHT_MS,...step,explanation:text});
      }
      return true;
    }
    classify(event,before,leg) {
      const state=event.nodes[event.actor], previous=before[event.actor];
      const base={text:label(event), phase:'Observation', focus:event.actor};
      if (['started','client_write','client_acknowledged','client_timeout','fault','failed','finished'].includes(event.kind)) {
        if (event.kind==='fault' && event.payload?.isolated===false) this.recovering.add(event.actor);
        return {...base, phase:({started:'Start',client_write:'Propose',client_acknowledged:'Acknowledged',
          client_timeout:'Timeout',fault:'Network',failed:'Error',finished:'Complete'})[event.kind]};
      }
      if (event.kind==='metrics' && ['Candidate','Leader'].includes(state.role)
          && (state.role!==previous.role || state.term!==previous.term)
          && this.once(`role:${event.actor}:${state.term}:${state.role}`)) {
        return {...base,phase:'Election',text:`Node ${event.actor} ${state.role==='Leader'?'became leader':'started an election'} · term ${state.term}`};
      }
      if (leg && event.kind==='received' && leg.send.description==='RequestVote'
          && this.once(`request:${leg.from}:${term(leg.send.payload)}`)) {
        return {...base,phase:'Election',text:leg.text,flight:leg};
      }
      if (leg && event.kind==='response_received' && leg.send.description==='RequestVote response') {
        const request=this.sends.get(`sent:${event.message_id}`);
        const granted=leg.send.payload?.Ok?.vote_granted;
        if (request && typeof granted==='boolean' && this.once(`vote:${leg.to}:${term(request.payload)}:${granted}`))
          return {...base,phase:'Election',text:leg.text,flight:leg};
      }
      if (leg && event.kind==='received' && leg.send.description==='AppendEntries') {
        const entries=(leg.send.payload.entries || []).filter(normal);
        const fresh=entries.filter(e=>this.once(`entry:${leg.to}:${logKey(e)}`));
        if (fresh.length) return {...base,phase:'Replicate',text:leg.text,flight:leg};
        // An empty RPC carrying a new application commit index is significant.
        const committed=leg.send.payload.leader_commit;
        const entry=committed && (state.log[committed.index] || previous.log[committed.index]);
        if (normal(entry) && (previous.committed===null || previous.committed<committed.index)
            && this.once(`commit-rpc:${leg.to}:${logKey(entry)}`))
          return {...base,phase:'Commit announcement',text:`Advertise commit index ${committed.index}`,flight:{...leg,text:`AppendEntries · commit index ${committed.index}`}};
      }
      if (event.kind==='committed') {
        const entry=state.log[state.committed], key=logKey(entry);
        if (normal(entry) && !this.firstCommits.has(key)) {
          this.firstCommits.add(key);
          return {...base,phase:'Commit',text:`Entry ${state.committed} committed at Node ${event.actor}`};
        }
      }
      if (event.kind==='applied' && normal(event.payload)) {
        const key=logKey(event.payload), recovery=this.recovering.has(event.actor);
        if (!this.firstApplications.has(key) || recovery) {
          this.firstApplications.add(key); this.recovering.delete(event.actor);
          return {...base,phase:recovery?'Catch up':'Apply',text:`Node ${event.actor} applied entry ${event.payload.log_id.index} · ${valueLabel(event.payload)}`};
        }
      }
      if (event.kind==='dropped' && this.once(`drop:${event.actor}:${event.peer}`))
        return {...base,phase:'Delivery blocked'};
      if (['log_truncated','snapshot_installed'].includes(event.kind)) return {...base,phase:'Recovery'};
      return null;
    }
    leg(event) {
      const kind=event.kind==='received'?'sent':event.kind==='response_received'?'response_sent':null;
      const send=kind && this.sends.get(`${kind}:${event.message_id}`);
      return send && send.actor===event.peer && send.peer===event.actor ? flight(send,event) : null;
    }
    next(cursor,mode) {
      if (mode==='teaching') return this.steps.find(step=>step.sequence>cursor);
      const event=this.events[cursor];
      return event && {event,sequence:event.sequence,text:label(event),phase:'Observation',focus:event.actor,
        hold:HOLD_MS,flightMs:FLIGHT_MS,flight:this.leg(event),explanation:this.explain(event.sequence)};
    }
    explain(cursor) {
      const event=this.events[cursor-1], step=this.steps.find(s=>s.sequence===cursor);
      if(step)return step.explanation;
      return explanation(event,this.snapshot(cursor-1),event?this.leg(event):null);
    }
    snapshot(cursor) { return this.events[cursor-1]?.nodes ?? initial(this.count); }
    duration(mode='teaching') {
      const steps=mode==='teaching'?this.steps:this.events.map(e=>({hold:HOLD_MS,flightMs:FLIGHT_MS,flight:this.leg(e)}));
      return steps.reduce((total,s)=>total+s.hold+(s.flight?s.flightMs:0),0);
    }
  }
  class Player {
    constructor({trace,active,animate,retarget,change,now=()=>performance.now(),schedule=(fn,ms)=>setTimeout(fn,ms),unschedule=id=>clearTimeout(id)}) {
      Object.assign(this,{trace,active,animate,retarget,change,now,schedule,unschedule});
      this.cursor=0; this.playing=false; this.follow=false; this.mode='teaching'; this.rate=1;
      this.pending=null; this.current=null; this.animation=null; this.timer=null;
      this.remaining=0; this.hidden=false; this.reduced=false; this.generation=0;
    }
    stopClock() {
      if (this.timer!==null) {
        this.remaining=Math.max(0,this.remaining-(this.now()-this.started)*this.rate);
        this.unschedule(this.timer); this.timer=null;
      }
      this.animation?.pause();
    }
    pause() { this.stopClock(); this.playing=false; this.follow=false; this.change('pause'); }
    play(follow=false) {
      if (!follow && this.cursor===this.trace.events.length && this.trace.events.length && !this.active() && this.remaining===0 && !this.animation) this.seek(0);
      this.playing=true; this.follow=follow; this.change('play'); this.resume();
    }
    load(trace) { this.trace=trace; this.seek(0); }
    seek(cursor) {
      this.stopClock(); this.generation++; this.animation?.cancel(); this.animation=null;
      this.pending=null; this.current=null; this.remaining=0;
      this.cursor=Math.max(0,Math.min(cursor,this.trace.events.length));
      this.playing=false; this.follow=false; this.change('seek');
    }
    speed(rate) { this.stopClock(); this.rate=rate; if(this.animation)this.animation.playbackRate=rate;
      if(this.playing)this.resume(); this.change('speed'); }
    visibility(hidden) { this.hidden=hidden; if(hidden)this.stopClock(); else if(this.playing)this.resume(); }
    reduce(value) {
      this.reduced=value;
      if(value && this.animation) {
        this.stopClock(); this.generation++; this.animation.cancel(); this.animation=null;
        this.arrive(this.pending);
      }
    }
    resume() {
      if(!this.playing || this.hidden) return;
      if(this.animation) { this.animation.play(); return; }
      if(this.timer!==null) return;
      if(this.remaining>0) {
        this.started=this.now();
        this.timer=this.schedule(()=>{this.timer=null;this.remaining=0;this.next();},this.remaining/this.rate);
      } else this.next();
    }
    wake() { if(this.playing && !this.animation && this.timer===null)this.resume(); }
    next() {
      if(!this.playing || this.hidden) return;
      const step=this.trace.next(this.cursor,this.mode);
      if(!step) {
        this.cursor=this.trace.events.length; this.change('quiet');
        if(!this.follow || !this.active()) { this.playing=false; this.change('end'); }
        return;
      }
      if(step.sequence>this.cursor+1) { this.cursor=step.sequence-1; this.change('quiet'); }
      this.pending=step; this.change('flight');
      if(step.flight && !this.reduced) {
        const generation=this.generation;
        this.animation=this.animate(step.flight,step.flightMs);
        if(this.animation) {
          this.animation.playbackRate=this.rate;
          this.animation.finished.then(()=>{
            if(generation!==this.generation)return;
            const completed=this.animation; this.animation=null; completed.cancel(); this.arrive(step);
          },()=>{});
          return;
        }
      }
      this.arrive(step);
    }
    arrive(step) {
      this.pending=null; this.current=step; this.cursor=step.sequence;
      this.remaining=step.hold; this.change('arrive'); this.resume();
    }
    relayout() {
      this.stopClock(); this.change('layout');
      if(this.animation)this.retarget?.(this.pending.flight,this.animation);
      if(this.playing)this.resume();
    }
  }
  function edges(a,b,gap=8) {
    const start=[a.x+a.width/2,a.y+a.height/2], end=[b.x+b.width/2,b.y+b.height/2];
    const dx=end[0]-start[0], dy=end[1]-start[1];
    const fraction=rect=>Math.min(dx?rect.width/2/Math.abs(dx):Infinity,dy?rect.height/2/Math.abs(dy):Infinity);
    const extra=gap/Math.hypot(dx,dy), f=fraction(a)+extra, t=fraction(b)+extra;
    return [[start[0]+dx*f,start[1]+dy*f],[end[0]-dx*t,end[1]-dy*t]];
  }
  function blocked(a,b,rect,gap=8) {
    const low=[rect.x-gap,rect.y-gap],high=[rect.x+rect.width+gap,rect.y+rect.height+gap];
    let start=0,end=1;
    for(let axis=0;axis<2;axis++) {
      const delta=b[axis]-a[axis];
      if(!delta){if(a[axis]<=low[axis] || a[axis]>=high[axis])return false;continue;}
      const times=[(low[axis]-a[axis])/delta,(high[axis]-a[axis])/delta].sort((x,y)=>x-y);
      start=Math.max(start,times[0]);end=Math.min(end,times[1]);
    }
    return start<end-1e-8;
  }
  function route(from,to,cards,width,height) {
    const direct=edges(from,to),others=cards.filter(card=>card!==from && card!==to);
    if(!others.some(card=>blocked(...direct,card)))return direct;
    const xs=new Set(),ys=new Set();
    for(const rect of cards) {
      [rect.x-8,rect.x+rect.width/2,rect.x+rect.width+8].forEach(x=>xs.add(x));
      [rect.y-8,rect.y+rect.height/2,rect.y+rect.height+8].forEach(y=>ys.add(y));
    }
    const points=[];
    for(const x of xs)for(const y of ys)if(x>=0 && y>=0 && x<=width && y<=height
      && !cards.some(r=>x>r.x-8 && x<r.x+r.width+8 && y>r.y-8 && y<r.y+r.height+8))points.push([x,y]);
    const ports=r=>[[r.x-8,r.y+r.height/2],[r.x+r.width+8,r.y+r.height/2],
      [r.x+r.width/2,r.y-8],[r.x+r.width/2,r.y+r.height+8]];
    const source=ports(from),target=ports(to),distance=points.map(()=>Infinity),previous=points.map(()=>-1),visited=new Set();
    points.forEach((p,i)=>{if(source.some(a=>same(a,p)))distance[i]=0;});
    let finish=-1;
    while(visited.size<points.length) {
      let index=-1;
      for(let i=0;i<points.length;i++)if(!visited.has(i) && Number.isFinite(distance[i]) && (index<0 || distance[i]<distance[index]))index=i;
      if(index<0)break;
      if(target.some(p=>same(p,points[index]))){finish=index;break;}
      visited.add(index);
      points.forEach((p,i)=>{
        if(visited.has(i) || (p[0]!==points[index][0] && p[1]!==points[index][1]))return;
        if(cards.some(r=>blocked(points[index],p,r)))return;
        const next=distance[index]+Math.hypot(p[0]-points[index][0],p[1]-points[index][1]);
        if(next<distance[i]){distance[i]=next;previous[i]=index;}
      });
    }
    if(finish<0)return direct;
    const result=[];for(let i=finish;i>=0;i=previous[i])result.unshift(points[i]);
    return result.filter((p,i)=>!i || i===result.length-1 ||
      (result[i-1][0]!==p[0] || result[i+1][0]!==p[0]) && (result[i-1][1]!==p[1] || result[i+1][1]!==p[1]));
  }
  function payloadPoint(point,width,height) {
    return [Math.max(70,Math.min(width-70,point[0])),Math.max(22,Math.min(height-22,point[1]))];
  }
  function frames(points,bounds=null) {
    const distances=[0];for(let i=1;i<points.length;i++)distances.push(distances.at(-1)+Math.hypot(points[i][0]-points[i-1][0],points[i][1]-points[i-1][1]));
    return points.map((point,i)=>{
      const p=bounds?payloadPoint(point,bounds.width,bounds.height):point;
      return {transform:`translate(${p[0]}px,${p[1]}px)`,offset:distances.at(-1)?distances[i]/distances.at(-1):i/(points.length-1)};
    });
  }
  return {Trace,Player,initial,label,normal,edges,route,frames,blocked,short,payloadSummary,payloadPoint,explanation,readingPause,TEACHING_FLIGHT_MS,FLIGHT_MS,HOLD_MS};
})();
if(typeof module!=='undefined')module.exports=RaftMotion;
