export function renderHeroScene(scene) {
  if (scene === "kinetic") {
    const tokens = ["@files", "git log", "herdr run", "ssh prod", "SELECT *", "diff --review", "SFTP", "workspace://"];
    return `<div class="hero-scene scene-kinetic" aria-hidden="true">
      <div class="hero-scene-motion">
        <div class="kinetic-crosshair"></div>
        <div class="kinetic-core"><strong>HERDR</strong><span>shared context</span></div>
        <div class="command-cloud">${tokens.map((token, i) => `<span class="float-token" style="--i:${i};--angle:${i * 45}deg">${token}</span>`).join("")}</div>
        <div class="kinetic-caption">01 — 05 / tools in formation</div>
      </div>
    </div>`;
  }

  if (scene === "orbit") {
    const nodes = [
      ["Herdr", "herdr-agents", 0],
      ["Files", "files", 72],
      ["Git", "git-graph", 144],
      ["SSH", "ssh-sftp", 216],
      ["Data", "database", 288],
    ];
    return `<div class="hero-scene scene-orbit" aria-hidden="true">
      <div class="hero-scene-motion orbit-universe">
        <div class="aurora aurora-a"></div><div class="aurora aurora-b"></div>
        <div class="orbit-ring orbit-ring-a"></div><div class="orbit-ring orbit-ring-b"></div><div class="orbit-ring orbit-ring-c"></div>
        <div class="orbit-core"><span>Y</span><small>workspace</small></div>
        ${nodes.map(([label, id, angle], i) => `<div class="orbit-node orbit-node-${i}" data-orbit-target="${id}" style="--node-angle:${angle}deg;--node-radius:${i % 2 ? "12.4rem" : "10.2rem"}"><span class="orbit-node-inner">${label}</span></div>`).join("")}
        <svg class="orbit-trace" viewBox="0 0 600 600"><path d="M110 300C110 195 195 110 300 110S490 195 490 300 405 490 300 490 110 405 110 300Z"/><path d="M165 300c0-75 60-135 135-135s135 60 135 135-60 135-135 135-135-60-135-135Z"/></svg>
      </div>
    </div>`;
  }

  if (scene === "cartography") {
    return `<div class="hero-scene scene-cartography" aria-hidden="true">
      <div class="hero-scene-motion map-canvas">
        <div class="map-grid"></div>
        <svg class="map-routes" viewBox="0 0 800 560" preserveAspectRatio="none">
          <path class="route route-main" d="M54 412C180 380 160 195 302 210S445 405 560 318 633 116 758 132"/>
          <path class="route route-branch" d="M302 210C370 176 409 110 484 86"/>
          <path class="route route-branch" d="M560 318C620 386 684 404 752 376"/>
        </svg>
        <div class="map-node node-a"><b>LOCAL</b><span>workspace</span></div>
        <div class="map-node node-b"><b>HERDR</b><span>agents</span></div>
        <div class="map-node node-c"><b>REMOTE</b><span>ssh / sftp</span></div>
        <div class="map-node node-d"><b>DATA</b><span>schema / query</span></div>
        <div class="map-coordinate">N 24° · E 120° / context route 05</div>
      </div>
    </div>`;
  }

  if (scene === "modular") {
    const modules = ["HERDR", "FILES", "GIT", "SSH", "DATA", "DIFF", "TERM", "QUERY", "SFTP", "LOG", "CTX", "LOCAL"];
    return `<div class="hero-scene scene-modular" aria-hidden="true">
      <div class="hero-scene-motion modular-field">
        ${modules.map((label, i) => `<div class="module-tile module-${i}" style="--i:${i}"><span>${label}</span><i></i></div>`).join("")}
        <div class="module-cursor"><span>task</span></div>
      </div>
    </div>`;
  }

  if (scene === "editorial") {
    return `<div class="hero-scene scene-editorial" aria-hidden="true">
      <div class="hero-scene-motion editorial-board">
        <div class="editorial-index">YUZORA / FIELD NOTE 01</div>
        <div class="editorial-bigword">WORK<br>BENCH</div>
        <div class="editorial-rule"></div>
        <div class="editorial-note note-a"><b>01</b><span>Herdr thinks beside the work.</span></div>
        <div class="editorial-note note-b"><b>03</b><span>Git history becomes spatial.</span></div>
        <div class="editorial-note note-c"><b>05</b><span>Data stays in the same context.</span></div>
        <div class="editorial-stamp">LOCAL / EXPLICIT / REVIEWABLE</div>
      </div>
    </div>`;
  }

  return `<div class="hero-scene scene-cinematic" aria-hidden="true">
    <div class="hero-scene-motion cinema-stage">
      <div class="cinema-glow"></div>
      <div class="cinema-frame">
        <div class="cinema-chrome"><i></i><i></i><i></i><span>yuzora / workspace</span></div>
        <div class="cinema-body">
          <div class="cinema-rail"><b>Y</b><i></i><i></i><i></i><i></i></div>
          <div class="cinema-tree"><strong>PROJECT</strong><span>▾ src</span><span>　app.tsx</span><span>　workspace.ts</span><span>▾ agents</span><span class="active">　herdr.ts</span></div>
          <div class="cinema-editor"><div class="code-line w70"></div><div class="code-line w45"></div><div class="code-line w82"></div><div class="code-line w58"></div><div class="code-line w72"></div><div class="code-line w38"></div><div class="code-line w64"></div></div>
          <div class="cinema-agent"><div class="agent-head"><span>Herdr</span><i>running</i></div><p>Reading the current workspace…</p><div class="agent-change">+ 3 reviewed changes</div><button tabindex="-1">Approve</button></div>
        </div>
        <div class="cinema-status"><span>main</span><span>localhost</span><span>Herdr ready</span></div>
      </div>
      <div class="cinema-timeline"><i class="active"></i><i></i><i></i><i></i><i></i><span>01 / 05</span></div>
    </div>
  </div>`;
}

