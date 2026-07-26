<script lang="ts">
  import { onMount } from "svelte";
  import {
    Activity, ArrowRight, Blocks, Check, ChevronDown, CircleGauge,
    CircleStop, Database, FileClock, FileSliders, Gauge, KeyRound,
    Layers3, LoaderCircle, Network, Play, Plus, RefreshCw, RotateCcw,
    Save, Search, Server, Settings2, ShieldCheck, SquareTerminal, Trash2,
    Unplug, X, Zap
  } from "lucide-svelte";
  import * as api from "./lib/api";
  import type { AppConfig, LogEntry, Mapping, ModelConfig, Provider, Status } from "./lib/types";

  type Page = "dashboard" | "providers" | "models" | "mappings" | "codex" | "logs" | "settings";
  const nav: { id: Page; label: string; icon: typeof CircleGauge }[] = [
    { id: "dashboard", label: "仪表盘", icon: CircleGauge },
    { id: "providers", label: "供应商", icon: Server },
    { id: "models", label: "模型", icon: Blocks },
    { id: "mappings", label: "映射", icon: Network },
    { id: "codex", label: "Codex 设置", icon: FileSliders },
    { id: "logs", label: "日志", icon: SquareTerminal },
    { id: "settings", label: "设置", icon: Settings2 }
  ];

  let page: Page = "dashboard";
  let config: AppConfig = { providers: [], mappings: [], settings: { bindAddress: "127.0.0.1", port: 8787, requestTimeoutSeconds: 120, maxBodyMb: 20, restartCodexOnChange: true, startOnLaunch: false } };
  let status: Status = { running: false, address: "127.0.0.1:8787", startedAt: null, requestCount: 0, errorCount: 0, configInjected: false, configPath: "~/.codex/config.toml", backupPath: null };
  let logs: LogEntry[] = [];
  let busy = false;
  let loading = true;
  let toast = "";
  let search = "";
  let providerDialog = false;
  let mappingDialog = false;
  let editingProvider: Provider | null = null;
  let editingMapping: Mapping | null = null;
  let testingId = "";
  let logTimer: ReturnType<typeof setInterval> | undefined;

  const uid = () => crypto.randomUUID();
  const blankProvider = (): Provider => ({ id: uid(), name: "", baseUrl: "", apiKey: "", protocol: "responses", enabled: true, models: [] });
  const blankMapping = (): Mapping => ({ id: uid(), codexModel: "", providerId: config.providers[0]?.id ?? "", upstreamModel: "", enabled: true });
  const providerName = (id: string) => config.providers.find(p => p.id === id)?.name ?? "未知供应商";
  const modelTotal = () => config.providers.reduce((n, p) => n + p.models.length, 0);
  const enabledModels = () => config.providers.reduce((n, p) => n + p.models.filter(m => m.enabled).length, 0);
  const enabledProviders = () => config.providers.filter(p => p.enabled).length;

  onMount(() => {
    void (async () => {
      try {
        [config, status, logs] = await Promise.all([api.loadConfig(), api.getStatus(), api.getLogs()]);
        logTimer = setInterval(refreshRuntime, 1800);
      } catch (e) { showToast(String(e), true); }
      finally { loading = false; }
    })();
    return () => clearInterval(logTimer);
  });

  async function refreshRuntime() {
    try { [status, logs] = await Promise.all([api.getStatus(), api.getLogs()]); } catch { /* app may be closing */ }
  }
  function showToast(message: string, error = false) {
    toast = `${error ? "错误：" : ""}${message}`;
    setTimeout(() => { toast = ""; }, 3200);
  }
  async function persist(message = "设置已保存") {
    try {
      config = await api.saveConfig(config);
      showToast(message);
    } catch (e) {
      try { config = await api.loadConfig(); } catch { /* keep the current form if reload fails */ }
      showToast(String(e), true);
    }
  }
  async function toggleProxy(start: boolean) {
    busy = true;
    try {
      status = start ? await api.startProxy() : await api.stopProxy();
      showToast(start ? "代理已启动；已注入配置并尝试重启 ChatGPT/Codex" : "代理已停止；Codex 已关闭，原始配置已恢复");
      logs = await api.getLogs();
    } catch (e) { showToast(String(e), true); }
    finally { busy = false; }
  }
  function openProvider(provider?: Provider) {
    editingProvider = structuredClone(provider ?? blankProvider());
    providerDialog = true;
  }
  async function commitProvider() {
    if (!editingProvider?.name.trim() || !editingProvider.baseUrl.trim()) return showToast("请填写名称和 Base URL", true);
    editingProvider.baseUrl = editingProvider.baseUrl.replace(/\/+$/, "");
    const i = config.providers.findIndex(p => p.id === editingProvider?.id);
    if (i >= 0) config.providers[i] = editingProvider; else config.providers = [...config.providers, editingProvider];
    providerDialog = false; editingProvider = null;
    await persist("供应商已保存");
  }
  async function removeProvider(id: string) {
    if (!confirm("删除该供应商及关联映射？此操作无法撤销。")) return;
    config.providers = config.providers.filter(p => p.id !== id);
    config.mappings = config.mappings.filter(m => m.providerId !== id);
    await persist("供应商已删除");
  }
  async function runProviderTest(id: string) {
    testingId = id;
    try {
      const models = await api.testProvider(id);
      const p = config.providers.find(x => x.id === id);
      if (p) {
        const old = new Map(p.models.map(m => [m.id, m]));
        p.models = models.map(model => ({
          ...model,
          enabled: old.get(model.id)?.enabled ?? true,
          contextWindow: model.contextWindow || old.get(model.id)?.contextWindow || 1050000
        }));
        await persist(`连接成功，发现 ${models.length} 个模型`);
      }
    } catch (e) { showToast(String(e), true); }
    finally { testingId = ""; }
  }
  async function addModelToMapping(provider: Provider, model: ModelConfig) {
    if (status.running) return showToast("请先停止代理，再添加模型映射", true);
    if (config.mappings.some(m => m.codexModel === model.id)) {
      alert(`Codex 模型名称“${model.id}”已存在映射，未重复添加。`);
      return;
    }
    const next = structuredClone(config);
    next.mappings = [...next.mappings, { id: uid(), codexModel: model.id, providerId: provider.id, upstreamModel: model.id, enabled: true }];
    try {
      config = await api.saveConfig(next);
      showToast(`已将 ${model.id} 添加到映射`);
    } catch (e) {
      try { config = await api.loadConfig(); } catch { /* keep the current form if reload fails */ }
      showToast(String(e), true);
    }
  }
  async function toggleModel(model: ModelConfig) {
    model.enabled = !model.enabled;
    await persist(model.enabled ? "模型已启用" : "模型已禁用");
  }
  function openMapping(mapping?: Mapping) {
    editingMapping = structuredClone(mapping ?? blankMapping()); mappingDialog = true;
  }
  async function commitMapping() {
    if (status.running) return showToast("请先停止代理，再保存模型映射", true);
    if (!editingMapping?.codexModel.trim() || !editingMapping.providerId || !editingMapping.upstreamModel.trim()) return showToast("请完整填写映射", true);
    const next = structuredClone(config);
    const i = next.mappings.findIndex(m => m.id === editingMapping?.id);
    if (i >= 0) next.mappings[i] = editingMapping; else next.mappings = [...next.mappings, editingMapping];
    try {
      config = await api.saveConfig(next);
      mappingDialog = false; editingMapping = null;
      showToast("模型映射已保存");
    } catch (e) {
      try { config = await api.loadConfig(); } catch { /* keep the current form if reload fails */ }
      showToast(String(e), true);
    }
  }
  async function removeMapping(id: string) {
    if (status.running) return showToast("请先停止代理，再删除模型映射", true);
    config.mappings = config.mappings.filter(m => m.id !== id); await persist("映射已删除");
  }
  async function restoreConfig() {
    if (!confirm("确认用启动前的备份恢复 Codex 配置？")) return;
    try { status = await api.restoreCodex(); showToast("Codex 配置已恢复"); } catch (e) { showToast(String(e), true); }
  }
  async function backupConfig() {
    try { status = await api.backupCodex(); showToast("Codex 配置已备份"); } catch (e) { showToast(String(e), true); }
  }
  async function refreshCatalog() {
    try { const count = await api.refreshCatalog(); showToast(`模型目录已刷新，共 ${count} 个模型`); } catch (e) { showToast(String(e), true); }
  }
  async function restartCodexApp() {
    try { await api.restartCodex(); showToast("Codex 已重启"); } catch (e) { showToast(String(e), true); }
  }
  async function clearLogsNow() { await api.clearLogs(); logs = []; showToast("日志已清空"); }
  const time = (unix: number) => new Date(unix * 1000).toLocaleTimeString("zh-CN", { hour12: false });
