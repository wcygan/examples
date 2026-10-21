const assert=require('node:assert/strict');
const {spawn}=require('node:child_process');
const path=require('node:path');
const {Trace,Player,HOLD_MS,FLIGHT_MS,edges,route,frames,blocked,payloadSummary,payloadPoint,short,explanation,readingPause,TEACHING_FLIGHT_MS}=require('../src/ui/motion.js');
const state=()=>({role:'Follower',term:1,leader:1,available:true,isolated:false,log:{},committed:null,applied:null,values:{}});
const snapshot=()=>Object.fromEntries([1,2,3].map(id=>[id,state()]));
const event=(sequence,kind,actor=1,payload={})=>({sequence,kind,actor,payload,nodes:snapshot(),description:kind});
function flightTrace() {
  const trace=new Trace(3),send=event(1,'sent',1,{vote:{leader_id:{term:1,node_id:1}}});
  Object.assign(send,{peer:2,message_id:1,description:'RequestVote'});
  const receipt={...send,sequence:2,kind:'received',actor:2,peer:1,nodes:snapshot()};
  trace.append(send);trace.append(receipt);return trace;
}
function harness(trace=flightTrace()) {
  let now=0,serial=0;const timers=new Map(),animations=[];
  const player=new Player({trace,active:()=>false,change:()=>{},now:()=>now,
    schedule:(fn,delay)=>{const id=++serial;timers.set(id,{fn,at:now+delay});return id;},
    unschedule:id=>timers.delete(id),retarget:()=>{},animate:(leg,duration)=>{
      let resolve;const animation={duration,finished:new Promise(r=>resolve=r),playbackRate:1,currentTime:400,
        pause(){this.paused=true;},play(){this.paused=false;},cancel(){this.cancelled=true;},finish(){resolve();}};
      animations.push(animation);return animation;
    }});
  function tick(ms) {
    const end=now+ms;
    while(true){const job=[...timers].sort((a,b)=>a[1].at-b[1].at).find(([,v])=>v.at<=end);
      if(!job)break;now=job[1].at;timers.delete(job[0]);job[1].fn();}
    now=end;
  }
  return {player,animations,timers,tick};
}
async function clockTests() {
  const h=harness(),p=h.player;p.play();const a=p.animation;
  assert.equal(p.cursor,1);assert.equal(a.currentTime,400);assert.equal(a.duration,TEACHING_FLIGHT_MS);
  p.pause();assert.equal(p.animation,a);assert.equal(a.paused,true);assert.equal(p.cursor,1);
  p.play();assert.equal(p.animation,a);assert.equal(a.currentTime,400);
  p.speed(2);assert.equal(p.animation,a);assert.equal(a.playbackRate,2);assert.equal(a.currentTime,400);
  p.visibility(true);assert.equal(a.paused,true);h.tick(5000);assert.equal(p.cursor,1);
  p.visibility(false);assert.equal(p.animation,a);assert.equal(a.paused,false);
  a.finish();await Promise.resolve();assert.equal(p.cursor,2);assert.equal(p.remaining,p.current.hold);
  h.tick(100);p.pause();assert.equal(p.remaining,p.current.hold-200);
  p.speed(.5);p.play();h.tick(p.remaining / .5 - 1);assert.equal(p.playing,true);h.tick(1);assert.equal(p.playing,false);
  const stale=harness();stale.player.play();const old=stale.player.animation;
  stale.player.seek(0);old.finish();await Promise.resolve();assert.equal(stale.player.cursor,0);
  assert.equal(stale.player.animation,null);
  const switching=harness();switching.player.play();const oldFlight=switching.player.animation;
  const shorter=new Trace(5);shorter.append({...event(1,'started'),nodes:RaftInitial(5)});
  switching.player.load(shorter);oldFlight.finish();await Promise.resolve();
  assert.equal(switching.player.cursor,0);assert.equal(switching.player.trace,shorter);assert.equal(switching.player.animation,null);
  switching.player.seek(1);switching.player.load(new Trace(3));assert.equal(switching.player.cursor,0);
  assert.equal(switching.player.pending,null);assert.equal(switching.player.playing,false);
  const quiet=harness();quiet.player.reduce(true);quiet.player.play();
  assert.equal(quiet.animations.length,0);assert.equal(quiet.player.cursor,2);assert.equal(quiet.player.remaining,quiet.player.current.hold);
  quiet.tick(quiet.player.remaining-1);assert.equal(quiet.player.playing,true);quiet.tick(1);assert.equal(quiet.player.playing,false);
  const changing=harness();changing.player.play();changing.player.pause();changing.player.reduce(true);
  assert.equal(changing.player.cursor,2);assert.equal(changing.player.playing,false);assert.equal(changing.player.remaining,changing.player.current.hold);
  const resizing=harness();resizing.player.play();const resizeFlight=resizing.player.animation;
  resizing.player.relayout();assert.equal(resizing.player.animation,resizeFlight);assert.equal(resizeFlight.currentTime,400);
}
async function continuousSpeedTests() {
  const h=harness(),p=h.player;p.play();const flight=p.animation;
  p.speed(.73);assert.equal(p.animation,flight);assert.equal(flight.currentTime,400);assert.equal(flight.playbackRate,.73);
  p.speed(1.37);assert.equal(p.animation,flight);assert.equal(flight.currentTime,400);
  flight.finish();await Promise.resolve();const hold=p.remaining;
  h.tick(100);p.speed(.41);assert.equal(p.remaining,hold-137);assert.equal(h.timers.size,1);
  h.tick(100);p.speed(.83);assert.equal(p.remaining,hold-178);assert.equal(h.timers.size,1);
  p.pause();const frozen=p.remaining;p.speed(1.23);h.tick(10000);
  assert.equal(p.remaining,frozen);assert.equal(p.playing,false);assert.equal(h.timers.size,0);
  p.play();h.tick(p.remaining/1.23-1);assert.equal(p.playing,true);h.tick(1);assert.equal(p.playing,false);
  assert.equal(readingPause('Short explanation.'),3200);assert.equal(readingPause('word '.repeat(100)),7000);
  assert(readingPause('word '.repeat(25))>readingPause('word '.repeat(15)));
  const all=harness();all.player.mode='all';all.player.play();all.tick(HOLD_MS);
  assert.equal(all.player.animation.duration,FLIGHT_MS);all.player.animation.finish();await Promise.resolve();assert.equal(all.player.remaining,HOLD_MS);
}
function classifierTests() {
  const timed=flightTrace();
  assert.equal(timed.duration('all'),HOLD_MS*2+FLIGHT_MS);
  assert.equal(timed.duration(),timed.steps[0].hold+TEACHING_FLIGHT_MS);
  const trace=new Trace(3),command={log_id:{index:2,leader_id:{term:1,node_id:1}},payload:{Normal:{key:'lesson',value:'first'}}};
  const sent=event(1,'sent',1,{entries:[],leader_commit:command.log_id});
  Object.assign(sent,{peer:2,message_id:7,description:'AppendEntries'});
  sent.nodes[2].log[2]=command;trace.append(sent);
  const received={...sent,sequence:2,kind:'received',actor:2,peer:1,nodes:structuredClone(sent.nodes)};
  trace.append(received);assert.equal(trace.steps.length,1);assert.equal(trace.steps[0].phase,'Commit announcement');assert(trace.steps[0].flight.text.includes('commit index'));
  assert.equal(trace.steps[0].flight.from,1);assert.equal(trace.steps[0].flight.to,2);
  const metric=event(3,'metrics');metric.nodes=structuredClone(received.nodes);trace.append(metric);
  assert.equal(trace.steps.length,1);assert.equal(trace.events.length,3);
  assert.equal(trace.append(metric),false);assert.equal(trace.events.length,3);
  assert.throws(()=>trace.append({...metric,sequence:5}),/sequence gap/);
  const heartbeat=new Trace(3);const heart={...sent,payload:{entries:[],leader_commit:null},nodes:snapshot()};
  heartbeat.append(heart);heartbeat.append({...heart,sequence:2,kind:'received',actor:2,peer:1});
  assert.equal(heartbeat.steps.length,0);assert.equal(heartbeat.next(0,'all').sequence,1);
  const missing=new Trace(3);missing.append({...received,sequence:1});assert.equal(missing.steps.length,0);
  const [a,b]=edges({x:0,y:0,width:100,height:100},{x:300,y:0,width:100,height:100});
  assert.deepEqual(a,[108,50]);assert.deepEqual(b,[292,50]);
}
function routeTests() {
  for(const count of [3,4,5])for(const narrow of [false,true]) {
    const width=narrow?334:610,height=narrow&&count===5?610:390;
    const positions=count===3?[[.5,.25],[.23,.75],[.77,.75]]:count===4?[[.25,.25],[.75,.25],[.25,.75],[.75,.75]]
      :narrow?[[.5,.16],[.25,.5],[.75,.5],[.25,.84],[.75,.84]]:[[.25,.25],[.75,.25],[.165,.75],[.5,.75],[.835,.75]];
    const cardWidth=width*(count===5&&!narrow?.31:.42);
    const cards=positions.map(([x,y])=>({x:x*width-cardWidth/2,y:y*height-80,width:cardWidth,height:160}));
    for(const a of cards)for(const b of cards)if(a!==b) {
      const path=route(a,b,cards,width,height);assert(path.length>=2);
      for(let i=1;i<path.length;i++)for(const obstacle of cards.filter(r=>r!==a&&r!==b))
        assert(!blocked(path[i-1],path[i],obstacle),`route obscured: ${count}/${narrow}`);
      const cardFrames=frames(path,{width,height});
      for(const point of path) {
        const [x,y]=payloadPoint(point,width,height);
        assert(x>=70 && x+70<=width && y>=22 && y+22<=height,'Payload card escaped the scene');
      }
      assert.equal(cardFrames[0].offset,0);assert.equal(cardFrames.at(-1).offset,1);
      const keyframes=frames(path);assert.equal(keyframes[0].offset,0);assert.equal(keyframes.at(-1).offset,1);
      assert(keyframes.every(f=>Number.isFinite(f.offset)));
    }
  }
}
async function liveTests() {
  const trace=new Trace(3),h=harness(trace),p=h.player;
  p.active=()=>true;trace.append(event(1,'metrics'));p.play(true);
  assert.equal(p.cursor,1);assert.equal(p.playing,true);assert.equal(h.timers.size,0);
  trace.append(event(2,'client_write'));p.wake();assert.equal(p.cursor,2);assert.equal(p.remaining,p.current.hold);
  p.pause();trace.append(event(3,'finished'));p.wake();assert.equal(p.cursor,2);
  p.active=()=>false;p.play(true);h.tick(p.remaining + trace.steps.at(-1).hold);assert.equal(p.cursor,3);assert.equal(p.playing,false);
}

