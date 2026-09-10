// Concept only: no IPC, network requests, PTY input or production imports of this module.
import { useEffect, useState, type CSSProperties } from 'react'
import { Bot, ChevronRight, Code2, Database, FileText, Files, GitBranch, Globe, Laptop, Layers, Moon, Send, Server, Settings, ShieldCheck, Smartphone, Sun, Terminal, Wifi, WifiOff } from 'lucide-react'
import { Button } from '@/components/ui/button'
import { Badge } from '@/components/ui/badge'
import { Tabs, TabsContent, TabsList, TabsTrigger } from '@/components/ui/tabs'
import { ToggleGroup, ToggleGroupItem } from '@/components/ui/toggle-group'
import { Select, SelectContent, SelectGroup, SelectItem, SelectTrigger, SelectValue } from '@/components/ui/select'
import { ScrollArea } from '@/components/ui/scroll-area'
import { Field, FieldContent, FieldDescription, FieldLabel } from '@/components/ui/field'
import { Switch } from '@/components/ui/switch'
import { Textarea } from '@/components/ui/textarea'
import { SpaceCharacter } from '@/app/workbench/SpaceCharacter'
import { loadAppearanceSettings } from '@/app/workbench/settingsStorage'
import { applyAccentPreference } from '@/theme/accent'
import './mobile-demo.css'

type Status = 'working' | 'blocked' | 'idle' | 'done' | 'unknown'
const agents: {id:string;name:string;title:string;en:string;state:Status;branch:string;pane:string}[] = [
  {id:'codex-ui',name:'Codex',title:'調整側欄與工作分頁',en:'Refine sidebar and work tabs',state:'blocked',branch:'feat/mobile-companion',pane:'pane-1'},
  {id:'claude-docs',name:'Claude',title:'檢視文件變更',en:'Review documentation changes',state:'done',branch:'feat/mobile-companion',pane:'pane-2'},
  {id:'codex-shell',name:'Codex',title:'整理視窗互動',en:'Refine window interactions',state:'working',branch:'feat/mobile-companion',pane:'pane-3'},
  {id:'claude-preview',name:'Claude',title:'等待下一步',en:'Waiting for input',state:'idle',branch:'feat/mobile-companion',pane:'pane-4'},
  {id:'codex-unknown',name:'Codex',title:'連線狀態未確認',en:'Connection not confirmed',state:'unknown',branch:'main',pane:'pane-5'},
  {id:'claude-tests',name:'Claude',title:'整理驗證記錄',en:'Organize validation notes',state:'working',branch:'main',pane:'pane-6'},
  {id:'codex-review',name:'Codex',title:'等待確認修改範圍',en:'Confirm change scope',state:'blocked',branch:'main',pane:'pane-7'},
  {id:'claude-idle',name:'Claude',title:'待命中',en:'Standing by',state:'idle',branch:'main',pane:'pane-8'},
]
const statusText:Record<Status,[string,string]> = {working:['工作中','Working'],blocked:['需回覆','Needs reply'],idle:['待命','Idle'],done:['回覆待查看','Unseen reply'],unknown:['未知','Unknown']}
const files = ['README.md','src/app/AppShell.tsx','src/app/workbench/workbench-shell.css']
const frameSizes:Record<string,number> = {'375':812,'390':844,'430':932}