</script>

{#if loading}
  <main class="loading"><div class="brand-mark"><Zap size={24} /></div><LoaderCircle class="spin" size={26}/><span>正在准备工作区…</span></main>
{:else}
  <div class="shell">
    <aside>
      <div class="brand"><div class="brand-mark"><Zap size={19}/></div><div><strong>OnlyCodex</strong><small>PROXY DESKTOP</small></div></div>
      <nav>
        <p>工作区</p>
        {#each nav as item}
          <button class:active={page === item.id} onclick={() => page = item.id}>
            <item.icon size={18}/><span>{item.label}</span>
            {#if item.id === "logs" && status.errorCount > 0}<em>{status.errorCount}</em>{/if}
          </button>
        {/each}
      </nav>
      <div class="sidebar-state">
        <div class="status-dot" class:online={status.running}></div>
        <div><strong>{status.running ? "代理运行中" : "代理已停止"}</strong><small>{status.address}</small></div>
      </div>
    </aside>

    <section class="workspace">
      <header>
        <div><span class="eyebrow">OPENAI RESPONSES BRIDGE</span><h1>{nav.find(n => n.id === page)?.label}</h1></div>
        <div class="header-actions">
          <button class="icon-button" onclick={refreshRuntime} title="刷新"><RefreshCw size={17}/></button>
          <div class="runtime-pill"><span class:online={status.running}></span>{status.running ? "ONLINE" : "OFFLINE"}</div>
        </div>
      </header>

      <div class="content">
        {#if page === "dashboard"}
          <div class="hero-grid">
            <section class="hero-card">
              <div class="hero-copy">
                <span class="section-kicker"><Activity size={14}/> SYSTEM CONTROL</span>
                <h2>{status.running ? "代理正在接管 Codex 流量" : "让 Codex 使用你的任意模型"}</h2>
                <p>{status.running ? `本地 Responses API 已监听 ${status.address}，所有启用映射均可用。` : "一键完成代理启动、配置安全备份与 Codex 注入。停止时原配置会被完整恢复。"}</p>
              </div>
              <div class="power-actions">
                <button class="power start" disabled={busy || status.running} onclick={() => toggleProxy(true)}>
                  {#if busy && !status.running}<LoaderCircle class="spin" size={24}/>{:else}<Play size={24} fill="currentColor"/>{/if}
                  <span><strong>启动代理</strong><small>START PROXY</small></span><ArrowRight size={20}/>
                </button>
                <button class="power stop" disabled={busy || !status.running} onclick={() => toggleProxy(false)}>
                  {#if busy && status.running}<LoaderCircle class="spin" size={24}/>{:else}<CircleStop size={24}/>{/if}
                  <span><strong>停止并恢复</strong><small>STOP & RESTORE</small></span>
                </button>
              </div>
            </section>
            <section class="health-card">
              <div class="radar"><div></div><div></div><div></div><span class:online={status.running}></span></div>
              <div><span class="mono-label">SERVICE HEALTH</span><strong>{status.running ? "100%" : "—"}</strong><small>{status.running ? "本地服务响应正常" : "等待启动"}</small></div>
            </section>
          </div>

          <div class="stats-grid">
            <article><div class="stat-icon coral"><Server size={20}/></div><div><small>启用供应商</small><strong>{enabledProviders()}</strong><span>/ {config.providers.length} TOTAL</span></div></article>
            <article><div class="stat-icon blue"><Layers3 size={20}/></div><div><small>可用模型</small><strong>{enabledModels()}</strong><span>/ {modelTotal()} DISCOVERED</span></div></article>
            <article><div class="stat-icon lime"><Network size={20}/></div><div><small>活动映射</small><strong>{config.mappings.filter(m => m.enabled).length}</strong><span>CODEX ROUTES</span></div></article>
            <article><div class="stat-icon amber"><Gauge size={20}/></div><div><small>已处理请求</small><strong>{status.requestCount}</strong><span>{status.errorCount} ERRORS</span></div></article>
          </div>

          <div class="split-grid">
            <section class="panel">
              <div class="panel-title"><div><span class="section-kicker">ROUTING TABLE</span><h3>当前模型映射</h3></div><button class="text-button" onclick={() => page = "mappings"}>管理映射 <ArrowRight size={15}/></button></div>
              {#if config.mappings.length === 0}<div class="empty compact"><Network size={28}/><span>尚未配置映射</span><button onclick={() => openMapping()}>创建第一个映射</button></div>
              {:else}<div class="route-list">{#each config.mappings.slice(0, 5) as m}<div><code>{m.codexModel}</code><ArrowRight size={14}/><span>{providerName(m.providerId)}</span><strong>{m.upstreamModel}</strong><i class:enabled={m.enabled}></i></div>{/each}</div>{/if}
            </section>
            <section class="panel live-log">
              <div class="panel-title"><div><span class="section-kicker">LIVE ACTIVITY</span><h3>最近日志</h3></div><button class="text-button" onclick={() => page = "logs"}>全部日志 <ArrowRight size={15}/></button></div>
              <div class="mini-terminal">{#if logs.length === 0}<span class="muted">暂无运行日志</span>{:else}{#each logs.slice(-6).reverse() as log}<div><time>{time(log.timestamp)}</time><b class:error={log.level === "ERROR"}>{log.level}</b><span>{log.message}</span></div>{/each}{/if}</div>
            </section>
          </div>

        {:else if page === "providers"}
          <div class="page-toolbar"><div><p>连接 OpenAI-compatible 或原生 Responses API 服务。</p></div><button class="primary" onclick={() => openProvider()}><Plus size={17}/> 添加供应商</button></div>
          {#if config.providers.length === 0}<section class="empty large"><div class="empty-art"><Server size={36}/></div><h3>还没有供应商</h3><p>添加一个 API 服务，并自动发现它提供的模型。</p><button class="primary" onclick={() => openProvider()}><Plus size={17}/> 添加第一个供应商</button></section>
          {:else}<div class="card-grid">{#each config.providers as provider}<article class="provider-card">
            <div class="provider-head"><div class="provider-logo">{provider.name.slice(0, 2).toUpperCase()}</div><div><h3>{provider.name}</h3><span class:enabled={provider.enabled} class="badge">{provider.enabled ? "ACTIVE" : "DISABLED"}</span></div><button class="icon-button" onclick={() => openProvider(provider)}><Settings2 size={17}/></button></div>
            <div class="provider-url"><Unplug size={15}/><span>{provider.baseUrl}</span></div>
            <div class="provider-meta"><span><Layers3 size={15}/>{provider.models.length} 模型</span><span><KeyRound size={15}/>{provider.apiKey ? "已配置密钥" : "无需密钥"}</span></div>
            <div class="card-actions"><button disabled={testingId === provider.id} onclick={() => runProviderTest(provider.id)}>{#if testingId === provider.id}<LoaderCircle class="spin" size={15}/>{:else}<Activity size={15}/>{/if}测试并同步模型</button><button class="danger-ghost" onclick={() => removeProvider(provider.id)}><Trash2 size={15}/></button></div>
          </article>{/each}</div>{/if}

        {:else if page === "models"}
          <div class="page-toolbar"><p>这里是供应商模型；只有在“映射”页创建的 Codex 模型名称才会写入 Codex 列表。</p><label class="search"><Search size={16}/><input bind:value={search} placeholder="搜索模型…"/></label></div>
          {#if modelTotal() === 0}<section class="empty large"><div class="empty-art"><Blocks size={36}/></div><h3>没有可用模型</h3><p>先添加供应商，然后点击“测试并同步模型”。</p><button class="primary" onclick={() => page = "providers"}>前往供应商</button></section>
          {:else}<div class="model-groups">{#each config.providers as provider}{#if provider.models.some(m => `${m.id} ${m.name}`.toLowerCase().includes(search.toLowerCase()))}<section class="panel model-group"><div class="model-provider"><div><div class="provider-logo small">{provider.name.slice(0,2).toUpperCase()}</div><strong>{provider.name}</strong></div><span>{provider.models.filter(m => m.enabled).length} / {provider.models.length} 已启用</span></div>
            <div class="table"><div class="table-head"><span>模型</span><span>上下文窗口</span><span>状态</span><span>操作</span></div>{#each provider.models.filter(m => `${m.id} ${m.name}`.toLowerCase().includes(search.toLowerCase())) as model}<div class="table-row"><div><strong>{model.name}</strong><code>{model.id}</code></div><label class="number"><input type="number" bind:value={model.contextWindow} onchange={() => persist("模型参数已保存")}/><span>tokens</span></label><button title={model.enabled ? "禁用模型" : "启用模型"} class:checked={model.enabled} class="switch" onclick={() => toggleModel(model)}><span></span></button><button class="add-mapping" disabled={status.running || config.mappings.some(m => m.codexModel === model.id)} title={config.mappings.some(m => m.codexModel === model.id) ? "该模型已存在映射" : "添加到映射"} onclick={() => addModelToMapping(provider, model)}><Plus size={13}/> {config.mappings.some(m => m.codexModel === model.id) ? "已添加" : "添加到映射"}</button></div>{/each}</div>
          </section>{/if}{/each}</div>{/if}

        {:else if page === "mappings"}
          <div class="page-toolbar"><p>每条已启用映射会生成一个 Codex 模型；修改前必须先停止代理。</p><button class="primary" disabled={config.providers.length === 0 || status.running} onclick={() => openMapping()}><Plus size={17}/> 新建映射</button></div>
          {#if config.mappings.length === 0}<section class="empty large"><div class="empty-art"><Network size={36}/></div><h3>映射表为空</h3><p>例如将 <code>gpt-5.4</code> 映射到 <code>Ollama:qwen3-coder</code>。</p><button class="primary" disabled={config.providers.length === 0 || status.running} onclick={() => openMapping()}><Plus size={17}/> 创建映射</button></section>
          {:else}<section class="panel mapping-panel"><div class="mapping-head"><span>CODEX MODEL</span><span>ROUTES TO</span><span>STATUS</span><span></span></div>{#each config.mappings as m}<div class="mapping-row"><code>{m.codexModel}</code><div class="route-target"><span>{providerName(m.providerId)}</span><ArrowRight size={14}/><strong>{m.upstreamModel}</strong></div><button disabled={status.running} title={m.enabled ? "禁用映射" : "启用映射"} class:checked={m.enabled} class="switch" onclick={() => {m.enabled = !m.enabled; persist("映射状态已更新")}}><span></span></button><div class="row-buttons"><button disabled={status.running} class="icon-button" title="编辑映射" onclick={() => openMapping(m)}><Settings2 size={16}/></button><button disabled={status.running} class="icon-button danger-ghost" title="删除映射" onclick={() => removeMapping(m.id)}><Trash2 size={16}/></button></div></div>{/each}</section>{/if}

        {:else if page === "codex"}
          <div class="codex-grid"><section class="panel config-state"><span class="section-kicker"><ShieldCheck size={14}/> CONFIG SAFETY</span><div class="big-state"><div class:active={status.configInjected}><FileClock size={32}/></div><div><h3>{status.configInjected ? "代理配置已注入" : "原始配置受保护"}</h3><p>{status.configInjected ? "Codex 当前通过 OnlyCodex 路由。停止代理会关闭 Codex 并恢复备份。" : "启动时会先逐字备份现有 config.toml，再进行结构化修改。"}</p></div></div><div class="path-row"><span>配置文件</span><code>{status.configPath}</code></div><div class="path-row"><span>安全备份</span><code>{status.backupPath ?? "尚未创建"}</code></div><div class="button-row"><button onclick={() => api.openCodexConfig()}><FileSliders size={16}/> 打开配置</button><button onclick={backupConfig}><FileClock size={16}/> 立即备份</button><button onclick={restoreConfig} disabled={!status.backupPath || status.running}><RotateCcw size={16}/> 恢复</button></div><div class="button-row"><button onclick={refreshCatalog}><RefreshCw size={16}/> 刷新模型目录</button><button onclick={restartCodexApp}><RotateCcw size={16}/> 重启 Codex</button></div></section>
          <section class="panel"><span class="section-kicker">INJECTED PROVIDER</span><div class="config-preview"><div><span>01</span><code>model_catalog_json = <b>"~/.codex/model-catalogs/opencodex.json"</b></code></div><div><span>02</span><code>[model_providers.opencodex_proxy]</code></div><div><span>03</span><code>base_url = <b>"http://{config.settings.bindAddress}:{config.settings.port}/v1"</b></code></div><div><span>04</span><code>wire_api = <b>"responses"</b></code></div><div><span>05</span><code>requires_openai_auth = <b>true</b></code></div></div><p class="note"><ShieldCheck size={16}/> 保留 Codex 模型目录、历史、记忆和插件上下文；模型请求仍由 OnlyCodex 使用供应商密钥直连。</p></section></div>

        {:else if page === "logs"}
          <div class="page-toolbar"><div class="log-filters"><span class="runtime-pill"><span class:online={status.running}></span>LIVE</span><span>{logs.length} 条记录</span></div><button onclick={clearLogsNow}><Trash2 size={16}/> 清空日志</button></div>
          <section class="terminal"><div class="terminal-bar"><div><i></i><i></i><i></i></div><span>opencodex-proxy — {status.address}</span></div><div class="terminal-body">{#if logs.length === 0}<div class="terminal-empty">代理运行日志会实时显示在这里。</div>{:else}{#each logs as log}<div class="log-line"><time>{new Date(log.timestamp * 1000).toLocaleString("zh-CN", { hour12: false })}</time><b class:error={log.level === "ERROR"} class:warn={log.level === "WARN"}>{log.level.padEnd(5)}</b><span>{log.message}</span></div>{/each}{/if}</div></section>

        {:else if page === "settings"}
          <div class="settings-grid"><section class="panel form-panel"><div class="panel-title"><div><span class="section-kicker">NETWORK</span><h3>代理服务</h3></div></div><label><span>监听地址<small>仅建议使用本机环回地址</small></span><input bind:value={config.settings.bindAddress}/></label><label><span>端口<small>Codex 连接的本地端口</small></span><input type="number" bind:value={config.settings.port}/></label><label><span>请求超时<small>等待上游响应的最大秒数</small></span><input type="number" bind:value={config.settings.requestTimeoutSeconds}/></label><label><span>请求体上限<small>保护代理免受超大请求影响（MB）</small></span><input type="number" bind:value={config.settings.maxBodyMb}/></label></section>
          <section class="panel form-panel"><div class="panel-title"><div><span class="section-kicker">BEHAVIOR</span><h3>行为选项</h3></div></div><label class="toggle-label"><span>启动代理时重启 Codex<small>启动代理后重新打开 ChatGPT.app；停止代理时始终关闭 Codex</small></span><button title="切换 Codex 自动重启" class:checked={config.settings.restartCodexOnChange} class="switch" onclick={() => config.settings.restartCodexOnChange = !config.settings.restartCodexOnChange}><span></span></button></label><label class="toggle-label"><span>打开应用时启动代理<small>桌面应用启动后自动接管流量</small></span><button title="切换自动启动" class:checked={config.settings.startOnLaunch} class="switch" onclick={() => config.settings.startOnLaunch = !config.settings.startOnLaunch}><span></span></button></label><div class="settings-actions"><button onclick={() => api.revealDataDir()}><Database size={16}/> 打开数据目录</button><button class="primary" onclick={() => persist()}><Save size={16}/> 保存设置</button></div></section></div>
        {/if}
      </div>
    </section>
  </div>
{/if}

{#if providerDialog && editingProvider}
  <div class="modal-backdrop" role="presentation" onclick={(e) => e.target === e.currentTarget && (providerDialog = false)}><section class="modal"><div class="modal-head"><div><span class="section-kicker">PROVIDER</span><h2>{config.providers.some(p => p.id === editingProvider?.id) ? "编辑供应商" : "添加供应商"}</h2></div><button class="icon-button" title="关闭" onclick={() => providerDialog = false}><X size={19}/></button></div><div class="modal-body"><label><span>显示名称</span><input bind:value={editingProvider.name} placeholder="例如 Nexaportai-grok"/></label><label><span>Base URL</span><input bind:value={editingProvider.baseUrl} placeholder="https://api.example.com/v1"/></label><label><span>API 密钥</span><input type="password" bind:value={editingProvider.apiKey} placeholder="sk-…（本地服务可留空）"/></label><label><span>上游协议</span><div class="select-wrap"><select bind:value={editingProvider.protocol}><option value="responses">Responses（默认）</option><option value="chat_completions">Chat Completions（兼容模式）</option></select><ChevronDown size={16}/></div></label><label class="toggle-label"><span>启用该供应商</span><button title="切换供应商状态" class:checked={editingProvider.enabled} class="switch" onclick={() => editingProvider && (editingProvider.enabled = !editingProvider.enabled)}><span></span></button></label></div><div class="modal-actions"><button onclick={() => providerDialog = false}>取消</button><button class="primary" onclick={commitProvider}><Check size={16}/> 保存供应商</button></div></section></div>
{/if}

{#if mappingDialog && editingMapping}
  <div class="modal-backdrop" role="presentation" onclick={(e) => e.target === e.currentTarget && (mappingDialog = false)}><section class="modal"><div class="modal-head"><div><span class="section-kicker">MODEL ROUTE</span><h2>配置模型映射</h2></div><button class="icon-button" title="关闭" onclick={() => mappingDialog = false}><X size={19}/></button></div><div class="modal-body"><label><span>Codex 模型名称</span><input bind:value={editingMapping.codexModel} placeholder="例如 gpt-5.4"/></label><label><span>供应商</span><div class="select-wrap"><select bind:value={editingMapping.providerId} onchange={() => editingMapping && (editingMapping.upstreamModel = "")}>{#each config.providers as p}<option value={p.id}>{p.name}</option>{/each}</select><ChevronDown size={16}/></div></label><label><span>上游模型</span><input bind:value={editingMapping.upstreamModel} list="mapping-model-options" placeholder="选择或输入任意模型 ID"/><datalist id="mapping-model-options">{#each config.providers.find(p => p.id === editingMapping?.providerId)?.models.filter(m => m.enabled) ?? [] as model}<option value={model.id}>{model.name}</option>{/each}</datalist><small class="field-hint">可从已发现模型中选择，也可直接输入自定义模型 ID。</small></label></div><div class="modal-actions"><button onclick={() => mappingDialog = false}>取消</button><button class="primary" onclick={commitMapping}><Check size={16}/> 保存映射</button></div></section></div>
{/if}

{#if toast}<div class="toast">{toast}</div>{/if}
