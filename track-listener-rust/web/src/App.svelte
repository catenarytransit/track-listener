<script>
  import { onMount, tick } from 'svelte';
  let channels = [], status = null, clips = [], cursor = null;
  let channel = '', query = '', since = '', until = '', live = true, zone = 'local';
  let error = '', loading = true, loadedOlder = false, requestId = 0, filterTimer;
  let selected = null, audioEl, audioError = '', playRequest = 0, seekEnd = null;
  const timezones = {local:undefined, pacific:'America/Los_Angeles', utc:'UTC'};
  const formatter = (opts,z) => new Intl.DateTimeFormat(undefined, {...opts, timeZone:timezones[z]});
  function dateTime(ms,z) { return ms == null ? '—' : formatter({month:'short',day:'numeric',hour:'2-digit',minute:'2-digit',second:'2-digit',hour12:false,timeZoneName:'short'},z).format(ms); }
  function clock(ms,z) { return formatter({hour:'2-digit',minute:'2-digit',second:'2-digit',hour12:false},z).format(ms); }
  function duration(ms) { const s=Math.max(0,Math.round(ms/1000)); return `${Math.floor(s/60)}:${String(s%60).padStart(2,'0')}`; }
  function bytes(n) { return n >= 1073741824 ? `${(n/1073741824).toFixed(2)} GiB` : `${(n/1048576).toFixed(1)} MiB`; }
  function label(state) { return ({receiving:'Receiving',connecting:'Connecting',reconnecting:'Reconnecting',disabled:'Disabled',demo:'Demo',stopped:'Stopped',starting:'Starting'})[state] || state; }
  async function json(url) { const res = await fetch(url,{cache:'no-store'}); if (!res.ok) throw Error(`Request failed (${res.status})`); return res.json(); }
  function params(older) {
    const p = new URLSearchParams({limit:'50'});
    if (channel) p.set('channel',channel);
    if (query.trim()) p.set('q',query.trim());
    if (since) p.set('since',String(new Date(since).getTime()));
    if (until) p.set('until',String(new Date(until).getTime()));
    if (older && cursor) {p.set('before_ms',cursor.before_ms);p.set('before_id',cursor.before_id);}
    return p;
  }
  async function load(older=false, quiet=false) {
    const id=++requestId;
    if (!quiet) loading=true;
    try {
      const result=await json('/api/clips?'+params(older));
      if (id!==requestId) return;
      clips=older ? [...clips,...result.items.filter(c=>!clips.some(old=>old.id===c.id))] : result.items;
      cursor=result.next_cursor; loadedOlder=older; error='';
    } catch(e) { if(id===requestId) error=e.message; }
    finally { if(id===requestId) loading=false; }
  }
  async function refreshStatus() {
    try { [channels,status]=await Promise.all([json('/api/channels'),json('/api/status')]); }
    catch(e) { error=e.message; }
  }
  function changed() { clearTimeout(filterTimer); ++requestId; loadedOlder=false; filterTimer=setTimeout(()=>load(),250); }
  function choose(id) { channel=id; changed(); }
  function resume() { live=true; loadedOlder=false; load(); }
  onMount(()=>{
    refreshStatus();load();
    const timer=setInterval(()=>{refreshStatus();if(live&&!loadedOlder&&!loading)load(false,true);},3000);
    return ()=>{clearInterval(timer);clearTimeout(filterTimer);};
  });
  async function play(clip, sentence=null) {
    if (!clip.audio_available) return;
    const req=++playRequest;
    const same=selected?.id===clip.id;
    selected=clip; audioError=''; seekEnd=sentence?.end_ms==null?null:sentence.end_ms/1000;
    await tick();
    if(req!==playRequest || !audioEl) return;
    const start=(sentence?.start_ms??0)/1000;
    const startPlaying=()=>{if(req!==playRequest||!audioEl)return;try{audioEl.currentTime=start;audioEl.play().catch(()=>{if(req===playRequest)audioError='Press play to start audio.';});}catch(e){audioError='Unable to seek in this clip.';}};
    if(same && audioEl.readyState>=1) startPlaying();
    else { audioEl.addEventListener('loadedmetadata',startPlaying,{once:true}); audioEl.load(); }
  }
  function timeUpdate(){if(seekEnd!==null&&audioEl&&audioEl.currentTime>=seekEnd){audioEl.pause();seekEnd=null;}}
  function closePlayer(){++playRequest; audioEl?.pause();selected=null;seekEnd=null;audioError='';}
  $: online=channels.filter(c=>c.state==='receiving').length;
  $: used=status?.archive?.used_bytes??0;
  $: budget=status?.archive?.max_bytes??0;
  $: usage=budget?Math.min(100,100*used/budget):0;