function payloadTests() {
  const write={log_id:{index:7},payload:{Normal:{key:'lesson',value:'second'}}};
  assert.deepEqual(payloadSummary({description:'AppendEntries',payload:{entries:[write]}}),{title:'Replicate entry',detail:'#7 · lesson = second'});
  assert.deepEqual(payloadSummary({description:'AppendEntries',payload:{entries:[],leader_commit:{index:7}}}),{title:'Announce commit',detail:'Committed through #7'});
  assert.equal(payloadSummary({description:'RequestVote response',payload:{Ok:{vote_granted:false,vote:{leader_id:{term:4}}}}}).title,'Vote denied');
  assert.equal(payloadSummary({description:'AppendEntries response',payload:{Ok:'Conflict'}}).title,'Log mismatch');
  assert.equal(payloadSummary({description:'AppendEntries',payload:{entries:[{payload:{Membership:{}}}]}}).detail,'Cluster membership');
  assert.equal(short('x'.repeat(200)).length,24);assert.equal(short('x'.repeat(200)).at(-1),'…');
  const failed=event(1,'failed');assert(explanation(failed,snapshot()).includes('not a Raft commit'));
  const protocol=event(1,'applied',1,{log_id:{index:1},payload:'Blank'});
  assert(explanation(protocol,snapshot()).includes('without changing the client value'));
}
function browserTimerBindingTest() {
  const schedule=globalThis.setTimeout,unschedule=globalThis.clearTimeout;
  try {
    globalThis.setTimeout=function(){assert.equal(this,globalThis,'timer must not bind to Player');return 1;};
    globalThis.clearTimeout=function(){assert.equal(this,globalThis,'timer cancellation must not bind to Player');};
    const trace=new Trace(3);trace.append(event(1,'started'));
    const p=new Player({trace,active:()=>false,animate:()=>null,change:()=>{}});p.play();p.pause();
  } finally {globalThis.setTimeout=schedule;globalThis.clearTimeout=unschedule;}
}