export function renderProductVisual(product) {
  const video = product.media ? `<video class="product-video" muted loop playsinline preload="none" data-src="${product.media}" aria-label="${product.title} 產品操作示範"></video>` : "";

  if (product.visual === "agents") {
    return `<div class="visual-shell visual-agents">
      ${video}
      <div class="ui-fallback agent-fallback">
        <div class="mock-top"><span>Herdr / Codex</span><b>workspace linked</b></div>
        <div class="mock-chat"><p><i>Y</i><span>Review the database migration and keep the public interface unchanged.</span></p><p><i>H</i><span>I found three affected modules. I’ll prepare a reviewable diff before any write.</span></p></div>
        <div class="permission-card"><span>Tool request</span><strong>edit · src/data/migrate.ts</strong><div><button tabindex="-1">Deny</button><button tabindex="-1">Allow once</button></div></div>
      </div>
    </div>`;
  }

  if (product.visual === "files") {
    return `<div class="visual-shell visual-files">
      <div class="file-window">
        <div class="file-toolbar"><span>yuzora</span><i>⌘ P</i></div>
        <div class="file-layout">
          <div class="file-tree"><b>EXPLORER</b><span>▾ src</span><span>　▾ app</span><span class="selected">　　Workspace.tsx</span><span>　　HerdrPanel.tsx</span><span>　▾ workbench</span><span>　　GitGraph.tsx</span><span>▾ src-tauri</span><span>　ssh.rs</span></div>
          <div class="file-editor"><div class="editor-tabs"><span class="active">Workspace.tsx</span><span>HerdrPanel.tsx</span></div><pre><code><em>export</em> function Workspace() {
  <b>const</b> context = useSharedContext()

  <em>return</em> (
    &lt;Workbench context={context}&gt;
      &lt;HerdrPanel reviewable /&gt;
    &lt;/Workbench&gt;
  )
}</code></pre><div class="editor-minimap"></div></div>
        </div>
      </div>
    </div>`;
  }

  if (product.visual === "git") {
    return `<div class="visual-shell visual-git">
      ${video}
      <div class="ui-fallback git-fallback">
        <div class="git-toolbar"><span>Git Graph</span><i>main · 14 commits</i></div>
        <svg class="git-svg" viewBox="0 0 700 410" preserveAspectRatio="none" aria-hidden="true">
          <path class="git-lane lane-a" d="M80 26v358"/><path class="git-lane lane-b" d="M80 78C170 78 170 138 244 138v136c-74 0-74 62-164 62"/><path class="git-lane lane-c" d="M244 138C330 138 330 198 402 198v76c-72 0-72 0-158 0"/>
          ${[[80,52],[80,104],[244,138],[244,190],[402,220],[402,274],[244,310],[80,336]].map(([x,y], i) => `<circle cx="${x}" cy="${y}" r="${i === 0 ? 8 : 6}"/>`).join("")}
        </svg>
        <div class="commit-list"><p><b>Refine Herdr permission surface</b><span>2 min ago · Yuuzu</span></p><p><b>Connect graph selection to diff</b><span>18 min ago · main</span></p><p><b>Merge remote workspace controls</b><span>1 hr ago · feature/remote</span></p><p><b>Add database schema inspector</b><span>Yesterday · data-tools</span></p></div>
      </div>
    </div>`;
  }

  if (product.visual === "ssh") {
    return `<div class="visual-shell visual-ssh">
      ${video}
      <div class="ui-fallback ssh-fallback">
        <div class="remote-sidebar"><b>REMOTE HOSTS</b><span class="online"><i></i>production-eu</span><span><i></i>staging</span><span><i></i>home-lab</span><hr><b>SFTP</b><span>▾ /srv/yuzora</span><span>　releases</span><span>　logs</span></div>
        <div class="remote-terminal"><div class="term-tabs"><span class="active">production-eu</span><span>SFTP queue · 2</span></div><pre><code><i>yuuzu@production-eu</i>:<b>/srv/yuzora</b>$ systemctl status yuzora
● yuzora.service - Yuzora workspace
   Active: <strong>active (running)</strong>

yuuzu@production-eu:<b>/srv/yuzora</b>$ <span class="cursor">_</span></code></pre></div>
      </div>
    </div>`;
  }

  return `<div class="visual-shell visual-database">
    ${video}
    <div class="ui-fallback db-fallback">
      <div class="db-sidebar"><b>CONNECTIONS</b><span class="active">PostgreSQL · local</span><span>MySQL · staging</span><span>SQLite · cache</span><hr><b>SCHEMA</b><span>▾ public</span><span>　users</span><span>　workspaces</span><span>　agent_runs</span></div>
      <div class="db-main"><div class="query-editor"><span>query.sql</span><pre><code><em>SELECT</em> id, agent, status, duration_ms
<em>FROM</em> agent_runs
<em>WHERE</em> workspace_id = <b>'yuzora'</b>
<em>ORDER BY</em> created_at <em>DESC</em>;</code></pre></div><div class="result-table"><div><b>id</b><b>agent</b><b>status</b><b>duration_ms</b></div><div><span>1842</span><span>codex</span><span class="ok">done</span><span>8421</span></div><div><span>1841</span><span>claude</span><span class="ok">done</span><span>12044</span></div><div><span>1840</span><span>pi</span><span>review</span><span>6308</span></div></div></div>
    </div>
  </div>`;
}