</script>

<svelte:head><title>Track Listener · Radio archive</title></svelte:head>
<div class="shell">
  <aside>
    <div class="brand"><span class="brand-mark" aria-hidden="true"><svg viewBox="0 0 36 36" fill="none"><path d="M12 7v22M24 7v22M8 11h20M8 18h20M8 25h20" stroke="currentColor" stroke-width="2" stroke-linecap="round"/><path d="M3 5a18 18 0 0 1 30 0" stroke="currentColor" stroke-width="2" stroke-linecap="round"/></svg></span><div><strong>Track Listener</strong><small>RAIL RADIO ARCHIVE</small></div></div>
    <div class="sidebar-label">CHANNELS <span>{online}/{channels.length} receiving</span></div>
    <nav aria-label="Radio channels">
      <button class:active={!channel} on:click={()=>choose('')}><span class="all-icon">◉</span><span>All channels</span></button>
      {#each channels as c (c.id)}
        <button class:active={channel===c.id} on:click={()=>choose(c.id)} title={c.detail||label(c.state)}><span class:receiving={c.state==='receiving'} class="dot"></span><span class="channel-name">{c.name}<small>{label(c.state)}{c.dropped_clips ? ` · ${c.dropped_clips} dropped` : ''}</small></span></button>
      {/each}
    </nav>
    <div class="storage">
      <div class="sidebar-label">AUDIO RETENTION</div>
      <div class="storage-value">{bytes(used)} <small>/ {bytes(budget)}</small></div>
      <div class="meter" role="progressbar" aria-label="Audio budget usage" aria-valuenow={usage} aria-valuemin="0" aria-valuemax="100"><div style:width={`${usage}%`}></div></div>
      <p>{status?.archive?.mode==='ram'?'RAM only':'Persistent disk'} · oldest first</p>
      <p>{status?.archive?.retained_clips??0} retained clips</p>
      {#if status?.archive?.oldest_ms}<p>Oldest: {dateTime(status.archive.oldest_ms,zone)}</p>{/if}
      <p class="muted">Queue {status?.queue?.depth??0}/{status?.queue?.capacity??0}</p>
    </div>
    <footer>Self-hosted · CPU recognition<br/>Transcripts may contain errors.</footer>
  </aside>
  <main>
    <header class="page-header"><div><div class="eyebrow">MONITOR / TRANSCRIPTS</div><h1>{channel ? channels.find(c=>c.id===channel)?.name||channel : 'Radio transmissions'}</h1><p>Browse speech detected on railway radio channels.</p></div><div class="live-controls"><button class:live-on={live&&!loadedOlder} class="live-button" on:click={()=>{if(live&&!loadedOlder)live=false;else resume();}}><span class="dot"></span>{live&&!loadedOlder?'Live updates':'Resume live'}</button><button on:click={()=>load()} class="secondary">Refresh</button></div></header>
    {#if status?.demo}<div class="notice">Demo mode · synthetic recordings only. No live radio is connected.</div>{/if}
    <div class="stats"><div><span>Stored transmissions</span><strong>{status?.archive?.clips??'—'}</strong></div><div><span>Recognized sentences</span><strong>{status?.archive?.sentences??'—'}</strong></div><div><span>Receiving channels</span><strong>{online}<small> / {channels.length}</small></strong></div></div>
    <section class="filters" aria-label="Filter transcripts">
      <label class="search">Search transcript<input type="search" placeholder="Search words or train numbers…" bind:value={query} on:input={changed}/></label>
      <label>From (local)<input type="datetime-local" bind:value={since} on:change={changed}/></label>
      <label>Until (local)<input type="datetime-local" bind:value={until} on:change={changed}/></label>
      <label>Display time<select bind:value={zone}><option value="local">Local time</option><option value="pacific">Pacific time</option><option value="utc">UTC</option></select></label>
      <button class="secondary clear" on:click={()=>{query='';since='';until='';changed();}}>Clear</button>
    </section>
    {#if error}<div class="error" role="alert">{error} <button on:click={()=>load()}>Retry</button></div>{/if}
    <div class="list-heading"><span>{clips.length} transmissions shown</span><span>Newest first · estimated receiver time</span></div>
    {#if loading && !clips.length}<div class="empty">Loading transmissions…</div>{:else if !clips.length}<div class="empty"><strong>No transmissions found</strong><p>Check channel status, change the filters, or wait for speech.</p></div>{/if}
    <section class="transmissions" aria-label="Transmissions">
      {#each clips as clip (clip.id)}
        <article class:selected-card={selected?.id===clip.id}>
          <div class="clip-heading"><div class="clip-time"><strong>{dateTime(clip.started_ms,zone)}</strong><span>{duration(clip.duration_ms)} · {clip.channel_name}</span></div><div class="clip-actions"><span class="chip">{clip.status}</span>{#if clip.audio_available}<button class="play" on:click={()=>play(clip)}>▶ Play clip</button>{:else}<span class="expired">Audio unavailable</span>{/if}</div></div>
          {#if clip.sentences.length}
            {#each clip.sentences as sentence (sentence.ordinal)}
              <div class="sentence"><span class="sentence-time" title={sentence.start_ms==null?'Sentence timing unavailable; clip time shown':'Estimated sentence start'}>{sentence.start_ms==null?'~ ':''}{clock(clip.started_ms+(sentence.start_ms??0),zone)}</span><p>{sentence.text}</p>{#if clip.audio_available&&sentence.start_ms!=null}<button class="sentence-play" aria-label={'Play sentence '+(sentence.ordinal+1)} on:click={()=>play(clip,sentence)}>▶</button>{/if}</div>
            {/each}
          {:else}<p class="no-text">{clip.status==='pending'?'Recognition in progress…':clip.status==='error'||clip.status==='interrupted'?'Recognition unavailable — audio may still be playable.':'No speech recognized.'}</p>{/if}
          {#if clip.error}<small class="clip-error">{clip.error}</small>{/if}
        </article>
      {/each}
    </section>
    {#if cursor}<button class="older secondary" disabled={loading} on:click={()=>load(true)}>{loading?'Loading…':'Load older transmissions'}</button>{/if}
    <div class="end-note">Times refer to received audio, not guaranteed over-the-air transmission times.</div>
  </main>
</div>
{#if selected}
  <section class="player" aria-label="Audio playback"><div class="player-label"><div class="eyebrow">NOW PLAYING</div><strong>{selected.channel_name}</strong><small>{dateTime(selected.started_ms,zone)}</small></div><div class="audio-wrap"><audio bind:this={audioEl} src={'/api/clips/'+selected.id+'/audio'} controls preload="metadata" on:timeupdate={timeUpdate} on:error={()=>audioError='Audio could not be loaded. It may have expired.'} aria-label="Recorded radio clip"></audio>{#if audioError}<small role="status">{audioError}</small>{/if}</div><button class="close" on:click={closePlayer} aria-label="Close audio player">×</button></section>
{/if}