async function fixtureTests() {
  const binary=process.env.OPENRAFT_DEMO_BIN||path.resolve(__dirname,`../../../target/debug/openraft-demo${process.platform==='win32'?'.exe':''}`);
  const child=spawn(binary,['--port','0'],{stdio:['ignore','pipe','inherit']});
  try {
    const base=await new Promise((resolve,reject)=>{
      let output='';const timeout=setTimeout(()=>reject(Error('Console startup deadline')),5000);
      child.once('error',reject);child.once('exit',()=>reject(Error('Console exited during startup')));
      child.stdout.on('data',data=>{output+=data;const match=output.match(/Console: (http:\/\/[^\s]+)/);
        if(match){clearTimeout(timeout);resolve(match[1]);}});
    });
    let rawCount=0,teachingCount=0;
    for(let count=3;count<=5;count++)for(const scenario of ['replication','leader-loss','quorum-loss','catch-up']) {
      const response=await fetch(`${base}/api/runs`,{method:'POST',headers:{'content-type':'application/json'},body:JSON.stringify({scenario,node_count:count})});
      assert.equal(response.status,201);const {id}=await response.json();let run;
      const deadline=Date.now()+8000;
      do {
        run=await (await fetch(`${base}/api/runs/${id}`)).json();
        if(run.status==='running')await new Promise(r=>setTimeout(r,25));
        assert(Date.now()<deadline,`${scenario}/${count} deadline`);
      }while(run.status==='running');
      assert.equal(run.status,'finished');const original=JSON.stringify(run.events),trace=new Trace(count);
      for(const e of run.events)trace.append(e);
      assert.equal(JSON.stringify(trace.events),original);assert.equal(trace.events.length,run.events.length);
      assert(trace.steps.length<trace.events.length*.5,`${scenario}/${count}: teaching did not quiet routine observations`);
      rawCount+=trace.events.length;teachingCount+=trace.steps.length;
      let cursor=0;
      while(true){const step=trace.next(cursor,'teaching');if(!step)break;
        assert(step.sequence>cursor);
        assert.equal(step.hold,readingPause(step.explanation));
        assert(step.hold>=3200 && step.hold<=7000);
        assert.equal(step.flightMs,TEACHING_FLIGHT_MS);assert.deepEqual(trace.snapshot(step.sequence),run.events[step.sequence-1].nodes);
        if(step.flight){const receipt=step.flight.receipt,send=step.flight.send;
          assert(send.sequence<receipt.sequence);assert.equal(send.message_id,receipt.message_id);
          assert.equal(receipt.peer,send.actor);assert.equal(send.peer,receipt.actor);}
        assert(step.explanation.length>30 && step.explanation.length<300);
        assert(!step.explanation.includes('undefined'));
        assert.equal(trace.explain(step.sequence),step.explanation,'Seeking retains the teaching explanation');
        if(step.flight) {
          assert.deepEqual(step.flight.payload,payloadSummary(step.flight.send));
          assert(short(step.flight.payload.title).length<=24 && short(step.flight.payload.detail).length<=24);
          assert(explanation(step.event,trace.snapshot(step.sequence-1),step.flight,step.phase,true).length>30);
        }
        if(step.phase==='Replicate')assert(step.explanation.includes('does not yet mean'));
        if(step.phase==='Commit')assert(step.explanation.includes('applying'));
        if(step.phase==='Catch up')assert(step.explanation.includes('recovering changes'));
        if(step.event.kind==='client_timeout')assert(step.explanation.includes('does not prove'));
        if(step.event.kind==='fault')assert(step.explanation.includes(!step.event.nodes[step.event.actor].available?'stopped':step.event.payload.isolated===false?'rejoined':'isolated'));
        cursor=step.sequence;
      }
      assert.equal(cursor,run.events.length);
      for(let i=0;i<=run.events.length;i++)assert.deepEqual(trace.snapshot(i),i?run.events[i-1].nodes:RaftInitial(count));
      for(let i=1;i<=run.events.length;i++)assert(!trace.explain(i).includes('undefined'),`Missing explanation at ${scenario}/${count}/${i}`);
      assert(trace.steps.some(s=>s.phase==='Commit'));assert(trace.steps.some(s=>s.phase==='Apply'));
      if(scenario==='catch-up')assert(trace.steps.some(s=>s.phase==='Catch up'&&s.focus===count));
      if(scenario==='quorum-loss')assert(trace.steps.some(s=>s.event.kind==='client_timeout'));
    }
    console.log(`12 real trace combinations: teaching ${(teachingCount/rawCount*100).toFixed(1)}% of raw observations`);
  } finally {child.kill();}
}
const RaftInitial=require('../src/ui/motion.js').initial;
(async()=>{classifierTests();routeTests();payloadTests();browserTimerBindingTest();await clockTests();await continuousSpeedTests();await liveTests();await fixtureTests();console.log('Motion, continuity, projection, geometry and snapshot checks passed');})().catch(e=>{console.error(e);process.exitCode=1;});