export function MobileDemo() {
  const [english,setEnglish] = useState(false)
  const t = (zh:string,en:string) => english ? en : zh
  const [width,setWidth] = useState('390')
  const [dark,setDark] = useState(()=>{const theme=loadAppearanceSettings().theme;return theme==='dark'||(theme==='auto'&&window.matchMedia('(prefers-color-scheme: dark)').matches)})
  useEffect(()=>{applyAccentPreference(loadAppearanceSettings().accent)},[])
  useEffect(()=>{document.documentElement.classList.toggle('dark',dark)},[dark])
  const [page,setPage] = useState('agents')
  const [scope,setScope] = useState('feature')
  const [selected,setSelected] = useState(agents[0])
  const [view,setView] = useState('terminal')
  const [file,setFile] = useState(files[0])
  const [service,setService] = useState(false)
  const [autoStart,setAutoStart] = useState(false)
  const [paired,setPaired] = useState(false)
  const [control,setControl] = useState(false)
  const [scenario,setScenario] = useState('normal')
  const [drafts,setDrafts] = useState<Record<string,string>>({})
  const draft = drafts[selected.id] ?? ''
  const setDraft = (value:string) => setDrafts(current=>({...current,[selected.id]:value}))
  const [notice,setNotice] = useState('')
  const [demoInputs,setDemoInputs] = useState<Record<string,string[]>>({})
  const [previewTool,setPreviewTool] = useState('files')
  const docs = scope === 'docs'
  const live = service && paired && scenario === 'normal' && !docs
  const canInput = live && control && selected.state !== 'unknown'
  const reason = docs ? t('Docs 的 Herdr Session 已停止。請在電腦上重新啟動。','The Docs Herdr Session is stopped. Restart it on your computer.') : !service ? t('先在設定模擬啟動 Web Service。','Simulate starting Web Service in Settings first.') : !paired ? t('服務已啟動，等待手機配對（模擬）。','Service started; waiting for phone pairing (demo).') : scenario === 'stale' ? t('連線已中斷，以下為上次資料；重新連線後才能控制。','Disconnected. Showing last data; reconnect before controlling.') : scenario === 'unsupported' ? t('此 Session 不支援遠端輸入，仍可查看資料。','This Session does not support remote input; viewing is available.') : scenario === 'error' ? t('服務無法連線，請檢查電腦上的服務狀態後重試。','Cannot reach the service. Check it on your computer and retry.') : !control ? t('目前是觀察者。請先在電腦端授予控制權。','Observer mode. Grant control on the computer first.') : selected.state === 'unknown' ? t('Agent 狀態未知，待重新確認所屬 pane 後才能輸入。','Agent state is unknown. Confirm the owning pane before sending input.') : t('控制者 · 輸入只保留在這個模擬頁。','Controller · Input stays in this demo page.')
  function openAgent(agent:typeof selected) { setSelected(agent); setScope(agent.branch === 'main' ? 'main' : 'feature'); setPage('work'); setView('terminal'); setNotice('') }
  function stopService() {setService(false);setPaired(false);setControl(false);setNotice(t('模擬服務已停止，配對與控制權已清除。','Demo stopped; pairing and control were cleared.'))}
  function restartApp() {setService(autoStart);setPaired(false);setControl(false);setScenario('normal');setNotice(t('已模擬 App 開啟；請重新配對。','App launch simulated; pair again.'))}
  function setTheme() {const next=!dark;setDark(next);document.documentElement.classList.toggle('dark',next)}
  const visibleAgents = agents.filter(a=>scope === 'all' || a.branch === (scope === 'main' ? 'main':'feat/mobile-companion'))
  const activePane = Number(selected.pane.split('-')[1])

  return <div className="mobile-studio">
    <div className="mobile-studio-toolbar">
      <a href="?theme=light&accent=violet"><Laptop aria-hidden="true"/>{t('回桌面模擬','Desktop demo')}</a>
      <div><strong>Yuzora Mobile</strong><span>{t('概念原型 · 不連線真實服務','Concept prototype · No live service')}</span></div>
      <ToggleGroup type="single" value={width} onValueChange={v=>v&&setWidth(v)} aria-label={t('手機尺寸','Phone size')}>
        {Object.keys(frameSizes).map(size=><ToggleGroupItem key={size} value={size}>{size}</ToggleGroupItem>)}
      </ToggleGroup>
      <Button variant="ghost" size="icon" aria-label={t('切換明暗','Toggle theme')} onClick={setTheme}>{dark?<Sun/>:<Moon/>}</Button>
      <Button variant="ghost" onClick={()=>setEnglish(!english)}>{english?'繁中':'EN'}</Button>
    </div>
    <div className="mobile-stage">
      <div className="mobile-device" style={{'--device-width':`${width}px`,'--device-height':`${frameSizes[width]}px`} as CSSProperties}>
        <div className="mobile-phone">
          <header className="mobile-header"><span className="mobile-wordmark">y.</span><strong>Yuzora</strong><Badge variant="outline">DEMO</Badge><Button variant="ghost" aria-label={t('連線設定','Connection settings')} onClick={()=>setPage('settings')}>{live?<Wifi/>:<WifiOff/>}<span>{live ? control?t('控制者','Controller'):t('觀察者','Observer') : t('未連線','Offline')}</span></Button></header>
          <div className="mobile-context">
            <span className="mobile-avatar" style={{'--space-color':docs?'#baa5e3':'#efd18d','--character-ink':'#493c46'} as CSSProperties}><SpaceCharacter character={{shell:'box',face:'curious',detail:'none',motion:true}} portrait/></span>
            <div><Select value={scope} onValueChange={v=>{setScope(v);setNotice('');if(v==='main')setSelected(agents[4]);if(v==='feature')setSelected(agents[0]);if(v==='docs')setPage('agents')}}><SelectTrigger aria-label={t('Space、分支與 Session','Space, branch and Session')}><SelectValue/></SelectTrigger><SelectContent><SelectGroup><SelectItem value="all">All · {t('已載入快照','Loaded snapshots')}</SelectItem><SelectItem value="feature">Yuzora · feat/mobile-companion</SelectItem><SelectItem value="main">Yuzora · main</SelectItem><SelectItem value="docs">Docs · main</SelectItem></SelectGroup></SelectContent></Select><small>Herdr {docs?'docs':'default'} · {docs?t('Server 已停止','Server stopped'):t('電腦：Yuuzu 的 Mac','Computer: Yuuzu’s Mac')}</small></div>
          </div>
          <div className="mobile-demo-note">{t('模擬資料與操作 · 未連接任何電腦','Demo data and actions · No computer connected')}</div>
          <Tabs value={page} onValueChange={setPage} className="mobile-main-tabs">
            <TabsContent value="agents" className="mobile-panel">
              <ScrollArea className="mobile-scroll" contentClassName="mobile-reading">
                <div className="mobile-section-title"><div><small>{t('保持工作脈絡','STAY IN CONTEXT')}</small><h1>{t('你的 Agents','Your Agents')}</h1></div><Badge variant="secondary">{docs?0:visibleAgents.length}</Badge></div>
                {docs ? <div className="mobile-empty"><Server/><h2>{t('Session 已停止','Session stopped')}</h2><p>{reason}</p><p>{t('上次資料不代表即時狀態。','Last data is not a live status.')}</p></div> : <>
                  <p className="mobile-muted">{t('先處理需要回覆的工作，再看新的輸出。','Reply where needed, then review new output.')}</p>
                  <div className="mobile-agent-list">{visibleAgents.map(a=><Button variant="ghost" key={a.id} onClick={()=>openAgent(a)} className="mobile-agent-row"><span className="mobile-state-dot" data-state={a.state}/><span><strong>{t(a.title,a.en)}</strong><small>{a.name} · {a.pane}</small></span><span className="mobile-state-label" data-state={a.state}>{t(...statusText[a.state])}</span><ChevronRight/></Button>)}</div>
                  <p className="mobile-footnote">{t('「回覆待查看」只代表尚未檢視，不保證任務成功。All 僅彙整已載入的 Session。','“Unseen reply” does not imply success. All includes loaded Sessions only.')}</p>
                </>}
              </ScrollArea>
            </TabsContent>
            <TabsContent value="work" forceMount hidden={page!=='work'} className="mobile-panel">
              <div className="mobile-work-title"><span className="mobile-state-dot" data-state={selected.state}/><strong>{t(selected.title,selected.en)}</strong><small>{selected.name}</small></div>
              <ToggleGroup className="mobile-view-switcher" type="single" value={view} onValueChange={v=>v&&setView(v)} aria-label={t('工作內容','Work content')}><ToggleGroupItem value="terminal"><Terminal/>{t('終端機','Terminal')}</ToggleGroupItem><ToggleGroupItem value="file"><FileText/>{t('文件','File')}</ToggleGroupItem><ToggleGroupItem value="diff"><GitBranch/>Diff</ToggleGroupItem></ToggleGroup>
              <ScrollArea className="mobile-scroll" contentClassName="mobile-reading">
                <p className="mobile-inline-notice">{reason}</p>
                {docs ? <div className="mobile-empty"><Server/><h2>{t('Session 已停止','Session stopped')}</h2><p>{t('此 Session 沒有可觀察的 pane。','There is no observable pane in this Session.')}</p></div> : view==='terminal' ? <>
                  <div className="mobile-pane-heading"><code>{selected.pane}</code><span>Herdr default · {selected.branch}</span></div>
                  <div className="mobile-pane-picker" aria-label={t('同一分支的 panes','Panes in this branch')}>{agents.filter(a=>a.branch===selected.branch).map(a=><Button key={a.id} variant={a.id===selected.id?'secondary':'ghost'} aria-pressed={a.id===selected.id} onClick={()=>{setSelected(a);setNotice('')}}>{a.pane}</Button>)}</div>
                  <pre className="mobile-terminal" aria-label={t('示意終端機輸出','Sample terminal output')}>{`# DEMO · ${selected.name}\n# Session: default\n# Pane: ${activePane}\n\n$ git diff --stat\n AppShell.tsx       | 12 +++++---\n workbench.css      |  8 ++++--\n\n${t('已準備好側欄修改供你檢視。','Sidebar changes are ready to review.')}\n${t('要保留原本的視窗邊距嗎？','Keep the existing window margin?')}\n\n${t('這是固定範例輸出。','This is static sample output.')}${(demoInputs[selected.id]??[]).map(input=>'\n\n[DEMO input] '+input).join('')}`}</pre>
                </> : view==='file' ? <article className="mobile-document"><small>{file}</small><h1>Yuzora Mobile</h1><p>{t('離開電腦，也能接續正在進行的工作。','Keep up with your work while away from your computer.')}</p><h2>{t('先知道發生什麼事','Start with context')}</h2><p>{t('Space、分支與 Herdr Session 保持可見。一次觀察一個 pane，必要時再切換。','Space, branch and Herdr Session stay visible. Observe one pane at a time.')}</p><h2>{t('操作前確認控制權','Confirm control first')}</h2><p>{t('手機預設為觀察者，只有取得控制權後才能輸入。','The phone starts as an observer. Input requires control.')}</p><p>{t('文件展示僅為原型，不會寫入檔案。','Document display is a prototype and does not write files.')}</p></article> : <div className="mobile-diff"><small>{file} · {t('單欄 Diff 示意','Unified diff sample')}</small><pre><span>  .sidebar {'\n'}</span><del>-   padding-top: 4px;{'\n'}</del><ins>+   padding-top: 12px;{'\n'}</ins><span>  {'}'}</span></pre><p>{t('手機預設單欄，減少左右捲動。','Unified view reduces horizontal scrolling on phones.')}</p></div>}
              </ScrollArea>
              {view==='terminal'&&!docs&&<form className="mobile-composer" onSubmit={e=>{e.preventDefault();if(canInput&&draft.trim()){setDemoInputs(current=>({...current,[selected.id]:[...(current[selected.id]??[]),draft]}));setNotice(t('模擬輸入已保留於本頁；未送往 Herdr。','Demo input recorded locally; nothing sent to Herdr.'));setDraft('')}}}><Field><FieldLabel htmlFor="mobile-input">{t('回覆 Agent','Reply to Agent')}</FieldLabel><Textarea id="mobile-input" value={draft} onChange={e=>setDraft(e.target.value)} disabled={!canInput} placeholder={canInput?t('輸入模擬回覆…','Type a demo reply…'):t('取得控制權後才能輸入','Control is required to type')} rows={2}/></Field><Button type="submit" disabled={!canInput||!draft.trim()}><Send data-icon="inline-start"/>{t('模擬傳送','Simulate send')}</Button></form>}
            </TabsContent>
            <TabsContent value="tools" className="mobile-panel"><ScrollArea className="mobile-scroll" contentClassName="mobile-reading">
              <div className="mobile-section-title"><div><small>{t('查看產物','REVIEW ARTIFACTS')}</small><h1>{t('工作工具','Tools')}</h1></div></div>
              <div className="mobile-tool-grid">{[{id:'files',label:t('檔案','Files'),icon:Files},{id:'git',label:'Git',icon:GitBranch},{id:'preview',label:t('預覽','Preview'),icon:Globe},{id:'database',label:t('資料庫','Database'),icon:Database},{id:'sftp',label:'SSH / SFTP',icon:Server}].map(item=><Button key={item.id} variant={previewTool===item.id?'secondary':'ghost'} aria-pressed={previewTool===item.id} onClick={()=>setPreviewTool(item.id)}><item.icon data-icon="inline-start"/>{item.label}</Button>)}</div>
              <h2>{previewTool==='files'?t('這個分支的文件','Files in this branch'):previewTool==='git'?t('待檢視的變更','Changes to review'):previewTool==='preview'?t('網站預覽','Website preview'):t('共用工具','Shared tools')}</h2>
              {['files','git'].includes(previewTool)?<div className="mobile-file-list">{files.map((name,index)=><Button variant="ghost" key={name} onClick={()=>{setFile(name);setView(previewTool==='git'?'diff':'file');setPage('work')}}><FileText data-icon="inline-start"/><span>{name}</span>{previewTool==='git'&&<small>{index===0?'A':'M'}</small>}<ChevronRight/></Button>)}</div>:previewTool==='preview'?<div className="mobile-preview-sample"><Badge variant="outline">DEMO</Badge><h2>{t('工作，在這裡接續。','Pick up where you left off.')}</h2><p>{t('Preview 版面示意，不連接 localhost 或外部網站。','Preview sample; no localhost or external site is loaded.')}</p></div>:<div className="mobile-empty"><Server/><p>{t('資料庫與 SSH／SFTP 為跨 Space 共用工具。','Database and SSH/SFTP are shared across Spaces.')}</p><p>{t('手機連線管理與權限流程尚未實作。此處僅保留入口示意。','Mobile connection management is not implemented. This is an entry-point concept.')}</p></div>}
            </ScrollArea></TabsContent>
            <TabsContent value="settings" className="mobile-panel"><ScrollArea className="mobile-scroll" contentClassName="mobile-reading">
              <div className="mobile-section-title"><div><small>{t('從電腦接續','CONNECT FROM YOUR COMPUTER')}</small><h1>{t('設定','Settings')}</h1></div><Settings/></div>
              <section className="mobile-service"><div className="mobile-service-title"><Wifi/><h2>Web Service</h2><Badge variant="outline">{service?t('已啟動 · 模擬','Started · Demo'):t('已停止','Stopped')}</Badge></div><p>{t('電腦端設定的手機版示意。實際服務啟動入口將位於桌面 App 設定。','Mobile concept of desktop settings. The actual service will be started from the desktop app.')}</p>
                <Field orientation="horizontal"><FieldContent><FieldLabel htmlFor="auto-service">{t('App 開啟時啟動服務','Start service when app opens')}</FieldLabel><FieldDescription>{t('只記錄這次模擬的選擇。','Applies to this demo session only.')}</FieldDescription></FieldContent><Switch id="auto-service" checked={autoStart} onCheckedChange={setAutoStart}/></Field>
                <div className="mobile-service-actions"><Button onClick={()=>{if(service)stopService();else{setService(true);setScenario('normal');setNotice('')}}}>{service?t('模擬停止服務','Simulate stop'):t('模擬啟動服務','Simulate start')}</Button><Button variant="outline" onClick={restartApp}>{t('模擬開啟 App','Simulate app launch')}</Button></div>
                {service&&<div className="mobile-pair"><code>https://your-mac.local:port</code><small>{t('示意位址，不能連線','Example address; not connectable')}</small><p>{t('使用手機配對，並在電腦確認後開始觀察。','Pair your phone and confirm on your computer to observe.')}</p><Button variant="secondary" disabled={paired} onClick={()=>{setPaired(true);setControl(false);setScenario('normal')}}><Smartphone data-icon="inline-start"/>{paired?t('已模擬配對','Paired · Demo'):t('模擬完成配對','Simulate pairing')}</Button></div>}
              </section>
              <section className="mobile-service"><h2>{t('手機控制權','Phone control')}</h2><p>{reason}</p><Button variant="outline" disabled={!live} onClick={()=>setControl(!control)}><ShieldCheck data-icon="inline-start"/>{control?t('模擬釋放控制權','Simulate release'):t('模擬電腦授予控制權','Simulate desktop approval')}</Button><p className="mobile-footnote">{t('關閉手機頁面不等於停止 Herdr process。','Closing this page does not stop Herdr processes.')}</p></section>
              <section className="mobile-service"><h2>{t('測試情境','Test scenarios')}</h2><Select value={scenario} onValueChange={v=>{setScenario(v);if(v!=='normal')setControl(false)}}><SelectTrigger aria-label={t('連線測試情境','Connection test scenario')}><SelectValue/></SelectTrigger><SelectContent><SelectGroup><SelectItem value="normal">{t('正常','Normal')}</SelectItem><SelectItem value="stale">{t('連線中斷 / stale','Disconnected / stale')}</SelectItem><SelectItem value="unsupported">{t('不支援輸入','Input unsupported')}</SelectItem><SelectItem value="error">{t('連線錯誤','Connection error')}</SelectItem></SelectGroup></SelectContent></Select>{scenario!=='normal'&&<Button variant="secondary" onClick={()=>{setScenario('normal');setControl(false)}}>{t('模擬重新連線','Simulate reconnect')}</Button>}</section>
              <section className="mobile-service"><h2>{t('外觀','Appearance')}</h2><div className="mobile-service-actions"><Button variant="outline" onClick={setTheme}>{dark?<Sun data-icon="inline-start"/>:<Moon data-icon="inline-start"/>}{t('切換明暗','Toggle theme')}</Button><Button variant="outline" onClick={()=>applyAccentPreference('violet')}>{t('紫羅蘭','Violet')}</Button><Button variant="outline" onClick={()=>applyAccentPreference('lime')}>{t('青檸','Lime')}</Button></div></section>
            </ScrollArea></TabsContent>
            {notice&&<p className="mobile-action-notice" role="status">{notice}</p>}
            <TabsList className="mobile-bottom-nav" aria-label={t('手機導覽','Mobile navigation')}>
              <TabsTrigger value="work"><Code2/><span>{t('工作','Work')}</span></TabsTrigger><TabsTrigger value="agents"><Bot/><span>Agents</span></TabsTrigger><TabsTrigger value="tools"><Layers/><span>{t('工具','Tools')}</span></TabsTrigger><TabsTrigger value="settings"><Settings/><span>{t('設定','Settings')}</span></TabsTrigger>
            </TabsList>
          </Tabs>
        </div>
      </div>
      <aside className="mobile-studio-notes"><Badge variant="outline">MOBILE CONCEPT</Badge><h2>{t('接續工作，\n不丟掉上下文。','Your work,\nstill in context.')}</h2><p>{t('一個 Space、一個分支、一次專注在一個 pane。需要你回覆的工作先出現。','One Space, one branch, one pane at a time. Work that needs you comes first.')}</p><ol><li>{t('設定 → 模擬啟動 Web Service','Settings → Simulate service start')}</li><li>{t('模擬配對 → 電腦授予控制權','Pair → Simulate desktop approval')}</li><li>{t('Agents → 選取 → 模擬回覆','Agents → Select → Simulate reply')}</li></ol><p>{t('全部狀態與輸出皆為示意。此頁不啟動伺服器、不操作 Herdr，也不對區域網路開放連線。','All state and output are examples. This page starts no server, controls no Herdr runtime and opens no network listener.')}</p><small>{t('尺寸選擇是 CSS 畫板，不是真機驗收。','Sizes are CSS artboards, not device acceptance.')}</small></aside>
    </div>
  </div>
}
